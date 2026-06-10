use crate::AgentKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentDefinition {
    pub kind: AgentKind,
    pub label: &'static str,
    pub command: &'static str,
}
