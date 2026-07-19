use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use hz_core::{HzError, HzResult};
use serde::{Deserialize, Serialize};

const HZ_DIR: &str = ".hz";
const CONFIG_FILE: &str = "hz.toml";
const ENVIRONMENT_DIR: &str = "environment";
const POSTCREATE_SCRIPT: &str = "postcreate";
const PREREMOVE_SCRIPT: &str = "preremove";

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HzConfig {
    #[serde(default)]
    pub lifecycle: LifecycleConfig,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LifecycleConfig {
    pub postcreate: Option<Vec<String>>,
    pub preremove: Option<Vec<String>>,
}

impl HzConfig {
    pub fn load(workspace: &Path) -> HzResult<Self> {
        let path = config_path(workspace);
        if !path.exists() {
            return Ok(Self::default());
        }
        let contents = fs::read_to_string(&path)?;
        let mut value = toml::from_str::<toml::Value>(&contents).map_err(|error| {
            HzError::Usage(format!("failed to parse {}: {error}", path.display()))
        })?;
        migrate_legacy_config(&mut value);
        value
            .try_into()
            .map_err(|error| HzError::Usage(format!("failed to parse {}: {error}", path.display())))
    }
}

fn migrate_legacy_config(value: &mut toml::Value) {
    let Some(config) = value.as_table_mut() else {
        return;
    };
    for section in ["worktree", "list", "color"] {
        config.remove(section);
    }
    let Some(lifecycle) = config
        .get_mut("lifecycle")
        .and_then(toml::Value::as_table_mut)
    else {
        return;
    };
    // Legacy hooks were explicitly requested with --setup/--cleanup. The new
    // hooks run by default, so silently promoting these keys would execute
    // commands that users did not opt into after upgrading.
    lifecycle.remove("setup");
    lifecycle.remove("cleanup");
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitConfig {
    pub at: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InitializedConfig {
    pub config_path: PathBuf,
    pub postcreate_path: PathBuf,
    pub preremove_path: PathBuf,
    pub config_created: bool,
    pub postcreate_created: bool,
    pub preremove_created: bool,
}

pub fn init_config(input: InitConfig) -> HzResult<InitializedConfig> {
    let at = fs::canonicalize(input.at)?;
    let config_path = config_path(&at);
    let environment = at.join(HZ_DIR).join(ENVIRONMENT_DIR);
    let postcreate_path = environment.join(POSTCREATE_SCRIPT);
    let preremove_path = environment.join(PREREMOVE_SCRIPT);
    let config_created = write_new_file(&config_path, default_config())?;
    let postcreate_created = write_new_script(&postcreate_path, default_postcreate())?;
    let preremove_created = write_new_script(&preremove_path, default_preremove())?;
    Ok(InitializedConfig {
        config_path,
        postcreate_path,
        preremove_path,
        config_created,
        postcreate_created,
        preremove_created,
    })
}

pub fn config_path(workspace: &Path) -> PathBuf {
    workspace.join(HZ_DIR).join(CONFIG_FILE)
}

fn write_new_file(path: &Path, contents: &str) -> HzResult<bool> {
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

fn write_new_script(path: &Path, contents: &str) -> HzResult<bool> {
    let created = write_new_file(path, contents)?;
    if created {
        make_executable(path)?;
    }
    Ok(created)
}

#[cfg(unix)]
fn make_executable(path: &Path) -> HzResult<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> HzResult<()> {
    Ok(())
}

fn default_config() -> &'static str {
    "[lifecycle]\n# postcreate = [\".hz/environment/postcreate\"]\n# preremove = [\".hz/environment/preremove\"]\n"
}

fn default_postcreate() -> &'static str {
    "#!/usr/bin/env sh\nset -eu\n\n# Prepare a newly-created workspace here.\n"
}

fn default_preremove() -> &'static str {
    "#!/usr/bin/env sh\nset -eu\n\n# Clean up a workspace before it moves to trash.\n"
}
