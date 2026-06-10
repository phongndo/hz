use std::{
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use crate::{
    Registry, WorktreeEntry, WorktreeSource, WorktreeStatus, find_target_worktree, same_path,
};
use hz_core::{HzError, HzResult, paths::WorktreeTarget};

pub(crate) fn resolve_repo(repo: Option<&Path>, registry: &Registry) -> HzResult<PathBuf> {
    let current = hz_git::repository_root(repo)?;
    resolve_current_repo(registry, &current)
}

pub(crate) fn resolve_current_repo(registry: &Registry, current: &Path) -> HzResult<PathBuf> {
    let main = hz_git::main_worktree(current)?;
    let worktrees = repo_identity_worktrees(current)?;
    Ok(
        resolve_registered_repo_with_worktrees(registry, current, &main, &worktrees)
            .unwrap_or(main),
    )
}

pub(crate) fn resolve_repo_with_git_worktrees(
    repo: Option<&Path>,
    registry: &Registry,
) -> HzResult<(PathBuf, Vec<hz_git::GitWorktree>)> {
    let current = hz_git::repository_root(repo)?;
    let git_worktrees = hz_git::list_worktrees(&current)?;
    let main = git_worktrees
        .first()
        .map(|worktree| worktree.path.clone())
        .ok_or_else(|| {
            HzError::Usage(format!(
                "git worktree list returned no entries for {}; unexpected repository state",
                current.display()
            ))
        })?;
    let repo = resolve_registered_repo_with_worktrees(registry, &current, &main, &git_worktrees)
        .unwrap_or(main);
    Ok((repo, git_worktrees))
}

fn repo_identity_worktrees(current: &Path) -> HzResult<Vec<hz_git::GitWorktree>> {
    if hz_git::source_control(current)? == hz_git::SourceControl::Jj {
        hz_git::list_worktrees(current)
    } else {
        Ok(Vec::new())
    }
}

pub(crate) fn resolve_registered_repo_with_worktrees(
    registry: &Registry,
    current: &Path,
    main: &Path,
    worktrees: &[hz_git::GitWorktree],
) -> Option<PathBuf> {
    registry
        .find_by_path(current)
        .or_else(|| {
            worktrees
                .iter()
                .find_map(|worktree| registry.find_by_path(&worktree.path))
        })
        .or_else(|| registry.find_by_repo(main))
        .map(|entry| entry.repo.clone())
}

pub(crate) fn resolve_target(
    registry: &Registry,
    repo: &Path,
    target: &str,
) -> HzResult<WorktreeTarget> {
    if target == "local" {
        return Ok(WorktreeTarget {
            name: "local".to_owned(),
            path: hz_git::main_worktree(repo)?,
        });
    }

    let entry = find_entry(registry, repo, target)?;
    Ok(WorktreeTarget {
        name: entry.handle,
        path: entry.path,
    })
}

pub(crate) fn find_entry(
    registry: &Registry,
    repo: &Path,
    target: &str,
) -> HzResult<WorktreeEntry> {
    find_target_worktree(registry, repo, target)?
        .ok_or_else(|| HzError::Usage(format!("unknown worktree: {target}")))
}

pub(crate) fn add_git_worktrees(
    entries: &mut Vec<WorktreeEntry>,
    repo: &Path,
    worktrees: Vec<hz_git::GitWorktree>,
) {
    let source_control = hz_git::source_control(repo).unwrap_or(hz_git::SourceControl::Git);
    for (index, worktree) in worktrees.into_iter().enumerate() {
        if index == 0 || same_path(&worktree.path, repo) {
            continue;
        }

        if let Some(entry) = entries
            .iter_mut()
            .find(|entry| same_path(&entry.path, &worktree.path))
        {
            if source_control.worktree_label_kind() == hz_git::WorktreeLabelKind::Branch
                || entry.source != WorktreeSource::Managed
            {
                entry.branch = worktree.branch;
            }
            continue;
        }

        entries.push(source_control_entry(repo, worktree, source_control));
    }
}

#[cfg(test)]
pub(crate) fn git_entry(repo: &Path, worktree: hz_git::GitWorktree) -> WorktreeEntry {
    let source_control = hz_git::source_control(repo).unwrap_or(hz_git::SourceControl::Git);
    source_control_entry(repo, worktree, source_control)
}

pub(crate) fn source_control_entry(
    repo: &Path,
    worktree: hz_git::GitWorktree,
    source_control: hz_git::SourceControl,
) -> WorktreeEntry {
    let handle = match source_control.worktree_label_kind() {
        hz_git::WorktreeLabelKind::Branch => git_worktree_handle(repo, &worktree),
        hz_git::WorktreeLabelKind::WorkspaceName => worktree
            .branch
            .clone()
            .unwrap_or_else(|| git_worktree_handle(repo, &worktree)),
    };
    let created_at_unix = worktree_path_timestamp(&worktree.path);
    let source = WorktreeSource::unmanaged_for_source_control(source_control);

    WorktreeEntry {
        id: handle.clone(),
        handle,
        repo: repo.to_path_buf(),
        path: worktree.path,
        branch: worktree.branch,
        base: None,
        source,
        created_at_unix,
        modified_at_unix: created_at_unix,
        status: WorktreeStatus::Unknown,
    }
}

pub(crate) fn worktree_path_timestamp(path: &Path) -> u64 {
    path.metadata()
        .ok()
        .and_then(|metadata| metadata.created().or_else(|_| metadata.modified()).ok())
        .and_then(|timestamp| timestamp.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |duration| duration.as_secs())
}

pub(crate) fn git_worktree_handle(repo: &Path, worktree: &hz_git::GitWorktree) -> String {
    let path_name = worktree
        .path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned());
    let repo_name = repo
        .file_name()
        .map(|name| name.to_string_lossy().into_owned());

    if path_name.is_some()
        && path_name == repo_name
        && let Some(parent_name) = worktree
            .path
            .parent()
            .and_then(|parent| parent.file_name())
            .map(|name| name.to_string_lossy().into_owned())
    {
        return parent_name;
    }

    path_name.unwrap_or_else(|| worktree.path.display().to_string())
}
