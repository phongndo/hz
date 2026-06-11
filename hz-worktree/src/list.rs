use std::path::{Path, PathBuf};

use crate::{
    FindWorktree, FindWorktrees, ListWorktrees, LocalWorktree, LocalWorktreeInfo, Registry,
    WorktreeEntry, WorktreeStatus, add_git_worktrees, find_target_entry, linked_worktree_exists,
    resolve_repo, resolve_repo_with_git_worktrees, same_path, worktree_path_timestamp,
};
use hz_core::{HzError, HzResult};

pub fn list(input: ListWorktrees) -> HzResult<Vec<WorktreeEntry>> {
    let mut entries = list_targets(input)?;
    refresh_worktree_state(&mut entries);

    Ok(entries)
}

pub fn list_targets(input: ListWorktrees) -> HzResult<Vec<WorktreeEntry>> {
    let registry = Registry::load()?;
    let (repo, git_worktrees) = resolve_repo_with_git_worktrees(input.repo.as_deref(), &registry)?;
    let mut entries = discover_entries_with_git_worktrees(&registry, &repo, git_worktrees);
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
