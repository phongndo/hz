use std::{path::PathBuf, process::ExitCode};

use clap::{Args, Parser, Subcommand};
use hz_core::HzResult;

#[derive(Debug, Parser)]
#[command(name = "hz", version, about = "Parallel agent workspace CLI")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    #[command(alias = "wt")]
    Worktree {
        #[command(subcommand)]
        command: WorktreeCommand,
    },
    #[command(about = "Create a Git worktree for a parallel agent")]
    New(NewWorktreeArgs),
    #[command(about = "Print the directory for a worktree")]
    Switch(SwitchWorktreeArgs),
    #[command(about = "Print source and destination worktree handoff context")]
    Handoff(HandoffWorktreeArgs),
    #[command(about = "Render a Git diff")]
    Diff(DiffArgs),
    #[command(about = "Open the hz terminal UI")]
    Tui,
}

#[derive(Debug, Subcommand)]
enum WorktreeCommand {
    #[command(about = "Create a Git worktree for a parallel agent")]
    New(NewWorktreeArgs),
    #[command(about = "Print the directory for a worktree")]
    Switch(SwitchWorktreeArgs),
    #[command(about = "Print source and destination worktree handoff context")]
    Handoff(HandoffWorktreeArgs),
}

#[derive(Debug, Args)]
struct NewWorktreeArgs {
    name: String,
    #[arg(long)]
    repo: Option<PathBuf>,
    #[arg(long)]
    path: Option<PathBuf>,
    #[arg(long)]
    base: Option<String>,
    #[arg(long)]
    branch: Option<String>,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct SwitchWorktreeArgs {
    target: String,
    #[arg(long)]
    repo: Option<PathBuf>,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct HandoffWorktreeArgs {
    from: String,
    to: String,
    #[arg(long)]
    repo: Option<PathBuf>,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct DiffArgs {
    #[arg(long)]
    repo: Option<PathBuf>,
    #[arg(long)]
    base: Option<String>,
    #[arg(long)]
    stat: bool,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("hz: {error}");
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
            WorktreeCommand::Switch(args) => switch_worktree(args),
            WorktreeCommand::Handoff(args) => handoff_worktree(args),
        },
        Some(Command::New(args)) => create_worktree(args),
        Some(Command::Switch(args)) => switch_worktree(args),
        Some(Command::Handoff(args)) => handoff_worktree(args),
        Some(Command::Diff(args)) => {
            let output = hz_command::diff(hz_command::DiffOptions {
                repo: args.repo,
                base: args.base,
                stat: args.stat,
            })?;
            print!("{output}");
            Ok(())
        }
        Some(Command::Tui) => hz_tui::run(),
    }
}

fn create_worktree(args: NewWorktreeArgs) -> HzResult<()> {
    let _ = args.json;
    hz_command::create_worktree(hz_command::CreateWorktree {
        name: args.name,
        repo: args.repo,
        path: args.path,
        base: args.base,
        branch: args.branch,
    })?;
    Ok(())
}

fn switch_worktree(args: SwitchWorktreeArgs) -> HzResult<()> {
    let _ = args.json;
    hz_command::switch_worktree(hz_command::SwitchWorktree {
        target: args.target,
        repo: args.repo,
    })?;
    Ok(())
}

fn handoff_worktree(args: HandoffWorktreeArgs) -> HzResult<()> {
    let _ = args.json;
    hz_command::handoff_worktree(hz_command::HandoffWorktree {
        from: args.from,
        to: args.to,
        repo: args.repo,
    })?;
    Ok(())
}
