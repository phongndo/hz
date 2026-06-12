pub mod claude;
pub mod codex;
pub mod pi;

pub use claude::CLAUDE;
pub use codex::CODEX;
pub use pi::PI;

use crate::AgentDefinition;

pub const BUILTIN_AGENTS: &[AgentDefinition] = &[PI, CODEX, CLAUDE];
