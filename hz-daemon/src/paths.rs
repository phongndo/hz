use std::{env, fs, io, path::PathBuf};

use hz_core::{HzError, HzResult};
use std::os::unix::fs::{FileTypeExt, PermissionsExt};

use crate::protocol::{AGENTS_FILE, PID_FILE, SOCKET_FILE};

pub(crate) fn prepare_runtime_dir() -> HzResult<PathBuf> {
    let path = runtime_dir()?;
    fs::create_dir_all(&path)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
    Ok(path)
}

pub(crate) fn remove_stale_socket() -> HzResult<()> {
    let path = socket_path()?;
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_socket() => {
            fs::remove_file(path)?;
            Ok(())
        }
        Ok(_) => Err(HzError::Usage(format!(
            "refusing to remove non-socket daemon path: {}",
            path.display()
        ))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn socket_path() -> HzResult<PathBuf> {
    Ok(runtime_dir()?.join(SOCKET_FILE))
}

pub(crate) fn pid_path() -> HzResult<PathBuf> {
    Ok(runtime_dir()?.join(PID_FILE))
}

pub(crate) fn agents_file() -> HzResult<PathBuf> {
    Ok(agents_dir()?.join(AGENTS_FILE))
}

pub(crate) fn agents_dir() -> HzResult<PathBuf> {
    Ok(state_dir()?.join("agents"))
}

fn runtime_dir() -> HzResult<PathBuf> {
    if let Some(path) = env_path("HZ_RUNTIME_DIR") {
        return Ok(path);
    }
    if let Some(path) = env_path("XDG_RUNTIME_DIR") {
        return Ok(path.join("hz"));
    }
    if let Some(home) = env_path("HOME") {
        return Ok(home.join(".hz").join("run"));
    }

    Err(HzError::Usage(
        "HOME is not set or empty; set HZ_RUNTIME_DIR for the hz daemon".to_owned(),
    ))
}

fn state_dir() -> HzResult<PathBuf> {
    if let Some(path) = env_path("HZ_STATE_DIR") {
        return Ok(path);
    }
    if let Some(path) = env_path("HZ_RUNTIME_DIR") {
        return Ok(path.join("state"));
    }
    if let Some(home) = env_path("HOME") {
        return Ok(home.join(".hz"));
    }

    Err(HzError::Usage(
        "HOME is not set or empty; set HZ_STATE_DIR for hz agent sessions".to_owned(),
    ))
}

fn env_path(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}
