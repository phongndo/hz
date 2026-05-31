use std::{
    collections::HashSet,
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
    pub target: Option<String>,
    pub mode: HandoffMode,
    pub repo: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListWorktrees {
    pub repo: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalWorktree {
    pub repo: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LocalWorktreeInfo {
    pub repo: PathBuf,
    pub path: PathBuf,
    pub branch: Option<String>,
    pub status: WorktreeStatus,
    pub modified_at_unix: u64,
    pub handoff_from: Option<String>,
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
    pub branch: Option<String>,
    pub base: Option<String>,
    pub source: WorktreeSource,
}

#[derive(Debug, Serialize)]
pub struct WorktreeHandoff {
    pub repo: PathBuf,
    pub mode: HandoffMode,
    pub branch: Option<String>,
    pub from: WorktreeTarget,
    pub to: WorktreeTarget,
    pub changed: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandoffMode {
    #[default]
    Patch,
    Branch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorktreeSource {
    Managed,
    Git,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorktreeStatus {
    Clean,
    Dirty,
    #[default]
    Unknown,
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
    #[serde(default)]
    pub modified_at_unix: u64,
    #[serde(default)]
    pub status: WorktreeStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct HandoffLink {
    repo: PathBuf,
    branch: String,
    path: PathBuf,
    handle: String,
    local_restore_branch: Option<String>,
    updated_at_unix: u64,
}

pub fn create(input: CreateWorktree) -> HzResult<CreatedWorktree> {
    let mut registry = Registry::load()?;
    let repo = resolve_repo(input.repo.as_deref(), &registry)?;
    let name = input.name;
    let handle = match name.clone() {
        Some(name) => name,
        None => generate_unique_handle(&registry, &repo)?,
    };
    let branch = derive_worktree_branch(name.as_deref(), input.branch.as_deref());
    validate_worktree_name("worktree handle", &handle)?;
    if let Some(branch) = &branch {
        validate_worktree_name("worktree branch", branch)?;
    }

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

    hz_git::add_worktree(&repo, &path, branch.as_deref(), input.base.as_deref())?;
    let path = fs::canonicalize(&path)?;

    let entry = WorktreeEntry {
        id: id.clone(),
        handle: handle.clone(),
        repo: repo.clone(),
        path: path.clone(),
        branch: branch.clone(),
        base: input.base.clone(),
        source: WorktreeSource::Managed,
        created_at_unix: unix_now()?,
        modified_at_unix: 0,
        status: WorktreeStatus::Unknown,
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

fn derive_worktree_branch(name: Option<&str>, branch: Option<&str>) -> Option<String> {
    branch.or(name).map(str::to_owned)
}

pub fn path(input: PathWorktree) -> HzResult<WorktreeTarget> {
    let registry = Registry::load()?;
    let repo = resolve_repo(input.repo.as_deref(), &registry)?;
    resolve_target(&registry, &repo, &input.target)
}

pub fn handoff(input: HandoffWorktree) -> HzResult<WorktreeHandoff> {
    let mut registry = Registry::load()?;
    let current = hz_git::repository_root(input.repo.as_deref())?;
    let main = hz_git::main_worktree(&current)?;
    let repo = resolve_registered_repo(&registry, &current, &main).unwrap_or(main);

    if input.mode == HandoffMode::Patch {
        return handoff_patch(&mut registry, repo, current, input.target);
    }

    if same_path(&current, &repo) {
        let (branch, destination) =
            resolve_local_branch_handoff_target(&registry, &repo, &current, input.target)?;
        handoff_local_to_worktree(&mut registry, repo, current, branch, destination)
    } else {
        let branch = resolve_handoff_branch(input.target, &current)?;
        handoff_worktree_to_local(&mut registry, repo, current, branch)
    }
}

fn handoff_patch(
    registry: &mut Registry,
    repo: PathBuf,
    current: PathBuf,
    target: Option<String>,
) -> HzResult<WorktreeHandoff> {
    if same_path(&current, &repo) {
        handoff_patch_from_local(registry, repo, current, target)
    } else {
        handoff_patch_to_local(registry, repo, current, target)
    }
}

fn handoff_patch_to_local(
    registry: &mut Registry,
    repo: PathBuf,
    current: PathBuf,
    target: Option<String>,
) -> HzResult<WorktreeHandoff> {
    if let Some(target) = target
        && target != "local"
    {
        return Err(HzError::Usage(
            "patch handoff from a linked worktree only supports local as the destination"
                .to_owned(),
        ));
    }

    let from = target_for_path(registry, &repo, &current)?;
    let to = WorktreeTarget {
        name: "local".to_owned(),
        path: repo.clone(),
    };
    ensure_clean(&repo, "destination")?;
    let patch = hz_git::diff_patch(&current)?;
    let changed = hz_git::apply_patch(&repo, &patch)?;
    let branch = hz_git::current_branch(&current)?;

    if let Some(branch) = &branch {
        let local_restore_branch = hz_git::current_branch(&repo)?.filter(|local| local != branch);
        registry.remember_handoff(&repo, branch, &current, &from.name, local_restore_branch)?;
        registry.save()?;
    }

    Ok(WorktreeHandoff {
        repo,
        mode: HandoffMode::Patch,
        branch,
        from,
        to,
        changed,
    })
}

fn handoff_patch_from_local(
    registry: &mut Registry,
    repo: PathBuf,
    current: PathBuf,
    target: Option<String>,
) -> HzResult<WorktreeHandoff> {
    let branch = hz_git::current_branch(&current)?;
    let destination = match target {
        Some(target) => find_target_worktree(registry, &repo, &target)?
            .ok_or_else(|| HzError::Usage(format!("unknown worktree target: {target}")))?,
        None => {
            let branch = branch.clone().ok_or_else(|| {
                HzError::Usage(
                    "local worktree is detached; pass a worktree target for patch handoff"
                        .to_owned(),
                )
            })?;
            find_handoff_destination(registry, &repo, &branch)?
        }
    };
    let from = WorktreeTarget {
        name: "local".to_owned(),
        path: current.clone(),
    };
    let to = WorktreeTarget {
        name: destination.handle.clone(),
        path: destination.path.clone(),
    };
    ensure_clean(&destination.path, "destination")?;
    let patch = hz_git::diff_patch(&current)?;
    let changed = hz_git::apply_patch(&destination.path, &patch)?;

    Ok(WorktreeHandoff {
        repo,
        mode: HandoffMode::Patch,
        branch,
        from,
        to,
        changed,
    })
}

fn resolve_local_branch_handoff_target(
    registry: &Registry,
    repo: &Path,
    current: &Path,
    target: Option<String>,
) -> HzResult<(String, Option<WorktreeEntry>)> {
    let current_branch = hz_git::current_branch(current)?.ok_or_else(|| {
        HzError::Usage(
            "local worktree is detached; check out the branch before handing it off".to_owned(),
        )
    })?;

    let Some(target) = target else {
        return Ok((current_branch, None));
    };

    if let Some(destination) = find_target_worktree(registry, repo, &target)? {
        return Ok((current_branch, Some(destination)));
    }

    validate_handoff_branch_name(&target)?;
    Ok((target, None))
}

fn resolve_handoff_branch(branch: Option<String>, current: &Path) -> HzResult<String> {
    let branch = match branch {
        Some(branch) => branch,
        None => hz_git::current_branch(current)?.ok_or_else(|| {
            HzError::Usage("current worktree is detached; pass the branch to hand off".to_owned())
        })?,
    };

    validate_handoff_branch_name(&branch)?;
    Ok(branch)
}

fn validate_handoff_branch_name(branch: &str) -> HzResult<()> {
    validate_worktree_name("handoff branch", branch)?;
    if branch.is_empty() {
        return Err(HzError::Usage("handoff branch cannot be empty".to_owned()));
    }

    Ok(())
}

fn handoff_worktree_to_local(
    registry: &mut Registry,
    repo: PathBuf,
    current: PathBuf,
    branch: String,
) -> HzResult<WorktreeHandoff> {
    let from = target_for_path(registry, &repo, &current)?;
    let to = WorktreeTarget {
        name: "local".to_owned(),
        path: repo.clone(),
    };
    let source_branch = hz_git::current_branch(&current)?;
    validate_handoff_source_branch(&current, source_branch.as_deref(), &branch)?;
    ensure_branch_exists(&repo, &branch)?;
    ensure_clean(&current, "source")?;
    ensure_clean(&repo, "destination")?;

    let local_restore_branch = hz_git::current_branch(&repo)?.filter(|local| local != &branch);
    let detached_source = source_branch.as_deref() == Some(&branch);
    if detached_source {
        hz_git::switch_detached(&current)?;
    }
    if hz_git::current_branch(&repo)?.as_deref() != Some(&branch)
        && let Err(error) = hz_git::switch_branch(&repo, &branch)
    {
        if detached_source {
            let rollback = hz_git::switch_branch(&current, &branch);
            return Err(match rollback {
                Ok(()) => error,
                Err(rollback_error) => HzError::Usage(format!(
                    "{error}; rollback failed, source worktree is not on {branch}: {rollback_error}"
                )),
            });
        }
        return Err(error);
    }

    registry.remember_handoff(&repo, &branch, &current, &from.name, local_restore_branch)?;
    registry.save()?;

    Ok(WorktreeHandoff {
        repo,
        mode: HandoffMode::Branch,
        branch: Some(branch),
        from,
        to,
        changed: true,
    })
}

fn handoff_local_to_worktree(
    registry: &mut Registry,
    repo: PathBuf,
    current: PathBuf,
    branch: String,
    destination: Option<WorktreeEntry>,
) -> HzResult<WorktreeHandoff> {
    let source_branch = hz_git::current_branch(&current)?;
    validate_handoff_source_branch(&current, source_branch.as_deref(), &branch)?;
    if source_branch.is_none() {
        return Err(HzError::Usage(
            "local worktree is detached; check out the branch before handing it off".to_owned(),
        ));
    }
    ensure_branch_exists(&repo, &branch)?;
    let destination = match destination {
        Some(destination) => destination,
        None => find_handoff_destination(registry, &repo, &branch)?,
    };
    let from = WorktreeTarget {
        name: "local".to_owned(),
        path: current.clone(),
    };
    let to = WorktreeTarget {
        name: destination.handle.clone(),
        path: destination.path.clone(),
    };
    ensure_clean(&current, "source")?;
    ensure_clean(&destination.path, "destination")?;

    if let Some(restore_branch) = registry
        .handoff_link(&repo, &branch)
        .and_then(|link| link.local_restore_branch.as_deref())
        .filter(|restore_branch| *restore_branch != branch.as_str())
    {
        hz_git::switch_branch(&current, restore_branch)?;
    } else {
        hz_git::switch_detached(&current)?;
    }

    if let Err(error) = hz_git::switch_branch(&destination.path, &branch) {
        let rollback = hz_git::switch_branch(&current, &branch);
        return Err(match rollback {
            Ok(()) => error,
            Err(rollback_error) => HzError::Usage(format!(
                "{error}; rollback failed, local worktree is not on {branch}: {rollback_error}"
            )),
        });
    }

    registry.forget_handoff(&repo, &branch);
    registry.save()?;

    Ok(WorktreeHandoff {
        repo,
        mode: HandoffMode::Branch,
        branch: Some(branch),
        from,
        to,
        changed: true,
    })
}

fn find_target_worktree(
    registry: &Registry,
    repo: &Path,
    target: &str,
) -> HzResult<Option<WorktreeEntry>> {
    Ok(find_target_entry(
        discover_entries(registry, repo)?,
        repo,
        target,
    ))
}

fn find_target_entry(
    entries: Vec<WorktreeEntry>,
    repo: &Path,
    target: &str,
) -> Option<WorktreeEntry> {
    entries
        .into_iter()
        .find(|entry| !same_path(&entry.path, repo) && matches_target(entry, target))
}

fn validate_handoff_source_branch(
    current: &Path,
    source_branch: Option<&str>,
    branch: &str,
) -> HzResult<()> {
    if let Some(source_branch) = source_branch
        && source_branch != branch
    {
        return Err(HzError::Usage(format!(
            "{} is on branch {source_branch}, not {branch}",
            current.display()
        )));
    }

    Ok(())
}

fn ensure_branch_exists(repo: &Path, branch: &str) -> HzResult<()> {
    if hz_git::branch_exists(repo, branch)? {
        Ok(())
    } else {
        Err(HzError::Usage(format!("unknown branch: {branch}")))
    }
}

fn ensure_clean(path: &Path, label: &str) -> HzResult<()> {
    let state = hz_git::worktree_state(path)?;
    if state.dirty {
        return Err(HzError::Usage(format!(
            "{label} worktree has uncommitted changes: {}",
            path.display()
        )));
    }

    Ok(())
}

fn target_for_path(registry: &Registry, repo: &Path, path: &Path) -> HzResult<WorktreeTarget> {
    if same_path(path, repo) {
        return Ok(WorktreeTarget {
            name: "local".to_owned(),
            path: repo.to_path_buf(),
        });
    }

    discover_entries(registry, repo)?
        .into_iter()
        .find(|entry| same_path(&entry.path, path))
        .map(|entry| WorktreeTarget {
            name: entry.handle,
            path: entry.path,
        })
        .ok_or_else(|| {
            HzError::Usage(format!(
                "current worktree is not linked to {}",
                repo.display()
            ))
        })
}

fn find_handoff_destination(
    registry: &Registry,
    repo: &Path,
    branch: &str,
) -> HzResult<WorktreeEntry> {
    let entries = discover_entries(registry, repo)?;
    if let Some(entry) = entries
        .iter()
        .find(|entry| entry.branch.as_deref() == Some(branch) && !same_path(&entry.path, repo))
    {
        return Ok(entry.clone());
    }

    if let Some(link) = registry.handoff_link(repo, branch)
        && linked_worktree_exists(repo, &link.path)?
    {
        return Ok(WorktreeEntry {
            id: link.handle.clone(),
            handle: link.handle.clone(),
            repo: repo.to_path_buf(),
            path: link.path.clone(),
            branch: Some(branch.to_owned()),
            base: None,
            source: WorktreeSource::Git,
            created_at_unix: link.updated_at_unix,
            modified_at_unix: link.updated_at_unix,
            status: WorktreeStatus::Unknown,
        });
    }

    Err(HzError::Usage(format!(
        "no linked worktree found for branch: {branch}"
    )))
}

fn linked_worktree_exists(repo: &Path, path: &Path) -> HzResult<bool> {
    Ok(hz_git::list_worktrees(repo)?
        .into_iter()
        .any(|worktree| same_path(&worktree.path, path)))
}

pub fn list(input: ListWorktrees) -> HzResult<Vec<WorktreeEntry>> {
    let registry = Registry::load()?;
    let repo = resolve_repo(input.repo.as_deref(), &registry)?;
    let mut entries = discover_entries(&registry, &repo)?;
    refresh_worktree_state(&mut entries);

    sort_worktree_entries(&mut entries);
    Ok(entries)
}

pub fn local(input: LocalWorktree) -> HzResult<LocalWorktreeInfo> {
    let registry = Registry::load()?;
    let repo = resolve_repo(input.repo.as_deref(), &registry)?;
    let branch = hz_git::current_branch(&repo)?;
    let state = hz_git::worktree_state(&repo)?;
    let status = if state.dirty {
        WorktreeStatus::Dirty
    } else {
        WorktreeStatus::Clean
    };
    let modified_at_unix = if state.modified_at_unix == 0 {
        worktree_path_timestamp(&repo)
    } else {
        state.modified_at_unix
    };
    let handoff_from = local_handoff_from(&registry, &repo, branch.as_deref())?;

    Ok(LocalWorktreeInfo {
        repo: repo.clone(),
        path: repo,
        branch,
        status,
        modified_at_unix,
        handoff_from,
    })
}

pub fn current_path(_input: ListWorktrees) -> HzResult<PathBuf> {
    hz_git::repository_root(None)
}

fn local_handoff_from(
    registry: &Registry,
    repo: &Path,
    branch: Option<&str>,
) -> HzResult<Option<String>> {
    let Some(branch) = branch else {
        return Ok(None);
    };
    let Some(link) = registry.handoff_link(repo, branch) else {
        return Ok(None);
    };
    if linked_worktree_exists(repo, &link.path)? {
        Ok(Some(link.handle.clone()))
    } else {
        Ok(None)
    }
}

fn discover_entries(registry: &Registry, repo: &Path) -> HzResult<Vec<WorktreeEntry>> {
    let mut entries: Vec<_> = registry
        .entries
        .iter()
        .filter(|entry| same_path(&entry.repo, repo))
        .cloned()
        .collect();

    add_git_worktrees(&mut entries, repo, hz_git::list_worktrees(repo)?);
    Ok(entries)
}

fn sort_worktree_entries(entries: &mut [WorktreeEntry]) {
    entries.sort_by(|left, right| {
        right
            .created_at_unix
            .cmp(&left.created_at_unix)
            .then_with(|| left.handle.cmp(&right.handle))
    });
}

fn refresh_worktree_state(entries: &mut [WorktreeEntry]) {
    for entry in entries {
        match hz_git::worktree_state(&entry.path) {
            Ok(state) => {
                entry.status = if state.dirty {
                    WorktreeStatus::Dirty
                } else {
                    WorktreeStatus::Clean
                };
                entry.modified_at_unix = if state.modified_at_unix == 0 {
                    entry.created_at_unix
                } else {
                    state.modified_at_unix
                };
            }
            Err(_) => {
                entry.status = WorktreeStatus::Unknown;
                entry.modified_at_unix = worktree_path_timestamp(&entry.path);
            }
        }
    }
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

    Err(HzError::Usage(format!(
        "unknown worktree: {}",
        input.target
    )))
}

pub fn remove_found(entry: WorktreeEntry) -> HzResult<WorktreeEntry> {
    remove_found_with_force(entry, false)
}

pub fn remove_found_with_force(entry: WorktreeEntry, force: bool) -> HzResult<WorktreeEntry> {
    match entry.source {
        WorktreeSource::Managed => remove_registered_entry_with_force(entry, force),
        WorktreeSource::Git => {
            hz_git::remove_worktree_with_force(&entry.repo, &entry.path, force)?;
            Ok(entry)
        }
    }
}

fn remove_registered_entry_with_force(
    entry: WorktreeEntry,
    force: bool,
) -> HzResult<WorktreeEntry> {
    let mut registry = Registry::load()?;
    let index = registry
        .entries
        .iter()
        .position(|registered| {
            same_path(&registered.repo, &entry.repo)
                && registered.id == entry.id
                && same_path(&registered.path, &entry.path)
        })
        .ok_or_else(|| HzError::Usage(format!("unknown worktree: {}", entry.handle)))?;
    let entry = registry.entries.remove(index);

    hz_git::remove_worktree_with_force(&entry.repo, &entry.path, force)?;
    registry.save()?;

    Ok(entry)
}

fn resolve_repo(repo: Option<&Path>, registry: &Registry) -> HzResult<PathBuf> {
    let current = hz_git::repository_root(repo)?;
    let main = hz_git::main_worktree(&current)?;
    Ok(resolve_registered_repo(registry, &current, &main).unwrap_or(main))
}

fn resolve_registered_repo(registry: &Registry, current: &Path, main: &Path) -> Option<PathBuf> {
    registry
        .find_by_path(current)
        .or_else(|| registry.find_by_repo(main))
        .map(|entry| entry.repo.clone())
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
    find_target_worktree(registry, repo, target)?
        .ok_or_else(|| HzError::Usage(format!("unknown worktree: {target}")))
}

fn add_git_worktrees(
    entries: &mut Vec<WorktreeEntry>,
    repo: &Path,
    worktrees: Vec<hz_git::GitWorktree>,
) {
    for (index, worktree) in worktrees.into_iter().enumerate() {
        if index == 0 || same_path(&worktree.path, repo) {
            continue;
        }

        if let Some(entry) = entries
            .iter_mut()
            .find(|entry| same_path(&entry.path, &worktree.path))
        {
            entry.branch = worktree.branch;
            continue;
        }

        entries.push(git_entry(repo, worktree));
    }
}

fn git_entry(repo: &Path, worktree: hz_git::GitWorktree) -> WorktreeEntry {
    let handle = git_worktree_handle(repo, &worktree);
    let created_at_unix = worktree_path_timestamp(&worktree.path);

    WorktreeEntry {
        id: handle.clone(),
        handle,
        repo: repo.to_path_buf(),
        path: worktree.path,
        branch: worktree.branch,
        base: None,
        source: WorktreeSource::Git,
        created_at_unix,
        modified_at_unix: created_at_unix,
        status: WorktreeStatus::Unknown,
    }
}

fn worktree_path_timestamp(path: &Path) -> u64 {
    path.metadata()
        .ok()
        .and_then(|metadata| metadata.created().or_else(|_| metadata.modified()).ok())
        .and_then(|timestamp| timestamp.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |duration| duration.as_secs())
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
    #[serde(default)]
    handoffs: Vec<HandoffLink>,
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

    fn find_by_repo(&self, repo: &Path) -> Option<&WorktreeEntry> {
        self.entries
            .iter()
            .find(|entry| same_path(&entry.repo, repo))
    }

    fn handoff_link(&self, repo: &Path, branch: &str) -> Option<&HandoffLink> {
        self.handoffs
            .iter()
            .find(|link| same_path(&link.repo, repo) && link.branch == branch)
    }

    fn remember_handoff(
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

    fn forget_handoff(&mut self, repo: &Path, branch: &str) {
        self.handoffs
            .retain(|link| !same_path(&link.repo, repo) || link.branch != branch);
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

fn generate_unique_handle(registry: &Registry, repo: &Path) -> HzResult<String> {
    let targets = discover_entries(registry, repo)?
        .iter()
        .flat_map(worktree_targets)
        .collect();
    Ok(generate_unique_handle_from_seed_with_targets(
        handle_seed(),
        handle_space_size(),
        &targets,
    ))
}

#[cfg(test)]
fn generate_unique_handle_from_seed(registry: &Registry, repo: &Path, seed: u128) -> String {
    generate_unique_handle_from_seed_with_limit(registry, repo, seed, handle_space_size())
}

#[cfg(test)]
fn generate_unique_handle_from_seed_with_limit(
    registry: &Registry,
    repo: &Path,
    seed: u128,
    max_attempts: u128,
) -> String {
    let targets = registry
        .entries
        .iter()
        .filter(|entry| same_path(&entry.repo, repo))
        .flat_map(worktree_targets)
        .collect();
    generate_unique_handle_from_seed_with_targets(seed, max_attempts, &targets)
}

fn generate_unique_handle_from_seed_with_targets(
    seed: u128,
    max_attempts: u128,
    targets: &HashSet<String>,
) -> String {
    for attempt in 0..max_attempts {
        let handle = generate_handle_from_seed(seed, attempt);
        if !targets.contains(&handle) {
            return handle;
        }
    }

    let fallback = generate_handle_from_seed(seed, max_attempts);
    for suffix in 2.. {
        let handle = format!("{fallback}-{suffix}");
        if !targets.contains(&handle) {
            return handle;
        }
    }

    unreachable!("suffix search is unbounded")
}

fn worktree_targets(entry: &WorktreeEntry) -> Vec<String> {
    let mut targets = vec![entry.id.clone(), entry.handle.clone()];
    if let Some(branch) = &entry.branch {
        targets.push(branch.clone());
    }
    targets
}

const HANDLE_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
const HANDLE_LENGTH: usize = 4;

fn generate_handle_from_seed(seed: u128, attempt: u128) -> String {
    let mut offset = mixed_handle_offset(seed).wrapping_add(attempt) % handle_space_size();
    let mut handle = [0_u8; HANDLE_LENGTH];

    for character in handle.iter_mut().rev() {
        *character = HANDLE_ALPHABET[(offset % HANDLE_ALPHABET.len() as u128) as usize];
        offset /= HANDLE_ALPHABET.len() as u128;
    }

    String::from_utf8(handle.to_vec()).expect("handle alphabet should be valid UTF-8")
}

fn handle_seed() -> u128 {
    let mut bytes = [0_u8; 16];
    if fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .is_ok()
    {
        return u128::from_le_bytes(bytes);
    }

    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default()
}

fn mixed_handle_offset(seed: u128) -> u128 {
    let mut value = seed;
    value ^= seed / HANDLE_ALPHABET.len() as u128;
    value ^= seed >> 32;
    value ^= seed >> 64;
    value
}

fn handle_space_size() -> u128 {
    (HANDLE_ALPHABET.len() as u128).pow(HANDLE_LENGTH as u32)
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

        assert_eq!(handle.len(), 4);
        assert!(
            handle
                .chars()
                .all(|character| { character.is_ascii_lowercase() || character.is_ascii_digit() })
        );
    }

    #[test]
    fn generated_handle_space_is_four_lowercase_alphanumeric_characters() {
        assert_eq!(HANDLE_ALPHABET.len(), 36);
        assert_eq!(HANDLE_LENGTH, 4);
        assert_eq!(handle_space_size(), 1_679_616);

        assert_eq!(generate_handle_from_seed(0, 0), "aaaa");
        assert_eq!(generate_handle_from_seed(0, 35), "aaa9");
        assert_eq!(generate_handle_from_seed(0, 36), "aaba");
    }

    #[test]
    fn generated_handle_mixes_timestamp_shaped_seeds() {
        let base = 1_700_000_000_000_000_000_u128;
        let mut handles = (0..8)
            .map(|index| generate_handle_from_seed(base + index * 1_000_000_000, 0))
            .collect::<Vec<_>>();
        handles.sort();
        handles.dedup();

        assert!(
            handles.len() > 1,
            "timestamp-shaped seeds should not collapse to one handle"
        );
    }

    #[test]
    fn worktree_branch_derivation_keeps_only_unnamed_worktrees_detached() {
        assert_eq!(derive_worktree_branch(None, None), None);
        assert_eq!(
            derive_worktree_branch(Some("feature/ui"), None).as_deref(),
            Some("feature/ui")
        );
        assert_eq!(
            derive_worktree_branch(None, Some("feature/explicit")).as_deref(),
            Some("feature/explicit")
        );
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

        let max_attempts = 3;
        for attempt in 0..max_attempts {
            registry
                .entries
                .push(test_entry(&repo, generate_handle_from_seed(seed, attempt)));
        }

        assert_eq!(
            generate_unique_handle_from_seed_with_limit(&registry, &repo, seed, max_attempts),
            format!("{}-2", generate_handle_from_seed(seed, max_attempts))
        );
    }

    #[test]
    fn generated_unique_handle_skips_live_worktree_targets() {
        let seed = 0;
        let targets = HashSet::from([generate_handle_from_seed(seed, 0)]);

        assert_eq!(
            generate_unique_handle_from_seed_with_targets(seed, handle_space_size(), &targets),
            generate_handle_from_seed(seed, 1)
        );
    }

    #[test]
    fn generated_unique_handle_suffix_skips_live_worktree_targets() {
        let seed = 0;
        let max_attempts = 1;
        let fallback = generate_handle_from_seed(seed, max_attempts);
        let targets = HashSet::from([generate_handle_from_seed(seed, 0), format!("{fallback}-2")]);

        assert_eq!(
            generate_unique_handle_from_seed_with_targets(seed, max_attempts, &targets),
            format!("{fallback}-3")
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
    fn repo_resolution_uses_registered_repo_for_managed_linked_worktree() {
        let repo = PathBuf::from("/repo/hz");
        let linked = PathBuf::from("/worktrees/managed");
        let registry = Registry {
            entries: vec![WorktreeEntry {
                id: "managed-id".to_owned(),
                handle: "managed".to_owned(),
                repo: repo.clone(),
                path: linked.clone(),
                branch: Some("managed".to_owned()),
                base: None,
                source: WorktreeSource::Managed,
                created_at_unix: 0,
                modified_at_unix: 0,
                status: WorktreeStatus::Unknown,
            }],
            handoffs: Vec::new(),
        };

        assert_eq!(
            resolve_registered_repo(&registry, &linked, &repo),
            Some(repo)
        );
    }

    #[test]
    fn repo_resolution_uses_registered_primary_for_unmanaged_linked_worktree() {
        let repo = PathBuf::from("/repo/hz");
        let unmanaged = PathBuf::from("/worktrees/unmanaged");
        let registry = Registry {
            entries: vec![test_entry(&repo, "managed".to_owned())],
            handoffs: Vec::new(),
        };

        assert_eq!(
            resolve_registered_repo(&registry, &unmanaged, &repo),
            Some(repo)
        );
    }

    #[test]
    fn repo_resolution_falls_back_when_registry_has_no_repo_match() {
        let repo = PathBuf::from("/repo/hz");
        let other_repo = PathBuf::from("/repo/other");
        let unmanaged = PathBuf::from("/worktrees/unmanaged");
        let registry = Registry {
            entries: vec![test_entry(&other_repo, "managed".to_owned())],
            handoffs: Vec::new(),
        };

        assert_eq!(resolve_registered_repo(&registry, &unmanaged, &repo), None);
    }

    #[test]
    fn registry_entries_default_added_state_fields() {
        let registry: Registry = serde_json::from_str(
            r#"{
              "entries": [
                {
                  "id": "managed-id",
                  "handle": "managed",
                  "repo": "/repo/hz",
                  "path": "/worktrees/managed",
                  "branch": "managed",
                  "base": null,
                  "source": "managed",
                  "created_at_unix": 42
                }
              ]
            }"#,
        )
        .unwrap();

        assert_eq!(registry.entries[0].modified_at_unix, 0);
        assert_eq!(registry.entries[0].status, WorktreeStatus::Unknown);
        assert!(registry.handoffs.is_empty());
    }

    #[test]
    fn registry_remembers_one_handoff_per_branch() {
        let repo = PathBuf::from("/repo/hz");
        let first = PathBuf::from("/worktrees/first");
        let second = PathBuf::from("/worktrees/second");
        let mut registry = Registry::default();

        registry
            .remember_handoff(
                &repo,
                "feature/ui",
                &first,
                "first",
                Some("main".to_owned()),
            )
            .unwrap();
        registry
            .remember_handoff(&repo, "feature/ui", &second, "second", None)
            .unwrap();

        let link = registry.handoff_link(&repo, "feature/ui").unwrap();
        assert_eq!(link.path, second);
        assert_eq!(link.handle, "second");
        assert_eq!(registry.handoffs.len(), 1);
    }

    #[test]
    fn handoff_source_branch_must_match_requested_branch() {
        let error = validate_handoff_source_branch(
            &PathBuf::from("/worktrees/current"),
            Some("feature/other"),
            "feature/ui",
        )
        .unwrap_err();

        assert_eq!(
            error.to_string(),
            "/worktrees/current is on branch feature/other, not feature/ui"
        );
        assert!(
            validate_handoff_source_branch(
                &PathBuf::from("/worktrees/current"),
                Some("feature/ui"),
                "feature/ui"
            )
            .is_ok()
        );
        assert!(
            validate_handoff_source_branch(
                &PathBuf::from("/worktrees/current"),
                None,
                "feature/ui"
            )
            .is_ok()
        );
    }

    #[test]
    fn local_handoff_target_can_match_detached_codex_worktree_handle() {
        let repo = PathBuf::from("/repo/hz");
        let entry = git_entry(
            &repo,
            hz_git::GitWorktree {
                path: PathBuf::from("/Users/dev/.codex/worktrees/708e/hz"),
                branch: None,
            },
        );

        let destination = find_target_entry(vec![entry], &repo, "708e").unwrap();

        assert_eq!(destination.handle, "708e");
        assert_eq!(destination.branch, None);
        assert_eq!(
            destination.path,
            PathBuf::from("/Users/dev/.codex/worktrees/708e/hz")
        );
    }

    #[test]
    fn worktree_entries_sort_newest_first_with_handle_tiebreaker() {
        let repo = PathBuf::from("/repo/hz");
        let mut entries = vec![
            WorktreeEntry {
                created_at_unix: 20,
                modified_at_unix: 0,
                status: WorktreeStatus::Unknown,
                ..test_entry(&repo, "zeta".to_owned())
            },
            WorktreeEntry {
                created_at_unix: 30,
                modified_at_unix: 0,
                status: WorktreeStatus::Unknown,
                ..test_entry(&repo, "beta".to_owned())
            },
            WorktreeEntry {
                created_at_unix: 30,
                modified_at_unix: 0,
                status: WorktreeStatus::Unknown,
                ..test_entry(&repo, "alpha".to_owned())
            },
        ];

        sort_worktree_entries(&mut entries);

        let handles: Vec<_> = entries.iter().map(|entry| entry.handle.as_str()).collect();
        assert_eq!(handles, vec!["alpha", "beta", "zeta"]);
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
    fn git_worktree_merge_refreshes_registered_branch_and_skips_registered_path() {
        let repo = PathBuf::from("/repo/hz");
        let mut entries = vec![WorktreeEntry {
            id: "managed-id".to_owned(),
            handle: "managed".to_owned(),
            repo: repo.clone(),
            path: PathBuf::from("/worktrees/managed"),
            branch: None,
            base: None,
            source: WorktreeSource::Managed,
            created_at_unix: 0,
            modified_at_unix: 0,
            status: WorktreeStatus::Unknown,
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
                    branch: Some("helloworld".to_owned()),
                },
                hz_git::GitWorktree {
                    path: PathBuf::from("/Users/dev/.codex/worktrees/bd16/hz"),
                    branch: None,
                },
            ],
        );

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].branch.as_deref(), Some("helloworld"));
        assert_eq!(entries[1].handle, "bd16");
        assert_eq!(entries[1].source, WorktreeSource::Git);

        let entry = find_target_entry(entries, &repo, "helloworld").unwrap();
        assert_eq!(entry.handle, "managed");
        assert_eq!(entry.source, WorktreeSource::Managed);
    }

    #[test]
    fn git_worktree_merge_skips_primary_when_repo_is_linked_worktree() {
        let repo = PathBuf::from("/Users/dev/.codex/worktrees/current/hz");
        let mut entries = Vec::new();

        add_git_worktrees(
            &mut entries,
            &repo,
            vec![
                hz_git::GitWorktree {
                    path: PathBuf::from("/repo/hz"),
                    branch: Some("main".to_owned()),
                },
                hz_git::GitWorktree {
                    path: repo.clone(),
                    branch: Some("feature/current".to_owned()),
                },
                hz_git::GitWorktree {
                    path: PathBuf::from("/Users/dev/.codex/worktrees/other/hz"),
                    branch: Some("feature/other".to_owned()),
                },
            ],
        );

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].branch.as_deref(), Some("feature/other"));
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
            modified_at_unix: 0,
            status: WorktreeStatus::Unknown,
        }
    }
}
