use std::{
    env, fs,
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, ExitCode},
};

use clap::{
    Args, Parser, Subcommand, ValueEnum,
    builder::styling::{AnsiColor, Styles},
};
use crossterm::terminal as crossterm_terminal;
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
  hz cd feature/ui
  hz handoff feature/ui";

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
    #[command(about = "Apply changes between local and a linked worktree")]
    Handoff(HandoffWorktreeArgs),
    #[command(about = "Install shell integration into your shell rc file")]
    Init(InitArgs),
    #[command(about = "Print shell integration script")]
    Shell(InitArgs),
    #[command(about = "Render a Git diff")]
    Diff(DiffArgs),
    #[command(about = "Open the hz terminal UI")]
    Tui,
    #[command(name = "__complete", hide = true)]
    Complete(CompleteArgs),
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
    #[command(about = "Apply changes between local and a linked worktree")]
    Handoff(HandoffWorktreeArgs),
}

#[derive(Debug, Args)]
struct NewWorktreeArgs {
    name: Option<String>,
    #[arg(short = 'r', long)]
    repo: Option<PathBuf>,
    #[arg(short = 'p', long)]
    path: Option<PathBuf>,
    #[arg(short = 'B', long)]
    base: Option<String>,
    #[arg(short = 'b', long)]
    branch: Option<String>,
    #[arg(short = 'j', long)]
    json: bool,
    #[arg(short = 'd', long)]
    debug: bool,
    #[arg(long, hide = true)]
    path_only: bool,
}

#[derive(Debug, Args)]
struct PathWorktreeArgs {
    target: Option<String>,
    #[arg(short = 'r', long)]
    repo: Option<PathBuf>,
    #[arg(short = 'j', long)]
    json: bool,
    #[arg(long, hide = true)]
    path_only: bool,
}

#[derive(Debug, Args)]
struct ListWorktreeArgs {
    #[arg(short = 'r', long)]
    repo: Option<PathBuf>,
    #[arg(short = 'j', long)]
    json: bool,
}

#[derive(Debug, Args)]
struct RemoveWorktreeArgs {
    target: String,
    #[arg(short = 'r', long)]
    repo: Option<PathBuf>,
    #[arg(short = 'j', long)]
    json: bool,
    #[arg(short = 'f', long, alias = "yes")]
    force: bool,
    #[arg(short = 'd', long)]
    debug: bool,
}

#[derive(Debug, Args)]
struct HandoffWorktreeArgs {
    target: Option<String>,
    #[arg(short = 'b', long)]
    branch: bool,
    #[arg(short = 'r', long)]
    repo: Option<PathBuf>,
    #[arg(short = 'j', long)]
    json: bool,
    #[arg(long, hide = true)]
    path_only: bool,
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
    #[arg(short = 'r', long)]
    repo: Option<PathBuf>,
    #[arg(short = 'b', long)]
    base: Option<String>,
    #[arg(short = 's', long)]
    stat: bool,
}

#[derive(Debug, Args)]
struct CompleteArgs {
    kind: CompletionKind,
    #[arg(short = 'r', long)]
    repo: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum CompletionKind {
    WorktreeTargets,
    RemovableWorktrees,
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
        Some(Command::Complete(args)) => complete(args),
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
    let worktrees = hz_command::list_worktrees(hz_command::ListWorktrees {
        repo: args.repo.clone(),
    })?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&worktrees)?);
    } else {
        let local = hz_command::local_worktree(hz_command::LocalWorktree {
            repo: args.repo.clone(),
        })?;
        let current_path =
            hz_command::current_worktree_path(hz_command::ListWorktrees { repo: None }).ok();
        let terminal = io::stdout().is_terminal();
        print!(
            "{}",
            render_worktree_list_with_context(
                &local,
                &worktrees,
                current_path.as_deref(),
                terminal,
                list_glyphs(terminal && !ascii_output_requested()),
                terminal.then(terminal_width).flatten(),
            )
        );
    }

    Ok(())
}

