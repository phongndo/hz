use std::{
    io::{self, IsTerminal, Write},
    path::PathBuf,
    process::{Command as ProcessCommand, ExitCode},
};

use clap::{
    Args, Parser, Subcommand, ValueEnum,
    builder::styling::{AnsiColor, Styles},
};
use hz_core::HzResult;

const HELP_TEMPLATE: &str = "\
{before-help}{name} {version}
{about-with-newline}
usage:
  {usage}

commands:
{subcommands}

options:
{options}

examples:
  hz new feature/ui
  hz ls
  hz rm -f feature/ui
  hz cd feature/ui";

#[derive(Debug, Parser)]
#[command(
    name = "hz",
    version,
    about = "Parallel agent workspace CLI",
    help_template = HELP_TEMPLATE,
    next_help_heading = "options",
    subcommand_help_heading = "commands",
    styles = help_styles()
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

fn help_styles() -> Styles {
    Styles::styled()
        .header(AnsiColor::Cyan.on_default().bold())
        .usage(AnsiColor::Cyan.on_default().bold())
        .literal(AnsiColor::White.on_default().bold())
        .placeholder(AnsiColor::White.on_default())
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
    Path(PathWorktreeArgs),
    #[command(alias = "ls", about = "List worktrees")]
    List(ListWorktreeArgs),
    #[command(alias = "rm", about = "Remove a worktree")]
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
    Path(PathWorktreeArgs),
    #[command(alias = "ls", about = "List worktrees")]
    List(ListWorktreeArgs),
    #[command(alias = "rm", about = "Remove a worktree")]
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
struct PathWorktreeArgs {
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
    json: bool,
}

#[derive(Debug, Args)]
struct RemoveWorktreeArgs {
    target: String,
    #[arg(long)]
    repo: Option<PathBuf>,
    #[arg(long)]
    json: bool,
    #[arg(short = 'f', long, alias = "yes")]
    force: bool,
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
    let debug = args.debug;
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
    } else if debug {
        print!(
            "{}",
            render_created_worktree(&created, io::stdout().is_terminal())
        );
    }

    Ok(())
}

fn path_worktree(args: PathWorktreeArgs) -> HzResult<()> {
    let _ = args.path_only;
    let target = hz_command::path_worktree(hz_command::PathWorktree {
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

fn list_worktrees(args: ListWorktreeArgs) -> HzResult<()> {
    let worktrees = hz_command::list_worktrees(hz_command::ListWorktrees { repo: args.repo })?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&worktrees)?);
    } else {
        print!(
            "{}",
            render_worktree_list_with_style(&worktrees, io::stdout().is_terminal())
        );
    }

    Ok(())
}

#[cfg(test)]
fn render_worktree_list(worktrees: &[hz_command::WorktreeEntry]) -> String {
    render_worktree_list_with_style(worktrees, false)
}

fn render_worktree_list_with_style(worktrees: &[hz_command::WorktreeEntry], color: bool) -> String {
    if worktrees.is_empty() {
        return String::new();
    }

    let name_width = worktrees
        .iter()
        .map(|worktree| display_width(worktree_branch_or_handle(worktree)))
        .chain([6])
        .max()
        .expect("width candidates should not be empty");
    let modified_width = worktrees
        .iter()
        .map(|worktree| display_width(&format_modified_at(worktree_display_timestamp(worktree))))
        .chain([8])
        .max()
        .expect("width candidates should not be empty");
    let mut output = String::new();

    let branch_header = styled_cell("branch", name_width, StyleColor::Cyan, color);
    let status_header = styled_cell("status", 6, StyleColor::Cyan, color);
    let modified_header = styled_cell("modified", modified_width, StyleColor::Cyan, color);
    let path_header = styled("path", StyleColor::Cyan, color);
    output.push_str(&format!(
        "  {branch_header}  {status_header}  {modified_header}  {path_header}\n"
    ));
    for worktree in worktrees {
        let name = worktree_branch_or_handle(worktree);
        let status = worktree_status_label(worktree.status);
        let modified = format_modified_at(worktree_display_timestamp(worktree));
        let path = worktree.path.display().to_string();
        let marker = styled("*", StyleColor::Green, color);
        let name = styled_cell(name, name_width, StyleColor::White, color);
        let status = styled_cell(status, 6, worktree_status_color(worktree.status), color);
        let modified = styled_cell(&modified, modified_width, StyleColor::White, color);
        let path = styled(&path, StyleColor::White, color);
        output.push_str(&format!("{marker} {name}  {status}  {modified}  {path}\n"));
    }

    output
}

