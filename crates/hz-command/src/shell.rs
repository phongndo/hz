use std::{
    env, fs,
    io::{self, ErrorKind, Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use hz_core::{HzError, HzResult};

pub fn shell_init_line(shell: Shell) -> &'static str {
    match shell {
        Shell::Zsh => r#"eval "$(hz shell zsh)""#,
        Shell::Bash => r#"eval "$(hz shell bash)""#,
        Shell::Fish => "hz shell fish | source",
    }
}

pub fn shell_init_comment() -> &'static str {
    "# hz shell integration"
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellInit {
    pub path: PathBuf,
    pub line: &'static str,
    pub changed: bool,
}

pub fn install_shell_integration(shell: Shell) -> HzResult<ShellInit> {
    let path = shell_rc_path(shell)?;
    let line = shell_init_line(shell);
    let changed = install_line(&path, line)?;

    Ok(ShellInit {
        path,
        line,
        changed,
    })
}

pub fn shell_integration(shell: Shell) -> &'static str {
    match shell {
        Shell::Zsh => include_str!("shell/hz.zsh"),
        Shell::Bash => include_str!("shell/hz.bash"),
        Shell::Fish => include_str!("shell/hz.fish"),
    }
}

pub(crate) fn shell_rc_path(shell: Shell) -> HzResult<PathBuf> {
    shell_rc_path_from_env(
        shell,
        env_path("HOME"),
        env::var_os("ZDOTDIR").map(PathBuf::from),
        env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
    )
}

pub(crate) fn shell_rc_path_from_env(
    shell: Shell,
    home: Option<PathBuf>,
    zdotdir: Option<PathBuf>,
    xdg_config_home: Option<PathBuf>,
) -> HzResult<PathBuf> {
    let home = non_empty_path(home);
    let zdotdir = non_empty_path(zdotdir);
    let xdg_config_home = non_empty_path(xdg_config_home);
    match shell {
        Shell::Zsh => {
            let dotdir = match zdotdir {
                Some(path) => path,
                None => require_home(home)?,
            };
            Ok(dotdir.join(".zshrc"))
        }
        Shell::Bash => Ok(require_home(home)?.join(".bashrc")),
        Shell::Fish => {
            let config_home = match xdg_config_home {
                Some(path) => path,
                None => require_home(home)?.join(".config"),
            };
            Ok(config_home.join("fish").join("config.fish"))
        }
    }
}

pub(crate) fn env_path(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
}

pub(crate) fn non_empty_path(path: Option<PathBuf>) -> Option<PathBuf> {
    path.filter(|path| !path.as_os_str().is_empty())
}

pub(crate) fn require_home(home: Option<PathBuf>) -> HzResult<PathBuf> {
    home.ok_or_else(|| HzError::Usage("HOME is not set or empty".to_owned()))
}

pub(crate) fn install_line(path: &Path, line: &'static str) -> HzResult<bool> {
    let write_path = shell_rc_write_path(path)?;
    // Once an existing rc symlink is resolved, use that same write path for the
    // snapshot and final rename so one install does not mix target contents and
    // metadata from different paths.
    let (existing, existing_metadata) = read_shell_rc(&write_path)?;

    if existing.lines().any(|existing_line| existing_line == line) {
        return Ok(false);
    }

    if let Some(metadata) = existing_metadata.as_ref() {
        if metadata.permissions().readonly() {
            return Err(io::Error::new(
                ErrorKind::PermissionDenied,
                format!("shell rc file is read-only: {}", write_path.display()),
            )
            .into());
        }
    }

    create_parent_dir(&write_path)?;

    if let Some(metadata) = existing_metadata.as_ref() {
        write_install_backup(path, &existing, metadata)?;
    }

    let mut next = existing;
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    next.push_str(shell_init_comment());
    next.push('\n');
    next.push_str(line);
    next.push('\n');

    atomic_write_shell_rc(&write_path, &next, existing_metadata.as_ref())?;
    Ok(true)
}