#[cfg(test)]
fn render_worktree_list(worktrees: &[hz_command::WorktreeEntry]) -> String {
    render_worktree_list_with_style(worktrees, false)
}

#[cfg(test)]
fn render_worktree_list_with_style(worktrees: &[hz_command::WorktreeEntry], color: bool) -> String {
    render_worktree_rows(
        &worktree_rows(None, worktrees, None),
        color,
        list_glyphs(color),
        None,
    )
}

fn render_worktree_list_with_context(
    local: &hz_command::LocalWorktreeInfo,
    worktrees: &[hz_command::WorktreeEntry],
    current_path: Option<&Path>,
    color: bool,
    glyphs: ListGlyphs,
    terminal_width: Option<usize>,
) -> String {
    render_worktree_rows(
        &worktree_rows(Some(local), worktrees, current_path),
        color,
        glyphs,
        terminal_width,
    )
}

#[derive(Debug)]
struct WorktreeListRow {
    target: String,
    status: hz_command::WorktreeStatus,
    modified_at_unix: u64,
    path: PathBuf,
    local: bool,
    current: bool,
}

fn worktree_rows(
    local: Option<&hz_command::LocalWorktreeInfo>,
    worktrees: &[hz_command::WorktreeEntry],
    current_path: Option<&Path>,
) -> Vec<WorktreeListRow> {
    let mut rows = Vec::new();

    if let Some(local) = local {
        rows.push(WorktreeListRow {
            target: "local".to_owned(),
            status: local.status,
            modified_at_unix: local.modified_at_unix,
            path: local.path.clone(),
            local: true,
            current: current_path.is_some_and(|current| same_path(&local.path, current)),
        });
    }

    rows.extend(worktrees.iter().map(|worktree| WorktreeListRow {
        target: worktree_branch_or_handle(worktree).to_owned(),
        status: worktree.status,
        modified_at_unix: worktree_display_timestamp(worktree),
        path: worktree.path.clone(),
        local: false,
        current: current_path.is_some_and(|current| same_path(&worktree.path, current)),
    }));

    rows
}

fn render_worktree_rows(
    rows: &[WorktreeListRow],
    color: bool,
    glyphs: ListGlyphs,
    terminal_width: Option<usize>,
) -> String {
    if rows.is_empty() {
        return String::new();
    }

    let target_values: Vec<_> = rows.iter().map(|row| row.target.as_str()).collect();
    let modified_values: Vec<_> = rows
        .iter()
        .map(|row| format_modified_at(row.modified_at_unix))
        .collect();
    let path_values: Vec<_> = rows.iter().map(|row| display_path(&row.path)).collect();
    let status_values: Vec<_> = rows
        .iter()
        .map(|row| worktree_status_label(row.status, glyphs))
        .collect();

    if let Some(width) = terminal_width
        && width < 50
    {
        return render_compact_worktree_rows(
            rows,
            &target_values,
            &status_values,
            &modified_values,
            width,
            color,
            glyphs,
        );
    }

    let target_width = rows
        .iter()
        .map(|row| display_width(&row.target))
        .chain([6])
        .max()
        .expect("width candidates should not be empty");
    let modified_width = modified_values
        .iter()
        .map(|modified| display_width(modified))
        .chain([8])
        .max()
        .expect("width candidates should not be empty");
    let status_width = status_values
        .iter()
        .map(|status| display_width(status))
        .chain([2])
        .max()
        .expect("width candidates should not be empty");
    let show_path = terminal_width.is_none_or(|width| width >= 64);
    let column_widths = worktree_column_widths(WorktreeColumnInput {
        target_width,
        status_width,
        modified_width,
        path_width: max_display_width(&path_values).max(4),
        show_path,
        terminal_width,
    });
    let mut output = String::new();

    let marker_header = plain_cell("", column_widths.marker);
    let target_header = styled_cell("target", column_widths.target, StyleColor::Cyan, color);
    let status_header = styled_cell("st", column_widths.status, StyleColor::Cyan, color);
    let modified_header = styled_cell("modified", column_widths.modified, StyleColor::Cyan, color);
    let path_header = styled_cell("path", column_widths.path, StyleColor::Cyan, color);
    output.push_str(&format!(
        "{marker_header} {target_header} {status_header} {modified_header}"
    ));
    if show_path {
        output.push_str(&format!(" {path_header}"));
    }
    output.push('\n');

    for (index, row) in rows.iter().enumerate() {
        let marker = styled(
            &plain_cell(worktree_marker(row, glyphs), column_widths.marker),
            worktree_marker_color(row),
            color,
        );
        let target = styled_truncated_cell(
            target_values[index],
            column_widths.target,
            StyleColor::Magenta,
            color,
            glyphs,
        );
        let status = styled_cell(
            status_values[index],
            column_widths.status,
            worktree_status_color(row.status),
            color,
        );
        let modified = styled_cell(
            &modified_values[index],
            column_widths.modified,
            StyleColor::White,
            color,
        );
        output.push_str(&format!("{marker} {target} {status} {modified}"));
        if show_path {
            let path = styled_truncated_cell(
                &path_values[index],
                column_widths.path,
                StyleColor::White,
                color,
                glyphs,
            );
            output.push_str(&format!(" {path}"));
        }
        output.push('\n');
    }

    output
}

