mod agent;
mod args;
mod complete;
mod lifecycle;
mod removal;
mod repo_shell;
#[cfg(test)]
mod tests;
mod update;
mod worktree_output;

use std::{
    fmt,
    io::{self, IsTerminal, Write},
    process::ExitCode,
};

use clap::{CommandFactory, Parser};
use hz_core::{HzError, HzResult};

use crate::{
    agent::run_agent_command,
    args::{Cli, Command, WorktreeCommand},
    complete::complete,
    lifecycle::run_lifecycle,
    removal::{handoff_worktree, remove_worktree},
    repo_shell::{init_repo_or_shell, install_shell, shell_script},
    update::update,
    worktree_output::{
        StyleColor, create_worktree, fork_worktree, list_worktrees, path_worktree, pwd_worktree,
        styled,
    },
};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) if is_clean_exit_error(&error) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = write_stderr(format_args!(
                "{} {error}\n",
                styled("hz:", StyleColor::Red, io::stderr().is_terminal())
            ));
            ExitCode::from(1)
        }
    }
}

pub(crate) type CliResult<T> = Result<T, CliError>;

#[derive(Debug)]
pub(crate) enum CliError {
    Hz(HzError),
    StdoutBrokenPipe,
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Hz(error) => write!(formatter, "{error}"),
            Self::StdoutBrokenPipe => write!(formatter, "broken pipe"),
        }
    }
}

impl From<HzError> for CliError {
    fn from(error: HzError) -> Self {
        Self::Hz(error)
    }
}

impl From<io::Error> for CliError {
    fn from(error: io::Error) -> Self {
        Self::Hz(error.into())
    }
}

impl From<serde_json::Error> for CliError {
    fn from(error: serde_json::Error) -> Self {
        Self::Hz(error.into())
    }
}

pub(crate) fn write_stdout(args: fmt::Arguments<'_>) -> CliResult<()> {
    io::stdout()
        .lock()
        .write_fmt(args)
        .map_err(stdout_write_error)?;
    Ok(())
}

pub(crate) fn write_stderr(args: fmt::Arguments<'_>) -> HzResult<()> {
    io::stderr().lock().write_fmt(args)?;
    Ok(())
}

fn stdout_write_error(error: io::Error) -> CliError {
    if error.kind() == io::ErrorKind::BrokenPipe {
        CliError::StdoutBrokenPipe
    } else {
        error.into()
    }
}

fn is_clean_exit_error(error: &CliError) -> bool {
    matches!(error, CliError::StdoutBrokenPipe)
}

fn run() -> CliResult<()> {
    let cli = Cli::parse();

    match cli.command {
        None => write_default_help(io::stdout().lock()),
        Some(Command::Worktree { command }) => run_worktree_command(command),
        Some(Command::Agent { command }) => run_agent_command(command),
        Some(Command::New(args)) => create_worktree(args),
        Some(Command::Fork(args)) => fork_worktree(args),
        Some(Command::Path(args)) => path_worktree(args),
        Some(Command::List(args)) => list_worktrees(args),
        Some(Command::Pwd(args)) => pwd_worktree(args),
        Some(Command::Remove(args)) => remove_worktree(args),
        Some(Command::Handoff(args)) => handoff_worktree(args),
        Some(Command::Init(args)) => init_repo_or_shell(args),
        Some(Command::Install(args)) => install_shell(args),
        Some(Command::Setup(args)) => run_lifecycle(args, hz_command::LifecycleKind::Setup),
        Some(Command::Cleanup(args)) => run_lifecycle(args, hz_command::LifecycleKind::Cleanup),
        Some(Command::Shell(args)) => shell_script(args),
        Some(Command::Update(args)) => update(args),
        Some(Command::Complete(args)) => complete(args),
    }
}

fn run_worktree_command(command: WorktreeCommand) -> CliResult<()> {
    match command {
        WorktreeCommand::New(args) => create_worktree(args),
        WorktreeCommand::Fork(args) => fork_worktree(args),
        WorktreeCommand::Path(args) => path_worktree(args),
        WorktreeCommand::List(args) => list_worktrees(args),
        WorktreeCommand::Pwd(args) => pwd_worktree(args),
        WorktreeCommand::Remove(args) => remove_worktree(args),
        WorktreeCommand::Handoff(args) => handoff_worktree(args),
    }
}

fn write_default_help(mut writer: impl Write) -> CliResult<()> {
    let mut command = Cli::command();
    command
        .write_help(&mut writer)
        .map_err(stdout_write_error)?;
    writer.write_all(b"\n").map_err(stdout_write_error)?;
    Ok(())
}

#[cfg(test)]
fn write_all_ignore_broken_pipe(mut writer: impl Write, bytes: &[u8]) -> CliResult<()> {
    match writer.write_all(bytes) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => return Ok(()),
        Err(error) => return Err(stdout_write_error(error)),
    }

    match writer.flush() {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(error) => Err(stdout_write_error(error)),
    }
}
