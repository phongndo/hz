use std::path::Path;

use hz_core::HzResult;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceStatus {
    Clean,
    Dirty,
    Unknown,
}

/// An explicit source-control operation over an existing workspace.
///
/// Source-control adapters do not participate in workspace identity,
/// materialization, or lifecycle.
pub trait SourceControl: Send + Sync {
    fn kind(&self) -> &'static str;

    fn status(&self, workspace: &Path) -> HzResult<SourceStatus>;
}