#[derive(Clone, Copy)]
struct WorktreeColumnWidths {
    marker: usize,
    target: usize,
    status: usize,
    modified: usize,
    path: usize,
}

struct WorktreeColumnInput {
    target_width: usize,
    status_width: usize,
    modified_width: usize,
    path_width: usize,
    show_path: bool,
    terminal_width: Option<usize>,
}

fn worktree_column_widths(input: WorktreeColumnInput) -> WorktreeColumnWidths {
    let marker_width = 1;
    let mut widths = WorktreeColumnWidths {
        marker: marker_width,
        target: input.target_width,
        status: input.status_width,
        modified: input.modified_width,
        path: if input.show_path { input.path_width } else { 0 },
    };

    let Some(terminal_width) = input.terminal_width else {
        return widths;
    };

    let visible_columns = 3 + usize::from(input.show_path);
    let fixed_width = marker_width + visible_columns + input.status_width + input.modified_width;
    let available_width = terminal_width.saturating_sub(fixed_width);

    if input.show_path {
        let target_cap = (terminal_width / 3).max(12);
        let target_width = input.target_width.min(target_cap).max(6);
        let remaining = available_width.saturating_sub(target_width);
        if remaining >= 16 {
            widths.target = target_width;
            widths.path = remaining;
        } else {
            widths.target = target_width.min(available_width.saturating_sub(16)).max(6);
            widths.path = available_width.saturating_sub(widths.target);
        }
    } else {
        widths.target = input.target_width.min(available_width).max(6);
        widths.path = 0;
    }

    widths
}

fn render_compact_worktree_rows(
    rows: &[WorktreeListRow],
    target_values: &[&str],
    status_values: &[&str],
    modified_values: &[String],
    terminal_width: usize,
    color: bool,
    glyphs: ListGlyphs,
) -> String {
    let marker_width = 1;
    let status_width = status_values
        .iter()
        .map(|status| display_width(status))
        .chain([2])
        .max()
        .expect("width candidates should not be empty");
    let show_modified = terminal_width >= 36;
    let modified_width = if show_modified {
        modified_values
            .iter()
            .map(|modified| display_width(modified))
            .chain([8])
            .max()
            .expect("width candidates should not be empty")
    } else {
        0
    };
    let fixed_width =
        marker_width + 2 + status_width + usize::from(show_modified) * (modified_width + 1);
    let target_width = terminal_width.saturating_sub(fixed_width).max(6);
    let mut output = String::new();

    for (index, row) in rows.iter().enumerate() {
        let marker = styled(
            &plain_cell(worktree_marker(row, glyphs), marker_width),
            worktree_marker_color(row),
            color,
        );
        let target = styled_truncated_cell(
            target_values[index],
            target_width,
            StyleColor::Magenta,
            color,
            glyphs,
        );
        let status = styled_cell(
            status_values[index],
            status_width,
            worktree_status_color(row.status),
            color,
        );
        output.push_str(&format!("{marker} {target} {status}"));
        if show_modified {
            let modified = styled_cell(
                &modified_values[index],
                modified_width,
                StyleColor::White,
                color,
            );
            output.push_str(&format!(" {modified}"));
        }
        output.push('\n');
    }

    output
}

