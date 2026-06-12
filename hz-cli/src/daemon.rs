use crate::{
    CliResult,
    args::{DaemonArgs, DaemonCommand},
    stdout_write_error,
};

use hz_core::HzResult;

use std::{env, fmt, io, io::Write, path::PathBuf};

pub(crate) fn daemon(args: DaemonArgs) -> CliResult<()> {
    let mut backend = RealDaemon;
    let mut stdout = io::stdout().lock();

    daemon_with_backend(args, &mut backend, &mut stdout)
}

fn daemon_with_backend(
    args: DaemonArgs,
    backend: &mut impl DaemonBackend,
    stdout: &mut impl Write,
) -> CliResult<()> {
    match args.command {
        DaemonCommand::Start => {
            if backend.start()? {
                write_output(stdout, format_args!("hz daemon started\n"))?;
            } else {
                write_output(stdout, format_args!("hz daemon already running\n"))?;
            }
        }
        DaemonCommand::Stop => {
            if backend.stop()? {
                write_output(stdout, format_args!("hz daemon stopped\n"))?;
            } else {
                write_output(stdout, format_args!("hz daemon not running\n"))?;
            }
        }
        DaemonCommand::Status => match backend.status()? {
            Some(status) => write_output(stdout, format_args!("{status}\n"))?,
            None => write_output(stdout, format_args!("hz daemon stopped\n"))?,
        },
        DaemonCommand::Run(args) => {
            let cwd = resolve_agent_cwd(args.cwd)?;
            let session = backend.spawn_agent(hz_agent::SpawnAgent {
                kind: args.cli.into(),
                name: args.name,
                cwd: Some(cwd),
                args: args.args,
            })?;
            write_output(
                stdout,
                format_args!(
                    "started {} pid={} log={}\n",
                    session.id,
                    session.pid,
                    session.log_path.display()
                ),
            )?;
        }
        DaemonCommand::Agents => {
            for session in backend.list_agents()? {
                write_output(
                    stdout,
                    format_args!(
                        "{}\t{}\tpid={}\t{}\t{}\n",
                        session.id,
                        agent_label(session.kind),
                        session.pid,
                        status_label(&session.status),
                        session.log_path.display()
                    ),
                )?;
            }
        }
        DaemonCommand::StopAgent(args) => {
            let session = backend.stop_agent(&args.session)?;
            write_output(stdout, format_args!("stopped {}\n", session.id))?;
        }
        DaemonCommand::Logs(args) => {
            let contents = backend.read_agent_log(&args.session)?;
            write_output(stdout, format_args!("{contents}"))?;
        }
        DaemonCommand::Send(args) => {
            let session = backend.send_agent_input(&args.session, args.text.join(" "))?;
            write_output(stdout, format_args!("sent input to {}\n", session.id))?;
        }
        DaemonCommand::Attach(args) => {
            let session = backend.attach(args.session)?;
            write_output(stdout, format_args!("attached {session}\n"))?;
        }
        DaemonCommand::Detach(args) => {
            backend.detach(&args.session)?;
            write_output(stdout, format_args!("detached {}\n", args.session))?;
        }
        DaemonCommand::Serve => backend.run_foreground()?,
    }

    Ok(())
}

trait DaemonBackend {
    fn start(&mut self) -> HzResult<bool>;
    fn stop(&mut self) -> HzResult<bool>;
    fn status(&mut self) -> HzResult<Option<String>>;
    fn spawn_agent(&mut self, input: hz_agent::SpawnAgent) -> HzResult<hz_agent::AgentSession>;
    fn list_agents(&mut self) -> HzResult<Vec<hz_agent::AgentSession>>;
    fn stop_agent(&mut self, session: &str) -> HzResult<hz_agent::AgentSession>;
    fn read_agent_log(&mut self, session: &str) -> HzResult<String>;
    fn send_agent_input(
        &mut self,
        session: &str,
        input: String,
    ) -> HzResult<hz_agent::AgentSession>;
    fn attach(&mut self, session: Option<String>) -> HzResult<String>;
    fn detach(&mut self, session: &str) -> HzResult<()>;
    fn run_foreground(&mut self) -> HzResult<()>;
}

struct RealDaemon;

