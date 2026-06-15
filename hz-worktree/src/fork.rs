use std::path::Path;

use crate::{
    CreateWorktree, ForkWorktree, ForkedWorktree, Registry, WorktreeEntry, WorktreeSource,
    WorktreeStatus, create_with_registry_and_deferred_prune,
    remove_registered_entry_with_force_from_registry, resolve_repo,
};
use hz_core::{HzError, HzResult};

pub fn fork(input: ForkWorktree) -> HzResult<ForkedWorktree> {
    let mut registry = Registry::load_for_update()?;
    fork_with_registry(&mut registry, input)
}

pub(crate) fn fork_with_registry(
    registry: &mut Registry,
    input: ForkWorktree,
) -> HzResult<ForkedWorktree> {
    fork_with_registry_and_patch_applier(registry, input, hz_git::apply_patch)
}

pub(crate) fn fork_with_registry_and_patch_applier(
    registry: &mut Registry,
    input: ForkWorktree,
    apply_patch: impl FnOnce(&Path, &[u8]) -> HzResult<bool>,
) -> HzResult<ForkedWorktree> {
    let current = hz_git::repository_root(input.repo.as_deref())?;
    let repo = resolve_repo(input.repo.as_deref(), registry)?;
    let head = hz_git::current_head(&current)?;
    let patch = if input.include_diff {
        Some(hz_git::diff_patch(&current)?)
    } else {
        None
    };

    let (mut worktree, pending_prune) = create_with_registry_and_deferred_prune(
        registry,
        CreateWorktree {
            name: input.name,
            repo: Some(repo),
            path: input.path,
            base: Some(head),
            branch: None,
            detached: true,
            max_detached_worktrees: input.max_detached_worktrees,
            max_branch_worktrees: None,
        },
    )?;

    let changed = match patch {
        Some(patch) => apply_patch(&worktree.path, &patch).map_err(|error| {
            cleanup_created_fork(registry, created_worktree_entry(&worktree), error)
        })?,
        None => false,
    };
    worktree.warnings = pending_prune.prune(registry);

    Ok(ForkedWorktree { worktree, changed })
}

fn created_worktree_entry(created: &crate::CreatedWorktree) -> WorktreeEntry {
    WorktreeEntry {
        id: created.id.clone(),
        handle: created.handle.clone(),
        repo: created.repo.clone(),
        path: created.path.clone(),
        branch: created.branch.clone(),
        base: created.base.clone(),
        source: WorktreeSource::Managed,
        created_at_unix: 0,
        modified_at_unix: 0,
        status: WorktreeStatus::Unknown,
    }
}

fn cleanup_created_fork(
    registry: &mut Registry,
    destination: WorktreeEntry,
    error: HzError,
) -> HzError {
    match remove_registered_entry_with_force_from_registry(registry, destination, true) {
        Ok(_) => error,
        Err(cleanup_error) => HzError::Usage(format!(
            "{error}; additionally failed to remove created fork worktree: {cleanup_error}"
        )),
    }
}