#[derive(Clone, Copy)]
struct ListGlyphs {
    current: &'static str,
    local: &'static str,
    clean: &'static str,
    dirty: &'static str,
    unknown: &'static str,
    ellipsis: &'static str,
}

fn list_glyphs(unicode: bool) -> ListGlyphs {
    if unicode {
        ListGlyphs {
            current: "●",
            local: "⌂",
            clean: "✓",
            dirty: "!",
            unknown: "?",
            ellipsis: "…",
        }
    } else {
        ListGlyphs {
            current: "@",
            local: "~",
            clean: "ok",
            dirty: "!",
            unknown: "?",
            ellipsis: "...",
        }
    }
}

fn ascii_output_requested() -> bool {
    env::var_os("HZ_ASCII").is_some()
}

fn terminal_width() -> Option<usize> {
    crossterm_terminal::size()
        .ok()
        .map(|(columns, _)| usize::from(columns))
        .filter(|columns| *columns > 0)
}

fn worktree_marker(row: &WorktreeListRow, glyphs: ListGlyphs) -> &'static str {
    if row.current {
        glyphs.current
    } else if row.local {
        glyphs.local
    } else {
        " "
    }
}

fn worktree_marker_color(row: &WorktreeListRow) -> StyleColor {
    if row.current {
        StyleColor::Green
    } else {
        StyleColor::Cyan
    }
}

fn same_path(left: &Path, right: &Path) -> bool {
    left == right
        || fs::canonicalize(left)
            .ok()
            .zip(fs::canonicalize(right).ok())
            .is_some_and(|(left, right)| left == right)
}

fn display_path(path: &Path) -> String {
    let Some(home) = env::var_os("HOME").map(PathBuf::from) else {
        return path.display().to_string();
    };

    home_relative_path(path, &home)
        .or_else(|| {
            fs::canonicalize(&home)
                .ok()
                .and_then(|home| home_relative_path(path, &home))
        })
        .unwrap_or_else(|| path.display().to_string())
}

fn home_relative_path(path: &Path, home: &Path) -> Option<String> {
    if home.as_os_str().is_empty() {
        return None;
    }

    if path == home {
        return Some("~".to_owned());
    }

    let relative = path.strip_prefix(home).ok()?;
    Some(format!("~/{}", relative.display()))
}

