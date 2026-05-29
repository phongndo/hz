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
    #[arg(long)]
    debug: bool,
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
    #[arg(long)]
    debug: bool,
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
    } else if args.debug {
        println!("created {} at {}", created.branch, created.path.display());
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
        print!("{}", render_worktree_list(&worktrees));
    }

    Ok(())
}

fn render_worktree_list(worktrees: &[hz_command::WorktreeEntry]) -> String {
    if worktrees.is_empty() {
        return String::new();
    }

    let name_width = worktrees
        .iter()
        .map(|worktree| display_width(worktree_branch_or_handle(worktree)))
        .chain([6])
        .max()
        .expect("width candidates should not be empty");
    let path_width = worktrees
        .iter()
        .map(|worktree| display_width(&worktree.path.display().to_string()))
        .chain([4])
        .max()
        .expect("width candidates should not be empty");
    let mut output = String::new();

    output.push_str(&format!(
        "{:<name_width$}  {:<path_width$}  SOURCE\n",
        "branch", "PATH"
    ));
    for worktree in worktrees {
        let name = worktree_branch_or_handle(worktree);
        let path = worktree.path.display().to_string();
        let source = match worktree.source {
            hz_command::WorktreeSource::Managed => "managed",
            hz_command::WorktreeSource::Git => "git",
        };
        output.push_str(&format!(
            "{name:<name_width$}  {path:<path_width$}  {source}\n"
        ));
    }

    output
}

fn display_width(value: &str) -> usize {
    value.chars().count()
}

fn remove_worktree(args: RemoveWorktreeArgs) -> HzResult<()> {
    let removed = hz_command::remove_worktree(hz_command::RemoveWorktree {
        target: args.target,
        repo: args.repo,
    })?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&removed)?);
    } else if args.debug {
        println!("removed {}", worktree_branch_or_handle(&removed));
    }

    Ok(())
}

fn worktree_branch_or_handle(worktree: &hz_command::WorktreeEntry) -> &str {
    worktree.branch.as_deref().unwrap_or(&worktree.handle)
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn list_output_uses_branch_as_display_identifier() {
        let output = render_worktree_list(&[hz_command::WorktreeEntry {
            id: "entry-id".to_owned(),
            handle: "generated-handle".to_owned(),
            repo: PathBuf::from("/repo"),
            path: PathBuf::from("/worktrees/entry-id"),
            branch: Some("feature/ui".to_owned()),
            base: None,
            source: hz_command::WorktreeSource::Managed,
            created_at_unix: 0,
        }]);

        assert!(output.contains("branch"));
        assert!(output.contains("feature/ui"));
        assert!(!output.contains("generated-handle"));
    }

    #[test]
    fn list_output_is_empty_when_there_are_no_worktrees() {
        assert_eq!(render_worktree_list(&[]), "");
    }

    #[test]
    fn list_output_uses_handle_when_branch_is_missing() {
        let output = render_worktree_list(&[hz_command::WorktreeEntry {
            id: "entry-id".to_owned(),
            handle: "generated-handle".to_owned(),
            repo: PathBuf::from("/repo"),
            path: PathBuf::from("/worktrees/entry"),
            branch: None,
            base: None,
            source: hz_command::WorktreeSource::Git,
            created_at_unix: 0,
        }]);
        let row = output
            .lines()
            .nth(1)
            .expect("worktree row should be rendered");
        let columns: Vec<_> = row.split_whitespace().collect();

        assert_eq!(columns, vec!["generated-handle", "/worktrees/entry", "git"]);
    }

    #[test]
    fn list_output_widths_count_characters() {
        let output = render_worktree_list(&[hz_command::WorktreeEntry {
            id: "entry-id".to_owned(),
            handle: "generated-handle".to_owned(),
            repo: PathBuf::from("/repo"),
            path: PathBuf::from("/worktrees/entry"),
            branch: Some("ééééé".to_owned()),
            base: None,
            source: hz_command::WorktreeSource::Managed,
            created_at_unix: 0,
        }]);

        assert!(output.starts_with("branch  PATH"));
    }

    #[test]
    fn removed_worktree_display_identifier_prefers_branch() {
        let worktree = hz_command::WorktreeEntry {
            id: "entry-id".to_owned(),
            handle: "generated-handle".to_owned(),
            repo: PathBuf::from("/repo"),
            path: PathBuf::from("/worktrees/entry-id"),
            branch: Some("feature/ui".to_owned()),
            base: None,
            source: hz_command::WorktreeSource::Managed,
            created_at_unix: 0,
        };

        assert_eq!(worktree_branch_or_handle(&worktree), "feature/ui");
    }
}
