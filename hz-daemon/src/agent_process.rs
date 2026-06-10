use std::{
    env, fs,
    io::Write,
    os::unix::process::CommandExt,
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
};

use hz_agent::{AgentSession, AgentStatus, SpawnAgent};
use hz_core::{HzError, HzResult};

use crate::{
    paths::agents_dir,
    process::{generate_agent_id, process_is_running, terminate_process_group, unix_seconds},
    protocol::SendAgentInput,
    state::DaemonState,
};

#[derive(Debug)]
pub(crate) struct RunningAgent {
    child: Child,
    stdin: Option<ChildStdin>,
}

impl DaemonState {
    pub(crate) fn spawn_agent(&mut self, input: SpawnAgent) -> HzResult<AgentSession> {
        let id = generate_agent_id(input.kind)?;
        let command_name = input.kind.command().to_owned();
        let cwd = input.cwd.unwrap_or(env::current_dir()?);
        let agents_dir = agents_dir()?;
        fs::create_dir_all(&agents_dir)?;
        let log_path = agents_dir.join(format!("{id}.log"));
        let log = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)?;
        let started_at_unix = unix_seconds()?;

        let mut command = Command::new(&command_name);
        command
            .args(&input.args)
            .current_dir(&cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(Stdio::from(log))
            .process_group(0);

        let mut child = command.spawn()?;
        let stdin = child.stdin.take();
        let pid = child.id();
        let session = AgentSession {
            id: id.clone(),
            kind: input.kind,
            name: input.name,
            command: command_name,
            args: input.args,
            cwd,
            pid,
            log_path,
            status: AgentStatus::Running,
            started_at_unix,
            updated_at_unix: started_at_unix,
        };

        self.children
            .insert(id.clone(), RunningAgent { child, stdin });
        self.agents.agents.insert(id, session.clone());
        self.agents.save()?;
        Ok(session)
    }

    pub(crate) fn list_agents(&mut self) -> HzResult<Vec<AgentSession>> {
        self.refresh_agents();
        self.agents.save()?;
        Ok(self.agents.agents.values().cloned().collect())
    }

    pub(crate) fn stop_agent(&mut self, id: String) -> HzResult<AgentSession> {
        let session = self
            .agents
            .agents
            .get_mut(&id)
            .ok_or_else(|| HzError::Usage(format!("unknown agent session: {id}")))?;

        terminate_process_group(session.pid)?;
        if let Some(mut running) = self.children.remove(&id) {
            let _ = running.child.kill();
            let _ = running.child.wait();
        }
        session.status = AgentStatus::Stopped;
        session.updated_at_unix = unix_seconds()?;
        let session = session.clone();
        self.agents.save()?;
        Ok(session)
    }

    pub(crate) fn send_agent_input(&mut self, input: SendAgentInput) -> HzResult<AgentSession> {
        let running = self.children.get_mut(&input.id).ok_or_else(|| {
            HzError::Usage(format!(
                "agent session is not attached to this daemon: {}",
                input.id
            ))
        })?;
        let stdin = running.stdin.as_mut().ok_or_else(|| {
            HzError::Usage(format!("agent session does not accept input: {}", input.id))
        })?;
        stdin.write_all(input.input.as_bytes())?;
        stdin.write_all(b"\n")?;
        stdin.flush()?;

        self.agents
            .agents
            .get(&input.id)
            .cloned()
            .ok_or_else(|| HzError::Usage(format!("unknown agent session: {}", input.id)))
    }

    pub(crate) fn agent_log_path(&self, id: String) -> HzResult<PathBuf> {
        self.agents
            .agents
            .get(&id)
            .map(|session| session.log_path.clone())
            .ok_or_else(|| HzError::Usage(format!("unknown agent session: {id}")))
    }

    pub(crate) fn refresh_agents(&mut self) {
        let mut changed = false;
        let mut finished = Vec::new();
        for (id, running) in &mut self.children {
            match running.child.try_wait() {
                Ok(Some(status)) => {
                    if let Some(session) = self.agents.agents.get_mut(id) {
                        session.status = AgentStatus::Exited {
                            code: status.code(),
                        };
                        if let Ok(now) = unix_seconds() {
                            session.updated_at_unix = now;
                        }
                        changed = true;
                    }
                    finished.push(id.clone());
                }
                Ok(None) => {}
                Err(_) => {}
            }
        }

        for id in finished {
            self.children.remove(&id);
        }

        for session in self.agents.agents.values_mut() {
            if session.status == AgentStatus::Running
                && !self.children.contains_key(&session.id)
                && !process_is_running(session.pid)
            {
                session.status = AgentStatus::Unknown;
                if let Ok(now) = unix_seconds() {
                    session.updated_at_unix = now;
                }
                changed = true;
            }
        }

        if changed {
            let _ = self.agents.save();
        }
    }
}
