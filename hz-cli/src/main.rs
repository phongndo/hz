mod args;
mod complete;
mod lifecycle;
mod removal;
mod repo_shell;
#[cfg(test)]
mod tests;
mod tree_sitter;
mod update;
mod worktree_output;

use std::{
    fmt,
    io::{self, IsTerminal, Write},
    process::ExitCode,
};

use clap::Parser;
use hz_core::{HzError, HzResult};

use crate::{
    args::{Cli, Command, WorktreeCommand},
    complete::complete,
    lifecycle::run_lifecycle,
    removal::{handoff_worktree, remove_worktree},
    repo_shell::{init_repo_or_shell, install_shell, shell_script},
    tree_sitter::{diff_options, tree_sitter},
    update::update,
    worktree_output::{StyleColor, create_worktree, list_worktrees, path_worktree, styled},
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
        None => {
            let mut command = <Cli as clap::CommandFactory>::command();
            let mut stdout = io::stdout().lock();
            command
                .write_help(&mut stdout)
                .map_err(stdout_write_error)?;
            stdout.write_all(b"\n").map_err(stdout_write_error)?;
            Ok(())
        }
        Some(Command::Worktree { command }) => match command {
            WorktreeCommand::New(args) => create_worktree(args),
            WorktreeCommand::Path(args) => path_worktree(args),
            WorktreeCommand::List(args) => list_worktrees(args),
            WorktreeCommand::Remove(args) => remove_worktree(args),
            WorktreeCommand::Handoff(args) => handoff_worktree(args),
        },
        Some(Command::New(args)) => create_worktree(args),
        Some(Command::Path(args)) => path_worktree(args),
        Some(Command::List(args)) => list_worktrees(args),
        Some(Command::Remove(args)) => remove_worktree(args),
        Some(Command::Handoff(args)) => handoff_worktree(args),
        Some(Command::Init(args)) => init_repo_or_shell(args),
        Some(Command::Install(args)) => install_shell(args),
        Some(Command::Setup(args)) => run_lifecycle(args, hz_command::LifecycleKind::Setup),
        Some(Command::Cleanup(args)) => run_lifecycle(args, hz_command::LifecycleKind::Cleanup),
        Some(Command::Shell(args)) => shell_script(args),
        Some(Command::Update(args)) => update(args),
        Some(Command::Diff(args)) => {
            let stat = args.stat;
            let live_updates = !args.no_watch;
            let syntax_enabled = !args.no_syntax;
            let options = diff_options(args)?;
            if io::stdout().is_terminal() && !stat {
                hz_tui::run_diff_with_live_updates_and_syntax(
                    options,
                    live_updates,
                    syntax_enabled,
                )?;
                Ok(())
            } else {
                let output = hz_command::diff_bytes(options)?;
                write_stdout_bytes(&output)
            }
        }
        Some(Command::TreeSitter { command }) => tree_sitter(command),
        Some(Command::Complete(args)) => complete(args),
    }
}

fn write_stdout_bytes(output: &[u8]) -> CliResult<()> {
    write_all_ignore_broken_pipe(io::stdout().lock(), output)
}

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
