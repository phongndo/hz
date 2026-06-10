use std::{
    collections::{BTreeSet, HashMap},
    process,
    time::Instant,
};

use hz_agent::SpawnAgent;
use hz_core::{HzError, HzResult};

use crate::{
    agent_process::RunningAgent,
    agent_store::AgentStore,
    process::unix_seconds,
    protocol::{DaemonReply, SendAgentInput, json_payload},
};

#[derive(Debug)]
pub(crate) struct DaemonState {
    sessions: BTreeSet<String>,
    pub(crate) agents: AgentStore,
    pub(crate) children: HashMap<String, RunningAgent>,
    pid: u32,
    started_at_unix: u64,
    started: Instant,
}

impl DaemonState {
    pub(crate) fn new() -> HzResult<Self> {
        Ok(Self {
            sessions: BTreeSet::new(),
            agents: AgentStore::load()?,
            children: HashMap::new(),
            pid: process::id(),
            started_at_unix: unix_seconds()?,
            started: Instant::now(),
        })
    }

    pub(crate) fn handle(&mut self, line: &str) -> DaemonReply {
        self.refresh_agents();

        let mut parts = line.split_whitespace();
        let Some(command) = parts.next() else {
            return DaemonReply::error("empty request");
        };

        match command {
            "PING" => DaemonReply::ok(format!(
                "hz-daemon pid {} sessions {}",
                self.pid,
                self.sessions.len()
            )),
            "STATUS" => DaemonReply::ok(format!(
                "hz daemon running pid={} sessions={} started_at={} uptime_ms={}",
                self.pid,
                self.sessions.len(),
                self.started_at_unix,
                self.started.elapsed().as_millis()
            )),
            "ATTACH" => match exactly_one_arg(parts) {
                Ok(session) if valid_session(session) => {
                    self.sessions.insert(session.to_owned());
                    DaemonReply::ok(format!(
                        "attached {session} sessions {}",
                        self.sessions.len()
                    ))
                }
                Ok(_) => DaemonReply::error("invalid session id"),
                Err(message) => DaemonReply::error(message),
            },
            "DETACH" => match exactly_one_arg(parts) {
                Ok(session) if valid_session(session) => {
                    self.sessions.remove(session);
                    DaemonReply::ok(format!(
                        "detached {session} sessions {}",
                        self.sessions.len()
                    ))
                }
                Ok(_) => DaemonReply::error("invalid session id"),
                Err(message) => DaemonReply::error(message),
            },
            "SPAWN_AGENT" => match json_payload(line, "SPAWN_AGENT")
                .and_then(|payload| {
                    serde_json::from_str::<SpawnAgent>(payload).map_err(|error| error.to_string())
                })
                .and_then(|input| self.spawn_agent(input).map_err(|error| error.to_string()))
                .and_then(|session| {
                    serde_json::to_string(&session).map_err(|error| error.to_string())
                }) {
                Ok(payload) => DaemonReply::ok(payload),
                Err(message) => DaemonReply::error(message),
            },
            "LIST_AGENTS" => match self
                .list_agents()
                .and_then(|agents| serde_json::to_string(&agents).map_err(HzError::from))
            {
                Ok(payload) => DaemonReply::ok(payload),
                Err(error) => DaemonReply::error(error.to_string()),
            },
            "STOP_AGENT" => match exactly_one_arg(parts)
                .map(str::to_owned)
                .map_err(str::to_owned)
                .and_then(|id| self.stop_agent(id).map_err(|error| error.to_string()))
                .and_then(|session| {
                    serde_json::to_string(&session).map_err(|error| error.to_string())
                }) {
                Ok(payload) => DaemonReply::ok(payload),
                Err(message) => DaemonReply::error(message),
            },
            "SEND_AGENT_INPUT" => match json_payload(line, "SEND_AGENT_INPUT")
                .and_then(|payload| {
                    serde_json::from_str::<SendAgentInput>(payload)
                        .map_err(|error| error.to_string())
                })
                .and_then(|input| {
                    self.send_agent_input(input)
                        .map_err(|error| error.to_string())
                })
                .and_then(|session| {
                    serde_json::to_string(&session).map_err(|error| error.to_string())
                }) {
                Ok(payload) => DaemonReply::ok(payload),
                Err(message) => DaemonReply::error(message),
            },
            "AGENT_LOG" => match exactly_one_arg(parts)
                .map(str::to_owned)
                .map_err(str::to_owned)
                .and_then(|id| self.agent_log_path(id).map_err(|error| error.to_string()))
                .and_then(|path| serde_json::to_string(&path).map_err(|error| error.to_string()))
            {
                Ok(payload) => DaemonReply::ok(payload),
                Err(message) => DaemonReply::error(message),
            },
            "STOP" => DaemonReply::Stop("stopping".to_owned()),
            _ => DaemonReply::error("unknown request"),
        }
    }
}

fn exactly_one_arg<'a>(mut parts: impl Iterator<Item = &'a str>) -> Result<&'a str, &'static str> {
    let Some(arg) = parts.next() else {
        return Err("missing session id");
    };
    if parts.next().is_some() {
        return Err("too many arguments");
    }
    Ok(arg)
}

fn valid_session(session: &str) -> bool {
    !session.is_empty()
        && session.len() <= 128
        && session
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_state_tracks_attach_and_detach() {
        let mut state = DaemonState::new().unwrap();

        assert_eq!(
            state.handle("ATTACH session-1").line(),
            "OK attached session-1 sessions 1"
        );
        assert_eq!(
            state.handle("ATTACH session-1").line(),
            "OK attached session-1 sessions 1"
        );
        assert_eq!(
            state.handle("DETACH session-1").line(),
            "OK detached session-1 sessions 0"
        );
    }

    #[test]
    fn daemon_state_rejects_bad_session_ids() {
        let mut state = DaemonState::new().unwrap();

        assert_eq!(
            state.handle("ATTACH bad/session").line(),
            "ERR invalid session id"
        );
        assert_eq!(state.handle("ATTACH").line(), "ERR missing session id");
        assert_eq!(state.handle("ATTACH a b").line(), "ERR too many arguments");
    }

    #[test]
    fn session_ids_allow_only_shell_safe_bytes() {
        assert!(valid_session("session-1.ok_2"));
        assert!(!valid_session("bad/session"));
        assert!(!valid_session(""));
    }
}
