use std::path::{Path, PathBuf};

use crate::{
    CreateWorktree, ForkWorktree, ForkedWorktree, Registry, WorktreeEntry,
    create_with_registry_and_deferred_prune, remove_registered_entry_with_force_from_registry,
    resolve_repo, same_path, worktree_entry_from_created_worktree,
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
    let current = fork_source_worktree(input.repo.as_deref())?;
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

fn fork_source_worktree(repo: Option<&Path>) -> HzResult<PathBuf> {
    let hinted = repo
        .map(|repo| hz_git::repository_root(Some(repo)))
        .transpose()?;
    let current = hz_git::repository_root(None).ok();

    match (current, hinted) {
        (Some(current), Some(hinted)) if same_git_worktree_family(&current, &hinted)? => {
            Ok(current)
        }
        (_, Some(hinted)) => Ok(hinted),
        (Some(current), None) => Ok(current),
        (None, None) => hz_git::repository_root(None),
    }
}

fn same_git_worktree_family(left: &Path, right: &Path) -> HzResult<bool> {
    Ok(same_path(
        &hz_git::main_worktree(left)?,
        &hz_git::main_worktree(right)?,
    ))
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
