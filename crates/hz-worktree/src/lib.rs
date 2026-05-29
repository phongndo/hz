use std::{
    env, fs,
    io::{Read, Write},
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
pub struct PathWorktree {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindWorktree {
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
    validate_worktree_name("worktree handle", &handle)?;
    validate_worktree_name("worktree branch", &branch)?;

    if registry.find(&repo, &handle).is_some() {
        return Err(HzError::Usage(format!(
            "worktree handle already exists: {handle}"
        )));
    }

    let id = new_uuid_v4()?;
    let path = match input.path {
        Some(path) => resolve_worktree_path(&repo, path),
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
    let path = fs::canonicalize(&path)?;

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

pub fn path(input: PathWorktree) -> HzResult<WorktreeTarget> {
    let registry = Registry::load()?;
    let repo = resolve_repo(input.repo.as_deref(), &registry)?;
    resolve_target(&registry, &repo, &input.target)
}

pub fn handoff(input: HandoffWorktree) -> HzResult<WorktreeHandoff> {
    let registry = Registry::load()?;
    let repo = resolve_repo(input.repo.as_deref(), &registry)?;
    let from = resolve_target(&registry, &repo, &input.from)?;
    let to = resolve_target(&registry, &repo, &input.to)?;

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

    let _ = input.all;
    add_git_worktrees(&mut entries, &repo, hz_git::list_worktrees(&repo)?);

    entries.sort_by(|left, right| left.handle.cmp(&right.handle));
    Ok(entries)
}

pub fn find(input: FindWorktree) -> HzResult<WorktreeEntry> {
    let registry = Registry::load()?;
    let repo = resolve_repo(input.repo.as_deref(), &registry)?;
    find_entry(&registry, &repo, &input.target)
}

pub fn remove(input: RemoveWorktree) -> HzResult<WorktreeEntry> {
    let mut registry = Registry::load()?;
    let repo = resolve_repo(input.repo.as_deref(), &registry)?;
    if let Some(index) = registry
        .entries
        .iter()
        .position(|entry| same_path(&entry.repo, &repo) && matches_target(entry, &input.target))
    {
        let entry = registry.entries.remove(index);

        hz_git::remove_worktree(&repo, &entry.path)?;
        registry.save()?;

        return Ok(entry);
    }

    let entry = find_git_entry(&repo, &input.target)?;
    hz_git::remove_worktree(&repo, &entry.path)?;

    Ok(entry)
}

fn resolve_repo(repo: Option<&Path>, registry: &Registry) -> HzResult<PathBuf> {
    let current = hz_git::repository_root(repo)?;
    if let Some(entry) = registry.find_by_path(&current) {
        return Ok(entry.repo.clone());
    }

    Ok(current)
}

fn resolve_target(registry: &Registry, repo: &Path, target: &str) -> HzResult<WorktreeTarget> {
    if target == "local" {
        return Ok(WorktreeTarget {
            name: "local".to_owned(),
            path: hz_git::main_worktree(repo)?,
        });
    }

    let entry = find_entry(registry, repo, target)?;
    Ok(WorktreeTarget {
        name: entry.handle,
        path: entry.path,
    })
}

fn find_entry(registry: &Registry, repo: &Path, target: &str) -> HzResult<WorktreeEntry> {
    if let Some(entry) = registry.find(repo, target) {
        return Ok(entry.clone());
    }

    find_git_entry(repo, target)
}

fn find_git_entry(repo: &Path, target: &str) -> HzResult<WorktreeEntry> {
    hz_git::list_worktrees(repo)?
        .into_iter()
        .filter(|worktree| !same_path(&worktree.path, repo))
        .map(|worktree| git_entry(repo, worktree))
        .find(|entry| matches_target(entry, target))
        .ok_or_else(|| HzError::Usage(format!("unknown worktree: {target}")))
}

fn add_git_worktrees(
    entries: &mut Vec<WorktreeEntry>,
    repo: &Path,
    worktrees: Vec<hz_git::GitWorktree>,
) {
    for worktree in worktrees {
        if same_path(&worktree.path, repo)
            || entries
                .iter()
                .any(|entry| same_path(&entry.path, &worktree.path))
        {
            continue;
        }

        entries.push(git_entry(repo, worktree));
    }
}

fn git_entry(repo: &Path, worktree: hz_git::GitWorktree) -> WorktreeEntry {
    let handle = git_worktree_handle(repo, &worktree);

    WorktreeEntry {
        id: handle.clone(),
        handle,
        repo: repo.to_path_buf(),
        path: worktree.path,
        branch: worktree.branch,
        base: None,
        source: WorktreeSource::Git,
        created_at_unix: 0,
    }
}

fn git_worktree_handle(repo: &Path, worktree: &hz_git::GitWorktree) -> String {
    let path_name = worktree
        .path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned());
    let repo_name = repo
        .file_name()
        .map(|name| name.to_string_lossy().into_owned());

    if path_name.is_some()
        && path_name == repo_name
        && let Some(parent_name) = worktree
            .path
            .parent()
            .and_then(|parent| parent.file_name())
            .map(|name| name.to_string_lossy().into_owned())
    {
        return parent_name;
    }

    path_name.unwrap_or_else(|| worktree.path.display().to_string())
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

        let contents = fs::read_to_string(&path)?;
        serde_json::from_str(&contents).map_err(|error| {
            HzError::Usage(format!(
                "failed to parse registry {}: {error}",
                path.display()
            ))
        })
    }

    fn save(&self) -> HzResult<()> {
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

fn validate_worktree_name(label: &str, name: &str) -> HzResult<()> {
    if name == "local" {
        return Err(HzError::Usage(format!(
            "{label} 'local' is reserved for the repository root"
        )));
    }

    Ok(())
}

fn resolve_worktree_path(repo: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        repo.join(path)
    }
}

fn registry_temp_path(path: &Path) -> HzResult<PathBuf> {
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
    generate_unique_handle_from_seed(registry, repo, handle_seed())
}

fn generate_unique_handle_from_seed(registry: &Registry, repo: &Path, seed: u128) -> String {
    let max_attempts = handle_space_size();

    for attempt in 0..max_attempts {
        let handle = generate_handle_from_seed(seed, attempt);
        if registry.find(repo, &handle).is_none() {
            return handle;
        }
    }

    let fallback = generate_handle_from_seed(seed, max_attempts);
    for suffix in 2.. {
        let handle = format!("{fallback}-{suffix}");
        if registry.find(repo, &handle).is_none() {
            return handle;
        }
    }

    unreachable!("suffix search is unbounded")
}

const HANDLE_ADJECTIVES: &[&str] = &[
    "abelian",
    "analytic",
    "archimedean",
    "boolean",
    "cartesian",
    "computable",
    "differential",
    "euclidean",
    "gaussian",
    "geometric",
    "godelian",
    "harmonic",
    "hilbertian",
    "logical",
    "modular",
    "newtonian",
    "noetherian",
    "pythagorean",
    "recursive",
    "topological",
];

const HANDLE_NOUNS: &[&str] = &[
    "algorithm",
    "alpha",
    "axiom",
    "beta",
    "calculus",
    "chi",
    "delta",
    "epsilon",
    "eta",
    "fractal",
    "gamma",
    "iota",
    "kappa",
    "lambda",
    "lemma",
    "matrix",
    "omega",
    "phi",
    "proof",
    "sigma",
    "tensor",
    "theorem",
    "theta",
    "topology",
    "vector",
    "zeta",
];

fn generate_handle_from_seed(seed: u128, attempt: u128) -> String {
    let offset = seed + attempt;
    let left = HANDLE_ADJECTIVES[(offset as usize) % HANDLE_ADJECTIVES.len()];
    let right =
        HANDLE_NOUNS[((offset / HANDLE_ADJECTIVES.len() as u128) as usize) % HANDLE_NOUNS.len()];

    format!("{left}-{right}")
}

fn handle_seed() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default()
}

fn handle_space_size() -> u128 {
    (HANDLE_ADJECTIVES.len() * HANDLE_NOUNS.len()) as u128
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
        let handle = generate_handle_from_seed(0, 0);

        assert!(handle.contains('-'));
        assert!(
            handle
                .chars()
                .all(|character| { character.is_ascii_lowercase() || character == '-' })
        );
    }

    #[test]
    fn generated_handle_uses_adjective_noun_parts() {
        let handle = generate_handle_from_seed(0, 0);
        let (left, right) = handle
            .split_once('-')
            .expect("generated handle should have two parts");

        assert!(HANDLE_ADJECTIVES.contains(&left));
        assert!(HANDLE_NOUNS.contains(&right));
    }

    #[test]
    fn generated_unique_handle_searches_past_old_probe_window() {
        let repo = PathBuf::from("/repo");
        let seed = 0;
        let mut registry = Registry::default();

        for attempt in 0..128 {
            registry
                .entries
                .push(test_entry(&repo, generate_handle_from_seed(seed, attempt)));
        }

        assert_eq!(
            generate_unique_handle_from_seed(&registry, &repo, seed),
            generate_handle_from_seed(seed, 128)
        );
    }

    #[test]
    fn generated_unique_handle_uses_suffix_after_name_space_is_full() {
        let repo = PathBuf::from("/repo");
        let seed = 0;
        let mut registry = Registry::default();

        for attempt in 0..handle_space_size() {
            registry
                .entries
                .push(test_entry(&repo, generate_handle_from_seed(seed, attempt)));
        }

        assert_eq!(
            generate_unique_handle_from_seed(&registry, &repo, seed),
            "abelian-algorithm-2"
        );
    }

    #[test]
    fn local_is_reserved_for_repository_root() {
        assert!(validate_worktree_name("worktree handle", "feature").is_ok());

        let error = validate_worktree_name("worktree handle", "local").unwrap_err();
        assert_eq!(
            error.to_string(),
            "worktree handle 'local' is reserved for the repository root"
        );
    }

    #[test]
    fn relative_worktree_paths_are_resolved_from_repo_root() {
        let repo = PathBuf::from("/repo");

        assert_eq!(
            resolve_worktree_path(&repo, PathBuf::from("../worktree")),
            PathBuf::from("/repo/../worktree")
        );
        assert_eq!(
            resolve_worktree_path(&repo, PathBuf::from("/tmp/worktree")),
            PathBuf::from("/tmp/worktree")
        );
    }

    #[test]
    fn registry_temp_paths_are_unique_and_adjacent() {
        let registry = PathBuf::from("/config/hz/registry.json");
        let first = registry_temp_path(&registry).unwrap();
        let second = registry_temp_path(&registry).unwrap();

        assert_ne!(first, second);
        assert_eq!(first.parent(), registry.parent());
        assert!(
            first
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".registry.json.")
        );
    }

    #[test]
    fn git_worktree_handle_uses_parent_when_path_leaf_is_repo_name() {
        let handle = git_worktree_handle(
            &PathBuf::from("/repo/hz"),
            &hz_git::GitWorktree {
                path: PathBuf::from("/Users/dev/.codex/worktrees/bd16/hz"),
                branch: None,
            },
        );

        assert_eq!(handle, "bd16");
    }

    #[test]
    fn git_worktree_handle_uses_path_leaf_when_it_is_specific() {
        let handle = git_worktree_handle(
            &PathBuf::from("/repo/hz"),
            &hz_git::GitWorktree {
                path: PathBuf::from("/repo/hz-feature"),
                branch: None,
            },
        );

        assert_eq!(handle, "hz-feature");
    }

    #[test]
    fn git_worktree_handle_keeps_tool_directory_when_branch_exists() {
        let entry = git_entry(
            &PathBuf::from("/repo/hz"),
            hz_git::GitWorktree {
                path: PathBuf::from("/Users/dev/.codex/worktrees/bd16/hz"),
                branch: Some("feature/list".to_owned()),
            },
        );

        assert_eq!(entry.handle, "bd16");
        assert_eq!(entry.branch.as_deref(), Some("feature/list"));
        assert!(matches_target(&entry, "bd16"));
        assert!(matches_target(&entry, "feature/list"));
    }

    #[test]
    fn git_worktree_merge_skips_current_repo_and_registered_paths() {
        let repo = PathBuf::from("/repo/hz");
        let mut entries = vec![WorktreeEntry {
            id: "managed-id".to_owned(),
            handle: "managed".to_owned(),
            repo: repo.clone(),
            path: PathBuf::from("/worktrees/managed"),
            branch: Some("managed".to_owned()),
            base: None,
            source: WorktreeSource::Managed,
            created_at_unix: 0,
        }];

        add_git_worktrees(
            &mut entries,
            &repo,
            vec![
                hz_git::GitWorktree {
                    path: repo.clone(),
                    branch: Some("main".to_owned()),
                },
                hz_git::GitWorktree {
                    path: PathBuf::from("/worktrees/managed"),
                    branch: Some("managed".to_owned()),
                },
                hz_git::GitWorktree {
                    path: PathBuf::from("/Users/dev/.codex/worktrees/bd16/hz"),
                    branch: None,
                },
            ],
        );

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].handle, "bd16");
        assert_eq!(entries[1].source, WorktreeSource::Git);
    }

    fn test_entry(repo: &Path, handle: String) -> WorktreeEntry {
        WorktreeEntry {
            id: handle.clone(),
            handle: handle.clone(),
            repo: repo.to_path_buf(),
            path: PathBuf::from("/worktrees").join(&handle),
            branch: Some(handle),
            base: None,
            source: WorktreeSource::Managed,
            created_at_unix: 0,
        }
    }
}
