mod definition;
mod kind;
mod session;

pub mod providers;

pub use definition::AgentDefinition;
pub use kind::AgentKind;
pub use providers::{BUILTIN_AGENTS, CLAUDE, CODEX, PI};
pub use session::{AgentSession, AgentStatus, SpawnAgent};

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    #[test]
    fn agent_kind_definitions_match_builtin_constants() {
        assert_eq!(AgentKind::Pi.definition(), &PI);
        assert_eq!(AgentKind::Codex.definition(), &CODEX);
        assert_eq!(AgentKind::Claude.definition(), &CLAUDE);
        assert_eq!(
            BUILTIN_AGENTS
                .iter()
                .map(|agent| agent.kind)
                .collect::<Vec<_>>(),
            vec![AgentKind::Pi, AgentKind::Codex, AgentKind::Claude]
        );
    }

    #[test]
    fn spawn_agent_json_round_trips() {
        let input = SpawnAgent {
            kind: AgentKind::Claude,
            name: Some("review".to_owned()),
            cwd: Some(PathBuf::from("/repo")),
            args: vec!["--continue".to_owned()],
        };

        let json = serde_json::to_string(&input).unwrap();
        let decoded = serde_json::from_str::<SpawnAgent>(&json).unwrap();

        assert_eq!(decoded, input);
    }

    #[test]
    fn agent_session_json_round_trips() {
        let session = AgentSession {
            id: "claude-1".to_owned(),
            kind: AgentKind::Claude,
            name: Some("review".to_owned()),
            command: "claude".to_owned(),
            args: vec!["--continue".to_owned()],
            cwd: PathBuf::from("/repo"),
            pid: 42,
            log_path: PathBuf::from("/tmp/claude-1.log"),
            status: AgentStatus::Exited { code: Some(0) },
            started_at_unix: 1,
            updated_at_unix: 2,
        };

        let json = serde_json::to_string(&session).unwrap();
        let decoded = serde_json::from_str::<AgentSession>(&json).unwrap();

        assert_eq!(decoded, session);
    }
}
