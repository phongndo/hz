use std::{
    env, fs, io,
    io::{Read, Write},
    net::Shutdown,
    os::unix::{net::UnixStream, process::CommandExt},
    path::PathBuf,
    process::{Command, Stdio},
    time::Instant,
};

use hz_agent::{AgentSession, SpawnAgent};
use hz_core::{HzError, HzResult};
use serde::Serialize;

use crate::{
    paths::{remove_stale_socket, socket_path},
    process::generate_session_id,
    protocol::{CONNECT_TIMEOUT, START_POLL, START_TIMEOUT, SendAgentInput, parse_response},
};

pub struct DaemonSession {
    session: String,
    attached: bool,
}

impl DaemonSession {
    pub fn id(&self) -> &str {
        &self.session
    }

    pub fn detach(&mut self) -> HzResult<()> {
        if !self.attached {
            return Ok(());
        }

        request(&format!("DETACH {}", self.session))?;
        self.attached = false;
        Ok(())
    }
}

impl Drop for DaemonSession {
    fn drop(&mut self) {
        let _ = self.detach();
    }
}

pub fn attach_main_session() -> HzResult<DaemonSession> {
    ensure_running()?;
    let session = generate_session_id()?;
    request(&format!("ATTACH {session}"))?;

    Ok(DaemonSession {
        session,
        attached: true,
    })
}

pub fn start() -> HzResult<bool> {
    match request("PING") {
        Ok(_) => return Ok(false),
        Err(error) if is_not_running_error(&error) => {}
        Err(error) => return Err(error),
    }

    remove_stale_socket()?;

    let current_exe = env::current_exe()?;
    let mut command = Command::new(current_exe);
    command
        .args(["daemon", "__run"])
        .env("HZ_DAEMON_INTERNAL", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0);

    command.spawn()?;
    wait_for_daemon()?;
    Ok(true)
}

pub fn stop() -> HzResult<bool> {
    match request("STOP") {
        Ok(_) => Ok(true),
        Err(error) if is_not_running_error(&error) => Ok(false),
        Err(error) => Err(error),
    }
}

pub fn status() -> HzResult<Option<String>> {
    match request("STATUS") {
        Ok(status) => Ok(Some(status)),
        Err(error) if is_not_running_error(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

pub fn attach(session: Option<String>) -> HzResult<String> {
    ensure_running()?;
    let session = session.map_or_else(generate_session_id, Ok)?;
    request(&format!("ATTACH {session}"))?;
    Ok(session)
}

pub fn detach(session: &str) -> HzResult<()> {
    request(&format!("DETACH {session}"))?;
    Ok(())
}

pub fn spawn_agent(input: SpawnAgent) -> HzResult<AgentSession> {
    ensure_running()?;
    let payload = request_json("SPAWN_AGENT", &input)?;
    Ok(serde_json::from_str(&payload)?)
}

pub fn list_agents() -> HzResult<Vec<AgentSession>> {
    ensure_running()?;
    let payload = request("LIST_AGENTS")?;
    Ok(serde_json::from_str(&payload)?)
}

pub fn stop_agent(id: &str) -> HzResult<AgentSession> {
    ensure_running()?;
    let payload = request(&format!("STOP_AGENT {id}"))?;
    Ok(serde_json::from_str(&payload)?)
}

pub fn send_agent_input(id: &str, input: String) -> HzResult<AgentSession> {
    ensure_running()?;
    let payload = request_json(
        "SEND_AGENT_INPUT",
        &SendAgentInput {
            id: id.to_owned(),
            input,
        },
    )?;
    Ok(serde_json::from_str(&payload)?)
}

pub fn agent_log_path(id: &str) -> HzResult<PathBuf> {
    ensure_running()?;
    let payload = request(&format!("AGENT_LOG {id}"))?;
    Ok(serde_json::from_str(&payload)?)
}

pub fn read_agent_log(id: &str) -> HzResult<String> {
    let path = agent_log_path(id)?;
    fs::read_to_string(&path)
        .map_err(|error| HzError::Usage(format!("failed to read {}: {error}", path.display())))
}

pub(crate) fn request(line: &str) -> HzResult<String> {
    let socket_path = socket_path()?;
    let mut stream = UnixStream::connect(socket_path)?;
    let _ = stream.set_read_timeout(Some(CONNECT_TIMEOUT));
    let _ = stream.set_write_timeout(Some(CONNECT_TIMEOUT));

    stream.write_all(line.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.shutdown(Shutdown::Write)?;

    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    parse_response(response.trim_end())
}

pub(crate) fn is_not_running_error(error: &HzError) -> bool {
    matches!(error, HzError::Io(error) if not_running_error_kind(error.kind()))
}

fn ensure_running() -> HzResult<()> {
    if request("PING").is_ok() {
        return Ok(());
    }

    start()?;
    Ok(())
}

fn wait_for_daemon() -> HzResult<()> {
    let started = Instant::now();
    let mut last_error = None;

    while started.elapsed() < START_TIMEOUT {
        match request("PING") {
            Ok(_) => return Ok(()),
            Err(error) => last_error = Some(error),
        }

        std::thread::sleep(START_POLL);
    }

    Err(HzError::Usage(format!(
        "hz daemon did not become ready{}",
        last_error
            .map(|error| format!(": {error}"))
            .unwrap_or_default()
    )))
}

fn request_json<T: Serialize>(command: &str, value: &T) -> HzResult<String> {
    request(&format!("{command} {}", serde_json::to_string(value)?))
}

fn not_running_error_kind(kind: io::ErrorKind) -> bool {
    matches!(
        kind,
        io::ErrorKind::NotFound
            | io::ErrorKind::ConnectionRefused
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::BrokenPipe
    )
}
