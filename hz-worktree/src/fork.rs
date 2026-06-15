use std::path::Path;

use crate::{
    CreateWorktree, ForkWorktree, ForkedWorktree, Registry, WorktreeEntry,
    create_with_registry_and_deferred_prune, remove_registered_entry_with_force_from_registry,
    resolve_repo, worktree_entry_from_created_worktree,
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
            cleanup_created_fork(
                registry,
                worktree_entry_from_created_worktree(&worktree, 0),
                error,
            )
        })?,
        None => false,
    };
    worktree.warnings = pending_prune.prune(registry);

    Ok(ForkedWorktree { worktree, changed })
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