pub(crate) fn shell_rc_write_path(path: &Path) -> HzResult<PathBuf> {
    match fs::symlink_metadata(path) {
        // Preserve the user's dotfile symlink and atomically replace the target
        // that fs::write(path, ...) would have followed.
        Ok(metadata) if metadata.file_type().is_symlink() => Ok(fs::canonicalize(path)?),
        Ok(_) => Ok(path.to_path_buf()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(path.to_path_buf()),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn read_shell_rc(path: &Path) -> HzResult<(String, Option<fs::Metadata>)> {
    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok((String::new(), None)),
        Err(error) => return Err(error.into()),
    };
    let metadata = file.metadata()?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    Ok((contents, Some(metadata)))
}

pub(crate) fn write_install_backup(
    path: &Path,
    contents: &str,
    metadata: &fs::Metadata,
) -> HzResult<()> {
    let backup_path = shell_rc_backup_path(path)?;
    create_parent_dir(&backup_path)?;
    let mut temp_path = None;

    let result = (|| -> HzResult<()> {
        let (created_temp_path, mut file) = create_shell_rc_temp_file(&backup_path)?;
        temp_path = Some(created_temp_path.clone());

        file.set_permissions(metadata.permissions())?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        drop(file);

        // Link the fully-written temp file into place instead of writing the
        // backup path directly. hard_link is an atomic no-clobber create, so an
        // interrupted process leaves either no backup or a complete backup.
        let backup_created = match fs::hard_link(&created_temp_path, &backup_path) {
            Ok(()) => true,
            Err(error) if error.kind() == ErrorKind::AlreadyExists => false,
            Err(error) => return Err(error.into()),
        };

        if backup_created {
            sync_parent_dir(&backup_path)?;
        }

        fs::remove_file(&created_temp_path)?;
        temp_path = None;
        sync_parent_dir(&backup_path)?;
        Ok(())
    })();

    if let Some(temp_path) = temp_path {
        let _ = fs::remove_file(temp_path);
    }

    result
}

pub(crate) fn atomic_write_shell_rc(
    path: &Path,
    contents: &str,
    metadata: Option<&fs::Metadata>,
) -> HzResult<()> {
    let mut temp_path = None;
    let result = (|| -> HzResult<()> {
        let (created_temp_path, mut file) = create_shell_rc_temp_file(path)?;
        temp_path = Some(created_temp_path.clone());

        if let Some(metadata) = metadata {
            file.set_permissions(metadata.permissions())?;
        }
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        drop(file);

        fs::rename(&created_temp_path, path)?;
        temp_path = None;
        sync_parent_dir(path)?;
        Ok(())
    })();

    if let Err(error) = result {
        if let Some(temp_path) = temp_path {
            let _ = fs::remove_file(temp_path);
        }
        return Err(error);
    }

    Ok(())
}

pub(crate) fn create_shell_rc_temp_file(path: &Path) -> HzResult<(PathBuf, fs::File)> {
    for attempt in 0..16 {
        let temp_path = shell_rc_temp_path(path, attempt)?;
        match new_shell_rc_file_options()
            .write(true)
            .create_new(true)
            .open(&temp_path)
        {
            Ok(file) => return Ok((temp_path, file)),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }

    Err(HzError::Usage(format!(
        "failed to create a unique temporary shell rc file for {}",
        path.display()
    )))
}

pub(crate) fn new_shell_rc_file_options() -> fs::OpenOptions {
    let mut options = fs::OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

pub(crate) fn shell_rc_temp_path(path: &Path, attempt: u32) -> HzResult<PathBuf> {
    let file_name = path.file_name().ok_or_else(|| {
        HzError::Usage(format!(
            "shell rc path has no file name: {}",
            path.display()
        ))
    })?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| HzError::Usage(format!("system clock is before unix epoch: {error}")))?
        .as_nanos();

    Ok(path.with_file_name(format!(
        ".{}.{}.{}.{}.tmp",
        file_name.to_string_lossy(),
        std::process::id(),
        timestamp,
        attempt
    )))
}

pub(crate) fn shell_rc_backup_path(path: &Path) -> HzResult<PathBuf> {
    let file_name = path.file_name().ok_or_else(|| {
        HzError::Usage(format!(
            "shell rc path has no file name: {}",
            path.display()
        ))
    })?;
    Ok(path.with_file_name(format!("{}.bak", file_name.to_string_lossy())))
}

pub(crate) fn create_parent_dir(path: &Path) -> HzResult<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

#[cfg(unix)]
pub(crate) fn sync_parent_dir(path: &Path) -> HzResult<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let directory = fs::File::open(parent)?;
    directory.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn sync_parent_dir(_path: &Path) -> HzResult<()> {
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    Zsh,
    Bash,
    Fish,
}
