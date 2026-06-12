use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::AgentKind;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpawnAgent {
    pub kind: AgentKind,
    pub name: Option<String>,
    pub cwd: Option<PathBuf>,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSession {
    pub id: String,
    pub kind: AgentKind,
    pub name: Option<String>,
    pub command: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub pid: u32,
    pub log_path: PathBuf,
    pub status: AgentStatus,
    pub started_at_unix: u64,
    pub updated_at_unix: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentStatus {
    Running,
    Exited { code: Option<i32> },
    Stopped,
    Unknown,
}
