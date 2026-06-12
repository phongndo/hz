use crate::{
    CliResult,
    args::{AiCliArg, DaemonArgs, DaemonCommand},
    write_stdout,
};

use hz_core::HzError;

use std::{env, fs, path::PathBuf};

pub(crate) fn daemon(args: DaemonArgs) -> CliResult<()> {
    match args.command {
        DaemonCommand::Start => {
            if hz_daemon::start()? {
                write_stdout(format_args!("hz daemon started\n"))?;
            } else {
                write_stdout(format_args!("hz daemon already running\n"))?;
            }
        }
        DaemonCommand::Stop => {
            if hz_daemon::stop()? {
                write_stdout(format_args!("hz daemon stopped\n"))?;
            } else {
                write_stdout(format_args!("hz daemon not running\n"))?;
            }
        }
        DaemonCommand::Status => match hz_daemon::status()? {
            Some(status) => write_stdout(format_args!("{status}\n"))?,
            None => write_stdout(format_args!("hz daemon stopped\n"))?,
        },
        DaemonCommand::Run(args) => {
            let cwd = resolve_agent_cwd(args.cwd)?;
            let session = hz_daemon::spawn_agent(hz_agent::SpawnAgent {
                kind: agent_kind(args.cli),
                name: args.name,
                cwd: Some(cwd),
                args: args.args,
            })?;
            write_stdout(format_args!(
                "started {} pid={} log={}\n",
                session.id,
                session.pid,
                session.log_path.display()
            ))?;
        }
        DaemonCommand::Agents => {
            for session in hz_daemon::list_agents()? {
                write_stdout(format_args!(
                    "{}\t{}\tpid={}\t{}\t{}\n",
                    session.id,
                    agent_label(session.kind),
                    session.pid,
                    status_label(&session.status),
                    session.log_path.display()
                ))?;
            }
        }
        DaemonCommand::StopAgent(args) => {
            let session = hz_daemon::stop_agent(&args.session)?;
            write_stdout(format_args!("stopped {}\n", session.id))?;
        }
        DaemonCommand::Logs(args) => {
            let path = hz_daemon::agent_log_path(&args.session)?;
            let contents = fs::read_to_string(&path).map_err(|error| {
                HzError::Usage(format!("failed to read {}: {error}", path.display()))
            })?;
            write_stdout(format_args!("{contents}"))?;
        }
        DaemonCommand::Send(args) => {
            let session = hz_daemon::send_agent_input(&args.session, args.text.join(" "))?;
            write_stdout(format_args!("sent input to {}\n", session.id))?;
        }
        DaemonCommand::Attach(args) => {
            let session = hz_daemon::attach(args.session)?;
            write_stdout(format_args!("attached {session}\n"))?;
        }
        DaemonCommand::Detach(args) => {
            hz_daemon::detach(&args.session)?;
            write_stdout(format_args!("detached {}\n", args.session))?;
        }
        DaemonCommand::Serve => hz_daemon::run_foreground()?,
    }

    Ok(())
}

fn agent_kind(kind: AiCliArg) -> hz_agent::AgentKind {
    match kind {
        AiCliArg::Pi => hz_agent::AgentKind::Pi,
        AiCliArg::Codex => hz_agent::AgentKind::Codex,
        AiCliArg::Claude => hz_agent::AgentKind::Claude,
    }
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
}
