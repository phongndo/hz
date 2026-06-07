use std::path::Path;

use crate::{
    Registry, WorktreeEntry, WorktreeSource, remove_registered_entry_from_registry, same_path,
};
use hz_core::{HzError, HzResult};

pub(crate) fn detached_worktree_prune_candidates(
    registry: &Registry,
    repo: &Path,
    current_hint: Option<&Path>,
    max_detached_worktrees: usize,
) -> HzResult<Vec<WorktreeEntry>> {
    let current = hz_git::repository_root(current_hint).ok();
    let git_worktrees = hz_git::list_worktrees(repo)?;

    select_detached_worktree_prune_candidates(
        registry,
        repo,
        max_detached_worktrees,
        current.as_deref(),
        &git_worktrees,
        clean_worktree,
    )
}

pub(crate) fn select_detached_worktree_prune_candidates(
    registry: &Registry,
    repo: &Path,
    max_detached_worktrees: usize,
    current: Option<&Path>,
    git_worktrees: &[hz_git::GitWorktree],
    is_clean: impl Fn(&Path) -> bool,
) -> HzResult<Vec<WorktreeEntry>> {
    if max_detached_worktrees == 0 {
        return Ok(Vec::new());
    }

    let mut detached: Vec<_> = registry
        .entries
        .iter()
        .filter(|entry| {
            same_path(&entry.repo, repo)
                && entry.source == WorktreeSource::Managed
                && entry.branch.is_none()
                && git_worktrees.iter().any(|worktree| {
                    same_path(&worktree.path, &entry.path) && worktree.branch.is_none()
                })
        })
        .cloned()
        .collect();
    let prune_count = detached
        .len()
        .saturating_add(1)
        .saturating_sub(max_detached_worktrees);
    if prune_count == 0 {
        return Ok(Vec::new());
    }

    detached.sort_by(|left, right| {
        left.created_at_unix
            .cmp(&right.created_at_unix)
            .then_with(|| left.handle.cmp(&right.handle))
    });

    let candidates: Vec<_> = detached
        .into_iter()
        .filter(|entry| current.is_none_or(|current| !same_path(&entry.path, current)))
        .filter(|entry| is_clean(&entry.path))
        .take(prune_count)
        .collect();

    if candidates.len() < prune_count {
        return Err(HzError::Usage(format!(
            "detached worktree limit {max_detached_worktrees} would be exceeded; not enough clean detached worktrees can be auto-removed"
        )));
    }

    Ok(candidates)
}

pub(crate) fn prune_detached_worktrees(
    registry: &mut Registry,
    candidates: Vec<WorktreeEntry>,
) -> HzResult<()> {
    for entry in candidates {
        remove_registered_entry_from_registry(registry, &entry, false)?;
        registry.save()?;
    }

    Ok(())
}

pub(crate) fn detached_prune_warning(error: HzError) -> String {
    format!("created worktree, but failed to prune detached worktrees: {error}")
}

pub(crate) fn clean_worktree(path: &Path) -> bool {
    hz_git::worktree_state(path).is_ok_and(|state| !state.dirty)
}
