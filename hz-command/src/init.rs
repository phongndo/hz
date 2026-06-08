use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use crate::{CLEANUP_SCRIPT, ENVIRONMENT_DIR, HZ_DIR, SETUP_SCRIPT, config_path, config_repo};
use hz_core::HzResult;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitRepo {
    pub repo: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepoInit {
    pub repo: PathBuf,
    pub config_path: PathBuf,
    pub setup_path: PathBuf,
    pub cleanup_path: PathBuf,
    pub config_created: bool,
    pub setup_created: bool,
    pub cleanup_created: bool,
}

pub fn init_repo(input: InitRepo) -> HzResult<RepoInit> {
    let repo = config_repo(input.repo.as_deref())?;
    let config_path = config_path(&repo);
    let lifecycle_path = repo.join(HZ_DIR).join(ENVIRONMENT_DIR);
    let setup_path = lifecycle_path.join(SETUP_SCRIPT);
    let cleanup_path = lifecycle_path.join(CLEANUP_SCRIPT);

    let config_created = write_new_file(&config_path, default_config())?;
    let setup_created = write_new_script(&setup_path, default_setup_script())?;
    let cleanup_created = write_new_script(&cleanup_path, default_cleanup_script())?;

    Ok(RepoInit {
        repo,
        config_path,
        setup_path,
        cleanup_path,
        config_created,
        setup_created,
        cleanup_created,
    })
}

pub(crate) fn write_new_file(path: &Path, contents: &str) -> HzResult<bool> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(mut file) => {
            use std::io::Write;
            file.write_all(contents.as_bytes())?;
            Ok(true)
        }
        Err(error) if error.kind() == ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn write_new_script(path: &Path, contents: &str) -> HzResult<bool> {
    let created = write_new_file(path, contents)?;
    if created {
        make_executable(path)?;
    }
    Ok(created)
}

#[cfg(unix)]
pub(crate) fn make_executable(path: &Path) -> HzResult<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn make_executable(_path: &Path) -> HzResult<()> {
    Ok(())
}

pub(crate) fn default_config() -> &'static str {
    "[worktree]\nmax_detached = 15\n# default_base = \"dev\"\n# user_managed_roots = [\"~/.codex/worktrees\"]\n\n[list]\nheaders = \"auto\"\ncolumns = [\"marker\", \"target\", \"status\", \"modified\", \"path\"]\n\n[color]\nmode = \"auto\"\nscheme = \"terminal\"\n\n[lifecycle]\nsetup = [\".hz/environment/setup\"]\ncleanup = [\".hz/environment/cleanup\"]\n"
}

pub(crate) fn default_setup_script() -> &'static str {
    "#!/usr/bin/env sh\nset -eu\n\n# Add repo setup commands here.\n"
}

pub(crate) fn default_cleanup_script() -> &'static str {
    "#!/usr/bin/env sh\nset -eu\n\n# Add repo cleanup commands here.\n"
}
