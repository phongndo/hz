mod args;
mod complete;
mod repo_shell;
#[cfg(test)]
mod tests;
mod update;
mod workspace_output;

use std::{
    fmt,
    io::{self, Write},
    process::ExitCode,
    sync::atomic::{AtomicBool, Ordering},
};

use clap::{CommandFactory, Parser};
use hz_core::{HzError, HzResult};

use crate::{
    args::{Cli, Command, ConfigCommand, GitCommand, HgCommand},
    complete::complete,
    repo_shell::{install_shell, shell_script},
    update::update,
    workspace_output::{
        adopt, ancestors, config_init, doctor, gc, git_handoff, git_status, init_workspace,
        list_workspaces, mercurial_status, new_workspace, path_workspace, pin_workspaces,
        pwd_workspace, remove_workspace, restore_workspace,
    },
};

static MACHINE_OUTPUT: AtomicBool = AtomicBool::new(false);

fn main() -> ExitCode {
    let machine = machine_requested(std::env::args_os());
    MACHINE_OUTPUT.store(machine, Ordering::Relaxed);
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) if machine && error.exit_code() != 0 => {
            return finish(Err(CliError::from(HzError::Usage(error.to_string()))));
        }
        Err(error) => error.exit(),
    };
    finish(run(cli))
}

fn finish(result: CliResult<()>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) if is_clean_exit_error(&error) => ExitCode::SUCCESS,
        Err(error) => {
            if MACHINE_OUTPUT.load(Ordering::Relaxed) {
                let (code, path) = machine_error_fields(&error);
                let value = serde_json::json!({
                    "status": "error",
                    "error": {
                        "code": code,
                        "message": error.to_string(),
                        "path": path,
                    }
                });
                let _ = write_stderr(format_args!("{value}\n"));
            } else {
                let _ = write_stderr(format_args!("hz: {error}\n"));
            }
            ExitCode::from(1)
        }
    }
}

fn machine_requested<I, S>(args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    for argument in args.into_iter().skip(1) {
        let argument = argument.as_ref();
        if argument == "--" {
            break;
        }
        if argument == "--machine" {
            return true;
        }
    }
    false
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

fn run(cli: Cli) -> CliResult<()> {
    let machine = cli.machine;
    match cli.command {
        None => write_default_help(io::stdout().lock()),
        Some(Command::Init(args)) => init_workspace(args, machine),
        Some(Command::New(args)) => new_workspace(args, machine),
        Some(Command::Path(args)) => path_workspace(args, machine),
        Some(Command::List(args)) => list_workspaces(args, machine),
        Some(Command::Pwd(args)) => pwd_workspace(args, machine),
        Some(Command::Ancestors(args)) => ancestors(args, machine),
        Some(Command::Remove(args)) => remove_workspace(args, machine),
        Some(Command::Pin(args)) => pin_workspaces(args, machine, true),
        Some(Command::Unpin(args)) => pin_workspaces(args, machine, false),
        Some(Command::Restore(args)) => restore_workspace(args, machine),
        Some(Command::Gc(args)) => gc(args, machine),
        Some(Command::Adopt(args)) => adopt(args, machine),
        Some(Command::Doctor(args)) => doctor(args, machine),
        Some(Command::Git { command }) => match command {
            GitCommand::Status(args) => git_status(args, machine),
            GitCommand::Handoff(args) => git_handoff(args, machine),
        },
        Some(Command::Hg { command }) => match command {
            HgCommand::Status(args) => mercurial_status(args, machine),
        },
        Some(Command::Config { command }) => match command {
            ConfigCommand::Init(args) => config_init(args, machine),
        },
        Some(Command::Install(args)) => install_shell(args),
        Some(Command::Shell(args)) => shell_script(args),
        Some(Command::Update(args)) => update(args),
        Some(Command::Complete(args)) => complete(args),
    }
}

fn machine_error_fields(error: &CliError) -> (&'static str, Option<String>) {
    match error {
        CliError::Hz(HzError::Io(_)) => ("io", None),
        CliError::Hz(HzError::Json(_)) => ("json", None),
        CliError::Hz(HzError::UnknownWorkspace { .. }) => ("unknown_workspace", None),
        CliError::Hz(HzError::WorkspaceNotInitialized(path)) => (
            "workspace_not_initialized",
            Some(path.display().to_string()),
        ),
        CliError::Hz(HzError::MarkerMismatch(path)) => {
            ("marker_mismatch", Some(path.display().to_string()))
        }
        CliError::Hz(HzError::MissingMarker(path)) => {
            ("missing_marker", Some(path.display().to_string()))
        }
        CliError::Hz(HzError::CowUnavailable(_)) => ("cow_unavailable", None),
        CliError::Hz(HzError::Usage(_)) => ("usage", None),
        CliError::StdoutBrokenPipe => ("broken_pipe", None),
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
