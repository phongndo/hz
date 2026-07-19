use std::path::PathBuf;

use clap::{
    Args, Parser, Subcommand, ValueEnum,
    builder::styling::{AnsiColor, Styles},
};

pub(crate) const HELP_TEMPLATE: &str = "\
{about-with-newline}
usage:
  {usage}

commands:
{subcommands}

options:
{options}

examples:
  hz init
  hz new parser-fix
  hz ls --tree
  hz cd parser-fix
  hz git status
  hz git handoff root
  hz rm parser-fix
  hz gc";

pub(crate) const INSTALL_SCRIPT: &str = include_str!("../../../scripts/install.sh");
pub(crate) const RELEASE_REPO: &str = "phongndo/hz";

#[derive(Debug, Parser)]
#[command(
    name = "hz",
    version,
    about = "Copy-on-write workspaces for parallel development",
    help_template = HELP_TEMPLATE,
    next_help_heading = "options",
    subcommand_help_heading = "commands",
    styles = help_styles()
)]
pub(crate) struct Cli {
    /// Emit stable JSON and disable interactive shell behavior.
    #[arg(long, global = true)]
    pub(crate) machine: bool,
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
    #[command(about = "Initialize and register a workspace root")]
    Init(InitWorkspaceArgs),
    #[command(about = "Create an isolated child workspace")]
    New(NewWorkspaceArgs),
    #[command(alias = "cd", about = "Print a workspace path")]
    Path(PathWorkspaceArgs),
    #[command(alias = "ls", about = "List managed workspaces")]
    List(ListWorkspaceArgs),
    #[command(about = "Print the current workspace")]
    Pwd(PwdWorkspaceArgs),
    #[command(about = "List logical workspace ancestors")]
    Ancestors(AncestorsArgs),
    #[command(alias = "rm", about = "Move a workspace subtree to trash")]
    Remove(RemoveWorkspaceArgs),
    #[command(about = "Protect workspaces from retention policies")]
    Pin(TargetsArgs),
    #[command(about = "Make workspaces eligible for retention policies")]
    Unpin(TargetsArgs),
    #[command(about = "Restore a workspace subtree from trash")]
    Restore(RestoreWorkspaceArgs),
    #[command(about = "Physically delete trashed workspaces")]
    Gc(JsonArgs),
    #[command(about = "Adopt a managed workspace after its directory moved")]
    Adopt(AdoptArgs),
    #[command(about = "Inspect and optionally repair workspace state")]
    Doctor(DoctorArgs),
    #[command(about = "Git operations across Hz workspaces")]
    Git {
        #[command(subcommand)]
        command: GitCommand,
    },
    #[command(about = "Mercurial operations across Hz workspaces")]
    Hg {
        #[command(subcommand)]
        command: HgCommand,
    },
    #[command(about = "Manage Hz configuration")]
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    #[command(about = "Install shell integration into your shell rc file")]
    Install(ShellArgs),
    #[command(about = "Print shell integration script")]
    Shell(ShellArgs),
    #[command(about = "Update this curl-installed hz binary")]
    Update(UpdateArgs),
    #[command(name = "__complete", hide = true)]
    Complete(CompleteArgs),
}

#[derive(Debug, Args)]
pub(crate) struct InitWorkspaceArgs {
    pub(crate) path: Option<PathBuf>,
    #[arg(long)]
    pub(crate) here: bool,
    /// Use an explicit portable byte copy instead of native copy-on-write.
    #[arg(long)]
    pub(crate) copy: bool,
    #[arg(short = 'j', long)]
    pub(crate) json: bool,
    #[arg(long, hide = true)]
    pub(crate) path_only: bool,
}

#[derive(Debug, Args)]
pub(crate) struct NewWorkspaceArgs {
    pub(crate) name: Option<String>,
    #[arg(short = 'f', long = "from")]
    pub(crate) from: Option<PathBuf>,
    #[arg(short = 'i', long)]
    pub(crate) into: Option<PathBuf>,
    /// Skip known regenerable dependency, build, and cache artifacts.
    #[arg(long)]
    pub(crate) filtered: bool,
    #[arg(long)]
    pub(crate) no_hooks: bool,
    #[arg(short = 'j', long)]
    pub(crate) json: bool,
    #[arg(long, hide = true)]
    pub(crate) path_only: bool,
}

