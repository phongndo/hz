use std::path::PathBuf;

use hz_core::paths::WorktreeTarget;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateWorktree {
    pub name: Option<String>,
    pub repo: Option<PathBuf>,
    pub path: Option<PathBuf>,
    pub base: Option<String>,
    pub branch: Option<String>,
    pub max_detached_worktrees: Option<usize>,
    pub max_branch_worktrees: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathWorktree {
    pub target: String,
    pub repo: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoffWorktree {
    pub target: Option<String>,
    pub mode: HandoffMode,
    pub repo: Option<PathBuf>,
    pub create: bool,
    pub max_detached_worktrees: Option<usize>,
    pub max_branch_worktrees: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListWorktrees {
    pub repo: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalWorktree {
    pub repo: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LocalWorktreeInfo {
    pub repo: PathBuf,
    pub path: PathBuf,
    pub branch: Option<String>,
    pub status: WorktreeStatus,
    pub modified_at_unix: u64,
    pub handoff_from: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoveWorktree {
    pub target: String,
    pub repo: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindWorktree {
    pub target: String,
    pub repo: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindWorktrees {
    pub targets: Vec<String>,
    pub repo: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
pub struct CreatedWorktree {
    pub id: String,
    pub name: String,
    pub handle: String,
    pub repo: PathBuf,
    pub path: PathBuf,
    pub branch: Option<String>,
    pub base: Option<String>,
    pub source: WorktreeSource,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct WorktreeHandoff {
    pub repo: PathBuf,
    pub mode: HandoffMode,
    pub branch: Option<String>,
    pub from: WorktreeTarget,
    pub to: WorktreeTarget,
    pub changed: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandoffMode {
    #[default]
    Patch,
    Branch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorktreeSource {
    Managed,
    Git,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorktreeStatus {
    Clean,
    Dirty,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeEntry {
    pub id: String,
    pub handle: String,
    pub repo: PathBuf,
    pub path: PathBuf,
    pub branch: Option<String>,
    pub base: Option<String>,
    pub source: WorktreeSource,
    pub created_at_unix: u64,
    #[serde(default)]
    pub modified_at_unix: u64,
    #[serde(default)]
    pub status: WorktreeStatus,
}

pub const DEFAULT_MAX_DETACHED_WORKTREES: usize = 15;
pub const DEFAULT_MAX_BRANCH_WORKTREES: usize = 15;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct HandoffLink {
    pub(crate) repo: PathBuf,
    pub(crate) branch: String,
    pub(crate) path: PathBuf,
    pub(crate) handle: String,
    pub(crate) local_restore_branch: Option<String>,
    pub(crate) updated_at_unix: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PatchHandoffLink {
    pub(crate) repo: PathBuf,
    pub(crate) left_path: PathBuf,
    pub(crate) left_handle: String,
    pub(crate) right_path: PathBuf,
    pub(crate) right_handle: String,
    pub(crate) patch_hash: String,
    pub(crate) updated_at_unix: u64,
}
