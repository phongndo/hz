use std::path::PathBuf;

use hz_agent::{AgentSession, SpawnAgent};
use hz_core::{HzError, HzResult};

pub struct DaemonSession;

impl DaemonSession {
    pub fn id(&self) -> &str {
        ""
    }

    pub fn detach(&mut self) -> HzResult<()> {
        Ok(())
    }
}

pub fn attach_main_session() -> HzResult<DaemonSession> {
    Ok(DaemonSession)
}

pub fn start() -> HzResult<bool> {
    unsupported()
}

pub fn stop() -> HzResult<bool> {
    unsupported()
}

pub fn status() -> HzResult<Option<String>> {
    unsupported()
}

pub fn attach(_session: Option<String>) -> HzResult<String> {
    unsupported()
}

pub fn detach(_session: &str) -> HzResult<()> {
    unsupported()
}

pub fn spawn_agent(_input: SpawnAgent) -> HzResult<AgentSession> {
    unsupported()
}

pub fn list_agents() -> HzResult<Vec<AgentSession>> {
    unsupported()
}

pub fn stop_agent(_id: &str) -> HzResult<AgentSession> {
    unsupported()
}

pub fn send_agent_input(_id: &str, _input: String) -> HzResult<AgentSession> {
    unsupported()
}

pub fn agent_log_path(_id: &str) -> HzResult<PathBuf> {
    unsupported()
}

pub fn read_agent_log(_id: &str) -> HzResult<String> {
    unsupported()
}

pub fn run_foreground() -> HzResult<()> {
    unsupported()
}

fn unsupported<T>() -> HzResult<T> {
    Err(HzError::Usage(
        "hz daemon is only supported on Unix".to_owned(),
    ))
}
