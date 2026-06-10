mod definition;
mod kind;
mod session;

pub mod providers;

pub use definition::AgentDefinition;
pub use kind::AgentKind;
pub use providers::{BUILTIN_AGENTS, CLAUDE, CODEX, PI};
pub use session::{AgentSession, AgentStatus, SpawnAgent};
