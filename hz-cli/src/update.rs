use std::{
    env,
    ffi::{OsStr, OsString},
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, Stdio},
};

use hz_core::HzResult;

use crate::{
    CliResult,
    args::{INSTALL_SCRIPT, RELEASE_REPO, UpdateArgs},
};

pub(crate) fn update(args: UpdateArgs) -> CliResult<()> {
    let argv0 = env::args_os().next().ok_or_else(|| {
        hz_core::HzError::Usage("could not determine current executable".to_owned())
    })?;
    let binary = update_binary_name(&argv0)?;
    let install_dir = match args.install_dir {
        Some(path) => absolute_path(path)?,
        None => default_update_install_dir(&argv0)?,
    };
    check_update_install_dir(&install_dir, &binary)?;
    let version = args.version.unwrap_or_else(|| "latest".to_owned());
    let repo = update_repo(env::var_os("HZ_REPO"));

    let mut child = ProcessCommand::new("sh")
        .arg("-s")
        .env("HZ_REPO", repo)
        .env("HZ_INSTALL_DIR", install_dir)
        .env("HZ_VERSION", version)
        .env("HZ_CURRENT_VERSION", env!("CARGO_PKG_VERSION"))
        .env("HZ_BINARY", binary)
        .env("HZ_INSTALL_ACTION", "update")
        .stdin(Stdio::piped())
        .spawn()?;

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| hz_core::HzError::Usage("could not open installer stdin".to_owned()))?;
    stdin.write_all(INSTALL_SCRIPT.as_bytes())?;
    drop(stdin);

    let status = child.wait()?;
    if !status.success() {
        return Err(hz_core::HzError::Usage(format!(
            "update failed with status {}",
            status
                .code()
                .map_or_else(|| "unknown".to_owned(), |code| code.to_string())
        ))
        .into());
    }

    Ok(())
}

pub(crate) fn update_repo(repo: Option<OsString>) -> OsString {
    repo.filter(|repo| !repo.as_os_str().is_empty())
        .unwrap_or_else(|| OsString::from(RELEASE_REPO))
}

pub(crate) fn update_binary_name(argv0: &OsStr) -> HzResult<OsString> {
    Path::new(argv0)
        .file_name()
        .filter(|name| !name.is_empty())
        .map(OsString::from)
        .ok_or_else(|| {
            hz_core::HzError::Usage("could not determine current executable name".to_owned())
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ManagedUpdateInstall {
    Homebrew,
    Mise,
    Cargo,
    Nix,
    Asdf,
}

impl ManagedUpdateInstall {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Homebrew => "Homebrew",
            Self::Mise => "mise",
            Self::Cargo => "Cargo",
            Self::Nix => "Nix",
            Self::Asdf => "asdf",
        }
    }
}

pub(crate) fn check_update_install_dir(install_dir: &Path, binary: &OsStr) -> HzResult<()> {
    let Some(manager) = managed_update_install(install_dir, binary) else {
        return Ok(());
    };

    Err(hz_core::HzError::Usage(format!(
        "hz update only supports curl-installed hz binaries.\n\nThis path looks {}-managed:\n  {}\n\nReinstall with:\n  curl -fsSL https://raw.githubusercontent.com/phongndo/hz/main/scripts/install.sh | sh\n\nUse --install-dir DIR only for curl-installed targets outside package-manager directories.",
        manager.name(),
        install_dir.join(binary).display(),
    )))
}

pub(crate) fn managed_update_install(
    install_dir: &Path,
    binary: &OsStr,
) -> Option<ManagedUpdateInstall> {
    let target = install_dir.join(binary);

    classify_managed_update_path(&target)
        .or_else(|| {
            fs::read_link(&target).ok().and_then(|link| {
                let path = if link.is_absolute() {
                    link
                } else {
                    install_dir.join(link)
                };
                classify_managed_update_path(&path)
            })
        })
        .or_else(|| {
            fs::canonicalize(&target)
                .ok()
                .and_then(|path| classify_managed_update_path(&path))
        })
        .or_else(|| classify_managed_update_path(install_dir))
}

pub(crate) fn classify_managed_update_path(path: &Path) -> Option<ManagedUpdateInstall> {
    let path = path.to_string_lossy().replace('\\', "/");

    if path.starts_with("/opt/homebrew/")
        || path.starts_with("/home/linuxbrew/.linuxbrew/")
        || path.contains("/.linuxbrew/")
        || path.contains("/Cellar/")
    {
        return Some(ManagedUpdateInstall::Homebrew);
    }

    if path_has_dir(&path, "/.cargo/bin") {
        return Some(ManagedUpdateInstall::Cargo);
    }

    if path_has_dir(&path, "/.local/share/mise/shims")
        || path_has_dir(&path, "/.local/share/mise/installs")
        || path_has_dir(&path, "/.mise/shims")
        || path_has_dir(&path, "/.mise/installs")
    {
        return Some(ManagedUpdateInstall::Mise);
    }

    if path.starts_with("/nix/store/")
        || path_has_dir(&path, "/.nix-profile/bin")
        || path_has_dir(&path, "/.local/state/nix/profile/bin")
        || path.starts_with("/run/current-system/sw/bin")
    {
        return Some(ManagedUpdateInstall::Nix);
    }

    if path_has_dir(&path, "/.asdf/shims") || path_has_dir(&path, "/.asdf/installs") {
        return Some(ManagedUpdateInstall::Asdf);
    }

    None
}

pub(crate) fn path_has_dir(path: &str, dir: &str) -> bool {
    path.ends_with(dir) || path.contains(&format!("{dir}/"))
}

pub(crate) fn default_update_install_dir(argv0: &OsStr) -> HzResult<PathBuf> {
    let argv0_path = Path::new(argv0);
    if argv0_path.components().count() > 1 {
        return invocation_parent_dir(argv0_path);
    }

    let binary = update_binary_name(argv0)?;
    if let Some(path) = env::var_os("PATH") {
        for dir in env::split_paths(&path) {
            if dir.join(Path::new(&binary)).is_file() {
                return absolute_path(dir);
            }
        }
    }

    current_exe_parent_dir()
}

pub(crate) fn invocation_parent_dir(path: &Path) -> HzResult<PathBuf> {
    let parent = path.parent().ok_or_else(|| {
        hz_core::HzError::Usage("could not determine current executable directory".to_owned())
    })?;
    absolute_path(parent.to_path_buf())
}

pub(crate) fn current_exe_parent_dir() -> HzResult<PathBuf> {
    let executable = env::current_exe()?;
    let parent = executable.parent().ok_or_else(|| {
        hz_core::HzError::Usage("could not determine current executable directory".to_owned())
    })?;
    absolute_path(parent.to_path_buf())
}

pub(crate) fn absolute_path(path: PathBuf) -> HzResult<PathBuf> {
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(env::current_dir()?.join(path))
    }
}
