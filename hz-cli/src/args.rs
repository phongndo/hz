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
  hz ls
  hz rm -f feature/ui
  hz setup feature/ui
  hz cleanup feature/ui
  hz cd feature/ui
  hz handoff feature/ui
  hz ts add ruby elixir";

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
    #[command(alias = "cd", about = "Print the directory for a worktree")]
    Path(PathWorktreeArgs),
    #[command(alias = "ls", about = "List worktrees")]
    List(ListWorktreeArgs),
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
    #[command(
        about = "Review a Git diff",
        after_help = "\
examples:
  hz diff
  hz diff --base main
  hz diff --pr 123
  hz diff --pr https://github.com/owner/repo/pull/123"
    )]
    Diff(DiffArgs),
    #[command(about = "Manage the hz daemon")]
    Daemon(DaemonArgs),
    #[command(
        name = "ts",
        alias = "tree-sitter",
        about = "Manage diff syntax highlighting languages"
    )]
    TreeSitter {
        #[command(subcommand)]
        command: TreeSitterCommand,
    },
    #[command(name = "__complete", hide = true)]
    Complete(CompleteArgs),
}

#[derive(Debug, Subcommand)]
pub(crate) enum TreeSitterCommand {
    #[command(about = "Install and enable syntax highlighting languages")]
    Add(TreeSitterLanguagesArgs),
    #[command(about = "Update cached syntax highlighting parsers")]
    Update(TreeSitterUpdateArgs),
    #[command(alias = "remove", about = "Remove syntax highlighting languages")]
    Rm(TreeSitterLanguagesArgs),
    #[command(
        visible_alias = "ls",
        about = "List installed and enabled syntax highlighting languages"
    )]
    List,
    #[command(about = "List syntax highlighting languages")]
    Available(TreeSitterAvailableArgs),
    #[command(about = "Remove cached tree-sitter parser libraries")]
    Clean,
    #[command(about = "Print tree-sitter cache and syntax config paths")]
    Path,
    #[command(about = "Validate enabled syntax highlighting languages")]
    Doctor,
}

#[derive(Debug, Args)]
pub(crate) struct TreeSitterLanguagesArgs {
    #[arg(value_name = "LANG", required = true)]
    pub(crate) languages: Vec<String>,
}

#[derive(Debug, Args)]
pub(crate) struct TreeSitterUpdateArgs {
    #[arg(value_name = "LANG", required_unless_present = "all")]
    pub(crate) languages: Vec<String>,
    #[arg(long, conflicts_with = "languages")]
    pub(crate) all: bool,
}

#[derive(Debug, Args)]
pub(crate) struct TreeSitterAvailableArgs {
    #[arg(long, conflicts_with = "enabled")]
    pub(crate) installed: bool,
    #[arg(long, conflicts_with = "installed")]
    pub(crate) enabled: bool,
}

#[derive(Debug, Subcommand)]
pub(crate) enum WorktreeCommand {
    #[command(about = "Create an isolated Git worktree for a task or agent")]
    New(NewWorktreeArgs),
    #[command(alias = "cd", about = "Print the directory for a worktree")]
    Path(PathWorktreeArgs),
    #[command(alias = "ls", about = "List worktrees")]
    List(ListWorktreeArgs),
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
    pub(crate) no_setup: bool,
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

#[derive(Debug, Args)]
pub(crate) struct DaemonArgs {
    #[command(subcommand)]
    pub(crate) command: DaemonCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum DaemonCommand {
    #[command(about = "Start the hz daemon")]
    Start,
    #[command(about = "Stop the hz daemon")]
    Stop,
    #[command(about = "Show hz daemon status")]
    Status,
    #[command(about = "Run an AI CLI under the hz daemon")]
    Run(DaemonRunArgs),
    #[command(alias = "ls", about = "List AI CLI sessions")]
    Agents,
    #[command(name = "stop-agent", about = "Stop an AI CLI session")]
    StopAgent(DaemonSessionArgs),
    #[command(about = "Print an AI CLI session log")]
    Logs(DaemonSessionArgs),
    #[command(about = "Send input to a daemon-owned AI CLI session")]
    Send(DaemonSendArgs),
    #[command(about = "Attach a session to the hz daemon")]
    Attach(DaemonAttachArgs),
    #[command(about = "Detach a session from the hz daemon")]
    Detach(DaemonDetachArgs),
    #[command(name = "__run", hide = true)]
    Serve,
}

#[derive(Debug, Args)]
pub(crate) struct DaemonRunArgs {
    #[arg(value_enum)]
    pub(crate) cli: AiCliArg,
    #[arg(short, long)]
    pub(crate) name: Option<String>,
    #[arg(short = 'C', long, value_name = "DIR")]
    pub(crate) cwd: Option<PathBuf>,
    #[arg(last = true)]
    pub(crate) args: Vec<String>,
}

#[derive(Debug, Args)]
pub(crate) struct DaemonSessionArgs {
    #[arg(value_name = "SESSION")]
    pub(crate) session: String,
}

#[derive(Debug, Args)]
pub(crate) struct DaemonSendArgs {
    #[arg(value_name = "SESSION")]
    pub(crate) session: String,
    #[arg(
        value_name = "TEXT",
        required = true,
        trailing_var_arg = true,
        allow_hyphen_values = true
    )]
    pub(crate) text: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum AiCliArg {
    Pi,
    Codex,
    #[value(alias = "claude-code", alias = "claudecode")]
    Claude,
}

#[derive(Debug, Args)]
pub(crate) struct DaemonAttachArgs {
    #[arg(value_name = "SESSION")]
    pub(crate) session: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct DaemonDetachArgs {
    #[arg(value_name = "SESSION")]
    pub(crate) session: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum ShellArg {
    Zsh,
    Bash,
    Fish,
}

#[derive(Debug, Args)]
pub(crate) struct DiffArgs {
    #[arg(value_name = "REV", num_args = 0..=2)]
    pub(crate) revs: Vec<String>,
    /// Fetch and review a GitHub pull request by number or URL.
    #[arg(
        long,
        value_name = "NUMBER|URL",
        conflicts_with_all = ["base", "revs", "staged", "unstaged", "no_untracked", "patch"]
    )]
    pub(crate) pr: Option<String>,
    #[arg(short = 'r', long)]
    pub(crate) repo: Option<PathBuf>,
    #[arg(short = 'b', long)]
    pub(crate) base: Option<String>,
    #[arg(long, conflicts_with = "unstaged", conflicts_with_all = ["base", "revs"])]
    pub(crate) staged: bool,
    #[arg(long, conflicts_with_all = ["base", "revs"])]
    pub(crate) unstaged: bool,
    #[arg(long = "no-untracked")]
    pub(crate) no_untracked: bool,
    /// Read an existing unified diff from FILE, or stdin when FILE is `-`.
    #[arg(long, value_name = "FILE")]
    pub(crate) patch: Option<PathBuf>,
    /// Disable live reload in the interactive diff viewer.
    #[arg(long = "no-watch")]
    pub(crate) no_watch: bool,
    /// Disable syntax highlighting in the interactive diff viewer.
    #[arg(long = "no-syntax")]
    pub(crate) no_syntax: bool,
    #[arg(short = 's', long)]
    pub(crate) stat: bool,
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
