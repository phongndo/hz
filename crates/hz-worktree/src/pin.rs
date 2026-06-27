use std::collections::HashSet;

use crate::{
    PinWorktrees, Registry, WorktreeEntry, WorktreeSource, discover_entries_with_git_worktrees,
    find_target_entry, resolve_repo_with_git_worktrees, same_path,
};
use hz_core::{HzError, HzResult};

pub fn pin(input: PinWorktrees) -> HzResult<Vec<WorktreeEntry>> {
    let mut registry = Registry::load_for_update()?;
    pin_with_registry(&mut registry, input)
}

pub(crate) fn pin_with_registry(
    registry: &mut Registry,
    input: PinWorktrees,
) -> HzResult<Vec<WorktreeEntry>> {
    let (repo, git_worktrees) = resolve_repo_with_git_worktrees(input.repo.as_deref(), registry)?;
    let entries = discover_entries_with_git_worktrees(registry, &repo, git_worktrees);
    let mut indexes = Vec::with_capacity(input.targets.len());
    let mut pinned_entries = Vec::with_capacity(input.targets.len());
    let mut seen = HashSet::new();

    for target in &input.targets {
        let entry =
            find_target_entry(&entries, &repo, target).ok_or_else(|| HzError::UnknownWorktree {
                target: target.clone(),
            })?;
        if entry.source != WorktreeSource::Managed {
            return Err(HzError::Usage(format!(
                "cannot {} unmanaged worktree: {target}",
                if input.pinned { "pin" } else { "unpin" }
            )));
        }
        if !seen.insert((entry.repo.clone(), entry.path.clone())) {
            return Err(HzError::Usage(format!(
                "duplicate worktree target: {target}"
            )));
        }

        let index = registry
            .entries
            .iter()
            .position(|registered| {
                registered.id == entry.id
                    && same_path(&registered.repo, &entry.repo)
                    && same_path(&registered.path, &entry.path)
            })
            .ok_or_else(|| HzError::UnknownWorktree {
                target: target.clone(),
            })?;
        indexes.push(index);

        let mut entry = entry.clone();
        entry.pinned = input.pinned;
        pinned_entries.push(entry);
    }

    let mut next_registry = registry.clone();
    for index in &indexes {
        next_registry.entries[*index].pinned = input.pinned;
    }
    next_registry.save()?;
    *registry = next_registry;

    Ok(pinned_entries)
}
