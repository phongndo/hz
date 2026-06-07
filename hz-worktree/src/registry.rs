#![allow(unused_imports)]

use crate::*;
use std::{
    collections::HashSet,
    env, fs,
    io::{self, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use hz_core::{HzError, HzResult, paths::WorktreeTarget};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct Registry {
    pub(crate) entries: Vec<WorktreeEntry>,
    #[serde(default)]
    pub(crate) handoffs: Vec<HandoffLink>,
    #[serde(default)]
    pub(crate) patch_handoffs: Vec<PatchHandoffLink>,
}

impl Registry {
    pub(crate) fn load_for_update() -> HzResult<LockedRegistry> {
        LockedRegistry::load()
    }

    pub(crate) fn load() -> HzResult<Self> {
        let path = registry_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }

        let contents = fs::read_to_string(&path)?;
        serde_json::from_str(&contents).map_err(|error| {
            HzError::Usage(format!(
                "failed to parse registry {}: {error}",
                path.display()
            ))
        })
    }

    pub(crate) fn save(&self) -> HzResult<()> {
        let path = registry_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let temp_path = registry_temp_path(&path)?;
        let mut created_temp = false;
        let save_result = (|| {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp_path)?;
            created_temp = true;
            file.write_all(serde_json::to_string_pretty(self)?.as_bytes())?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            drop(file);

            fs::rename(&temp_path, &path)?;
            created_temp = false;
            Ok(())
        })();

        if save_result.is_err() && created_temp {
            let _ = fs::remove_file(&temp_path);
        }

        save_result
    }

    pub(crate) fn find(&self, repo: &Path, target: &str) -> Option<&WorktreeEntry> {
        self.entries
            .iter()
            .find(|entry| same_path(&entry.repo, repo) && matches_target(entry, target))
    }

    pub(crate) fn find_by_path(&self, path: &Path) -> Option<&WorktreeEntry> {
        self.entries
            .iter()
            .find(|entry| same_path(&entry.path, path))
    }

    pub(crate) fn find_by_repo(&self, repo: &Path) -> Option<&WorktreeEntry> {
        self.entries
            .iter()
            .find(|entry| same_path(&entry.repo, repo))
    }

    pub(crate) fn handoff_link(&self, repo: &Path, branch: &str) -> Option<&HandoffLink> {
        self.handoffs
            .iter()
            .find(|link| same_path(&link.repo, repo) && link.branch == branch)
    }

    pub(crate) fn patch_handoff_link(
        &self,
        repo: &Path,
        left_path: &Path,
        right_path: &Path,
    ) -> Option<&PatchHandoffLink> {
        self.patch_handoffs.iter().find(|link| {
            same_path(&link.repo, repo)
                && ((same_path(&link.left_path, left_path)
                    && same_path(&link.right_path, right_path))
                    || (same_path(&link.left_path, right_path)
                        && same_path(&link.right_path, left_path)))
        })
    }

    pub(crate) fn latest_patch_handoff_for_path(
        &self,
        repo: &Path,
        path: &Path,
    ) -> Option<&PatchHandoffLink> {
        self.patch_handoffs
            .iter()
            .filter(|link| {
                same_path(&link.repo, repo)
                    && (same_path(&link.left_path, path) || same_path(&link.right_path, path))
            })
            .max_by_key(|link| link.updated_at_unix)
    }

    pub(crate) fn remember_handoff(
        &mut self,
        repo: &Path,
        branch: &str,
        path: &Path,
        handle: &str,
        local_restore_branch: Option<String>,
    ) -> HzResult<()> {
        self.forget_handoff(repo, branch);
        self.handoffs.push(HandoffLink {
            repo: repo.to_path_buf(),
            branch: branch.to_owned(),
            path: path.to_path_buf(),
            handle: handle.to_owned(),
            local_restore_branch,
            updated_at_unix: unix_now()?,
        });
        Ok(())
    }

    pub(crate) fn remember_patch_handoff(
        &mut self,
        repo: &Path,
        left: &WorktreeTarget,
        right: &WorktreeTarget,
        patch_hash: String,
    ) -> HzResult<()> {
        self.patch_handoffs.retain(|link| {
            !same_path(&link.repo, repo)
                || !((same_path(&link.left_path, &left.path)
                    && same_path(&link.right_path, &right.path))
                    || (same_path(&link.left_path, &right.path)
                        && same_path(&link.right_path, &left.path)))
        });
        self.patch_handoffs.push(PatchHandoffLink {
            repo: repo.to_path_buf(),
            left_path: left.path.clone(),
            left_handle: left.name.clone(),
            right_path: right.path.clone(),
            right_handle: right.name.clone(),
            patch_hash,
            updated_at_unix: unix_now()?,
        });
        Ok(())
    }

    pub(crate) fn forget_handoff(&mut self, repo: &Path, branch: &str) {
        self.handoffs
            .retain(|link| !same_path(&link.repo, repo) || link.branch != branch);
    }
}

pub(crate) struct LockedRegistry {
    pub(crate) _lock: RegistryLock,
    pub(crate) registry: Registry,
}

impl LockedRegistry {
    pub(crate) fn load() -> HzResult<Self> {
        let lock = RegistryLock::acquire()?;
        let registry = Registry::load()?;
        Ok(Self {
            _lock: lock,
            registry,
        })
    }

    pub(crate) fn save(&self) -> HzResult<()> {
        self.registry.save()
    }
}

impl std::ops::Deref for LockedRegistry {
    type Target = Registry;

    fn deref(&self) -> &Self::Target {
        &self.registry
    }
}

impl std::ops::DerefMut for LockedRegistry {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.registry
    }
}