fn worktree_status_label(status: hz_command::WorktreeStatus) -> &'static str {
    match status {
        hz_command::WorktreeStatus::Clean => "[✓]",
        hz_command::WorktreeStatus::Dirty => "[!]",
        hz_command::WorktreeStatus::Unknown => "[?]",
    }
}

fn worktree_status_color(status: hz_command::WorktreeStatus) -> StyleColor {
    match status {
        hz_command::WorktreeStatus::Clean => StyleColor::Green,
        hz_command::WorktreeStatus::Dirty => StyleColor::Yellow,
        hz_command::WorktreeStatus::Unknown => StyleColor::Red,
    }
}

fn worktree_display_timestamp(worktree: &hz_command::WorktreeEntry) -> u64 {
    if worktree.modified_at_unix == 0 {
        worktree.created_at_unix
    } else {
        worktree.modified_at_unix
    }
}

fn format_modified_at(timestamp: u64) -> String {
    if timestamp == 0 {
        return "-".to_owned();
    }

    format_unix_timestamp(timestamp).unwrap_or_else(|| timestamp.to_string())
}

fn format_unix_timestamp(timestamp: u64) -> Option<String> {
    let timestamp = timestamp.to_string();
    let gnu_timestamp = format!("@{timestamp}");
    run_date_command(["-r", timestamp.as_str(), "+%b %e %H:%M"])
        .or_else(|| run_date_command(["-d", gnu_timestamp.as_str(), "+%b %e %H:%M"]))
}

fn run_date_command<const N: usize>(args: [&str; N]) -> Option<String> {
    let output = ProcessCommand::new("date").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }

    let formatted = String::from_utf8(output.stdout).ok()?;
    let formatted = formatted.trim_end();
    if formatted.is_empty() {
        None
    } else {
        Some(formatted.to_owned())
    }
}

#[cfg(test)]
fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn render_created_worktree(created: &hz_command::CreatedWorktree, color: bool) -> String {
    let mut output = format!(
        "{} {}  {}\n",
        styled("+", StyleColor::Green, color),
        styled("created", StyleColor::Green, color),
        styled(&created.branch, StyleColor::White, color)
    );

    if created.handle != created.branch {
        output.push_str(&render_field(
            "handle",
            &created.handle,
            StyleColor::White,
            color,
        ));
    }
    output.push_str(&render_field(
        "path",
        &created.path.display().to_string(),
        StyleColor::White,
        color,
    ));
    if let Some(base) = &created.base {
        output.push_str(&render_field("base", base, StyleColor::White, color));
    }

    output
}

fn render_removed_worktree(worktree: &hz_command::WorktreeEntry, color: bool) -> String {
    let mut output = format!(
        "{} {}  {}\n",
        styled("-", StyleColor::Yellow, color),
        styled("removed", StyleColor::Yellow, color),
        styled(
            worktree_branch_or_handle(worktree),
            StyleColor::White,
            color
        )
    );
    output.push_str(&render_field(
        "path",
        &worktree.path.display().to_string(),
        StyleColor::White,
        color,
    ));

    output
}

fn render_handoff(handoff: &hz_command::WorktreeHandoff, color: bool) -> String {
    let name_width = display_width(&handoff.from.name).max(display_width(&handoff.to.name));
    let mut output = render_field(
        "repo",
        &handoff.repo.display().to_string(),
        StyleColor::White,
        color,
    );
    output.push_str(&render_handoff_endpoint(
        "<",
        "from",
        &handoff.from.name,
        &handoff.from.path.display().to_string(),
        name_width,
        color,
    ));
    output.push_str(&render_handoff_endpoint(
        ">",
        "to",
        &handoff.to.name,
        &handoff.to.path.display().to_string(),
        name_width,
        color,
    ));

    output
}

fn render_handoff_endpoint(
    marker: &str,
    label: &str,
    name: &str,
    path: &str,
    name_width: usize,
    color: bool,
) -> String {
    format!(
        "{} {}  {}  {}\n",
        styled(marker, StyleColor::Cyan, color),
        styled_cell(label, 4, StyleColor::Cyan, color),
        styled_cell(name, name_width, StyleColor::White, color),
        styled(path, StyleColor::White, color)
    )
}

fn render_shell_init(shell: &str, init: &hz_command::ShellInit, color: bool) -> String {
    let (marker, status, status_color) = if init.changed {
        ("+", "installed", StyleColor::Green)
    } else {
        ("=", "exists", StyleColor::Yellow)
    };

    let mut output = format!(
        "{} {}  {}\n",
        styled(marker, status_color, color),
        styled(status, status_color, color),
        styled(shell, StyleColor::White, color)
    );
    output.push_str(&render_field(
        "path",
        &init.path.display().to_string(),
        StyleColor::White,
        color,
    ));

    output
}

