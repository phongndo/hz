use std::path::Path;

use hz_core::{HzError, HzResult};
use hz_scm::{SourceControl, SourceStatus};
use hz_workspace::Workspace;
use serde::Serialize;

use crate::workspace_manager;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GitWorkspaceStatus {
    pub workspace: Workspace,
    pub status: SourceStatus,
    pub head: Option<String>,
    pub branch: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GitHandoff {
    pub from: Workspace,
    pub to: Workspace,
    pub changed: bool,
}

pub fn git_status(at: impl AsRef<Path>, target: Option<&str>) -> HzResult<GitWorkspaceStatus> {
    let manager = workspace_manager()?;
    let workspace = manager
        .resolve_target(at, target, false)
        .map_err(HzError::from)?;
    require_git(&workspace)?;
    let status = hz_git::GitSourceControl.status(&workspace.path)?;
    let head = hz_git::current_head(&workspace.path).ok();
    let branch = hz_git::current_branch(&workspace.path)?;
    Ok(GitWorkspaceStatus {
        workspace,
        status,
        head,
        branch,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MercurialWorkspaceStatus {
    pub workspace: Workspace,
    pub status: SourceStatus,
    pub revision: Option<String>,
}

pub fn mercurial_status(
    at: impl AsRef<Path>,
    target: Option<&str>,
) -> HzResult<MercurialWorkspaceStatus> {
    let manager = workspace_manager()?;
    let workspace = manager
        .resolve_target(at, target, false)
        .map_err(HzError::from)?;
    require_source_control(&workspace, "hg", "Mercurial")?;
    let status = hz_hg::MercurialSourceControl.status(&workspace.path)?;
    let revision = hz_hg::revision(&workspace.path)?;
    Ok(MercurialWorkspaceStatus {
        workspace,
        status,
        revision,
    })
}

pub fn git_handoff(at: impl AsRef<Path>, target: Option<&str>) -> HzResult<GitHandoff> {
    let manager = workspace_manager()?;
    let source = manager.current(at.as_ref()).map_err(HzError::from)?;
    require_git(&source)?;
    let destination = match target {
        Some(target) => manager
            .resolve_target(at.as_ref(), Some(target), false)
            .map_err(HzError::from)?,
        None => manager
            .ancestors(&source.path, None)
            .map_err(HzError::from)?
            .into_iter()
            .next()
            .ok_or_else(|| {
                HzError::Usage(
                    "the root workspace has no parent; pass a destination workspace".to_owned(),
                )
            })?,
    };
    require_git(&destination)?;
    if source.id == destination.id {
        return Err(HzError::Usage(
            "Git handoff source and destination are the same workspace".to_owned(),
        ));
    }
    if hz_git::status(&destination.path)?.dirty {
        return Err(HzError::Usage(format!(
            "Git handoff destination is dirty: {}",
            destination.path.display()
        )));
    }
    let patch = hz_git::diff_patch(&source.path)?;
    let changed = hz_git::apply_patch(&destination.path, &patch)?;
    Ok(GitHandoff {
        from: source,
        to: destination,
        changed,
    })
}

fn require_git(workspace: &Workspace) -> HzResult<()> {
    require_source_control(workspace, "git", "Git")
}

fn require_source_control(workspace: &Workspace, kind: &str, name: &str) -> HzResult<()> {
    let metadata = match kind {
        "git" => ".git",
        "hg" => ".hg",
        _ => return Err(HzError::Usage(format!("unknown source control: {kind}"))),
    };
    if workspace.path.join(metadata).exists() {
        Ok(())
    } else {
        Err(HzError::Usage(format!(
            "workspace does not contain a {name} checkout: {}",
            workspace.path.display()
        )))
    }
}
