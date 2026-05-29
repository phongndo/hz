use std::{
    env, fs,
    io::Read,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use hz_core::{HzError, HzResult, paths::WorktreeTarget};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateWorktree {
    pub name: Option<String>,
    pub repo: Option<PathBuf>,
    pub path: Option<PathBuf>,
    pub base: Option<String>,
    pub branch: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitchWorktree {
    pub target: String,
    pub repo: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoffWorktree {
    pub from: String,
    pub to: String,
    pub repo: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListWorktrees {
    pub repo: Option<PathBuf>,
    pub all: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoveWorktree {
    pub target: String,
    pub repo: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
pub struct CreatedWorktree {
    pub id: String,
    pub name: String,
    pub handle: String,
    pub repo: PathBuf,
    pub path: PathBuf,
    pub branch: String,
    pub base: Option<String>,
    pub source: WorktreeSource,
}

#[derive(Debug, Serialize)]
pub struct WorktreeHandoff {
    pub repo: PathBuf,
    pub from: WorktreeTarget,
    pub to: WorktreeTarget,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorktreeSource {
    Managed,
    Git,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeEntry {
    pub id: String,
    pub handle: String,
    pub repo: PathBuf,
    pub path: PathBuf,
    pub branch: Option<String>,
    pub base: Option<String>,
    pub source: WorktreeSource,
    pub created_at_unix: u64,
}

pub fn create(input: CreateWorktree) -> HzResult<CreatedWorktree> {
    let mut registry = Registry::load()?;
    let repo = resolve_repo(input.repo.as_deref(), &registry)?;
    let handle = match input.name {
        Some(name) => name,
        None => generate_unique_handle(&registry, &repo),
    };
    let branch = input.branch.unwrap_or_else(|| handle.clone());

    if registry.find(&repo, &handle).is_some() {
        return Err(HzError::Usage(format!(
            "worktree handle already exists: {handle}"
        )));
    }

    let id = new_uuid_v4()?;
    let path = match input.path {
        Some(path) => path,
        None => default_worktree_path(&repo, &id)?,
    };

    if path.exists() {
        return Err(HzError::Usage(format!(
            "worktree path already exists: {}",
            path.display()
        )));
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    hz_git::add_worktree(&repo, &path, &branch, input.base.as_deref())?;

    let entry = WorktreeEntry {
        id: id.clone(),
        handle: handle.clone(),
        repo: repo.clone(),
        path: path.clone(),
        branch: Some(branch.clone()),
        base: input.base.clone(),
        source: WorktreeSource::Managed,
        created_at_unix: unix_now()?,
    };
    registry.entries.push(entry);
    registry.save()?;

    Ok(CreatedWorktree {
        id,
        name: handle.clone(),
        handle,
        repo,
        path,
        branch,
        base: input.base,
        source: WorktreeSource::Managed,
    })
}

pub fn switch(input: SwitchWorktree) -> HzResult<WorktreeTarget> {
    let registry = Registry::load()?;
    let repo = resolve_repo(input.repo.as_deref(), &registry)?;
    resolve_target(&repo, &input.target)
}

pub fn handoff(input: HandoffWorktree) -> HzResult<WorktreeHandoff> {
    let registry = Registry::load()?;
    let repo = resolve_repo(input.repo.as_deref(), &registry)?;
    let from = resolve_target(&repo, &input.from)?;
    let to = resolve_target(&repo, &input.to)?;

    Ok(WorktreeHandoff { repo, from, to })
}

pub fn list(input: ListWorktrees) -> HzResult<Vec<WorktreeEntry>> {
    let registry = Registry::load()?;
    let repo = resolve_repo(input.repo.as_deref(), &registry)?;
    let mut entries: Vec<_> = registry
        .entries
        .into_iter()
        .filter(|entry| same_path(&entry.repo, &repo))
        .collect();

    if input.all {
        for worktree in hz_git::list_worktrees(&repo)? {
            if entries.iter().any(|entry| entry.path == worktree.path) {
                continue;
            }

            let handle = worktree
                .branch
                .clone()
                .or_else(|| {
                    worktree
                        .path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                })
                .unwrap_or_else(|| worktree.path.display().to_string());

            entries.push(WorktreeEntry {
                id: handle.clone(),
                handle,
                repo: repo.clone(),
                path: worktree.path,
                branch: worktree.branch,
                base: None,
                source: WorktreeSource::Git,
                created_at_unix: 0,
            });
        }
    }

    entries.sort_by(|left, right| left.handle.cmp(&right.handle));
    Ok(entries)
}

pub fn remove(input: RemoveWorktree) -> HzResult<WorktreeEntry> {
    let mut registry = Registry::load()?;
    let repo = resolve_repo(input.repo.as_deref(), &registry)?;
    let index = registry
        .entries
        .iter()
        .position(|entry| same_path(&entry.repo, &repo) && matches_target(entry, &input.target))
        .ok_or_else(|| HzError::Usage(format!("unknown worktree: {}", input.target)))?;
    let entry = registry.entries.remove(index);

    hz_git::remove_worktree(&repo, &entry.path)?;
    registry.save()?;

    Ok(entry)
}

fn resolve_repo(repo: Option<&Path>, registry: &Registry) -> HzResult<PathBuf> {
    let current = hz_git::repository_root(repo)?;
    if let Some(entry) = registry.find_by_path(&current) {
        return Ok(entry.repo.clone());
    }

    Ok(current)
}

fn resolve_target(repo: &Path, target: &str) -> HzResult<WorktreeTarget> {
    if target == "local" {
        return Ok(WorktreeTarget {
            name: "local".to_owned(),
            path: repo.to_path_buf(),
        });
    }

    let registry = Registry::load()?;
    if let Some(entry) = registry.find(repo, target) {
        return Ok(WorktreeTarget {
            name: entry.handle.clone(),
            path: entry.path.clone(),
        });
    }

    for worktree in hz_git::list_worktrees(repo)? {
        let path_name = worktree
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned());
        if worktree.branch.as_deref() == Some(target) || path_name.as_deref() == Some(target) {
            return Ok(WorktreeTarget {
                name: target.to_owned(),
                path: worktree.path,
            });
        }
    }

    Err(HzError::Usage(format!("unknown worktree: {target}")))
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Registry {
    entries: Vec<WorktreeEntry>,
}

impl Registry {
    fn load() -> HzResult<Self> {
        let path = registry_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }

        let contents = fs::read_to_string(path)?;
        Ok(serde_json::from_str(&contents)?)
    }

    fn save(&self) -> HzResult<()> {
        let path = registry_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    fn find(&self, repo: &Path, target: &str) -> Option<&WorktreeEntry> {
        self.entries
            .iter()
            .find(|entry| same_path(&entry.repo, repo) && matches_target(entry, target))
    }

    fn find_by_path(&self, path: &Path) -> Option<&WorktreeEntry> {
        self.entries
            .iter()
            .find(|entry| same_path(&entry.path, path))
    }
}

fn same_path(left: &Path, right: &Path) -> bool {
    left == right
        || fs::canonicalize(left)
            .ok()
            .zip(fs::canonicalize(right).ok())
            .is_some_and(|(left, right)| left == right)
}

fn matches_target(entry: &WorktreeEntry, target: &str) -> bool {
    entry.id == target || entry.handle == target || entry.branch.as_deref() == Some(target)
}

fn default_worktree_path(repo: &Path, id: &str) -> HzResult<PathBuf> {
    let repo_name = repo
        .file_name()
        .ok_or_else(|| HzError::Usage(format!("repo path has no name: {}", repo.display())))?;
    Ok(home_dir()?
        .join(".hz")
        .join("worktrees")
        .join(repo_name)
        .join(id))
}

fn registry_path() -> HzResult<PathBuf> {
    let config_home = match env::var_os("XDG_CONFIG_HOME") {
        Some(path) => PathBuf::from(path),
        None => home_dir()?.join(".config"),
    };
    Ok(config_home.join("hz").join("registry.json"))
}

fn home_dir() -> HzResult<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| HzError::Usage("HOME is not set".to_owned()))
}

fn generate_unique_handle(registry: &Registry, repo: &Path) -> String {
    for attempt in 0..128 {
        let handle = generate_handle(attempt);
        if registry.find(repo, &handle).is_none() {
            return handle;
        }
    }

    generate_handle(128)
}

fn generate_handle(attempt: u128) -> String {
    const LEFT: &[&str] = &[
        "clear", "direct", "fast", "focus", "fresh", "local", "plain", "steady",
    ];
    const RIGHT: &[&str] = &[
        "branch", "change", "path", "patch", "shift", "stack", "task", "tree",
    ];

    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default()
        + attempt;
    let left = LEFT[(seed as usize) % LEFT.len()];
    let right = RIGHT[((seed / LEFT.len() as u128) as usize) % RIGHT.len()];

    format!("{left}-{right}")
}

fn unix_now() -> HzResult<u64> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| HzError::Usage(format!("system clock is before unix epoch: {error}")))?;
    Ok(duration.as_secs())
}

fn new_uuid_v4() -> HzResult<String> {
    let mut bytes = [0_u8; 16];
    fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;

    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    Ok(format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_handle_is_easy_to_type() {
        let handle = generate_handle(0);

        assert!(handle.contains('-'));
        assert!(
            handle
                .chars()
                .all(|character| { character.is_ascii_lowercase() || character == '-' })
        );
    }
}