impl DaemonBackend for RealDaemon {
    fn start(&mut self) -> HzResult<bool> {
        hz_daemon::start()
    }

    fn stop(&mut self) -> HzResult<bool> {
        hz_daemon::stop()
    }

    fn status(&mut self) -> HzResult<Option<String>> {
        hz_daemon::status()
    }

    fn spawn_agent(&mut self, input: hz_agent::SpawnAgent) -> HzResult<hz_agent::AgentSession> {
        hz_daemon::spawn_agent(input)
    }

    fn list_agents(&mut self) -> HzResult<Vec<hz_agent::AgentSession>> {
        hz_daemon::list_agents()
    }

    fn stop_agent(&mut self, session: &str) -> HzResult<hz_agent::AgentSession> {
        hz_daemon::stop_agent(session)
    }

    fn read_agent_log(&mut self, session: &str) -> HzResult<String> {
        hz_daemon::read_agent_log(session)
    }

    fn send_agent_input(
        &mut self,
        session: &str,
        input: String,
    ) -> HzResult<hz_agent::AgentSession> {
        hz_daemon::send_agent_input(session, input)
    }

    fn attach(&mut self, session: Option<String>) -> HzResult<String> {
        hz_daemon::attach(session)
    }

    fn detach(&mut self, session: &str) -> HzResult<()> {
        hz_daemon::detach(session)
    }

    fn run_foreground(&mut self) -> HzResult<()> {
        hz_daemon::run_foreground()
    }
}

fn write_output(writer: &mut impl Write, args: fmt::Arguments<'_>) -> CliResult<()> {
    writer.write_fmt(args).map_err(stdout_write_error)?;
    Ok(())
}

fn agent_label(kind: hz_agent::AgentKind) -> &'static str {
    kind.label()
}

fn status_label(status: &hz_agent::AgentStatus) -> String {
    match status {
        hz_agent::AgentStatus::Running => "running".to_owned(),
        hz_agent::AgentStatus::Exited { code } => code
            .map(|code| format!("exited({code})"))
            .unwrap_or_else(|| "exited".to_owned()),
        hz_agent::AgentStatus::Stopped => "stopped".to_owned(),
        hz_agent::AgentStatus::Unknown => "unknown".to_owned(),
    }
}

