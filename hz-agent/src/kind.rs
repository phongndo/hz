use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{AgentDefinition, providers};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentKind {
    Pi,
    Codex,
    Claude,
}

impl AgentKind {
    pub fn definition(self) -> &'static AgentDefinition {
        match self {
            Self::Pi => &providers::PI,
            Self::Codex => &providers::CODEX,
            Self::Claude => &providers::CLAUDE,
        }
    }

    pub fn command(self) -> &'static str {
        self.definition().command
    }

    pub fn label(self) -> &'static str {
        self.definition().label
    }
}

impl fmt::Display for AgentKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}