fn worktree_status_label(status: hz_command::WorktreeStatus, glyphs: ListGlyphs) -> &'static str {
    match status {
        hz_command::WorktreeStatus::Clean => glyphs.clean,
        hz_command::WorktreeStatus::Dirty => glyphs.dirty,
        hz_command::WorktreeStatus::Unknown => glyphs.unknown,
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
    let target = created.branch.as_deref().unwrap_or(&created.handle);
    let mut output = format!(
        "{} {}  {}\n",
        styled("+", StyleColor::Green, color),
        styled("created", StyleColor::Green, color),
        styled(target, StyleColor::White, color)
    );

    if created
        .branch
        .as_deref()
        .is_some_and(|branch| branch != created.handle)
    {
        output.push_str(&render_field(
            "handle",
            &created.handle,
            StyleColor::White,
            color,
        ));
    }
    if created.branch.is_none() {
        output.push_str(&render_field(
            "branch",
            "detached",
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
    output.push_str(&render_field(
        "mode",
        handoff_mode_label(handoff.mode),
        StyleColor::White,
        color,
    ));
    if let Some(branch) = &handoff.branch {
        output.push_str(&render_field("branch", branch, StyleColor::White, color));
    }
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

fn handoff_mode_label(mode: hz_command::HandoffMode) -> &'static str {
    match mode {
        hz_command::HandoffMode::Patch => "patch",
        hz_command::HandoffMode::Branch => "branch",
    }
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

fn max_display_width<T: AsRef<str>>(values: &[T]) -> usize {
    values
        .iter()
        .map(|value| display_width(value.as_ref()))
        .max()
        .unwrap_or(0)
}

#[derive(Clone, Copy)]
enum StyleColor {
    Green,
    Cyan,
    Magenta,
    Red,
    Yellow,
    White,
}

fn styled_cell(value: &str, width: usize, color: StyleColor, enabled: bool) -> String {
    styled(&plain_cell(value, width), color, enabled)
}

fn styled_truncated_cell(
    value: &str,
    width: usize,
    color: StyleColor,
    enabled: bool,
    glyphs: ListGlyphs,
) -> String {
    styled_cell(
        &truncate_middle(value, width, glyphs),
        width,
        color,
        enabled,
    )
}

fn plain_cell(value: &str, width: usize) -> String {
    format!(
        "{value}{}",
        " ".repeat(width.saturating_sub(display_width(value)))
    )
}

fn truncate_middle(value: &str, width: usize, glyphs: ListGlyphs) -> String {
    if display_width(value) <= width {
        return value.to_owned();
    }
    if width == 0 {
        return String::new();
    }

    let ellipsis_width = display_width(glyphs.ellipsis);
    if width <= ellipsis_width {
        return glyphs.ellipsis.chars().take(width).collect();
    }

    let available = width - ellipsis_width;
    let prefix_width = available / 2;
    let suffix_width = available - prefix_width;
    let prefix: String = value.chars().take(prefix_width).collect();
    let suffix: String = value
        .chars()
        .rev()
        .take(suffix_width)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();

    format!("{prefix}{}{suffix}", glyphs.ellipsis)
}

fn styled(value: &str, color: StyleColor, enabled: bool) -> String {
    if !enabled {
        return value.to_owned();
    }

    let code = match color {
        StyleColor::Green => "32",
        StyleColor::Cyan => "36",
        StyleColor::Magenta => "35",
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
        target: args.target,
        mode: if args.branch {
            hz_command::HandoffMode::Branch
        } else {
            hz_command::HandoffMode::Patch
        },
        repo: args.repo,
    })?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&handoff)?);
    } else if args.path_only {
        println!("{}", handoff.to.path.display());
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

fn complete(args: CompleteArgs) -> HzResult<()> {
    let include_local = args.kind == CompletionKind::WorktreeTargets;
    let Ok(candidates) = worktree_completion_candidates(args.repo, include_local) else {
        return Ok(());
    };

    for candidate in candidates {
        println!("{candidate}");
    }

    Ok(())
}

fn worktree_completion_candidates(
    repo: Option<PathBuf>,
    include_local: bool,
) -> HzResult<Vec<String>> {
    let worktrees = hz_command::list_worktrees(hz_command::ListWorktrees { repo })?;
    let mut candidates = Vec::new();

    if include_local {
        candidates.push("local".to_owned());
    }

    for worktree in worktrees {
        push_completion_candidate(&mut candidates, worktree.branch);
        push_completion_candidate(&mut candidates, Some(worktree.handle));
        push_completion_candidate(&mut candidates, Some(worktree.id));
    }

    Ok(candidates)
}

fn push_completion_candidate(candidates: &mut Vec<String>, candidate: Option<String>) {
    let Some(candidate) = candidate else {
        return;
    };

    if !candidate.is_empty() && !candidates.iter().any(|existing| existing == &candidate) {
        candidates.push(candidate);
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

        assert!(output.contains("target"));
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
            vec!["generated-handle", "?", "-", "/worktrees/entry"]
        );
    }

    #[test]
    fn home_relative_paths_use_tilde_only_for_home_children() {
        let home = PathBuf::from("/Users/dev");

        assert_eq!(
            home_relative_path(&PathBuf::from("/Users/dev/.hz/worktrees/hz"), &home).as_deref(),
            Some("~/.hz/worktrees/hz")
        );
        assert_eq!(
            home_relative_path(&PathBuf::from("/Users/dev"), &home).as_deref(),
            Some("~")
        );
        assert_eq!(
            home_relative_path(&PathBuf::from("/Users/dev-other/project"), &home),
            None
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

        assert!(output.starts_with("  target st modified path"));
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

        assert!(output.contains("st"));
        assert!(output.contains("modified"));
        assert!(output.contains("!"));
        assert!(output.contains("ok"));
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
        assert!(output.contains("\x1b[35mfeature/ui"));
        assert!(output.contains("\x1b[37m/worktrees/entry"));
        assert!(!output.contains("\x1b[34m"));
    }

    #[test]
    fn list_output_marks_current_worktree() {
        let local = hz_command::LocalWorktreeInfo {
            repo: PathBuf::from("/repo"),
            path: PathBuf::from("/repo"),
            branch: Some("main".to_owned()),
            status: hz_command::WorktreeStatus::Clean,
            modified_at_unix: 0,
            handoff_from: None,
        };
        let output = render_worktree_list_with_context(
            &local,
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
            Some(&PathBuf::from("/worktrees/entry")),
            false,
            list_glyphs(false),
            None,
        );

        let current_row = output
            .lines()
            .find(|line| line.contains("feature/ui"))
            .expect("current worktree row should be rendered");

        assert!(current_row.starts_with("@ feature/ui"));
        assert!(output.contains("~ local"));
        assert!(!output.contains("note"));
    }

    #[test]
    fn list_output_does_not_mark_local_without_current_worktree() {
        let local = hz_command::LocalWorktreeInfo {
            repo: PathBuf::from("/repo"),
            path: PathBuf::from("/repo"),
            branch: Some("main".to_owned()),
            status: hz_command::WorktreeStatus::Clean,
            modified_at_unix: 0,
            handoff_from: None,
        };
        let output =
            render_worktree_list_with_context(&local, &[], None, false, list_glyphs(false), None);

        let local_row = output
            .lines()
            .find(|line| line.contains("local"))
            .expect("local worktree row should be rendered");

        assert!(local_row.starts_with("~ local"));
        assert!(!local_row.starts_with("@ local"));
    }

    #[test]
    fn local_list_row_omits_note_column() {
        let local = hz_command::LocalWorktreeInfo {
            repo: PathBuf::from("/repo"),
            path: PathBuf::from("/repo"),
            branch: Some("feature/ui".to_owned()),
            status: hz_command::WorktreeStatus::Dirty,
            modified_at_unix: 0,
            handoff_from: Some("f7a7".to_owned()),
        };
        let output = render_worktree_list_with_context(
            &local,
            &[],
            Some(&PathBuf::from("/repo")),
            false,
            list_glyphs(false),
            None,
        );

        assert!(output.contains("@ local"));
        assert!(!output.contains("note"));
        assert!(!output.contains("branch feature/ui"));
        assert!(!output.contains("<- f7a7"));
    }

    #[test]
    fn list_output_can_render_unicode_glyphs() {
        let local = hz_command::LocalWorktreeInfo {
            repo: PathBuf::from("/repo"),
            path: PathBuf::from("/repo"),
            branch: Some("feature/ui".to_owned()),
            status: hz_command::WorktreeStatus::Clean,
            modified_at_unix: 0,
            handoff_from: Some("f7a7".to_owned()),
        };
        let output = render_worktree_list_with_context(
            &local,
            &[],
            Some(&PathBuf::from("/repo")),
            true,
            list_glyphs(true),
            None,
        );

        assert!(output.contains("●"));
        assert!(output.contains("✓"));
        assert!(!output.contains("note"));
        assert!(!output.contains("branch feature/ui"));
        assert!(!output.contains("← f7a7"));
    }

    #[test]
    fn list_output_truncates_to_terminal_width() {
        let local = hz_command::LocalWorktreeInfo {
            repo: PathBuf::from("/repo"),
            path: PathBuf::from("/repo"),
            branch: Some("main".to_owned()),
            status: hz_command::WorktreeStatus::Clean,
            modified_at_unix: 0,
            handoff_from: None,
        };
        let output = render_worktree_list_with_context(
            &local,
            &[hz_command::WorktreeEntry {
                id: "entry-id".to_owned(),
                handle: "generated-handle".to_owned(),
                repo: PathBuf::from("/repo"),
                path: PathBuf::from(
                    "/very/long/worktrees/path/that/would/otherwise/wrap/in/a/small/terminal",
                ),
                branch: Some(
                    "feat(worktree)/very-long-branch-name-that-would-push-the-table".to_owned(),
                ),
                base: None,
                source: hz_command::WorktreeSource::Managed,
                created_at_unix: 0,
                modified_at_unix: 0,
                status: hz_command::WorktreeStatus::Clean,
            }],
            Some(&PathBuf::from("/worktrees/entry")),
            false,
            list_glyphs(true),
            Some(72),
        );

        assert!(output.contains("…"));
        assert!(output.lines().all(|line| display_width(line) <= 72));
    }

    #[test]
    fn list_output_uses_compact_rows_for_tiny_terminals() {
        let output = render_worktree_rows(
            &[WorktreeListRow {
                target: "feat(worktree)/very-long-branch-name".to_owned(),
                status: hz_command::WorktreeStatus::Dirty,
                modified_at_unix: 0,
                path: PathBuf::from("/very/long/worktree/path"),
                local: false,
                current: false,
            }],
            false,
            list_glyphs(true),
            Some(32),
        );

        assert!(!output.contains("target"));
        assert!(output.lines().all(|line| display_width(line) <= 32));
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
                branch: Some("feature/ui".to_owned()),
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
    fn created_output_renders_detached_worktree_summary() {
        let output = render_created_worktree(
            &hz_command::CreatedWorktree {
                id: "entry-id".to_owned(),
                name: "generated-handle".to_owned(),
                handle: "generated-handle".to_owned(),
                repo: PathBuf::from("/repo"),
                path: PathBuf::from("/worktrees/entry"),
                branch: None,
                base: None,
                source: hz_command::WorktreeSource::Managed,
            },
            false,
        );

        assert!(output.starts_with("+ created  generated-handle"));
        assert!(output.contains("branch  detached"));
        assert!(output.contains("path    /worktrees/entry"));
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
                mode: hz_command::HandoffMode::Patch,
                branch: Some("feature/ui".to_owned()),
                from: hz_core::paths::WorktreeTarget {
                    name: "local".to_owned(),
                    path: PathBuf::from("/repo"),
                },
                to: hz_core::paths::WorktreeTarget {
                    name: "feature/ui".to_owned(),
                    path: PathBuf::from("/worktrees/entry"),
                },
                changed: true,
            },
            false,
        );

        assert!(output.contains("repo    /repo"));
        assert!(output.contains("mode    patch"));
        assert!(output.contains("branch  feature/ui"));
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
        let cli =
            Cli::try_parse_from(["hz", "rm", "-r", "/repo", "-j", "-d", "-f", "target"]).unwrap();

        match cli.command {
            Some(Command::Remove(args)) => {
                assert_eq!(args.target, "target");
                assert_eq!(args.repo, Some(PathBuf::from("/repo")));
                assert!(args.json);
                assert!(args.debug);
                assert!(args.force);
            }
            command => panic!("expected remove command, got {command:?}"),
        }
    }

    #[test]
    fn handoff_accepts_optional_branch_and_path_only() {
        let cli =
            Cli::try_parse_from(["hz", "handoff", "-r", "/repo", "-j", "feature/ui"]).unwrap();

        match cli.command {
            Some(Command::Handoff(args)) => {
                assert_eq!(args.target.as_deref(), Some("feature/ui"));
                assert_eq!(args.repo, Some(PathBuf::from("/repo")));
                assert!(args.json);
                assert!(!args.branch);
            }
            command => panic!("expected handoff command, got {command:?}"),
        }

        let cli = Cli::try_parse_from(["hz", "handoff", "708e", "-b", "--path-only"]).unwrap();
        match cli.command {
            Some(Command::Handoff(args)) => {
                assert_eq!(args.target.as_deref(), Some("708e"));
                assert!(args.branch);
                assert!(args.path_only);
            }
            command => panic!("expected handoff command, got {command:?}"),
        }

        let cli = Cli::try_parse_from(["hz", "handoff"]).unwrap();
        match cli.command {
            Some(Command::Handoff(args)) => assert_eq!(args.target, None),
            command => panic!("expected handoff command, got {command:?}"),
        }
    }

    #[test]
    fn creation_and_diff_accept_short_flags() {
        let cli = Cli::try_parse_from([
            "hz",
            "new",
            "-r",
            "/repo",
            "-p",
            "../wt",
            "-B",
            "main",
            "-b",
            "feature/ui",
            "-j",
            "-d",
            "handle",
        ])
        .unwrap();

        match cli.command {
            Some(Command::New(args)) => {
                assert_eq!(args.name.as_deref(), Some("handle"));
                assert_eq!(args.repo, Some(PathBuf::from("/repo")));
                assert_eq!(args.path, Some(PathBuf::from("../wt")));
                assert_eq!(args.base.as_deref(), Some("main"));
                assert_eq!(args.branch.as_deref(), Some("feature/ui"));
                assert!(args.json);
                assert!(args.debug);
            }
            command => panic!("expected new command, got {command:?}"),
        }

        let cli = Cli::try_parse_from(["hz", "diff", "-r", "/repo", "-b", "main", "-s"]).unwrap();
        match cli.command {
            Some(Command::Diff(args)) => {
                assert_eq!(args.repo, Some(PathBuf::from("/repo")));
                assert_eq!(args.base.as_deref(), Some("main"));
                assert!(args.stat);
            }
            command => panic!("expected diff command, got {command:?}"),
        }
    }

    #[test]
    fn path_and_list_accept_short_flags() {
        let cli = Cli::try_parse_from(["hz", "path", "-r", "/repo", "-j", "target"]).unwrap();
        match cli.command {
            Some(Command::Path(args)) => {
                assert_eq!(args.target.as_deref(), Some("target"));
                assert_eq!(args.repo, Some(PathBuf::from("/repo")));
                assert!(args.json);
            }
            command => panic!("expected path command, got {command:?}"),
        }

        let cli = Cli::try_parse_from(["hz", "ls", "-r", "/repo", "-j"]).unwrap();
        match cli.command {
            Some(Command::List(args)) => {
                assert_eq!(args.repo, Some(PathBuf::from("/repo")));
                assert!(args.json);
            }
            command => panic!("expected list command, got {command:?}"),
        }
    }

    #[test]
    fn hidden_completion_command_accepts_kind_and_repo() {
        let cli =
            Cli::try_parse_from(["hz", "__complete", "worktree-targets", "-r", "/repo"]).unwrap();

        match cli.command {
            Some(Command::Complete(args)) => {
                assert_eq!(args.kind, CompletionKind::WorktreeTargets);
                assert_eq!(args.repo, Some(PathBuf::from("/repo")));
            }
            command => panic!("expected complete command, got {command:?}"),
        }
    }

    #[test]
    fn completion_candidates_are_deduplicated() {
        let mut candidates = vec!["local".to_owned()];

        push_completion_candidate(&mut candidates, Some("feature/ui".to_owned()));
        push_completion_candidate(&mut candidates, Some("feature/ui".to_owned()));
        push_completion_candidate(&mut candidates, Some(String::new()));
        push_completion_candidate(&mut candidates, None);

        assert_eq!(candidates, vec!["local", "feature/ui"]);
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
