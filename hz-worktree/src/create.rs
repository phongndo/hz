#![allow(unused_imports)]

use crate::*;
use std::{
    collections::HashSet,
    env, fs,
    io::{self, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use hz_core::{HzError, HzResult, paths::WorktreeTarget};
use serde::{Deserialize, Serialize};

pub fn create(input: CreateWorktree) -> HzResult<CreatedWorktree> {
    let mut registry = Registry::load_for_update()?;
    create_with_registry(&mut registry, input)
}

pub(crate) fn create_with_registry(
    registry: &mut Registry,
    input: CreateWorktree,
) -> HzResult<CreatedWorktree> {
    let repo = resolve_repo(input.repo.as_deref(), registry)?;
    let name = input.name;
    let handle = match name.clone() {
        Some(name) => name,
        None => generate_unique_handle(registry, &repo)?,
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

    let prune_candidates = if branch.is_none() {
        detached_worktree_prune_candidates(
            registry,
            &repo,
            input.repo.as_deref(),
            input
                .max_detached_worktrees
                .unwrap_or(DEFAULT_MAX_DETACHED_WORKTREES),
        )?
    } else {
        Vec::new()
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
    registry.entries.push(entry);
    registry.save()?;
    let warnings = match prune_detached_worktrees(registry, prune_candidates) {
        Ok(()) => Vec::new(),
        Err(error) => vec![detached_prune_warning(error)],
    };

    Ok(CreatedWorktree {
        id,
        name: handle.clone(),
        handle,
        repo,
        path,
        branch,
        base: input.base,
        source: WorktreeSource::Managed,
        warnings,
    })
}

pub(crate) fn derive_worktree_branch(name: Option<&str>, branch: Option<&str>) -> Option<String> {
    branch.or(name).map(str::to_owned)
}

pub fn path(input: PathWorktree) -> HzResult<WorktreeTarget> {
    let registry = Registry::load()?;
    let repo = resolve_repo(input.repo.as_deref(), &registry)?;
    resolve_target(&registry, &repo, &input.target)
}