fn render_field(label: &str, value: &str, value_color: StyleColor, color: bool) -> String {
    format!(
        "  {}  {}\n",
        styled_cell(label, 6, StyleColor::Cyan, color),
        styled(value, value_color, color)
    )
}

fn display_width(value: &str) -> usize {
    value.chars().count()
}

#[derive(Clone, Copy)]
enum StyleColor {
    Green,
    Cyan,
    Red,
    Yellow,
    White,
}

fn styled_cell(value: &str, width: usize, color: StyleColor, enabled: bool) -> String {
    styled(&format!("{value:<width$}"), color, enabled)
}

fn styled(value: &str, color: StyleColor, enabled: bool) -> String {
    if !enabled {
        return value.to_owned();
    }

    let code = match color {
        StyleColor::Green => "32",
        StyleColor::Cyan => "36",
        StyleColor::Red => "31",
        StyleColor::Yellow => "33",
        StyleColor::White => "37",
    };

    format!("\x1b[{code}m{value}\x1b[0m")
}

fn remove_worktree(args: RemoveWorktreeArgs) -> HzResult<()> {
    let debug = args.debug;
    let force = args.force;
    let candidate = hz_command::find_worktree(hz_command::FindWorktree {
        target: args.target.clone(),
        repo: args.repo.clone(),
    })?;

    if should_confirm_unmanaged_removal(&args, &candidate)?
        && !confirm_unmanaged_removal(&candidate)?
    {
        eprintln!("not removed");
        return Ok(());
    }

    let removed = hz_command::remove_found_worktree_with_force(candidate, force)?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&removed)?);
    } else if debug {
        print!(
            "{}",
            render_removed_worktree(&removed, io::stdout().is_terminal())
        );
    }

    Ok(())
}

fn should_confirm_unmanaged_removal(
    args: &RemoveWorktreeArgs,
    worktree: &hz_command::WorktreeEntry,
) -> HzResult<bool> {
    should_confirm_unmanaged_removal_with_stdin(args, worktree, io::stdin().is_terminal())
}

fn should_confirm_unmanaged_removal_with_stdin(
    args: &RemoveWorktreeArgs,
    worktree: &hz_command::WorktreeEntry,
    stdin_is_terminal: bool,
) -> HzResult<bool> {
    if worktree.source == hz_command::WorktreeSource::Managed || args.force {
        return Ok(false);
    }

    if args.json {
        return Err(hz_core::HzError::Usage(
            "refusing to remove unmanaged worktree in --json mode without --force".to_owned(),
        ));
    }

    if !stdin_is_terminal {
        return Err(hz_core::HzError::Usage(
            "refusing to prompt for unmanaged worktree removal without a terminal; use --force"
                .to_owned(),
        ));
    }

    Ok(true)
}

