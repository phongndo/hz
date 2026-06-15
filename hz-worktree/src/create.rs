use std::{fs, path::Path};

use crate::{
    CreateWorktree, CreatedWorktree, DEFAULT_MAX_BRANCH_WORKTREES, DEFAULT_MAX_DETACHED_WORKTREES,
    PathWorktree, Registry, WorktreeEntry, WorktreeSource, WorktreeStatus, branch_prune_warning,
    branch_worktree_prune_candidates, default_worktree_path, detached_prune_warning,
    detached_worktree_prune_candidates, generate_unique_handle, new_uuid_v4, prune_worktrees,
    resolve_repo, resolve_target, resolve_worktree_path, unix_now, validate_worktree_name,
};
use hz_core::{HzError, HzResult, paths::WorktreeTarget};

pub fn create(input: CreateWorktree) -> HzResult<CreatedWorktree> {
    let mut registry = Registry::load_for_update()?;
    create_with_registry(&mut registry, input)
}

pub(crate) fn create_with_registry(
    registry: &mut Registry,
    input: CreateWorktree,
) -> HzResult<CreatedWorktree> {
    let (mut created, pending_prune) = create_with_registry_and_deferred_prune(registry, input)?;
    created.warnings = pending_prune.prune(registry);
    Ok(created)
}

pub(crate) fn create_with_registry_and_deferred_prune(
    registry: &mut Registry,
    input: CreateWorktree,
) -> HzResult<(CreatedWorktree, PendingWorktreePrune)> {
    let repo = resolve_repo(input.repo.as_deref(), registry)?;
    let name = input.name;
    let handle = match name.clone() {
        Some(name) => name,
        None => generate_unique_handle(registry, &repo)?,
    };
    if input.detached && input.branch.is_some() {
        return Err(HzError::Usage(
            "detached worktree cannot also create a branch".to_owned(),
        ));
    }
    let branch = if input.detached {
        None
    } else {
        derive_worktree_branch(name.as_deref(), input.branch.as_deref())
    };
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

    let branch_backed = branch.is_some();
    let prune_candidates = if branch_backed {
        branch_worktree_prune_candidates(
            registry,
            &repo,
            input.repo.as_deref(),
            input
                .max_branch_worktrees
                .unwrap_or(DEFAULT_MAX_BRANCH_WORKTREES),
        )?
    } else {
        detached_worktree_prune_candidates(
            registry,
            &repo,
            input.repo.as_deref(),
            input
                .max_detached_worktrees
                .unwrap_or(DEFAULT_MAX_DETACHED_WORKTREES),
        )?
    };

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
    let mut next_registry = registry.clone();
    next_registry.entries.push(entry);
    if let Err(error) = next_registry.save() {
        return Err(rollback_created_worktree(
            &repo,
            &path,
            branch.as_deref(),
            error,
        ));
    }
    *registry = next_registry;

    Ok((
        CreatedWorktree {
            id,
            name: handle.clone(),
            handle,
            repo,
            path,
            branch,
            base: input.base,
            source: WorktreeSource::Managed,
            warnings: Vec::new(),
        },
        PendingWorktreePrune {
            candidates: prune_candidates,
            branch_backed,
        },
    ))
}

pub(crate) struct PendingWorktreePrune {
    candidates: Vec<WorktreeEntry>,
    branch_backed: bool,
}

impl PendingWorktreePrune {
    pub(crate) fn prune(self, registry: &mut Registry) -> Vec<String> {
        match prune_worktrees(registry, self.candidates) {
            Ok(()) => Vec::new(),
            Err(error) if self.branch_backed => vec![branch_prune_warning(error)],
            Err(error) => vec![detached_prune_warning(error)],
        }
    }
}

pub(crate) fn derive_worktree_branch(name: Option<&str>, branch: Option<&str>) -> Option<String> {
    branch.or(name).map(str::to_owned)
}

pub(crate) fn rollback_created_worktree(
    repo: &Path,
    path: &Path,
    branch: Option<&str>,
    save_error: HzError,
) -> HzError {
    let rollback = {
        let mut errors = Vec::new();

        let worktree_removed = match hz_git::remove_worktree_with_force(repo, path, true) {
            Ok(()) => true,
            Err(error) => {
                errors.push(format!("worktree: {error}"));
                false
            }
        };
        if worktree_removed && let Some(branch) = branch {
            match hz_git::branch_exists(repo, branch) {
                Ok(true) => {
                    if let Err(error) = hz_git::delete_branch(repo, branch) {
                        errors.push(format!("branch {branch}: {error}"));
                    }
                }
                Ok(false) => {}
                Err(error) => errors.push(format!("branch {branch}: {error}")),
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(HzError::Usage(format!(
                "failed to roll back created git state: {}",
                errors.join("; ")
            )))
        }
    };

    match rollback {
        Ok(()) => save_error,
        Err(rollback_error) => HzError::Usage(format!(
            "{save_error}; rollback failed, created git state was not fully restored: {rollback_error}"
        )),
    }
}

pub fn path(input: PathWorktree) -> HzResult<WorktreeTarget> {
    let registry = Registry::load()?;
    let repo = resolve_repo(input.repo.as_deref(), &registry)?;
    resolve_target(&registry, &repo, &input.target)
}