pub(crate) struct RegistryLock {
    pub(crate) file: fs::File,
}

impl RegistryLock {
    pub(crate) fn acquire() -> HzResult<Self> {
        let registry_path = registry_path()?;
        let lock_path = registry_lock_path(&registry_path)?;
        Self::acquire_path(&lock_path)
    }

    pub(crate) fn acquire_path(lock_path: &Path) -> HzResult<Self> {
        if Self::is_held_by_current_thread() {
            return Err(HzError::Usage(
                "registry lock is already held by this thread".to_owned(),
            ));
        }

        if let Some(parent) = lock_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let file = open_registry_lock_file(lock_path)?;
        lock_registry_file(&file)?;
        REGISTRY_LOCK_HELD.with(|held| held.set(true));
        Ok(Self { file })
    }

    pub(crate) fn is_held_by_current_thread() -> bool {
        REGISTRY_LOCK_HELD.with(|held| held.get())
    }
}

impl Drop for RegistryLock {
    fn drop(&mut self) {
        let _ = unlock_registry_file(&self.file);
        REGISTRY_LOCK_HELD.with(|held| held.set(false));
    }
}

thread_local! {
    static REGISTRY_LOCK_HELD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

// Git worktree removal is a Git side effect, not a registry mutation. Reuse an
// existing registry critical section when one is already held so internal
// callers cannot re-enter flock on a second file descriptor and deadlock.
pub(crate) fn run_with_registry_lock_for_git_side_effect<T>(
    operation: impl FnOnce() -> HzResult<T>,
) -> HzResult<T> {
    if RegistryLock::is_held_by_current_thread() {
        return operation();
    }

    let _registry_lock = RegistryLock::acquire()?;
    operation()
}

pub(crate) fn same_path(left: &Path, right: &Path) -> bool {
    left == right
        || fs::canonicalize(left)
            .ok()
            .zip(fs::canonicalize(right).ok())
            .is_some_and(|(left, right)| left == right)
}

pub(crate) fn matches_target(entry: &WorktreeEntry, target: &str) -> bool {
    entry.id == target || entry.handle == target || entry.branch.as_deref() == Some(target)
}

pub(crate) fn validate_worktree_name(label: &str, name: &str) -> HzResult<()> {
    if name == "local" {
        return Err(HzError::Usage(format!(
            "{label} 'local' is reserved for the repository root"
        )));
    }

    Ok(())
}

pub(crate) fn resolve_worktree_path(repo: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        repo.join(path)
    }
}

pub(crate) fn registry_temp_path(path: &Path) -> HzResult<PathBuf> {
    let file_name = path.file_name().ok_or_else(|| {
        HzError::Usage(format!(
            "registry path has no file name: {}",
            path.display()
        ))
    })?;
    Ok(path.with_file_name(format!(
        ".{}.{}.tmp",
        file_name.to_string_lossy(),
        new_uuid_v4()?
    )))
}

pub(crate) fn registry_lock_path(path: &Path) -> HzResult<PathBuf> {
    let file_name = path.file_name().ok_or_else(|| {
        HzError::Usage(format!(
            "registry path has no file name: {}",
            path.display()
        ))
    })?;
    let mut lock_file_name = file_name.to_os_string();
    lock_file_name.push(".lock");
    Ok(path.with_file_name(lock_file_name))
}

pub(crate) fn open_registry_lock_file(path: &Path) -> io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

pub(crate) fn lock_registry_file(file: &fs::File) -> io::Result<()> {
    loop {
        match fs2::FileExt::lock_exclusive(file) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
}

pub(crate) fn unlock_registry_file(file: &fs::File) -> io::Result<()> {
    fs2::FileExt::unlock(file)
}

pub(crate) fn default_worktree_path(repo: &Path, id: &str) -> HzResult<PathBuf> {
    let repo_name = repo
        .file_name()
        .ok_or_else(|| HzError::Usage(format!("repo path has no name: {}", repo.display())))?;
    Ok(home_dir()?
        .join(".hz")
        .join("worktrees")
        .join(repo_name)
        .join(id))
}

pub(crate) fn registry_path() -> HzResult<PathBuf> {
    #[cfg(test)]
    if let Some(path) = registry_path_override() {
        return Ok(path);
    }

    registry_path_from_env(
        env_path("HOME"),
        env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
    )
}

#[cfg(test)]
std::thread_local! {
    static REGISTRY_PATH_OVERRIDE: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(crate) fn registry_path_override() -> Option<PathBuf> {
    REGISTRY_PATH_OVERRIDE.with(|path| path.borrow().clone())
}

#[cfg(test)]
pub(crate) struct RegistryPathOverrideGuard(Option<PathBuf>);

#[cfg(test)]
impl RegistryPathOverrideGuard {
    pub(crate) fn set(path: PathBuf) -> Self {
        Self(REGISTRY_PATH_OVERRIDE.with(|override_path| override_path.replace(Some(path))))
    }
}

#[cfg(test)]
impl Drop for RegistryPathOverrideGuard {
    fn drop(&mut self) {
        REGISTRY_PATH_OVERRIDE.with(|override_path| {
            override_path.replace(self.0.take());
        });
    }
}

pub(crate) fn registry_path_from_env(
    home: Option<PathBuf>,
    xdg_config_home: Option<PathBuf>,
) -> HzResult<PathBuf> {
    let config_home = match non_empty_path(xdg_config_home) {
        Some(path) => path,
        None => require_home(non_empty_path(home))?.join(".config"),
    };
    Ok(config_home.join("hz").join("registry.json"))
}

pub(crate) fn home_dir() -> HzResult<PathBuf> {
    require_home(env_path("HOME"))
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
