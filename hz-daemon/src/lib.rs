mod protocol;

#[cfg(unix)]
mod agent_process;
#[cfg(unix)]
mod agent_store;
#[cfg(unix)]
mod client;
#[cfg(unix)]
mod paths;
#[cfg(unix)]
mod process;
#[cfg(unix)]
mod server;
#[cfg(unix)]
mod state;

#[cfg(not(unix))]
mod unsupported;

#[cfg(unix)]
pub use client::{
    DaemonSession, agent_log_path, attach, attach_main_session, detach, list_agents,
    read_agent_log, send_agent_input, spawn_agent, start, status, stop, stop_agent,
};
#[cfg(unix)]
pub use server::run_foreground;

#[cfg(not(unix))]
pub use unsupported::{
    DaemonSession, agent_log_path, attach, attach_main_session, detach, list_agents,
    read_agent_log, run_foreground, send_agent_input, spawn_agent, start, status, stop, stop_agent,
};

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    use std::{
        env,
        ffi::{OsStr, OsString},
        fs,
        io::{BufRead, BufReader, Write},
        os::unix::net::UnixListener,
        path::{Path, PathBuf},
        sync::Mutex,
        thread,
        time::{SystemTime, UNIX_EPOCH},
    };

    use hz_agent::{AgentKind, AgentSession, AgentStatus, SpawnAgent};

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn public_api_round_trips_to_daemon_protocol() {
        let _lock = ENV_LOCK.lock().unwrap();
        let runtime = temp_dir("hz-daemon-public-api");
        let _env = EnvGuard::new(&runtime);
        crate::paths::prepare_runtime_dir().unwrap();

        let log_path = runtime.join("agent-1.log");
        fs::write(&log_path, "log line\n").unwrap();

        let session = agent_session(&log_path);
        let session_json = serde_json::to_string(&session).unwrap();
        let sessions_json = serde_json::to_string(&vec![session.clone()]).unwrap();
        let log_path_json = serde_json::to_string(&log_path).unwrap();
        let spawn_input = SpawnAgent {
            kind: AgentKind::Pi,
            name: Some("review".to_owned()),
            cwd: Some(runtime.clone()),
            args: vec!["--fast".to_owned()],
        };

        let server = fake_daemon(vec![
            expect("PING", "OK pong"),
            expect("STATUS", "OK ready"),
            expect("STOP", "OK stopping"),
            expect("PING", "OK pong"),
            expect(
                format!(
                    "SPAWN_AGENT {}",
                    serde_json::to_string(&spawn_input).unwrap()
                ),
                format!("OK {session_json}"),
            ),
            expect("PING", "OK pong"),
            expect("LIST_AGENTS", format!("OK {sessions_json}")),
            expect("PING", "OK pong"),
            expect("STOP_AGENT agent-1", format!("OK {session_json}")),
            expect("PING", "OK pong"),
            expect(
                "SEND_AGENT_INPUT {\"id\":\"agent-1\",\"input\":\"hello\"}",
                format!("OK {session_json}"),
            ),
            expect("PING", "OK pong"),
            expect("AGENT_LOG agent-1", format!("OK {log_path_json}")),
            expect("PING", "OK pong"),
            expect("AGENT_LOG agent-1", format!("OK {log_path_json}")),
            expect("PING", "OK pong"),
            expect("ATTACH client-1", "OK attached client-1"),
            expect("DETACH client-1", "OK detached client-1"),
        ]);

        assert!(!start().unwrap());
        assert_eq!(status().unwrap().as_deref(), Some("ready"));
        assert!(stop().unwrap());
        assert_eq!(spawn_agent(spawn_input).unwrap().id, "agent-1");
        assert_eq!(list_agents().unwrap().len(), 1);
        assert_eq!(stop_agent("agent-1").unwrap().id, "agent-1");
        assert_eq!(
            send_agent_input("agent-1", "hello".to_owned()).unwrap().id,
            "agent-1"
        );
        assert_eq!(agent_log_path("agent-1").unwrap(), log_path);
        assert_eq!(read_agent_log("agent-1").unwrap(), "log line\n");
        assert_eq!(attach(Some("client-1".to_owned())).unwrap(), "client-1");
        detach("client-1").unwrap();

        server.join().unwrap();
    }

    fn fake_daemon(expectations: Vec<ExpectedRequest>) -> thread::JoinHandle<()> {
        let listener = UnixListener::bind(crate::paths::socket_path().unwrap()).unwrap();

        thread::spawn(move || {
            for expected in expectations {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = String::new();
                BufReader::new(stream.try_clone().unwrap())
                    .read_line(&mut request)
                    .unwrap();
                let request = request.trim_end_matches(['\r', '\n']);

                assert_eq!(request, expected.request);
                stream.write_all(expected.response.as_bytes()).unwrap();
                stream.write_all(b"\n").unwrap();
            }
        })
    }

    struct ExpectedRequest {
        request: String,
        response: String,
    }

    fn expect(request: impl Into<String>, response: impl Into<String>) -> ExpectedRequest {
        ExpectedRequest {
            request: request.into(),
            response: response.into(),
        }
    }

    fn agent_session(log_path: &Path) -> AgentSession {
        AgentSession {
            id: "agent-1".to_owned(),
            kind: AgentKind::Pi,
            name: Some("review".to_owned()),
            command: "pi".to_owned(),
            args: vec!["--fast".to_owned()],
            cwd: log_path.parent().unwrap().to_path_buf(),
            pid: 42,
            log_path: log_path.to_path_buf(),
            status: AgentStatus::Running,
            started_at_unix: 1,
            updated_at_unix: 1,
        }
    }

    fn temp_dir(prefix: &str) -> PathBuf {
        PathBuf::from("/tmp").join(format!(
            "{prefix}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    struct EnvGuard {
        runtime: PathBuf,
        previous_runtime: Option<OsString>,
        previous_state: Option<OsString>,
    }

    impl EnvGuard {
        fn new(runtime: &Path) -> Self {
            let previous_runtime = env::var_os("HZ_RUNTIME_DIR");
            let previous_state = env::var_os("HZ_STATE_DIR");

            unsafe {
                env::set_var("HZ_RUNTIME_DIR", runtime);
                env::set_var("HZ_STATE_DIR", runtime.join("state"));
            }

            Self {
                runtime: runtime.to_path_buf(),
                previous_runtime,
                previous_state,
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            restore_env("HZ_RUNTIME_DIR", self.previous_runtime.as_deref());
            restore_env("HZ_STATE_DIR", self.previous_state.as_deref());
            let _ = fs::remove_dir_all(&self.runtime);
        }
    }

    fn restore_env(name: &str, value: Option<&OsStr>) {
        unsafe {
            match value {
                Some(value) => env::set_var(name, value),
                None => env::remove_var(name),
            }
        }
    }
}
