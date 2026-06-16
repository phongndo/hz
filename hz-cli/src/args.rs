use std::path::PathBuf;

use clap::{
    Args, Parser, Subcommand, ValueEnum,
    builder::styling::{AnsiColor, Styles},
};

pub(crate) const HELP_TEMPLATE: &str = "\
{before-help}{name} {version}
{about-with-newline}
usage:
  {usage}

commands:
{subcommands}

options:
{options}

examples:
  hz
  hz init
  hz install zsh
  hz new feature/ui
  hz fork
  hz ls
  hz pwd
  hz rm -f feature/ui
  hz setup feature/ui
  hz cleanup feature/ui
  hz cd feature/ui
  hz handoff feature/ui";

pub(crate) const INSTALL_SCRIPT: &str = include_str!("../../scripts/install.sh");
pub(crate) const RELEASE_REPO: &str = "phongndo/hz";

#[derive(Debug, Parser)]
#[command(
    name = "hz",
    version,
    about = "Terminal workspace manager for parallel AI agents",
    help_template = HELP_TEMPLATE,
    next_help_heading = "options",
    subcommand_help_heading = "commands",
    styles = help_styles()
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Option<Command>,
}

pub(crate) fn help_styles() -> Styles {
    Styles::styled()
        .header(AnsiColor::Cyan.on_default().bold())
        .usage(AnsiColor::Cyan.on_default().bold())
        .literal(AnsiColor::White.on_default().bold())
        .placeholder(AnsiColor::White.on_default())
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    #[command(alias = "wt")]
    Worktree {
        #[command(subcommand)]
        command: WorktreeCommand,
    },
    #[command(about = "Create an isolated Git worktree for a task or agent")]
    New(NewWorktreeArgs),
    #[command(about = "Fork the current worktree state into a detached worktree")]
    Fork(ForkWorktreeArgs),
    #[command(alias = "cd", about = "Print the directory for a worktree")]
    Path(PathWorktreeArgs),
    #[command(alias = "ls", about = "List worktrees")]
    List(ListWorktreeArgs),
    #[command(about = "Print the current worktree target")]
    Pwd(PwdWorktreeArgs),
    #[command(alias = "rm", about = "Remove one or more worktrees")]
    Remove(RemoveWorktreeArgs),
    #[command(about = "Apply changes between local and a linked worktree")]
    Handoff(HandoffWorktreeArgs),
    #[command(about = "Initialize hz repo lifecycle config")]
    Init(InitArgs),
    #[command(about = "Install shell integration into your shell rc file")]
    Install(ShellArgs),
    #[command(about = "Run the configured setup command for a worktree")]
    Setup(LifecycleArgs),
    #[command(about = "Run the configured cleanup command for a worktree")]
    Cleanup(LifecycleArgs),
    #[command(about = "Print shell integration script")]
    Shell(ShellArgs),
    #[command(
        about = "Update this hz binary from GitHub releases",
        after_help = "\
examples:
  hz update
  hz update --target-version 0.1.5
  hz update --install-dir ~/.local/bin
  hz update --force-self-update"
    )]
    Update(UpdateArgs),
    #[command(name = "__complete", hide = true)]
    Complete(CompleteArgs),
}

#[derive(Debug, Subcommand)]
pub(crate) enum WorktreeCommand {
    #[command(about = "Create an isolated Git worktree for a task or agent")]
    New(NewWorktreeArgs),
    #[command(about = "Fork the current worktree state into a detached worktree")]
    Fork(ForkWorktreeArgs),
    #[command(alias = "cd", about = "Print the directory for a worktree")]
    Path(PathWorktreeArgs),
    #[command(alias = "ls", about = "List worktrees")]
    List(ListWorktreeArgs),
    #[command(about = "Print the current worktree target")]
    Pwd(PwdWorktreeArgs),
    #[command(alias = "rm", about = "Remove one or more worktrees")]
    Remove(RemoveWorktreeArgs),
    #[command(about = "Apply changes between local and a linked worktree")]
    Handoff(HandoffWorktreeArgs),
}

