use std::path::{Path, PathBuf};

use crate::{
    FindWorktree, FindWorktrees, ListWorktrees, LocalWorktree, LocalWorktreeInfo, Registry,
    WorktreeEntry, WorktreeStatus, add_git_worktrees, find_target_entry, linked_worktree_exists,
    resolve_repo_with_git_worktrees, same_path, worktree_path_timestamp,
};
use hz_core::{HzError, HzResult};

const MAX_STATUS_WORKERS: usize = 8;

pub fn list(input: ListWorktrees) -> HzResult<Vec<WorktreeEntry>> {
    let (_, _, mut entries) = list_targets_with_repo(input)?;
    refresh_worktree_state(&mut entries);

    Ok(entries)
}

pub fn list_targets(input: ListWorktrees) -> HzResult<Vec<WorktreeEntry>> {
    let (_, _, entries) = list_targets_with_repo(input)?;
    Ok(entries)
}

pub fn list_targets_with_repo(
    input: ListWorktrees,
) -> HzResult<(PathBuf, PathBuf, Vec<WorktreeEntry>)> {
    let registry = Registry::load()?;
    let (repo, git_worktrees) = resolve_repo_with_git_worktrees(input.repo.as_deref(), &registry)?;
    let local_path = repo.clone();
    let mut entries = discover_entries_with_git_worktrees(&registry, &repo, git_worktrees);
    filter_entries_by_pin(&mut entries, input.pinned);
    sort_worktree_entries(&mut entries);

    Ok((repo, local_path, entries))
}

pub fn list_with_local(input: ListWorktrees) -> HzResult<(LocalWorktreeInfo, Vec<WorktreeEntry>)> {
    let registry = Registry::load()?;
    let (repo, git_worktrees) = resolve_repo_with_git_worktrees(input.repo.as_deref(), &registry)?;
    let branch = git_worktrees
        .first()
        .and_then(|worktree| worktree.branch.clone());
    let mut entries = discover_entries_with_git_worktrees(&registry, &repo, git_worktrees);
    filter_entries_by_pin(&mut entries, input.pinned);
    sort_worktree_entries(&mut entries);
    refresh_worktree_state(&mut entries);
    let local = local_from_resolved(&registry, repo, branch)?;

    Ok((local, entries))
}

pub(crate) fn filter_entries_by_pin(entries: &mut Vec<WorktreeEntry>, pinned: Option<bool>) {
    if let Some(pinned) = pinned {
        entries.retain(|entry| entry.pinned == pinned);
    }
}

pub fn local(input: LocalWorktree) -> HzResult<LocalWorktreeInfo> {
    let registry = Registry::load()?;
    let (repo, git_worktrees) = resolve_repo_with_git_worktrees(input.repo.as_deref(), &registry)?;
    let branch = git_worktrees
        .first()
        .and_then(|worktree| worktree.branch.clone());

    local_from_resolved(&registry, repo, branch)
}

pub fn local_path(input: LocalWorktree) -> HzResult<(PathBuf, PathBuf)> {
    let registry = Registry::load()?;
    let (repo, _) = resolve_repo_with_git_worktrees(input.repo.as_deref(), &registry)?;

    Ok((repo.clone(), repo))
}

fn local_from_resolved(
    registry: &Registry,
    repo: PathBuf,
    branch: Option<String>,
) -> HzResult<LocalWorktreeInfo> {
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
    let handoff_from = local_handoff_from(registry, &repo, branch.as_deref())?;

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

pub(crate) fn local_handoff_from(
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

pub(crate) fn discover_entries(registry: &Registry, repo: &Path) -> HzResult<Vec<WorktreeEntry>> {
    Ok(discover_entries_with_git_worktrees(
        registry,
        repo,
        hz_git::list_worktrees(repo)?,
    ))
}

pub(crate) fn discover_entries_with_git_worktrees(
    registry: &Registry,
    repo: &Path,
    git_worktrees: Vec<hz_git::GitWorktree>,
) -> Vec<WorktreeEntry> {
    let mut entries: Vec<_> = registry
        .entries
        .iter()
        .filter(|entry| same_path(&entry.repo, repo))
        .cloned()
        .collect();

    add_git_worktrees(&mut entries, repo, git_worktrees);
    entries
}

pub(crate) fn sort_worktree_entries(entries: &mut [WorktreeEntry]) {
    entries.sort_by(|left, right| {
        right
            .created_at_unix
            .cmp(&left.created_at_unix)
            .then_with(|| left.handle.cmp(&right.handle))
    });
}

pub(crate) fn refresh_worktree_state(entries: &mut [WorktreeEntry]) {
    let states = refreshed_worktree_states(entries);
    for (entry, state) in entries.iter_mut().zip(states) {
        entry.status = state.status;
        entry.modified_at_unix = state.modified_at_unix;
    }
}

#[derive(Debug, Clone, Copy)]
struct RefreshedWorktreeState {
    status: WorktreeStatus,
    modified_at_unix: u64,
}

fn refreshed_worktree_states(entries: &[WorktreeEntry]) -> Vec<RefreshedWorktreeState> {
    let worker_count = status_worker_count(entries.len());
    if worker_count == 1 {
        return entries.iter().map(refresh_entry_state).collect();
    }

    std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(worker_count);
        for worker_index in 0..worker_count {
            handles.push(scope.spawn(move || {
                entries
                    .iter()
                    .enumerate()
                    .skip(worker_index)
                    .step_by(worker_count)
                    .map(|(index, entry)| (index, refresh_entry_state(entry)))
                    .collect::<Vec<_>>()
            }));
        }

        let mut states = vec![
            RefreshedWorktreeState {
                status: WorktreeStatus::Unknown,
                modified_at_unix: 0,
            };
            entries.len()
        ];
        for handle in handles {
            for (index, state) in handle.join().expect("worktree status worker panicked") {
                states[index] = state;
            }
        }
        states
    })
}

fn status_worker_count(entry_count: usize) -> usize {
    let available = std::thread::available_parallelism()
        .map(|parallelism| parallelism.get())
        .unwrap_or(1);
    entry_count.min(available).clamp(1, MAX_STATUS_WORKERS)
}

fn refresh_entry_state(entry: &WorktreeEntry) -> RefreshedWorktreeState {
    match hz_git::worktree_state(&entry.path) {
        Ok(state) => RefreshedWorktreeState {
            status: if state.dirty {
                WorktreeStatus::Dirty
            } else {
                WorktreeStatus::Clean
            },
            modified_at_unix: if state.modified_at_unix == 0 {
                entry.created_at_unix
            } else {
                state.modified_at_unix
            },
        },
        Err(_) => RefreshedWorktreeState {
            status: WorktreeStatus::Unknown,
            modified_at_unix: worktree_path_timestamp(&entry.path),
        },
    }
}

pub fn find(input: FindWorktree) -> HzResult<WorktreeEntry> {
    let mut entries = find_many(FindWorktrees {
        targets: vec![input.target],
        repo: input.repo,
    })?;
    Ok(entries.remove(0))
}

pub fn find_many(input: FindWorktrees) -> HzResult<Vec<WorktreeEntry>> {
    let registry = Registry::load()?;
    let (repo, git_worktrees) = resolve_repo_with_git_worktrees(input.repo.as_deref(), &registry)?;
    let entries = discover_entries_with_git_worktrees(&registry, &repo, git_worktrees);

    input
        .targets
        .iter()
        .map(|target| {
            find_target_entry(&entries, &repo, target).ok_or_else(|| HzError::UnknownWorktree {
                target: target.clone(),
            })
        })
        .collect()
}
