use crate::{AgentDefinition, AgentKind};

pub const CLAUDE: AgentDefinition = AgentDefinition {
    kind: AgentKind::Claude,
    label: "claude",
    command: "claude",
};
