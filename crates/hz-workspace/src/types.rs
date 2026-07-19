use std::path::PathBuf;

use serde::Serialize;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CopyMode {
    Filtered,
    #[default]
    All,
}

impl CopyMode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Filtered => "filtered",
            Self::All => "all",
        }
    }

    pub(crate) fn from_stored(value: &str) -> Self {
        if value == "filtered" {
            Self::Filtered
        } else {
            Self::All
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceState {
    Creating,
    Active,
    Trashed,
    Unregistered,
}

impl WorkspaceState {
    pub(crate) fn from_stored(value: &str) -> Self {
        match value {
            "creating" => Self::Creating,
            "trashed" => Self::Trashed,
            "unregistered" => Self::Unregistered,
            _ => Self::Active,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Materializer {
    RegisteredRoot,
    Snapshot,
}

impl Materializer {
    pub(crate) fn from_stored(value: &str) -> Self {
        if value == "registered_root" {
            Self::RegisteredRoot
        } else {
            Self::Snapshot
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Workspace {
    pub id: String,
    pub root_id: String,
    pub parent_id: Option<String>,
    pub handle: String,
    pub path: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage_path: Option<PathBuf>,
    pub state: WorkspaceState,
    pub materializer: Materializer,
    pub strategy: String,
    pub copy_mode: CopyMode,
    pub pinned: bool,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum InitStrategy {
    #[default]
    CopyOnWrite,
    Copy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitWorkspace {
    pub at: PathBuf,
    pub here: bool,
    pub strategy: InitStrategy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InitOutcome {
    Registered,
    AlreadyInitialized,
    Converted,
    MarkerRestored,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InitializedWorkspace {
    pub workspace: Workspace,
    pub outcome: InitOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateWorkspace {
    pub from: PathBuf,
    pub handle: Option<String>,
    pub into: Option<PathBuf>,
    pub copy_mode: CopyMode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CreatedWorkspace {
    pub workspace: Workspace,
    pub source: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListScope {
    Family,
    Children,
    Roots,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListWorkspaces {
    pub of: Option<PathBuf>,
    pub scope: ListScope,
    pub pinned: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoveMode {
    Subtree,
    Children,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RemovedWorkspaces {
    pub selected: Workspace,
    pub removed: Vec<Workspace>,
    pub root_unregistered: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GarbageCollection {
    pub deleted: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DoctorIssue {
    pub workspace_id: String,
    pub path: PathBuf,
    pub message: String,
    pub fixed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DoctorReport {
    pub issues: Vec<DoctorIssue>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InitProgress {
    CreatingSubvolume,
    ImportingWorkspace,
    ImportedEntries { entries: u64 },
    ActivatingWorkspace,
    RemovingOriginal,
    RestoringMarker,
    RegisteringWorkspace,
}