fn resolve_agent_cwd(cwd: Option<PathBuf>) -> CliResult<PathBuf> {
    let current_dir = env::current_dir()?;
    Ok(match cwd {
        Some(cwd) if cwd.is_absolute() => cwd,
        Some(cwd) => current_dir.join(cwd),
        None => current_dir,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::{
        AiCliArg, DaemonAttachArgs, DaemonDetachArgs, DaemonRunArgs, DaemonSendArgs,
        DaemonSessionArgs,
    };
    use hz_agent::{AgentKind, AgentSession, AgentStatus, SpawnAgent};

    #[test]
    fn daemon_dispatches_start_stop_and_status() {
        let mut fake = FakeDaemon::new();
        fake.start_result = true;
        assert_eq!(
            run_daemon(&mut fake, DaemonCommand::Start),
            "hz daemon started\n"
        );

        fake.start_result = false;
        assert_eq!(
            run_daemon(&mut fake, DaemonCommand::Start),
            "hz daemon already running\n"
        );

        fake.stop_result = false;
        assert_eq!(
            run_daemon(&mut fake, DaemonCommand::Stop),
            "hz daemon not running\n"
        );

        fake.status_result = Some("hz daemon running".to_owned());
        assert_eq!(
            run_daemon(&mut fake, DaemonCommand::Status),
            "hz daemon running\n"
        );

        fake.status_result = None;
        assert_eq!(
            run_daemon(&mut fake, DaemonCommand::Status),
            "hz daemon stopped\n"
        );

        assert_eq!(
            fake.calls,
            vec!["start", "start", "stop", "status", "status"]
        );
    }

    #[test]
    fn daemon_run_spawns_requested_agent() {
        let mut fake = FakeDaemon::new();

        let output = run_daemon(
            &mut fake,
            DaemonCommand::Run(DaemonRunArgs {
                cli: AiCliArg::Codex,
                name: Some("review".to_owned()),
                cwd: Some(PathBuf::from("repo")),
                args: vec!["--fast".to_owned()],
            }),
        );

        assert_eq!(output, "started agent-1 pid=42 log=/tmp/agent-1.log\n");
        assert_eq!(fake.calls, vec!["spawn_agent"]);
        assert_eq!(fake.spawn_inputs.len(), 1);

        let input = &fake.spawn_inputs[0];
        assert_eq!(input.kind, AgentKind::Codex);
        assert_eq!(input.name.as_deref(), Some("review"));
        assert_eq!(input.cwd, Some(env::current_dir().unwrap().join("repo")));
        assert_eq!(input.args, vec!["--fast"]);
    }

    #[test]
    fn daemon_agents_lists_sessions() {
        let mut fake = FakeDaemon::new();
        fake.list_agents_result = vec![
            agent_session("agent-1", AgentKind::Pi, AgentStatus::Running),
            agent_session(
                "agent-2",
                AgentKind::Claude,
                AgentStatus::Exited { code: None },
            ),
        ];

        let output = run_daemon(&mut fake, DaemonCommand::Agents);

        assert_eq!(
            output,
            "agent-1\tpi\tpid=42\trunning\t/tmp/agent-1.log\n\
             agent-2\tclaude\tpid=42\texited\t/tmp/agent-2.log\n"
        );
        assert_eq!(fake.calls, vec!["list_agents"]);
    }

    #[test]
    fn daemon_dispatches_session_commands() {
        let mut fake = FakeDaemon::new();
        fake.read_log_result = "log line\n".to_owned();
        fake.attach_result = "client-1".to_owned();

        assert_eq!(
            run_daemon(
                &mut fake,
                DaemonCommand::StopAgent(DaemonSessionArgs {
                    session: "agent-1".to_owned(),
                }),
            ),
            "stopped agent-1\n"
        );
        assert_eq!(
            run_daemon(
                &mut fake,
                DaemonCommand::Logs(DaemonSessionArgs {
                    session: "agent-1".to_owned(),
                }),
            ),
            "log line\n"
        );
        assert_eq!(
            run_daemon(
                &mut fake,
                DaemonCommand::Send(DaemonSendArgs {
                    session: "agent-1".to_owned(),
                    text: vec!["hello".to_owned(), "daemon".to_owned()],
                }),
            ),
            "sent input to agent-1\n"
        );
        assert_eq!(
            run_daemon(
                &mut fake,
                DaemonCommand::Attach(DaemonAttachArgs {
                    session: Some("client-1".to_owned()),
                }),
            ),
            "attached client-1\n"
        );
        assert_eq!(
            run_daemon(
                &mut fake,
                DaemonCommand::Detach(DaemonDetachArgs {
                    session: "client-1".to_owned(),
                }),
            ),
            "detached client-1\n"
        );

        assert_eq!(
            fake.calls,
            vec![
                "stop_agent",
                "read_agent_log",
                "send_agent_input",
                "attach",
                "detach"
            ]
        );
        assert_eq!(fake.stop_agent_sessions, vec!["agent-1"]);
        assert_eq!(fake.read_log_sessions, vec!["agent-1"]);
        assert_eq!(
            fake.sent_inputs,
            vec![("agent-1".to_owned(), "hello daemon".to_owned())]
        );
        assert_eq!(fake.attached_sessions, vec![Some("client-1".to_owned())]);
        assert_eq!(fake.detached_sessions, vec!["client-1"]);
    }

    #[test]
    fn daemon_serve_runs_foreground_backend() {
        let mut fake = FakeDaemon::new();

        assert_eq!(run_daemon(&mut fake, DaemonCommand::Serve), "");
        assert_eq!(fake.calls, vec!["run_foreground"]);
    }

    #[test]
    fn daemon_run_cwd_defaults_to_client_current_dir() {
        assert_eq!(
            resolve_agent_cwd(None).unwrap(),
            env::current_dir().unwrap()
        );
    }

    #[test]
    fn daemon_run_cwd_resolves_relative_to_client_current_dir() {
        let cwd = resolve_agent_cwd(Some(PathBuf::from("relative"))).unwrap();

        assert!(cwd.is_absolute());
        assert_eq!(cwd, env::current_dir().unwrap().join("relative"));
    }

    fn run_daemon(fake: &mut FakeDaemon, command: DaemonCommand) -> String {
        let mut output = Vec::new();
        daemon_with_backend(DaemonArgs { command }, fake, &mut output).unwrap();
        String::from_utf8(output).unwrap()
    }

    struct FakeDaemon {
        calls: Vec<&'static str>,
        start_result: bool,
        stop_result: bool,
        status_result: Option<String>,
        spawn_inputs: Vec<SpawnAgent>,
        spawn_result: AgentSession,
        list_agents_result: Vec<AgentSession>,
        stop_agent_sessions: Vec<String>,
        stop_agent_result: AgentSession,
        read_log_sessions: Vec<String>,
        read_log_result: String,
        sent_inputs: Vec<(String, String)>,
        send_result: AgentSession,
        attached_sessions: Vec<Option<String>>,
        attach_result: String,
        detached_sessions: Vec<String>,
    }

    impl FakeDaemon {
        fn new() -> Self {
            let session = agent_session("agent-1", AgentKind::Pi, AgentStatus::Running);

            Self {
                calls: Vec::new(),
                start_result: true,
                stop_result: true,
                status_result: None,
                spawn_inputs: Vec::new(),
                spawn_result: session.clone(),
                list_agents_result: Vec::new(),
                stop_agent_sessions: Vec::new(),
                stop_agent_result: session.clone(),
                read_log_sessions: Vec::new(),
                read_log_result: String::new(),
                sent_inputs: Vec::new(),
                send_result: session,
                attached_sessions: Vec::new(),
                attach_result: String::new(),
                detached_sessions: Vec::new(),
            }
        }
    }

    impl DaemonBackend for FakeDaemon {
        fn start(&mut self) -> hz_core::HzResult<bool> {
            self.calls.push("start");
            Ok(self.start_result)
        }

        fn stop(&mut self) -> hz_core::HzResult<bool> {
            self.calls.push("stop");
            Ok(self.stop_result)
        }

        fn status(&mut self) -> hz_core::HzResult<Option<String>> {
            self.calls.push("status");
            Ok(self.status_result.clone())
        }

        fn spawn_agent(&mut self, input: SpawnAgent) -> hz_core::HzResult<AgentSession> {
            self.calls.push("spawn_agent");
            self.spawn_inputs.push(input);
            Ok(self.spawn_result.clone())
        }

        fn list_agents(&mut self) -> hz_core::HzResult<Vec<AgentSession>> {
            self.calls.push("list_agents");
            Ok(self.list_agents_result.clone())
        }

        fn stop_agent(&mut self, session: &str) -> hz_core::HzResult<AgentSession> {
            self.calls.push("stop_agent");
            self.stop_agent_sessions.push(session.to_owned());
            Ok(self.stop_agent_result.clone())
        }

        fn read_agent_log(&mut self, session: &str) -> hz_core::HzResult<String> {
            self.calls.push("read_agent_log");
            self.read_log_sessions.push(session.to_owned());
            Ok(self.read_log_result.clone())
        }

        fn send_agent_input(
            &mut self,
            session: &str,
            input: String,
        ) -> hz_core::HzResult<AgentSession> {
            self.calls.push("send_agent_input");
            self.sent_inputs.push((session.to_owned(), input));
            Ok(self.send_result.clone())
        }

        fn attach(&mut self, session: Option<String>) -> hz_core::HzResult<String> {
            self.calls.push("attach");
            self.attached_sessions.push(session);
            Ok(self.attach_result.clone())
        }

        fn detach(&mut self, session: &str) -> hz_core::HzResult<()> {
            self.calls.push("detach");
            self.detached_sessions.push(session.to_owned());
            Ok(())
        }

        fn run_foreground(&mut self) -> hz_core::HzResult<()> {
            self.calls.push("run_foreground");
            Ok(())
        }
    }

    fn agent_session(id: &str, kind: AgentKind, status: AgentStatus) -> AgentSession {
        AgentSession {
            id: id.to_owned(),
            kind,
            name: None,
            command: kind.command().to_owned(),
            args: Vec::new(),
            cwd: PathBuf::from("/tmp"),
            pid: 42,
            log_path: PathBuf::from(format!("/tmp/{id}.log")),
            status,
            started_at_unix: 0,
            updated_at_unix: 0,
        }
    }
}