#[derive(Debug, Args)]
pub(crate) struct NewWorktreeArgs {
    pub(crate) name: Option<String>,
    #[arg(short = 'r', long)]
    pub(crate) repo: Option<PathBuf>,
    #[arg(short = 'p', long)]
    pub(crate) path: Option<PathBuf>,
    #[arg(short = 'B', long)]
    pub(crate) base: Option<String>,
    #[arg(short = 'b', long)]
    pub(crate) branch: Option<String>,
    #[arg(long)]
    pub(crate) max_detached: Option<usize>,
    #[arg(long)]
    pub(crate) max_branch_worktrees: Option<usize>,
    #[arg(short = 'j', long)]
    pub(crate) json: bool,
    #[arg(short = 'd', long)]
    pub(crate) debug: bool,
    #[arg(long)]
    pub(crate) setup: bool,
    #[arg(long)]
    pub(crate) no_setup: bool,
    #[arg(long, hide = true)]
    pub(crate) path_only: bool,
}

#[derive(Debug, Args)]
pub(crate) struct ForkWorktreeArgs {
    pub(crate) name: Option<String>,
    #[arg(short = 'r', long)]
    pub(crate) repo: Option<PathBuf>,
    #[arg(short = 'p', long)]
    pub(crate) path: Option<PathBuf>,
    #[arg(long)]
    pub(crate) no_diff: bool,
    #[arg(long)]
    pub(crate) max_detached: Option<usize>,
    #[arg(short = 'j', long)]
    pub(crate) json: bool,
    #[arg(long, hide = true)]
    pub(crate) path_only: bool,
}

#[derive(Debug, Args)]
pub(crate) struct PathWorktreeArgs {
    pub(crate) target: Option<String>,
    #[arg(short = 'r', long)]
    pub(crate) repo: Option<PathBuf>,
    #[arg(short = 'j', long)]
    pub(crate) json: bool,
    #[arg(long, hide = true)]
    pub(crate) path_only: bool,
}

#[derive(Debug, Args)]
pub(crate) struct ListWorktreeArgs {
    #[arg(short = 'r', long)]
    pub(crate) repo: Option<PathBuf>,
    #[arg(short = 'j', long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct PwdWorktreeArgs {
    #[arg(short = 'r', long)]
    pub(crate) repo: Option<PathBuf>,
    #[arg(short = 'j', long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct RemoveWorktreeArgs {
    #[arg(value_name = "TARGET", required = true, num_args = 1..)]
    pub(crate) targets: Vec<String>,
    #[arg(short = 'r', long)]
    pub(crate) repo: Option<PathBuf>,
    #[arg(short = 'j', long)]
    pub(crate) json: bool,
    #[arg(short = 'f', long, alias = "yes")]
    pub(crate) force: bool,
    #[arg(short = 'd', long)]
    pub(crate) debug: bool,
    #[arg(long)]
    pub(crate) cleanup: bool,
    #[arg(long)]
    pub(crate) no_cleanup: bool,
}

#[derive(Debug, Args)]
pub(crate) struct HandoffWorktreeArgs {
    pub(crate) target: Option<String>,
    #[arg(short = 'b', long)]
    pub(crate) branch: bool,
    #[arg(short = 'n', long = "new")]
    pub(crate) create: bool,
    #[arg(long)]
    pub(crate) max_detached: Option<usize>,
    #[arg(long)]
    pub(crate) max_branch_worktrees: Option<usize>,
    #[arg(short = 'r', long)]
    pub(crate) repo: Option<PathBuf>,
    #[arg(short = 'j', long)]
    pub(crate) json: bool,
    #[arg(long, hide = true)]
    pub(crate) path_only: bool,
}

#[derive(Debug, Args)]
pub(crate) struct InitArgs {
    #[arg(value_enum)]
    pub(crate) shell: Option<ShellArg>,
    #[arg(short = 'r', long)]
    pub(crate) repo: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub(crate) struct ShellArgs {
    pub(crate) shell: ShellArg,
}

#[derive(Debug, Args)]
pub(crate) struct LifecycleArgs {
    pub(crate) target: Option<String>,
    #[arg(short = 'r', long)]
    pub(crate) repo: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub(crate) struct UpdateArgs {
    /// Release version to install, without or with the leading v.
    #[arg(long = "target-version", value_name = "VERSION")]
    pub(crate) version: Option<String>,
    /// Directory to update. Defaults to the directory containing the invoked hz.
    #[arg(long, value_name = "DIR")]
    pub(crate) install_dir: Option<PathBuf>,
    /// Allow hz update to overwrite a package-manager-managed binary.
    #[arg(long)]
    pub(crate) force_self_update: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum ShellArg {
    Zsh,
    Bash,
    Fish,
}

#[derive(Debug, Args)]
pub(crate) struct CompleteArgs {
    pub(crate) kind: CompletionKind,
    #[arg(short = 'r', long)]
    pub(crate) repo: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum CompletionKind {
    WorktreeTargets,
    RemovableWorktrees,
}
