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

pub fn remove(input: RemoveWorktree) -> HzResult<WorktreeEntry> {
    let mut registry = Registry::load_for_update()?;
    let repo = resolve_repo(input.repo.as_deref(), &registry)?;
    if let Some(index) = registry
        .entries
        .iter()
        .position(|entry| same_path(&entry.repo, &repo) && matches_target(entry, &input.target))
    {
        let entry = registry.entries.remove(index);

        hz_git::remove_worktree(&repo, &entry.path)?;
        registry.save()?;

        return Ok(entry);
    }

    Err(HzError::Usage(format!(
        "unknown worktree: {}",
        input.target
    )))
}

pub fn remove_found(entry: WorktreeEntry) -> HzResult<WorktreeEntry> {
    remove_found_with_force(entry, false)
}

pub fn remove_found_with_force(entry: WorktreeEntry, force: bool) -> HzResult<WorktreeEntry> {
    match entry.source {
        WorktreeSource::Managed => remove_registered_entry_with_force(entry, force),
        WorktreeSource::Git => remove_git_entry_with_force(entry, force),
    }
}

pub(crate) fn remove_git_entry_with_force(
    entry: WorktreeEntry,
    force: bool,
) -> HzResult<WorktreeEntry> {
    run_with_registry_lock_for_git_side_effect(|| {
        hz_git::remove_worktree_with_force(&entry.repo, &entry.path, force)?;
        Ok(entry)
    })
}

pub(crate) fn remove_registered_entry_with_force(
    entry: WorktreeEntry,
    force: bool,
) -> HzResult<WorktreeEntry> {
    let mut registry = Registry::load_for_update()?;
    remove_registered_entry_with_force_from_registry(&mut registry, entry, force)
}

pub(crate) fn remove_registered_entry_with_force_from_registry(
    registry: &mut Registry,
    entry: WorktreeEntry,
    force: bool,
) -> HzResult<WorktreeEntry> {
    let entry = remove_registered_entry_from_registry(registry, &entry, force)?;
    registry.save()?;

    Ok(entry)
}

pub(crate) fn remove_registered_entry_from_registry(
    registry: &mut Registry,
    entry: &WorktreeEntry,
    force: bool,
) -> HzResult<WorktreeEntry> {
    let index = registry
        .entries
        .iter()
        .position(|registered| {
            same_path(&registered.repo, &entry.repo)
                && registered.id == entry.id
                && same_path(&registered.path, &entry.path)
        })
        .ok_or_else(|| HzError::Usage(format!("unknown worktree: {}", entry.handle)))?;
    let entry = registry.entries[index].clone();

    hz_git::remove_worktree_with_force(&entry.repo, &entry.path, force)?;
    registry.entries.remove(index);

    Ok(entry)
}
