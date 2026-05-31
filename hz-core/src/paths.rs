use std::path::PathBuf;

use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct WorktreeTarget {
    pub name: String,
    pub path: PathBuf,
}
