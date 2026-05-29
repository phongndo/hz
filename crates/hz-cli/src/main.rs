use std::{path::PathBuf, process::ExitCode};

use clap::{Args, Parser, Subcommand, ValueEnum};
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
    #[command(alias = "cd", about = "Print the directory for a worktree")]
    Switch(SwitchWorktreeArgs),
    #[command(about = "Print the directory for a worktree")]
    Path(SwitchWorktreeArgs),
    #[command(alias = "ls", about = "List worktrees")]
    List(ListWorktreeArgs),
    #[command(alias = "rm", about = "Remove a managed worktree")]
    Remove(RemoveWorktreeArgs),
    #[command(about = "Print source and destination worktree handoff context")]
    Handoff(HandoffWorktreeArgs),
    #[command(about = "Install shell integration into your shell rc file")]
    Init(InitArgs),
    #[command(about = "Print shell integration script")]
    Shell(InitArgs),
    #[command(about = "Render a Git diff")]
    Diff(DiffArgs),
    #[command(about = "Open the hz terminal UI")]
    Tui,
}

#[derive(Debug, Subcommand)]
enum WorktreeCommand {
    #[command(about = "Create a Git worktree for a parallel agent")]
    New(NewWorktreeArgs),
    #[command(alias = "cd", about = "Print the directory for a worktree")]
    Switch(SwitchWorktreeArgs),
    #[command(about = "Print the directory for a worktree")]
    Path(SwitchWorktreeArgs),
    #[command(alias = "ls", about = "List worktrees")]
    List(ListWorktreeArgs),
    #[command(alias = "rm", about = "Remove a managed worktree")]
    Remove(RemoveWorktreeArgs),
    #[command(about = "Print source and destination worktree handoff context")]
    Handoff(HandoffWorktreeArgs),
}

#[derive(Debug, Args)]
struct NewWorktreeArgs {
    name: Option<String>,
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
    #[arg(long, hide = true)]
    path_only: bool,
}

#[derive(Debug, Args)]
struct SwitchWorktreeArgs {
    target: Option<String>,
    #[arg(long)]
    repo: Option<PathBuf>,
    #[arg(long)]
    json: bool,
    #[arg(long, hide = true)]
    path_only: bool,
}

#[derive(Debug, Args)]
struct ListWorktreeArgs {
    #[arg(long)]
    repo: Option<PathBuf>,
    #[arg(long)]
    all: bool,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct RemoveWorktreeArgs {
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
struct InitArgs {
    shell: ShellArg,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ShellArg {
    Zsh,
    Bash,
    Fish,
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
            WorktreeCommand::Path(args) => path_worktree(args),
            WorktreeCommand::List(args) => list_worktrees(args),
            WorktreeCommand::Remove(args) => remove_worktree(args),
            WorktreeCommand::Handoff(args) => handoff_worktree(args),
        },
        Some(Command::New(args)) => create_worktree(args),
        Some(Command::Switch(args)) => switch_worktree(args),
        Some(Command::Path(args)) => path_worktree(args),
        Some(Command::List(args)) => list_worktrees(args),
        Some(Command::Remove(args)) => remove_worktree(args),
        Some(Command::Handoff(args)) => handoff_worktree(args),
        Some(Command::Init(args)) => init_shell(args),
        Some(Command::Shell(args)) => shell_script(args),
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
    let created = hz_command::create_worktree(hz_command::CreateWorktree {
        name: args.name,
        repo: args.repo,
        path: args.path,
        base: args.base,
        branch: args.branch,
    })?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&created)?);
    } else if args.path_only {
        println!("{}", created.path.display());
    } else {
        println!("created {} at {}", created.handle, created.path.display());
    }

    Ok(())
}

fn switch_worktree(args: SwitchWorktreeArgs) -> HzResult<()> {
    let _ = args.path_only;
    let target = hz_command::switch_worktree(hz_command::SwitchWorktree {
        target: args.target.unwrap_or_else(|| "local".to_owned()),
        repo: args.repo,
    })?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&target)?);
    } else {
        println!("{}", target.path.display());
    }

    Ok(())
}

fn path_worktree(args: SwitchWorktreeArgs) -> HzResult<()> {
    switch_worktree(args)
}

fn list_worktrees(args: ListWorktreeArgs) -> HzResult<()> {
    let worktrees = hz_command::list_worktrees(hz_command::ListWorktrees {
        repo: args.repo,
        all: args.all,
    })?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&worktrees)?);
    } else {
        for worktree in worktrees {
            let branch = worktree.branch.as_deref().unwrap_or("-");
            let source = match worktree.source {
                hz_command::WorktreeSource::Managed => "managed",
                hz_command::WorktreeSource::Git => "git",
            };
            println!(
                "{}\t{}\t{}\t{}",
                worktree.handle,
                branch,
                worktree.path.display(),
                source
            );
        }
    }

    Ok(())
}

fn remove_worktree(args: RemoveWorktreeArgs) -> HzResult<()> {
    let removed = hz_command::remove_worktree(hz_command::RemoveWorktree {
        target: args.target,
        repo: args.repo,
    })?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&removed)?);
    } else {
        println!("removed {}", removed.handle);
    }

    Ok(())
}

fn handoff_worktree(args: HandoffWorktreeArgs) -> HzResult<()> {
    let handoff = hz_command::handoff_worktree(hz_command::HandoffWorktree {
        from: args.from,
        to: args.to,
        repo: args.repo,
    })?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&handoff)?);
    } else {
        println!("repo\t{}", handoff.repo.display());
        println!(
            "from\t{}\t{}",
            handoff.from.name,
            handoff.from.path.display()
        );
        println!("to\t{}\t{}", handoff.to.name, handoff.to.path.display());
    }

    Ok(())
}

fn init_shell(args: InitArgs) -> HzResult<()> {
    let shell = match args.shell {
        ShellArg::Zsh => hz_command::Shell::Zsh,
        ShellArg::Bash => hz_command::Shell::Bash,
        ShellArg::Fish => hz_command::Shell::Fish,
    };

    let init = hz_command::install_shell_integration(shell)?;
    if init.changed {
        println!("installed hz shell integration in {}", init.path.display());
    } else {
        println!(
            "hz shell integration already exists in {}",
            init.path.display()
        );
    }

    Ok(())
}

fn shell_script(args: InitArgs) -> HzResult<()> {
    let shell = match args.shell {
        ShellArg::Zsh => hz_command::Shell::Zsh,
        ShellArg::Bash => hz_command::Shell::Bash,
        ShellArg::Fish => hz_command::Shell::Fish,
    };

    print!("{}", hz_command::shell_integration(shell));
    Ok(())
}
