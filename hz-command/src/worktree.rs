use std::path::PathBuf;

use crate::{
    CreateWorktree, CreatedWorktree, FindWorktree, HandoffWorktree, HzConfig, LifecycleKind,
    ListWorktrees, LocalWorktree, LocalWorktreeInfo, PathWorktree, RemoveWorktree, WorktreeEntry,
    WorktreeHandoff, create_worktree_with_config_defaults, created_worktree_target,
    run_lifecycle_for_path, with_configured_handoff_limits,
};
use hz_core::{HzResult, path_utils::path_is_inside};

pub fn create_worktree(input: CreateWorktree) -> HzResult<CreatedWorktree> {
    hz_worktree::create(create_worktree_with_config_defaults(input)?)
}

pub fn create_worktree_with_lifecycle(
    input: CreateWorktree,
    run_setup: bool,
) -> HzResult<CreatedWorktree> {
    let created = create_worktree(input)?;
    if run_setup {
        let target = created_worktree_target(&created);
        run_lifecycle_for_path(&created.repo, &created.path, &target, LifecycleKind::Setup)?;
    }
    Ok(created)
}

pub fn path_worktree(input: PathWorktree) -> HzResult<hz_core::paths::WorktreeTarget> {
    hz_worktree::path(input)
}

pub fn handoff_worktree(input: HandoffWorktree) -> HzResult<WorktreeHandoff> {
    hz_worktree::handoff(with_configured_handoff_limits(input)?)
}

pub fn list_worktrees(input: ListWorktrees) -> HzResult<Vec<WorktreeEntry>> {
    hz_worktree::list(input)
}

pub fn list_worktree_targets(input: ListWorktrees) -> HzResult<Vec<WorktreeEntry>> {
    hz_worktree::list_targets(input)
}

pub fn local_worktree(input: LocalWorktree) -> HzResult<LocalWorktreeInfo> {
    hz_worktree::local(input)
}

pub fn current_worktree_path(input: ListWorktrees) -> HzResult<PathBuf> {
    hz_worktree::current_path(input)
}

pub fn find_worktree(input: FindWorktree) -> HzResult<WorktreeEntry> {
    hz_worktree::find(input)
}

pub fn is_user_managed_worktree_path(entry: &WorktreeEntry) -> HzResult<bool> {
    if hz_worktree::is_hz_worktree_path(&entry.repo, &entry.path)? {
        return Ok(true);
    }

    let config = HzConfig::load(&entry.repo)?;
    Ok(config
        .user_managed_worktree_roots(&entry.repo)?
        .iter()
        .any(|root| path_is_inside(&entry.path, root)))
}

pub fn remove_worktree(input: RemoveWorktree) -> HzResult<WorktreeEntry> {
    hz_worktree::remove(input)
}

pub fn remove_found_worktree(entry: WorktreeEntry) -> HzResult<WorktreeEntry> {
    hz_worktree::remove_found(entry)
}

pub fn remove_found_worktree_with_force(
    entry: WorktreeEntry,
    force: bool,
) -> HzResult<WorktreeEntry> {
    hz_worktree::remove_found_with_force(entry, force)
}