#[derive(Debug, Args)]
pub(crate) struct PathWorkspaceArgs {
    pub(crate) target: Option<String>,
    #[arg(short = 'a', long = "at")]
    pub(crate) at: Option<PathBuf>,
    #[arg(short = 'j', long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct ListWorkspaceArgs {
    #[arg(short = 'a', long = "at")]
    pub(crate) at: Option<PathBuf>,
    #[arg(long, conflicts_with = "children")]
    pub(crate) roots: bool,
    #[arg(long)]
    pub(crate) children: bool,
    #[arg(long)]
    pub(crate) tree: bool,
    #[arg(long, conflicts_with = "unpinned")]
    pub(crate) pinned: bool,
    #[arg(long)]
    pub(crate) unpinned: bool,
    #[arg(short = 'j', long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct PwdWorkspaceArgs {
    #[arg(short = 'a', long = "at")]
    pub(crate) at: Option<PathBuf>,
    #[arg(short = 'j', long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct AncestorsArgs {
    pub(crate) target: Option<String>,
    #[arg(short = 'a', long = "at")]
    pub(crate) at: Option<PathBuf>,
    #[arg(short = 'j', long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct RemoveWorkspaceArgs {
    pub(crate) target: Option<String>,
    #[arg(short = 'a', long = "at")]
    pub(crate) at: Option<PathBuf>,
    #[arg(long)]
    pub(crate) children: bool,
    #[arg(short = 'f', long)]
    pub(crate) force: bool,
    #[arg(long)]
    pub(crate) no_hooks: bool,
    #[arg(short = 'j', long)]
    pub(crate) json: bool,
    #[arg(long, hide = true)]
    pub(crate) path_only: bool,
}

#[derive(Debug, Args)]
pub(crate) struct TargetsArgs {
    #[arg(value_name = "TARGET", required = true, num_args = 1..)]
    pub(crate) targets: Vec<String>,
    #[arg(short = 'a', long = "at")]
    pub(crate) at: Option<PathBuf>,
    #[arg(short = 'j', long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct RestoreWorkspaceArgs {
    pub(crate) target: String,
    #[arg(short = 'a', long = "at")]
    pub(crate) at: Option<PathBuf>,
    #[arg(short = 'j', long)]
    pub(crate) json: bool,
    #[arg(long, hide = true)]
    pub(crate) path_only: bool,
}

#[derive(Debug, Args)]
pub(crate) struct JsonArgs {
    #[arg(short = 'j', long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct AdoptArgs {
    pub(crate) path: PathBuf,
    #[arg(short = 'j', long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct DoctorArgs {
    #[arg(long)]
    pub(crate) fix: bool,
    #[arg(short = 'j', long)]
    pub(crate) json: bool,
}

#[derive(Debug, Subcommand)]
pub(crate) enum GitCommand {
    #[command(about = "Show Git state for a workspace")]
    Status(SourceStatusArgs),
    #[command(about = "Apply the current workspace patch to another workspace")]
    Handoff(GitHandoffArgs),
}

#[derive(Debug, Args)]
pub(crate) struct SourceStatusArgs {
    pub(crate) target: Option<String>,
    #[arg(short = 'a', long = "at")]
    pub(crate) at: Option<PathBuf>,
    #[arg(short = 'j', long)]
    pub(crate) json: bool,
}

#[derive(Debug, Subcommand)]
pub(crate) enum HgCommand {
    #[command(about = "Show Mercurial state for a workspace")]
    Status(SourceStatusArgs),
}

#[derive(Debug, Args)]
pub(crate) struct GitHandoffArgs {
    pub(crate) target: Option<String>,
    #[arg(short = 'a', long = "at")]
    pub(crate) at: Option<PathBuf>,
    #[arg(short = 'j', long)]
    pub(crate) json: bool,
    #[arg(long, hide = true)]
    pub(crate) path_only: bool,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ConfigCommand {
    #[command(about = "Create workspace configuration and lifecycle scripts")]
    Init(ConfigInitArgs),
}

#[derive(Debug, Args)]
pub(crate) struct ConfigInitArgs {
    pub(crate) path: Option<PathBuf>,
    #[arg(short = 'j', long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct ShellArgs {
    pub(crate) shell: ShellArg,
}

#[derive(Debug, Args)]
pub(crate) struct UpdateArgs {
    #[arg(long = "target-version", value_name = "VERSION")]
    pub(crate) version: Option<String>,
    #[arg(long, value_name = "DIR")]
    pub(crate) install_dir: Option<PathBuf>,
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
    #[arg(short = 'a', long = "at")]
    pub(crate) at: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum CompletionKind {
    WorkspaceTargets,
    TrashTargets,
}
