use std::path::{Path, PathBuf};

use crate::{
    CreateWorktree, CreatedWorktree, HandoffMode, HandoffWorktree, Registry, WorktreeEntry,
    WorktreeHandoff, WorktreeSource, WorktreeStatus, create_with_registry, discover_entries,
    matches_target, remove_registered_entry_with_force_from_registry, resolve_registered_repo,
    same_path, unix_now, validate_worktree_name,
};
use hz_core::{HzError, HzResult, paths::WorktreeTarget};

pub fn handoff(input: HandoffWorktree) -> HzResult<WorktreeHandoff> {
    let mut registry = Registry::load_for_update()?;
    let current = hz_git::repository_root(input.repo.as_deref())?;
    let main = hz_git::main_worktree(&current)?;
    let repo = resolve_registered_repo(&registry, &current, &main).unwrap_or(main);

    if input.mode == HandoffMode::Patch {
        return handoff_patch(
            &mut registry,
            repo,
            current,
            input.target,
            input.create,
            input.max_detached_worktrees,
            input.max_branch_worktrees,
        );
    }

    if input.create {
        return Err(HzError::Usage(
            "--new only supports patch handoff".to_owned(),
        ));
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

pub(crate) fn handoff_patch(
    registry: &mut Registry,
    repo: PathBuf,
    current: PathBuf,
    target: Option<String>,
    create: bool,
    max_detached_worktrees: Option<usize>,
    max_branch_worktrees: Option<usize>,
) -> HzResult<WorktreeHandoff> {
    if same_path(&current, &repo) {
        handoff_patch_from_local(
            registry,
            repo,
            current,
            target,
            create,
            max_detached_worktrees,
            max_branch_worktrees,
        )
    } else {
        if create {
            return Err(HzError::Usage(
                "--new patch handoff must be run from local".to_owned(),
            ));
        }
        handoff_patch_to_local(registry, repo, current, target)
    }
}

pub(crate) fn handoff_patch_to_local(
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
    let patch = hz_git::diff_patch(&current)?;
    let branch = hz_git::current_branch(&current)?;
    let patch_hash = hz_git::hash_bytes(&repo, &patch)?;
    let mut next_registry = registry.clone();

    if let Some(branch) = &branch {
        let local_restore_branch = hz_git::current_branch(&repo)?.filter(|local| local != branch);
        next_registry.remember_handoff(
            &repo,
            branch,
            &current,
            &from.name,
            local_restore_branch,
        )?;
    }
    next_registry.remember_patch_handoff(&repo, &from, &to, patch_hash)?;

    let applied = apply_patch_handoff(registry, &repo, &from, &to, &patch)?;
    if let Err(error) = next_registry.save() {
        return Err(rollback_saved_patch_handoff(
            &to.path, &patch, applied, error,
        ));
    }
    *registry = next_registry;

    Ok(WorktreeHandoff {
        repo,
        mode: HandoffMode::Patch,
        branch,
        from,
        to,
        changed: applied.changed,
        warnings: Vec::new(),
    })
}

pub(crate) fn handoff_patch_from_local(
    registry: &mut Registry,
    repo: PathBuf,
    current: PathBuf,
    target: Option<String>,
    create: bool,
    max_detached_worktrees: Option<usize>,
    max_branch_worktrees: Option<usize>,
) -> HzResult<WorktreeHandoff> {
    let branch = hz_git::current_branch(&current)?;
    let (destination, warnings) = if create {
        create_handoff_destination(
            registry,
            &repo,
            target,
            max_detached_worktrees,
            max_branch_worktrees,
        )?
    } else {
        let destination = match target {
            Some(target) => find_target_worktree(registry, &repo, &target)?
                .ok_or_else(|| HzError::Usage(format!("unknown worktree target: {target}")))?,
            None => match find_patch_handoff_destination(registry, &repo, &current)? {
                Some(destination) => destination,
                None => {
                    let branch = branch.clone().ok_or_else(|| {
                        HzError::Usage(
                            "local worktree is detached; pass a worktree target for patch handoff"
                                .to_owned(),
                        )
                    })?;
                    find_handoff_destination(registry, &repo, &branch)?
                }
            },
        };
        (destination, Vec::new())
    };
    let from = WorktreeTarget {
        name: "local".to_owned(),
        path: current.clone(),
    };
    let to = WorktreeTarget {
        name: destination.handle.clone(),
        path: destination.path.clone(),
    };
    let patch = hz_git::diff_patch(&current)?;
    let patch_hash = hz_git::hash_bytes(&repo, &patch)?;
    let mut next_registry = registry.clone();
    next_registry.remember_patch_handoff(&repo, &from, &to, patch_hash)?;

    let applied = match apply_patch_handoff(registry, &repo, &from, &to, &patch) {
        Ok(applied) => applied,
        Err(error) if create => {
            return Err(cleanup_created_destination(registry, destination, error));
        }
        Err(error) => return Err(error),
    };
    if let Err(error) = next_registry.save() {
        let error = rollback_saved_patch_handoff(&to.path, &patch, applied, error);
        if create {
            return Err(cleanup_created_destination(registry, destination, error));
        }
        return Err(error);
    }
    *registry = next_registry;

    Ok(WorktreeHandoff {
        repo,
        mode: HandoffMode::Patch,
        branch,
        from,
        to,
        changed: applied.changed,
        warnings,
    })
}

pub(crate) fn create_handoff_destination(
    registry: &mut Registry,
    repo: &Path,
    target: Option<String>,
    max_detached_worktrees: Option<usize>,
    max_branch_worktrees: Option<usize>,
) -> HzResult<(WorktreeEntry, Vec<String>)> {
    let created = create_with_registry(
        registry,
        CreateWorktree {
            name: target,
            repo: Some(repo.to_path_buf()),
            path: None,
            base: None,
            branch: None,
            max_detached_worktrees,
            max_branch_worktrees,
        },
    )?;

    Ok(created_worktree_entry(created, unix_now()?))
}

pub(crate) fn created_worktree_entry(
    created: CreatedWorktree,
    created_at_unix: u64,
) -> (WorktreeEntry, Vec<String>) {
    let warnings = created.warnings;
    (
        WorktreeEntry {
            id: created.id,
            handle: created.handle,
            repo: created.repo,
            path: created.path,
            branch: created.branch,
            base: created.base,
            source: created.source,
            created_at_unix,
            modified_at_unix: 0,
            status: WorktreeStatus::Unknown,
        },
        warnings,
    )
}

pub(crate) struct AppliedPatchHandoff {
    pub(crate) changed: bool,
    pub(crate) previous_destination_patch: Option<Vec<u8>>,
}

pub(crate) struct GitCheckout {
    pub(crate) branch: Option<String>,
    pub(crate) head: String,
}

impl GitCheckout {
    pub(crate) fn current(path: &Path) -> HzResult<Self> {
        Ok(Self {
            branch: hz_git::current_branch(path)?,
            head: hz_git::current_head(path)?,
        })
    }

    pub(crate) fn restore(&self, path: &Path) -> HzResult<()> {
        match &self.branch {
            Some(branch) => hz_git::switch_branch(path, branch),
            None => hz_git::switch_detached_at(path, &self.head),
        }
    }
}

pub(crate) struct BranchHandoffRollback {
    pub(crate) path: PathBuf,
    pub(crate) checkout: GitCheckout,
}

#[derive(Default)]
pub(crate) struct AppliedBranchHandoff {
    pub(crate) rollbacks: Vec<BranchHandoffRollback>,
}

impl AppliedBranchHandoff {
    pub(crate) fn push(&mut self, path: &Path, checkout: GitCheckout) {
        self.rollbacks.push(BranchHandoffRollback {
            path: path.to_path_buf(),
            checkout,
        });
    }

    pub(crate) fn rollback(mut self) -> HzResult<()> {
        let mut errors = Vec::new();

        while let Some(rollback) = self.rollbacks.pop() {
            if let Err(error) = rollback.checkout.restore(&rollback.path) {
                errors.push(format!("{}: {error}", rollback.path.display()));
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(HzError::Usage(format!(
                "failed to restore one or more worktrees: {}",
                errors.join("; ")
            )))
        }
    }
}

pub(crate) fn apply_patch_handoff(
    registry: &Registry,
    repo: &Path,
    from: &WorktreeTarget,
    to: &WorktreeTarget,
    patch: &[u8],
) -> HzResult<AppliedPatchHandoff> {
    let destination_state = hz_git::worktree_state(&to.path)?;
    if !destination_state.dirty {
        return hz_git::apply_patch(&to.path, patch).map(|changed| AppliedPatchHandoff {
            changed,
            previous_destination_patch: None,
        });
    }

    let Some(link) = registry.patch_handoff_link(repo, &from.path, &to.path) else {
        return Err(dirty_destination_error(&to.path));
    };

    let destination_patch = hz_git::diff_patch(&to.path)?;
    let destination_patch_hash = hz_git::hash_bytes(repo, &destination_patch)?;
    if destination_patch_hash != link.patch_hash {
        return Err(HzError::Usage(format!(
            "destination worktree has uncommitted changes not created by the last handoff: {}",
            to.path.display()
        )));
    }

    hz_git::apply_patch_reverse(&to.path, &destination_patch)?;
    match hz_git::apply_patch(&to.path, patch) {
        Ok(changed) => Ok(AppliedPatchHandoff {
            changed: changed || !destination_patch.is_empty(),
            previous_destination_patch: Some(destination_patch),
        }),
        Err(error) => {
            let rollback = hz_git::apply_patch(&to.path, &destination_patch);
            Err(match rollback {
                Ok(_) => error,
                Err(rollback_error) => HzError::Usage(format!(
                    "{error}; rollback failed, destination worktree was not restored: {rollback_error}"
                )),
            })
        }
    }
}

pub(crate) fn rollback_saved_patch_handoff(
    destination: &Path,
    patch: &[u8],
    applied: AppliedPatchHandoff,
    save_error: HzError,
) -> HzError {
    let rollback = (|| -> HzResult<()> {
        if applied.changed {
            hz_git::apply_patch_reverse(destination, patch)?;
        }
        if let Some(previous_patch) = applied.previous_destination_patch {
            hz_git::apply_patch(destination, &previous_patch)?;
        }
        Ok(())
    })();

    match rollback {
        Ok(()) => save_error,
        Err(rollback_error) => HzError::Usage(format!(
            "{save_error}; rollback failed, destination worktree was not restored: {rollback_error}"
        )),
    }
}

pub(crate) fn cleanup_created_destination(
    registry: &mut Registry,
    destination: WorktreeEntry,
    error: HzError,
) -> HzError {
    match remove_registered_entry_with_force_from_registry(registry, destination, true) {
        Ok(_) => error,
        Err(cleanup_error) => HzError::Usage(format!(
            "{error}; additionally failed to remove created destination worktree: {cleanup_error}"
        )),
    }
}

pub(crate) fn dirty_destination_error(path: &Path) -> HzError {
    HzError::Usage(format!(
        "destination worktree has uncommitted changes: {}",
        path.display()
    ))
}

pub(crate) fn resolve_local_branch_handoff_target(
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

pub(crate) fn resolve_handoff_branch(branch: Option<String>, current: &Path) -> HzResult<String> {
    let branch = match branch {
        Some(branch) => branch,
        None => hz_git::current_branch(current)?.ok_or_else(|| {
            HzError::Usage("current worktree is detached; pass the branch to hand off".to_owned())
        })?,
    };

    validate_handoff_branch_name(&branch)?;
    Ok(branch)
}

pub(crate) fn validate_handoff_branch_name(branch: &str) -> HzResult<()> {
    validate_worktree_name("handoff branch", branch)?;
    if branch.is_empty() {
        return Err(HzError::Usage("handoff branch cannot be empty".to_owned()));
    }

    Ok(())
}

pub(crate) fn handoff_worktree_to_local(
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
    let source_checkout = GitCheckout::current(&current)?;
    validate_handoff_source_branch(&current, source_checkout.branch.as_deref(), &branch)?;
    ensure_branch_exists(&repo, &branch)?;
    ensure_clean(&current, "source")?;
    ensure_clean(&repo, "destination")?;

    let local_checkout = GitCheckout::current(&repo)?;
    let local_restore_branch = local_checkout
        .branch
        .clone()
        .filter(|local| local != &branch);
    let mut next_registry = registry.clone();
    next_registry.remember_handoff(&repo, &branch, &current, &from.name, local_restore_branch)?;

    let applied = apply_worktree_to_local_branch_handoff(
        &current,
        &repo,
        &branch,
        source_checkout,
        local_checkout,
    )?;
    if let Err(error) = next_registry.save() {
        return Err(rollback_saved_branch_handoff(applied, error));
    }
    *registry = next_registry;

    Ok(WorktreeHandoff {
        repo,
        mode: HandoffMode::Branch,
        branch: Some(branch),
        from,
        to,
        changed: true,
        warnings: Vec::new(),
    })
}

pub(crate) fn apply_worktree_to_local_branch_handoff(
    source: &Path,
    local: &Path,
    branch: &str,
    source_checkout: GitCheckout,
    local_checkout: GitCheckout,
) -> HzResult<AppliedBranchHandoff> {
    let mut applied = AppliedBranchHandoff::default();

    if source_checkout.branch.as_deref() == Some(branch) {
        hz_git::switch_detached(source)?;
        applied.push(source, source_checkout);
    }
    if local_checkout.branch.as_deref() != Some(branch) {
        if let Err(error) = hz_git::switch_branch(local, branch) {
            return Err(rollback_failed_branch_handoff(error, applied));
        }
        applied.push(local, local_checkout);
    }

    Ok(applied)
}

pub(crate) fn handoff_local_to_worktree(
    registry: &mut Registry,
    repo: PathBuf,
    current: PathBuf,
    branch: String,
    destination: Option<WorktreeEntry>,
) -> HzResult<WorktreeHandoff> {
    let source_checkout = GitCheckout::current(&current)?;
    validate_handoff_source_branch(&current, source_checkout.branch.as_deref(), &branch)?;
    if source_checkout.branch.is_none() {
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

    let restore_branch = registry
        .handoff_link(&repo, &branch)
        .and_then(|link| link.local_restore_branch.as_deref())
        .filter(|restore_branch| *restore_branch != branch.as_str())
        .map(str::to_owned);
    let destination_checkout = GitCheckout::current(&destination.path)?;
    let mut next_registry = registry.clone();
    next_registry.forget_handoff(&repo, &branch);

    let applied = apply_local_to_worktree_branch_handoff(
        &current,
        &destination.path,
        &branch,
        restore_branch.as_deref(),
        source_checkout,
        destination_checkout,
    )?;
    if let Err(error) = next_registry.save() {
        return Err(rollback_saved_branch_handoff(applied, error));
    }
    *registry = next_registry;

    Ok(WorktreeHandoff {
        repo,
        mode: HandoffMode::Branch,
        branch: Some(branch),
        from,
        to,
        changed: true,
        warnings: Vec::new(),
    })
}

pub(crate) fn apply_local_to_worktree_branch_handoff(
    local: &Path,
    destination: &Path,
    branch: &str,
    restore_branch: Option<&str>,
    local_checkout: GitCheckout,
    destination_checkout: GitCheckout,
) -> HzResult<AppliedBranchHandoff> {
    let mut applied = AppliedBranchHandoff::default();

    if let Some(restore_branch) = restore_branch {
        hz_git::switch_branch(local, restore_branch)?;
    } else {
        hz_git::switch_detached(local)?;
    }
    applied.push(local, local_checkout);

    if let Err(error) = hz_git::switch_branch(destination, branch) {
        return Err(rollback_failed_branch_handoff(error, applied));
    }
    applied.push(destination, destination_checkout);

    Ok(applied)
}

pub(crate) fn rollback_saved_branch_handoff(
    applied: AppliedBranchHandoff,
    save_error: HzError,
) -> HzError {
    match applied.rollback() {
        Ok(()) => save_error,
        Err(rollback_error) => HzError::Usage(format!(
            "{save_error}; rollback failed, branch handoff was not restored: {rollback_error}"
        )),
    }
}

pub(crate) fn rollback_failed_branch_handoff(
    error: HzError,
    applied: AppliedBranchHandoff,
) -> HzError {
    match applied.rollback() {
        Ok(()) => error,
        Err(rollback_error) => HzError::Usage(format!(
            "{error}; rollback failed, branch handoff was not restored: {rollback_error}"
        )),
    }
}

pub(crate) fn find_target_worktree(
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

pub(crate) fn find_target_entry(
    entries: Vec<WorktreeEntry>,
    repo: &Path,
    target: &str,
) -> Option<WorktreeEntry> {
    entries
        .into_iter()
        .find(|entry| !same_path(&entry.path, repo) && matches_target(entry, target))
}

pub(crate) fn validate_handoff_source_branch(
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

pub(crate) fn ensure_branch_exists(repo: &Path, branch: &str) -> HzResult<()> {
    if hz_git::branch_exists(repo, branch)? {
        Ok(())
    } else {
        Err(HzError::Usage(format!("unknown branch: {branch}")))
    }
}

pub(crate) fn ensure_clean(path: &Path, label: &str) -> HzResult<()> {
    let state = hz_git::worktree_state(path)?;
    if state.dirty {
        return Err(HzError::Usage(format!(
            "{label} worktree has uncommitted changes: {}",
            path.display()
        )));
    }

    Ok(())
}

pub(crate) fn target_for_path(
    registry: &Registry,
    repo: &Path,
    path: &Path,
) -> HzResult<WorktreeTarget> {
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

pub(crate) fn find_handoff_destination(
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

pub(crate) fn find_patch_handoff_destination(
    registry: &Registry,
    repo: &Path,
    current: &Path,
) -> HzResult<Option<WorktreeEntry>> {
    let Some(link) = registry.latest_patch_handoff_for_path(repo, current) else {
        return Ok(None);
    };
    let destination_path = if same_path(&link.left_path, current) {
        &link.right_path
    } else {
        &link.left_path
    };

    Ok(discover_entries(registry, repo)?
        .into_iter()
        .find(|entry| same_path(&entry.path, destination_path)))
}

pub(crate) fn linked_worktree_exists(repo: &Path, path: &Path) -> HzResult<bool> {
    Ok(hz_git::list_worktrees(repo)?
        .into_iter()
        .any(|worktree| same_path(&worktree.path, path)))
}