fn confirm_unmanaged_removal(worktree: &hz_command::WorktreeEntry) -> HzResult<bool> {
    let color = io::stderr().is_terminal();
    eprint!(
        "{} {} at {} is not managed by hz. Delete it with git worktree remove? [y/N] ",
        styled("!", StyleColor::Yellow, color),
        styled(
            worktree_branch_or_handle(worktree),
            StyleColor::White,
            color
        ),
        styled(
            &worktree.path.display().to_string(),
            StyleColor::White,
            color
        )
    );
    io::stderr().flush()?;

    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes" | "YES" | "Yes"))
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
        print!("{}", render_handoff(&handoff, io::stdout().is_terminal()));
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
    print!(
        "{}",
        render_shell_init(shell_name(args.shell), &init, io::stdout().is_terminal())
    );

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

fn shell_name(shell: ShellArg) -> &'static str {
    match shell {
        ShellArg::Zsh => "zsh",
        ShellArg::Bash => "bash",
        ShellArg::Fish => "fish",
    }
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
            modified_at_unix: 0,
            status: hz_command::WorktreeStatus::Unknown,
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
            modified_at_unix: 0,
            status: hz_command::WorktreeStatus::Unknown,
        }]);
        let row = output
            .lines()
            .nth(1)
            .expect("worktree row should be rendered");
        let columns: Vec<_> = row.split_whitespace().collect();

        assert_eq!(
            columns,
            vec!["*", "generated-handle", "[?]", "-", "/worktrees/entry"]
        );
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
            modified_at_unix: 0,
            status: hz_command::WorktreeStatus::Unknown,
        }]);

        assert!(output.starts_with("  branch  status  modified  path"));
    }

    #[test]
    fn list_output_omits_source_column() {
        let output = render_worktree_list(&[
            hz_command::WorktreeEntry {
                id: "managed-id".to_owned(),
                handle: "alpha".to_owned(),
                repo: PathBuf::from("/repo"),
                path: PathBuf::from("/worktrees/alpha"),
                branch: Some("alpha".to_owned()),
                base: None,
                source: hz_command::WorktreeSource::Managed,
                created_at_unix: 0,
                modified_at_unix: 0,
                status: hz_command::WorktreeStatus::Unknown,
            },
            hz_command::WorktreeEntry {
                id: "git-id".to_owned(),
                handle: "beta".to_owned(),
                repo: PathBuf::from("/repo"),
                path: PathBuf::from("/worktrees/beta"),
                branch: None,
                base: None,
                source: hz_command::WorktreeSource::Git,
                created_at_unix: 0,
                modified_at_unix: 0,
                status: hz_command::WorktreeStatus::Unknown,
            },
        ]);

        assert!(!output.contains("source"));
        assert!(!output.lines().any(|line| line.ends_with(" managed")));
        assert!(!output.lines().any(|line| line.ends_with(" git")));
    }

    #[test]
    fn list_output_renders_status_and_modified_columns() {
        let dirty_at = unix_now();
        let created_at = dirty_at.saturating_sub(60 * 60);
        let output = render_worktree_list(&[
            hz_command::WorktreeEntry {
                id: "dirty-id".to_owned(),
                handle: "dirty-worktree".to_owned(),
                repo: PathBuf::from("/repo"),
                path: PathBuf::from("/worktrees/dirty"),
                branch: Some("dirty-worktree".to_owned()),
                base: None,
                source: hz_command::WorktreeSource::Managed,
                created_at_unix: 0,
                modified_at_unix: dirty_at,
                status: hz_command::WorktreeStatus::Dirty,
            },
            hz_command::WorktreeEntry {
                id: "clean-id".to_owned(),
                handle: "clean-worktree".to_owned(),
                repo: PathBuf::from("/repo"),
                path: PathBuf::from("/worktrees/clean"),
                branch: Some("clean-worktree".to_owned()),
                base: None,
                source: hz_command::WorktreeSource::Managed,
                created_at_unix: created_at,
                modified_at_unix: 0,
                status: hz_command::WorktreeStatus::Clean,
            },
        ]);

        assert!(output.contains("status"));
        assert!(output.contains("modified"));
        assert!(output.contains("[!]"));
        assert!(output.contains("[✓]"));
        assert!(output.contains(&format_modified_at(dirty_at)));
        assert!(output.contains(&format_modified_at(created_at)));
    }

    #[test]
    fn list_output_can_render_terminal_color() {
        let output = render_worktree_list_with_style(
            &[hz_command::WorktreeEntry {
                id: "entry-id".to_owned(),
                handle: "generated-handle".to_owned(),
                repo: PathBuf::from("/repo"),
                path: PathBuf::from("/worktrees/entry"),
                branch: Some("feature/ui".to_owned()),
                base: None,
                source: hz_command::WorktreeSource::Managed,
                created_at_unix: 0,
                modified_at_unix: 0,
                status: hz_command::WorktreeStatus::Unknown,
            }],
            true,
        );

        assert!(output.contains("\x1b["));
        assert!(output.contains("*"));
        assert!(output.contains("\x1b[37mfeature/ui"));
        assert!(!output.contains("\x1b[34m"));
        assert!(!output.contains("\x1b[35m"));
    }

    #[test]
    fn created_output_renders_human_summary() {
        let output = render_created_worktree(
            &hz_command::CreatedWorktree {
                id: "entry-id".to_owned(),
                name: "generated-handle".to_owned(),
                handle: "generated-handle".to_owned(),
                repo: PathBuf::from("/repo"),
                path: PathBuf::from("/worktrees/entry"),
                branch: "feature/ui".to_owned(),
                base: Some("main".to_owned()),
                source: hz_command::WorktreeSource::Managed,
            },
            false,
        );

        assert!(output.starts_with("+ created  feature/ui"));
        assert!(output.contains("handle  generated-handle"));
        assert!(output.contains("path    /worktrees/entry"));
        assert!(output.contains("base    main"));
    }

    #[test]
    fn removed_output_renders_human_summary() {
        let output = render_removed_worktree(
            &hz_command::WorktreeEntry {
                id: "entry-id".to_owned(),
                handle: "generated-handle".to_owned(),
                repo: PathBuf::from("/repo"),
                path: PathBuf::from("/worktrees/entry"),
                branch: Some("feature/ui".to_owned()),
                base: None,
                source: hz_command::WorktreeSource::Managed,
                created_at_unix: 0,
                modified_at_unix: 0,
                status: hz_command::WorktreeStatus::Unknown,
            },
            false,
        );

        assert!(output.starts_with("- removed  feature/ui"));
        assert!(output.contains("path    /worktrees/entry"));
    }

    #[test]
    fn handoff_output_renders_human_summary() {
        let output = render_handoff(
            &hz_command::WorktreeHandoff {
                repo: PathBuf::from("/repo"),
                from: hz_core::paths::WorktreeTarget {
                    name: "local".to_owned(),
                    path: PathBuf::from("/repo"),
                },
                to: hz_core::paths::WorktreeTarget {
                    name: "feature/ui".to_owned(),
                    path: PathBuf::from("/worktrees/entry"),
                },
            },
            false,
        );

        assert!(output.contains("repo    /repo"));
        assert!(output.contains("< from  local"));
        assert!(output.contains("> to    feature/ui"));
    }

    #[test]
    fn shell_init_output_renders_status() {
        let output = render_shell_init(
            "zsh",
            &hz_command::ShellInit {
                path: PathBuf::from("/home/me/.zshrc"),
                line: "eval \"$(hz shell zsh)\"",
                changed: true,
            },
            false,
        );

        assert!(output.starts_with("+ installed  zsh"));
        assert!(output.contains("path    /home/me/.zshrc"));
    }

    #[test]
    fn remove_accepts_short_force_flag() {
        let cli = Cli::try_parse_from(["hz", "rm", "-f", "target"]).unwrap();

        match cli.command {
            Some(Command::Remove(args)) => {
                assert_eq!(args.target, "target");
                assert!(args.force);
            }
            command => panic!("expected remove command, got {command:?}"),
        }
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
            modified_at_unix: 0,
            status: hz_command::WorktreeStatus::Unknown,
        };

        assert_eq!(worktree_branch_or_handle(&worktree), "feature/ui");
    }

    #[test]
    fn unmanaged_json_removal_requires_force() {
        let args = remove_args(true, false);
        let worktree = test_entry(hz_command::WorktreeSource::Git);

        let error = should_confirm_unmanaged_removal(&args, &worktree).unwrap_err();

        assert_eq!(
            error.to_string(),
            "refusing to remove unmanaged worktree in --json mode without --force"
        );
    }

    #[test]
    fn force_skips_unmanaged_removal_confirmation() {
        let args = remove_args(false, true);
        let worktree = test_entry(hz_command::WorktreeSource::Git);

        assert!(!should_confirm_unmanaged_removal(&args, &worktree).unwrap());
    }

    #[test]
    fn managed_removal_skips_confirmation() {
        let args = remove_args(false, false);
        let worktree = test_entry(hz_command::WorktreeSource::Managed);

        assert!(!should_confirm_unmanaged_removal(&args, &worktree).unwrap());
    }

    #[test]
    fn unmanaged_non_interactive_removal_requires_force() {
        let args = remove_args(false, false);
        let worktree = test_entry(hz_command::WorktreeSource::Git);

        let error =
            should_confirm_unmanaged_removal_with_stdin(&args, &worktree, false).unwrap_err();

        assert_eq!(
            error.to_string(),
            "refusing to prompt for unmanaged worktree removal without a terminal; use --force"
        );
    }

    #[test]
    fn unmanaged_interactive_removal_confirms() {
        let args = remove_args(false, false);
        let worktree = test_entry(hz_command::WorktreeSource::Git);

        assert!(should_confirm_unmanaged_removal_with_stdin(&args, &worktree, true).unwrap());
    }

    fn remove_args(json: bool, force: bool) -> RemoveWorktreeArgs {
        RemoveWorktreeArgs {
            target: "target".to_owned(),
            repo: None,
            json,
            force,
            debug: false,
        }
    }

    fn test_entry(source: hz_command::WorktreeSource) -> hz_command::WorktreeEntry {
        hz_command::WorktreeEntry {
            id: "entry-id".to_owned(),
            handle: "generated-handle".to_owned(),
            repo: PathBuf::from("/repo"),
            path: PathBuf::from("/worktrees/entry-id"),
            branch: Some("feature/ui".to_owned()),
            base: None,
            source,
            created_at_unix: 0,
            modified_at_unix: 0,
            status: hz_command::WorktreeStatus::Unknown,
        }
    }
}
