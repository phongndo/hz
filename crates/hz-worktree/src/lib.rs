use std::path::PathBuf;

use hz_core::{HzError, HzResult, paths::WorktreeTarget};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateWorktree {
    pub name: String,
    pub repo: Option<PathBuf>,
    pub path: Option<PathBuf>,
    pub base: Option<String>,
    pub branch: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitchWorktree {
    pub target: String,
    pub repo: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoffWorktree {
    pub from: String,
    pub to: String,
    pub repo: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
pub struct CreatedWorktree {
    pub name: String,
    pub repo: PathBuf,
    pub path: PathBuf,
    pub branch: Option<String>,
    pub base: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct WorktreeHandoff {
    pub repo: PathBuf,
    pub from: WorktreeTarget,
    pub to: WorktreeTarget,
}

pub fn create(input: CreateWorktree) -> HzResult<CreatedWorktree> {
    let _ = input;
    Err(HzError::NotImplemented("worktree create"))
}

pub fn switch(input: SwitchWorktree) -> HzResult<WorktreeTarget> {
    let _ = input;
    Err(HzError::NotImplemented("worktree switch"))
}

pub fn handoff(input: HandoffWorktree) -> HzResult<WorktreeHandoff> {
    let _ = input;
    Err(HzError::NotImplemented("worktree handoff"))
}
