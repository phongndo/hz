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
    io::{self, IsTerminal},
    process::ExitCode,
};

use clap::Parser;
use hz_core::HzResult;

pub(crate) use args::*;
pub(crate) use complete::*;
pub(crate) use lifecycle::*;
pub(crate) use removal::*;
pub(crate) use repo_shell::*;
pub(crate) use tree_sitter::*;
pub(crate) use update::*;
pub(crate) use worktree_output::*;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!(
                "{} {error}",
                styled("hz:", StyleColor::Red, io::stderr().is_terminal())
            );
            ExitCode::from(1)
        }
    }
}

fn run() -> HzResult<()> {
    let cli = Cli::parse();

    match cli.command {
        None => {
            <Cli as clap::CommandFactory>::command().print_help()?;
            println!();
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
                hz_tui::run_diff_with_live_updates_and_syntax(options, live_updates, syntax_enabled)
            } else {
                let output = hz_command::diff(options)?;
                print!("{output}");
                Ok(())
            }
        }
        Some(Command::TreeSitter { command }) => tree_sitter(command),
        Some(Command::Complete(args)) => complete(args),
    }
}
