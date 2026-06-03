use std::{
    collections::HashSet,
    env,
    ffi::{OsStr, OsString},
    fs,
    io::{self, IsTerminal, Read, Write},
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, ExitCode, Stdio},
    sync::Arc,
};

use clap::{
    Args, Parser, Subcommand, ValueEnum,
    builder::styling::{AnsiColor, Styles},
};
use crossterm::terminal as crossterm_terminal;
use hz_core::HzResult;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

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
  hz init
  hz install zsh
  hz new feature/ui
  hz ls
  hz rm -f feature/ui
  hz setup feature/ui
  hz cleanup feature/ui
  hz cd feature/ui
  hz handoff feature/ui
  hz ts add rust mlir llvm";

const INSTALL_SCRIPT: &str = include_str!("../../scripts/install.sh");
const RELEASE_REPO: &str = "phongndo/hz";

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
  hz update --target-version 0.1.2
  hz update --install-dir ~/.local/bin"
    )]
    Update(UpdateArgs),
    #[command(about = "Review a Git diff")]
    Diff(DiffArgs),
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
enum TreeSitterCommand {
    #[command(about = "Install and enable syntax highlighting languages")]
    Add(TreeSitterLanguagesArgs),
    #[command(alias = "remove", about = "Remove syntax highlighting languages")]
    Rm(TreeSitterLanguagesArgs),
    #[command(about = "List installed and enabled syntax highlighting languages")]
    List,
    #[command(about = "List downloadable syntax highlighting languages")]
    Available,
    #[command(about = "Remove cached tree-sitter parser libraries")]
    Clean,
    #[command(about = "Print tree-sitter cache and config paths")]
    Path,
    #[command(about = "Validate enabled syntax highlighting languages")]
    Doctor,
}

#[derive(Debug, Args)]
struct TreeSitterLanguagesArgs {
    #[arg(value_name = "LANG", required = true)]
    languages: Vec<String>,
}

#[derive(Debug, Subcommand)]
enum WorktreeCommand {
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
    #[arg(long)]
    max_detached: Option<usize>,
    #[arg(short = 'j', long)]
    json: bool,
    #[arg(short = 'd', long)]
    debug: bool,
    #[arg(long)]
    no_setup: bool,
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
    #[arg(value_name = "TARGET", required = true, num_args = 1..)]
    targets: Vec<String>,
    #[arg(short = 'r', long)]
    repo: Option<PathBuf>,
    #[arg(short = 'j', long)]
    json: bool,
    #[arg(short = 'f', long, alias = "yes")]
    force: bool,
    #[arg(short = 'd', long)]
    debug: bool,
    #[arg(long)]
    no_cleanup: bool,
}

#[derive(Debug, Args)]
struct HandoffWorktreeArgs {
    target: Option<String>,
    #[arg(short = 'b', long)]
    branch: bool,
    #[arg(short = 'n', long = "new")]
    create: bool,
    #[arg(long)]
    max_detached: Option<usize>,
    #[arg(short = 'r', long)]
    repo: Option<PathBuf>,
    #[arg(short = 'j', long)]
    json: bool,
    #[arg(long, hide = true)]
    path_only: bool,
}

#[derive(Debug, Args)]
struct InitArgs {
    #[arg(value_enum)]
    shell: Option<ShellArg>,
    #[arg(short = 'r', long)]
    repo: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct ShellArgs {
    shell: ShellArg,
}

#[derive(Debug, Args)]
struct LifecycleArgs {
    target: Option<String>,
    #[arg(short = 'r', long)]
    repo: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct UpdateArgs {
    /// Release version to install, without or with the leading v.
    #[arg(long = "target-version", value_name = "VERSION")]
    version: Option<String>,
    /// Directory to update. Defaults to the directory containing the invoked hz.
    #[arg(long, value_name = "DIR")]
    install_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ShellArg {
    Zsh,
    Bash,
    Fish,
}

#[derive(Debug, Args)]
struct DiffArgs {
    #[arg(value_name = "REV", num_args = 0..=2)]
    revs: Vec<String>,
    #[arg(short = 'r', long)]
    repo: Option<PathBuf>,
    #[arg(short = 'b', long)]
    base: Option<String>,
    #[arg(long, conflicts_with = "unstaged", conflicts_with_all = ["base", "revs"])]
    staged: bool,
    #[arg(long, conflicts_with_all = ["base", "revs"])]
    unstaged: bool,
    #[arg(long = "no-untracked")]
    no_untracked: bool,
    /// Read an existing unified diff from FILE, or stdin when FILE is `-`.
    #[arg(long, value_name = "FILE")]
    patch: Option<PathBuf>,
    /// Disable live reload in the interactive diff viewer.
    #[arg(long = "no-watch")]
    no_watch: bool,
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
        Some(Command::Init(args)) => init_repo_or_shell(args),
        Some(Command::Install(args)) => install_shell(args),
        Some(Command::Setup(args)) => run_lifecycle(args, hz_command::LifecycleKind::Setup),
        Some(Command::Cleanup(args)) => run_lifecycle(args, hz_command::LifecycleKind::Cleanup),
        Some(Command::Shell(args)) => shell_script(args),
        Some(Command::Update(args)) => update(args),
        Some(Command::Diff(args)) => {
            let stat = args.stat;
            let live_updates = !args.no_watch;
            let options = diff_options(args)?;
            if io::stdout().is_terminal() && !stat {
                hz_tui::run_diff_with_live_updates(options, live_updates)
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

fn tree_sitter(command: TreeSitterCommand) -> HzResult<()> {
    match command {
        TreeSitterCommand::Add(args) => {
            let result = hz_command::syntax_add(&args.languages)?;
            print_tree_sitter_add_result(&result);
        }
        TreeSitterCommand::Rm(args) => {
            let result = hz_command::syntax_remove(&args.languages)?;
            print_tree_sitter_remove_result(&result);
        }
        TreeSitterCommand::List => {
            print_tree_sitter_statuses(&hz_command::syntax_statuses()?);
        }
        TreeSitterCommand::Available => {
            for language in hz_command::syntax_available_languages()? {
                println!("{language}");
            }
        }
        TreeSitterCommand::Clean => {
            hz_command::syntax_clean_cache()?;
            println!("cleaned tree-sitter parser cache");
        }
        TreeSitterCommand::Path => {
            println!("cache  {}", hz_command::syntax_cache_dir()?);
            println!("config {}", hz_command::syntax_config_path()?.display());
        }
        TreeSitterCommand::Doctor => {
            let report = hz_command::syntax_doctor()?;
            print_tree_sitter_statuses(&report.statuses);
            if report.issues.is_empty() {
                println!("ok");
            } else {
                for issue in report.issues {
                    println!("warning {}: {}", issue.language, issue.message);
                }
            }
        }
    }
    Ok(())
}

fn diff_options(args: DiffArgs) -> HzResult<hz_command::DiffOptions> {
    if let Some(patch) = args.patch {
        if args.base.is_some() || !args.revs.is_empty() {
            return Err(hz_core::HzError::Usage(
                "use --patch without revisions or --base".to_owned(),
            ));
        }
        if args.staged || args.unstaged || args.no_untracked {
            return Err(hz_core::HzError::Usage(
                "--staged, --unstaged, and --no-untracked do not apply to --patch".to_owned(),
            ));
        }

        return Ok(hz_command::DiffOptions {
            repo: args.repo,
            source: patch_source(patch)?,
            scope: hz_command::DiffScope::All,
            include_untracked: false,
            stat: args.stat,
        });
    }

    let source = match (args.base, args.revs.as_slice()) {
        (Some(base), []) => hz_command::DiffSource::Base(base),
        (Some(_), _) => {
            return Err(hz_core::HzError::Usage(
                "use either --base or positional revisions, not both".to_owned(),
            ));
        }
        (None, []) => hz_command::DiffSource::Worktree,
        (None, [base]) => hz_command::DiffSource::Base(base.clone()),
        (None, [left, right]) => hz_command::DiffSource::Range {
            left: left.clone(),
            right: right.clone(),
        },
        (None, _) => {
            return Err(hz_core::HzError::Usage(
                "hz diff accepts at most two revisions".to_owned(),
            ));
        }
    };

    let scope = if args.staged {
        hz_command::DiffScope::Staged
    } else if args.unstaged {
        hz_command::DiffScope::Unstaged
    } else {
        hz_command::DiffScope::All
    };

    Ok(hz_command::DiffOptions {
        repo: args.repo,
        source,
        scope,
        include_untracked: !args.no_untracked,
        stat: args.stat,
    })
}

fn patch_source(path: PathBuf) -> HzResult<hz_command::DiffSource> {
    if path == Path::new("-") {
        let mut patch = String::new();
        io::stdin().read_to_string(&mut patch)?;
        return Ok(hz_command::DiffSource::Patch(
            hz_command::PatchSource::Stdin(Arc::from(patch)),
        ));
    }

    Ok(hz_command::DiffSource::Patch(
        hz_command::PatchSource::File(path),
    ))
}

fn print_tree_sitter_add_result(result: &hz_command::SyntaxAddResult) {
    for language in &result.added {
        println!("+ enabled {language}");
    }
    for language in &result.already_enabled {
        println!("= enabled {language}");
    }
    for language in &result.without_highlights {
        println!("warning {language}: no bundled highlights query; diff will render plain text");
    }
}

fn print_tree_sitter_remove_result(result: &hz_command::SyntaxRemoveResult) {
    for language in &result.removed {
        println!("- disabled {language} in config");
    }
    for language in &result.missing {
        println!("= not enabled in config {language}");
    }
    for language in &result.cache_deleted {
        println!("- deleted parser cache {language}");
    }
    for language in &result.cache_missing {
        println!("= no parser cache {language}");
    }
}

fn print_tree_sitter_statuses(statuses: &[hz_command::SyntaxLanguageStatus]) {
    if statuses.is_empty() {
        println!("no tree-sitter languages enabled");
        return;
    }

    for status in statuses {
        println!(
            "{:<20} enabled={} installed={} highlights={}",
            status.language,
            yes_no(status.enabled),
            yes_no(status.installed),
            yes_no(status.has_highlights)
        );
    }
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn create_worktree(args: NewWorktreeArgs) -> HzResult<()> {
    let debug = args.debug;
    let run_setup = !args.no_setup;
    let created = hz_command::create_worktree_with_lifecycle(
        hz_command::CreateWorktree {
            name: args.name,
            repo: args.repo,
            path: args.path,
            base: args.base,
            branch: args.branch,
            max_detached_worktrees: args.max_detached,
        },
        run_setup,
    )?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&created)?);
    } else if args.path_only {
        println!("{}", created.path.display());
        print_warnings(&created.warnings, io::stderr().is_terminal());
    } else if debug {
        print!(
            "{}",
            render_created_worktree(&created, io::stdout().is_terminal())
        );
    } else {
        print_warnings(&created.warnings, io::stderr().is_terminal());
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
        let config = hz_command::load_repo_config(hz_command::LoadRepoConfig {
            repo: args.repo.clone(),
        })?;
        let local = hz_command::local_worktree(hz_command::LocalWorktree {
            repo: args.repo.clone(),
        })?;
        let current_path =
            hz_command::current_worktree_path(hz_command::ListWorktrees { repo: None }).ok();
        let terminal = io::stdout().is_terminal();
        let color = color_output_enabled(config.color.as_ref(), terminal);
        print!(
            "{}",
            render_worktree_list_with_options(
                &local,
                &worktrees,
                current_path.as_deref(),
                color,
                list_glyphs(terminal && !ascii_output_requested()),
                terminal.then(terminal_width).flatten(),
                list_options(config.list.as_ref(), config.color.as_ref()),
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

#[cfg(test)]
fn render_worktree_list_with_context(
    local: &hz_command::LocalWorktreeInfo,
    worktrees: &[hz_command::WorktreeEntry],
    current_path: Option<&Path>,
    color: bool,
    glyphs: ListGlyphs,
    terminal_width: Option<usize>,
) -> String {
    render_worktree_list_with_options(
        local,
        worktrees,
        current_path,
        color,
        glyphs,
        terminal_width,
        WorktreeListOptions::default(),
    )
}

fn render_worktree_list_with_options(
    local: &hz_command::LocalWorktreeInfo,
    worktrees: &[hz_command::WorktreeEntry],
    current_path: Option<&Path>,
    color: bool,
    glyphs: ListGlyphs,
    terminal_width: Option<usize>,
    options: WorktreeListOptions,
) -> String {
    render_worktree_rows_with_options(
        &worktree_rows(Some(local), worktrees, current_path),
        color,
        glyphs,
        terminal_width,
        options,
    )
}

#[derive(Debug, Clone)]
struct WorktreeListOptions {
    headers: hz_command::ListHeaders,
    columns: Vec<hz_command::ListColumn>,
    compact_columns: Vec<hz_command::ListColumn>,
    colors: ListColors,
}

impl Default for WorktreeListOptions {
    fn default() -> Self {
        Self {
            headers: hz_command::ListHeaders::Auto,
            columns: default_list_columns(),
            compact_columns: vec![
                hz_command::ListColumn::Marker,
                hz_command::ListColumn::Target,
                hz_command::ListColumn::Status,
            ],
            colors: ListColors::default(),
        }
    }
}

fn list_options(
    config: Option<&hz_command::ListConfig>,
    color_config: Option<&hz_command::ColorConfig>,
) -> WorktreeListOptions {
    let mut options = WorktreeListOptions::default();
    if let Some(config) = config {
        if let Some(headers) = config.headers {
            options.headers = headers;
        }
        if let Some(columns) = non_empty_columns(config.columns.as_deref()) {
            options.columns = columns.to_vec();
        }
        if let Some(columns) = non_empty_columns(config.compact_columns.as_deref()) {
            options.compact_columns = columns.to_vec();
        }
    }
    options.colors = list_colors(color_config);
    options
}

fn non_empty_columns(
    columns: Option<&[hz_command::ListColumn]>,
) -> Option<&[hz_command::ListColumn]> {
    columns.filter(|columns| !columns.is_empty())
}

fn default_list_columns() -> Vec<hz_command::ListColumn> {
    vec![
        hz_command::ListColumn::Marker,
        hz_command::ListColumn::Target,
        hz_command::ListColumn::Status,
        hz_command::ListColumn::Modified,
        hz_command::ListColumn::Path,
    ]
}

fn color_output_enabled(config: Option<&hz_command::ColorConfig>, terminal: bool) -> bool {
    match config.and_then(|config| config.mode) {
        Some(hz_command::ColorMode::Always) => true,
        Some(hz_command::ColorMode::Never) => false,
        Some(hz_command::ColorMode::Auto) | None => terminal,
    }
}

#[derive(Debug, Clone, Copy)]
struct ListColors {
    header: StyleColor,
    target: StyleColor,
    branch: StyleColor,
    handle: StyleColor,
    base: StyleColor,
    modified: StyleColor,
    path: StyleColor,
    clean: StyleColor,
    dirty: StyleColor,
    unknown: StyleColor,
    current: StyleColor,
    local: StyleColor,
}

impl Default for ListColors {
    fn default() -> Self {
        Self {
            header: StyleColor::Cyan,
            target: StyleColor::Magenta,
            branch: StyleColor::Magenta,
            handle: StyleColor::Magenta,
            base: StyleColor::White,
            modified: StyleColor::White,
            path: StyleColor::White,
            clean: StyleColor::Green,
            dirty: StyleColor::Yellow,
            unknown: StyleColor::Red,
            current: StyleColor::Green,
            local: StyleColor::Cyan,
        }
    }
}

fn list_colors(config: Option<&hz_command::ColorConfig>) -> ListColors {
    let mut colors = ListColors::default();
    let Some(config) = config else {
        return colors;
    };
    let Some(scheme_name) = config.scheme.as_deref() else {
        return colors;
    };
    if scheme_name == "terminal" {
        return colors;
    }
    let Some(scheme) = config.schemes.get(scheme_name) else {
        return colors;
    };

    if let Some(color) = parse_style_color(scheme.header.as_deref()) {
        colors.header = color;
    }
    if let Some(color) = parse_style_color(scheme.target.as_deref()) {
        colors.target = color;
    }
    if let Some(color) = parse_style_color(scheme.branch.as_deref()) {
        colors.branch = color;
    }
    if let Some(color) = parse_style_color(scheme.handle.as_deref()) {
        colors.handle = color;
    }
    if let Some(color) = parse_style_color(scheme.base.as_deref()) {
        colors.base = color;
    }
    if let Some(color) = parse_style_color(scheme.modified.as_deref()) {
        colors.modified = color;
    }
    if let Some(color) = parse_style_color(scheme.path.as_deref()) {
        colors.path = color;
    }
    if let Some(color) = parse_style_color(scheme.clean.as_deref()) {
        colors.clean = color;
    }
    if let Some(color) = parse_style_color(scheme.dirty.as_deref()) {
        colors.dirty = color;
    }
    if let Some(color) = parse_style_color(scheme.unknown.as_deref()) {
        colors.unknown = color;
    }
    if let Some(color) = parse_style_color(scheme.current.as_deref()) {
        colors.current = color;
    }
    if let Some(color) = parse_style_color(scheme.local.as_deref()) {
        colors.local = color;
    }

    colors
}

fn parse_style_color(value: Option<&str>) -> Option<StyleColor> {
    match value? {
        "black" => Some(StyleColor::Black),
        "red" => Some(StyleColor::Red),
        "green" => Some(StyleColor::Green),
        "yellow" => Some(StyleColor::Yellow),
        "blue" => Some(StyleColor::Blue),
        "magenta" => Some(StyleColor::Magenta),
        "cyan" => Some(StyleColor::Cyan),
        "white" => Some(StyleColor::White),
        _ => None,
    }
}

#[derive(Debug)]
struct WorktreeListRow {
    target: String,
    branch: Option<String>,
    handle: Option<String>,
    base: Option<String>,
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
            branch: local.branch.clone(),
            handle: None,
            base: None,
            status: local.status,
            modified_at_unix: local.modified_at_unix,
            path: local.path.clone(),
            local: true,
            current: current_path.is_some_and(|current| same_path(&local.path, current)),
        });
    }

    rows.extend(worktrees.iter().map(|worktree| WorktreeListRow {
        target: worktree_branch_or_handle(worktree).to_owned(),
        branch: worktree.branch.clone(),
        handle: Some(worktree.handle.clone()),
        base: worktree.base.clone(),
        status: worktree.status,
        modified_at_unix: worktree_display_timestamp(worktree),
        path: worktree.path.clone(),
        local: false,
        current: current_path.is_some_and(|current| same_path(&worktree.path, current)),
    }));

    rows
}

#[cfg(test)]
fn render_worktree_rows(
    rows: &[WorktreeListRow],
    color: bool,
    glyphs: ListGlyphs,
    terminal_width: Option<usize>,
) -> String {
    render_worktree_rows_with_options(
        rows,
        color,
        glyphs,
        terminal_width,
        WorktreeListOptions::default(),
    )
}

fn render_worktree_rows_with_options(
    rows: &[WorktreeListRow],
    color: bool,
    glyphs: ListGlyphs,
    terminal_width: Option<usize>,
    options: WorktreeListOptions,
) -> String {
    if rows.is_empty() {
        return String::new();
    }

    let compact = terminal_width.is_some_and(|width| width < 50);
    let columns = if compact {
        &options.compact_columns
    } else {
        &options.columns
    };
    let columns = if columns.is_empty() {
        default_list_columns()
    } else {
        columns.clone()
    };
    let show_headers = match options.headers {
        hz_command::ListHeaders::Always => true,
        hz_command::ListHeaders::Never => false,
        hz_command::ListHeaders::Auto => !compact,
    };
    let values: Vec<Vec<String>> = columns
        .iter()
        .map(|column| {
            rows.iter()
                .map(|row| list_cell_value(row, *column, glyphs))
                .collect()
        })
        .collect();
    let mut widths: Vec<usize> = columns
        .iter()
        .enumerate()
        .map(|(index, column)| {
            let header_width = if show_headers {
                display_width(list_column_header(*column))
            } else {
                0
            };
            values[index]
                .iter()
                .map(|value| display_width(value))
                .chain([header_width, list_column_min_width(*column)])
                .max()
                .expect("width candidates should not be empty")
        })
        .collect();

    shrink_list_columns(&columns, &mut widths, terminal_width);

    let mut output = String::new();

    if show_headers {
        for (index, column) in columns.iter().enumerate() {
            if index > 0 {
                output.push(' ');
            }
            output.push_str(&styled_cell(
                list_column_header(*column),
                widths[index],
                options.colors.header,
                color,
            ));
        }
        output.push('\n');
    }

    for row_index in 0..rows.len() {
        for (column_index, column) in columns.iter().enumerate() {
            if column_index > 0 {
                output.push(' ');
            }
            output.push_str(&styled_list_cell(
                &values[column_index][row_index],
                widths[column_index],
                *column,
                &rows[row_index],
                color,
                glyphs,
                options.colors,
            ));
        }
        output.push('\n');
    }

    output
}

fn list_cell_value(
    row: &WorktreeListRow,
    column: hz_command::ListColumn,
    glyphs: ListGlyphs,
) -> String {
    match column {
        hz_command::ListColumn::Marker => worktree_marker(row, glyphs).to_owned(),
        hz_command::ListColumn::Target => row.target.clone(),
        hz_command::ListColumn::Branch => row.branch.clone().unwrap_or_else(|| "-".to_owned()),
        hz_command::ListColumn::Handle => row.handle.clone().unwrap_or_else(|| "-".to_owned()),
        hz_command::ListColumn::Status => worktree_status_label(row.status, glyphs).to_owned(),
        hz_command::ListColumn::Base => row.base.clone().unwrap_or_else(|| "-".to_owned()),
        hz_command::ListColumn::Modified => format_modified_at(row.modified_at_unix),
        hz_command::ListColumn::Path => display_path(&row.path),
    }
}

fn list_column_header(column: hz_command::ListColumn) -> &'static str {
    match column {
        hz_command::ListColumn::Marker => "",
        hz_command::ListColumn::Target => "target",
        hz_command::ListColumn::Branch => "branch",
        hz_command::ListColumn::Handle => "handle",
        hz_command::ListColumn::Status => "st",
        hz_command::ListColumn::Base => "base",
        hz_command::ListColumn::Modified => "modified",
        hz_command::ListColumn::Path => "path",
    }
}

fn list_column_min_width(column: hz_command::ListColumn) -> usize {
    match column {
        hz_command::ListColumn::Marker => 1,
        hz_command::ListColumn::Status => 2,
        hz_command::ListColumn::Base => 4,
        hz_command::ListColumn::Modified => 1,
        hz_command::ListColumn::Path => 4,
        hz_command::ListColumn::Target
        | hz_command::ListColumn::Branch
        | hz_command::ListColumn::Handle => 6,
    }
}

fn shrink_list_columns(
    columns: &[hz_command::ListColumn],
    widths: &mut [usize],
    terminal_width: Option<usize>,
) {
    let Some(terminal_width) = terminal_width else {
        return;
    };
    if columns.is_empty() {
        return;
    }

    while list_row_width(widths) > terminal_width {
        let Some(index) = widths
            .iter()
            .enumerate()
            .filter(|(index, width)| **width > list_column_min_width(columns[*index]))
            .max_by_key(|(_, width)| **width)
            .map(|(index, _)| index)
        else {
            break;
        };
        widths[index] -= 1;
    }
}

fn list_row_width(widths: &[usize]) -> usize {
    widths.iter().sum::<usize>() + widths.len().saturating_sub(1)
}

fn styled_list_cell(
    value: &str,
    width: usize,
    column: hz_command::ListColumn,
    row: &WorktreeListRow,
    color: bool,
    glyphs: ListGlyphs,
    colors: ListColors,
) -> String {
    let value = truncate_middle(value, width, glyphs);
    match column {
        hz_command::ListColumn::Marker => styled(
            &plain_cell(&value, width),
            worktree_marker_color(row, colors),
            color,
        ),
        hz_command::ListColumn::Status => styled_cell(
            &value,
            width,
            worktree_status_color(row.status, colors),
            color,
        ),
        hz_command::ListColumn::Target => styled_cell(&value, width, colors.target, color),
        hz_command::ListColumn::Branch => styled_cell(&value, width, colors.branch, color),
        hz_command::ListColumn::Handle => styled_cell(&value, width, colors.handle, color),
        hz_command::ListColumn::Base => styled_cell(&value, width, colors.base, color),
        hz_command::ListColumn::Modified => styled_cell(&value, width, colors.modified, color),
        hz_command::ListColumn::Path => styled_cell(&value, width, colors.path, color),
    }
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

fn worktree_marker_color(row: &WorktreeListRow, colors: ListColors) -> StyleColor {
    if row.current {
        colors.current
    } else {
        colors.local
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

fn worktree_status_color(status: hz_command::WorktreeStatus, colors: ListColors) -> StyleColor {
    match status {
        hz_command::WorktreeStatus::Clean => colors.clean,
        hz_command::WorktreeStatus::Dirty => colors.dirty,
        hz_command::WorktreeStatus::Unknown => colors.unknown,
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
    for warning in &created.warnings {
        output.push_str(&render_field("warning", warning, StyleColor::Yellow, color));
    }

    output
}

fn print_warnings(warnings: &[String], color: bool) {
    for warning in warnings {
        eprintln!(
            "{} {warning}",
            styled("warning:", StyleColor::Yellow, color)
        );
    }
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
    for warning in &handoff.warnings {
        output.push_str(&render_field("warning", warning, StyleColor::Yellow, color));
    }

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

fn render_repo_init(init: &hz_command::RepoInit, color: bool) -> String {
    let changed = init.config_created || init.setup_created || init.cleanup_created;
    let (marker, status, status_color) = if changed {
        ("+", "initialized", StyleColor::Green)
    } else {
        ("=", "exists", StyleColor::Yellow)
    };

    let mut output = format!(
        "{} {}  repo\n",
        styled(marker, status_color, color),
        styled(status, status_color, color)
    );
    output.push_str(&render_field(
        "repo",
        &init.repo.display().to_string(),
        StyleColor::White,
        color,
    ));
    output.push_str(&render_created_field(
        "config",
        &init.config_path,
        init.config_created,
        color,
    ));
    output.push_str(&render_created_field(
        "setup",
        &init.setup_path,
        init.setup_created,
        color,
    ));
    output.push_str(&render_created_field(
        "cleanup",
        &init.cleanup_path,
        init.cleanup_created,
        color,
    ));

    output
}

fn render_created_field(label: &str, path: &Path, created: bool, color: bool) -> String {
    let state = if created { "created" } else { "exists" };
    render_field(
        label,
        &format!("{} ({state})", path.display()),
        StyleColor::White,
        color,
    )
}

fn render_lifecycle_run(run: &hz_command::LifecycleRun, color: bool) -> String {
    let label = lifecycle_kind_label(run.kind);
    let (marker, status, status_color) = if run.configured {
        ("+", label, StyleColor::Green)
    } else {
        ("=", "no-op", StyleColor::Yellow)
    };

    let mut output = format!(
        "{} {}  {}\n",
        styled(marker, status_color, color),
        styled(status, status_color, color),
        styled(&run.target, StyleColor::White, color)
    );
    output.push_str(&render_field(
        "path",
        &run.path.display().to_string(),
        StyleColor::White,
        color,
    ));

    output
}

fn lifecycle_kind_label(kind: hz_command::LifecycleKind) -> &'static str {
    match kind {
        hz_command::LifecycleKind::Setup => "setup",
        hz_command::LifecycleKind::Cleanup => "cleanup",
    }
}

fn render_field(label: &str, value: &str, value_color: StyleColor, color: bool) -> String {
    format!(
        "  {}  {}\n",
        styled_cell(label, 6, StyleColor::Cyan, color),
        styled(value, value_color, color)
    )
}

fn display_width(value: &str) -> usize {
    UnicodeWidthStr::width(value)
}

#[derive(Debug, Clone, Copy)]
enum StyleColor {
    Black,
    Green,
    Blue,
    Cyan,
    Magenta,
    Red,
    Yellow,
    White,
}

fn styled_cell(value: &str, width: usize, color: StyleColor, enabled: bool) -> String {
    styled(&plain_cell(value, width), color, enabled)
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
    let prefix = take_display_width(value, prefix_width);
    let suffix = take_display_width_from_end(value, suffix_width);

    format!("{prefix}{}{suffix}", glyphs.ellipsis)
}

fn take_display_width(value: &str, width: usize) -> String {
    let mut output = String::new();
    let mut used_width = 0;
    for character in value.chars() {
        let character_width = character.width().unwrap_or(0);
        if used_width + character_width > width {
            break;
        }
        used_width += character_width;
        output.push(character);
    }
    output
}

fn take_display_width_from_end(value: &str, width: usize) -> String {
    let mut output = Vec::new();
    let mut used_width = 0;
    for character in value.chars().rev() {
        let character_width = character.width().unwrap_or(0);
        if used_width + character_width > width {
            break;
        }
        used_width += character_width;
        output.push(character);
    }
    output.into_iter().rev().collect()
}

fn styled(value: &str, color: StyleColor, enabled: bool) -> String {
    if !enabled {
        return value.to_owned();
    }

    let code = match color {
        StyleColor::Black => "30",
        StyleColor::Green => "32",
        StyleColor::Blue => "34",
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
    let requested_target_count = args.targets.len();
    let candidates = find_removal_candidates(&args)?;
    let mut removable = Vec::new();
    let mut removed = Vec::new();

    for candidate in candidates {
        if candidate.confirm_unmanaged && !confirm_unmanaged_removal(&candidate.worktree)? {
            eprintln!("not removed");
            continue;
        }

        removable.push(candidate.worktree);
    }

    if !args.no_cleanup {
        for candidate in &removable {
            if should_run_cleanup_for_removal(candidate) {
                hz_command::run_lifecycle_for_entry(candidate, hz_command::LifecycleKind::Cleanup)?;
            }
        }
    }

    let mut removal_errors = Vec::new();
    for candidate in removable {
        let target = worktree_branch_or_handle(&candidate).to_owned();
        match hz_command::remove_found_worktree_with_force(candidate, force) {
            Ok(entry) => removed.push(entry),
            Err(error) => removal_errors.push(format!("{target}: {error}")),
        }
    }

    if args.json {
        println!(
            "{}",
            removed_worktrees_json(requested_target_count, &removed)?
        );
    } else if debug {
        for entry in &removed {
            print!(
                "{}",
                render_removed_worktree(entry, io::stdout().is_terminal())
            );
        }
    }

    if !removal_errors.is_empty() {
        return Err(hz_core::HzError::Usage(format!(
            "failed to remove one or more worktrees: {}",
            removal_errors.join("; ")
        )));
    }

    Ok(())
}

#[derive(Debug)]
struct RemovalCandidate {
    worktree: hz_command::WorktreeEntry,
    confirm_unmanaged: bool,
}

fn find_removal_candidates(args: &RemoveWorktreeArgs) -> HzResult<Vec<RemovalCandidate>> {
    let mut candidates = Vec::with_capacity(args.targets.len());
    let mut seen = HashSet::new();

    for target in &args.targets {
        let candidate = hz_command::find_worktree(hz_command::FindWorktree {
            target: target.clone(),
            repo: args.repo.clone(),
        })?;

        if !seen.insert((candidate.repo.clone(), candidate.path.clone())) {
            return Err(hz_core::HzError::Usage(format!(
                "duplicate worktree target: {target}"
            )));
        }

        let confirm_unmanaged = should_confirm_unmanaged_removal(args, &candidate)?;
        candidates.push(RemovalCandidate {
            worktree: candidate,
            confirm_unmanaged,
        });
    }

    Ok(candidates)
}

fn removed_worktrees_json(
    requested_target_count: usize,
    removed: &[hz_command::WorktreeEntry],
) -> HzResult<String> {
    if requested_target_count == 1
        && let [entry] = removed
    {
        return Ok(serde_json::to_string_pretty(entry)?);
    }

    Ok(serde_json::to_string_pretty(removed)?)
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

fn should_run_cleanup_for_removal(worktree: &hz_command::WorktreeEntry) -> bool {
    worktree.source == hz_command::WorktreeSource::Managed
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
        create: args.create,
        max_detached_worktrees: args.max_detached,
    })?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&handoff)?);
    } else if args.path_only {
        println!("{}", handoff.to.path.display());
        print_warnings(&handoff.warnings, io::stderr().is_terminal());
    } else {
        print!("{}", render_handoff(&handoff, io::stdout().is_terminal()));
    }

    Ok(())
}

fn init_repo_or_shell(args: InitArgs) -> HzResult<()> {
    if let Some(shell) = args.shell {
        if args.repo.is_some() {
            return Err(hz_core::HzError::Usage(
                "hz init <shell> does not accept --repo; use hz install <shell>".to_owned(),
            ));
        }
        return install_shell(ShellArgs { shell });
    }

    let init = hz_command::init_repo(hz_command::InitRepo { repo: args.repo })?;
    print!("{}", render_repo_init(&init, io::stdout().is_terminal()));

    Ok(())
}

fn install_shell(args: ShellArgs) -> HzResult<()> {
    let shell = shell_to_command(args.shell);

    let init = hz_command::install_shell_integration(shell)?;
    print!(
        "{}",
        render_shell_init(shell_name(args.shell), &init, io::stdout().is_terminal())
    );

    Ok(())
}

fn shell_script(args: ShellArgs) -> HzResult<()> {
    let shell = shell_to_command(args.shell);

    print!("{}", hz_command::shell_integration(shell));
    Ok(())
}

fn update(args: UpdateArgs) -> HzResult<()> {
    let argv0 = env::args_os().next().ok_or_else(|| {
        hz_core::HzError::Usage("could not determine current executable".to_owned())
    })?;
    let binary = update_binary_name(&argv0)?;
    let install_dir = match args.install_dir {
        Some(path) => absolute_path(path)?,
        None => default_update_install_dir(&argv0)?,
    };
    let version = args.version.unwrap_or_else(|| "latest".to_owned());
    let repo = update_repo(env::var_os("HZ_REPO"));

    let mut child = ProcessCommand::new("sh")
        .arg("-s")
        .env("HZ_REPO", repo)
        .env("HZ_INSTALL_DIR", install_dir)
        .env("HZ_VERSION", version)
        .env("HZ_BINARY", binary)
        .env("HZ_INSTALL_ACTION", "update")
        .stdin(Stdio::piped())
        .spawn()?;

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| hz_core::HzError::Usage("could not open installer stdin".to_owned()))?;
    stdin.write_all(INSTALL_SCRIPT.as_bytes())?;
    drop(stdin);

    let status = child.wait()?;
    if !status.success() {
        return Err(hz_core::HzError::Usage(format!(
            "update failed with status {}",
            status
                .code()
                .map_or_else(|| "unknown".to_owned(), |code| code.to_string())
        )));
    }

    Ok(())
}

fn update_repo(repo: Option<OsString>) -> OsString {
    repo.filter(|repo| !repo.as_os_str().is_empty())
        .unwrap_or_else(|| OsString::from(RELEASE_REPO))
}

fn update_binary_name(argv0: &OsStr) -> HzResult<OsString> {
    Path::new(argv0)
        .file_name()
        .filter(|name| !name.is_empty())
        .map(OsString::from)
        .ok_or_else(|| {
            hz_core::HzError::Usage("could not determine current executable name".to_owned())
        })
}

fn default_update_install_dir(argv0: &OsStr) -> HzResult<PathBuf> {
    let argv0_path = Path::new(argv0);
    if argv0_path.components().count() > 1 {
        return invocation_parent_dir(argv0_path);
    }

    let binary = update_binary_name(argv0)?;
    if let Some(path) = env::var_os("PATH") {
        for dir in env::split_paths(&path) {
            if dir.join(Path::new(&binary)).is_file() {
                return absolute_path(dir);
            }
        }
    }

    current_exe_parent_dir()
}

fn invocation_parent_dir(path: &Path) -> HzResult<PathBuf> {
    let parent = path.parent().ok_or_else(|| {
        hz_core::HzError::Usage("could not determine current executable directory".to_owned())
    })?;
    absolute_path(parent.to_path_buf())
}

fn current_exe_parent_dir() -> HzResult<PathBuf> {
    let executable = env::current_exe()?;
    let parent = executable.parent().ok_or_else(|| {
        hz_core::HzError::Usage("could not determine current executable directory".to_owned())
    })?;
    absolute_path(parent.to_path_buf())
}

fn absolute_path(path: PathBuf) -> HzResult<PathBuf> {
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(env::current_dir()?.join(path))
    }
}

fn run_lifecycle(args: LifecycleArgs, kind: hz_command::LifecycleKind) -> HzResult<()> {
    let run = hz_command::run_lifecycle(hz_command::RunLifecycle {
        target: args.target,
        repo: args.repo,
        kind,
    })?;
    print!("{}", render_lifecycle_run(&run, io::stdout().is_terminal()));
    Ok(())
}

fn shell_to_command(shell: ShellArg) -> hz_command::Shell {
    match shell {
        ShellArg::Zsh => hz_command::Shell::Zsh,
        ShellArg::Bash => hz_command::Shell::Bash,
        ShellArg::Fish => hz_command::Shell::Fish,
    }
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
    let worktrees = hz_command::list_worktree_targets(hz_command::ListWorktrees { repo })?;
    let mut candidates = Vec::new();

    if include_local {
        candidates.push("local".to_owned());
    }

    for worktree in worktrees {
        push_worktree_completion_candidate(&mut candidates, &worktree);
    }

    Ok(candidates)
}

fn push_worktree_completion_candidate(
    candidates: &mut Vec<String>,
    worktree: &hz_command::WorktreeEntry,
) {
    push_completion_candidate(
        candidates,
        Some(worktree_branch_or_handle(worktree).to_owned()),
    );
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
    use std::{collections::HashMap, path::PathBuf};

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
    fn list_output_widths_count_terminal_columns() {
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
    fn list_output_uses_configured_headers_and_columns() {
        let output = render_worktree_rows_with_options(
            &[WorktreeListRow {
                target: "feature/ui".to_owned(),
                branch: Some("feature/ui".to_owned()),
                handle: Some("f7a7".to_owned()),
                base: Some("dev".to_owned()),
                status: hz_command::WorktreeStatus::Clean,
                modified_at_unix: 0,
                path: PathBuf::from("/worktrees/entry"),
                local: false,
                current: false,
            }],
            false,
            list_glyphs(false),
            None,
            WorktreeListOptions {
                headers: hz_command::ListHeaders::Always,
                columns: vec![
                    hz_command::ListColumn::Branch,
                    hz_command::ListColumn::Base,
                    hz_command::ListColumn::Status,
                ],
                ..WorktreeListOptions::default()
            },
        );

        assert!(output.starts_with("branch"));
        assert!(output.contains("base"));
        assert!(output.contains("feature/ui"));
        assert!(output.contains("dev"));
        assert!(!output.contains("path"));
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
    fn list_output_can_render_custom_color_scheme() {
        let mut schemes = HashMap::new();
        schemes.insert(
            "blueprint".to_owned(),
            hz_command::ColorSchemeConfig {
                target: Some("blue".to_owned()),
                clean: Some("cyan".to_owned()),
                ..hz_command::ColorSchemeConfig::default()
            },
        );
        let options = WorktreeListOptions {
            colors: list_colors(Some(&hz_command::ColorConfig {
                mode: None,
                scheme: Some("blueprint".to_owned()),
                schemes,
            })),
            ..WorktreeListOptions::default()
        };
        let output = render_worktree_rows_with_options(
            &[WorktreeListRow {
                target: "feature/ui".to_owned(),
                branch: Some("feature/ui".to_owned()),
                handle: Some("f7a7".to_owned()),
                base: None,
                status: hz_command::WorktreeStatus::Clean,
                modified_at_unix: 0,
                path: PathBuf::from("/worktrees/entry"),
                local: false,
                current: false,
            }],
            true,
            list_glyphs(false),
            None,
            options,
        );

        assert!(output.contains("\x1b[34mfeature/ui"));
        assert!(output.contains("\x1b[36mok"));
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
                branch: Some("feat(worktree)/very-long-branch-name".to_owned()),
                handle: Some("handle".to_owned()),
                base: None,
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
    fn compact_rows_truncate_configured_columns_to_terminal_width() {
        let output = render_worktree_rows_with_options(
            &[WorktreeListRow {
                target: "feat(worktree)/very-long-branch-name".to_owned(),
                branch: Some("feat(worktree)/very-long-branch-name".to_owned()),
                handle: Some("handle".to_owned()),
                base: None,
                status: hz_command::WorktreeStatus::Dirty,
                modified_at_unix: 0,
                path: PathBuf::from("/very/long/worktree/path"),
                local: false,
                current: false,
            }],
            false,
            list_glyphs(true),
            Some(36),
            WorktreeListOptions {
                compact_columns: vec![
                    hz_command::ListColumn::Marker,
                    hz_command::ListColumn::Target,
                    hz_command::ListColumn::Path,
                ],
                ..WorktreeListOptions::default()
            },
        );

        assert!(output.contains("…"));
        assert!(output.lines().all(|line| display_width(line) <= 36));
    }

    #[test]
    fn display_width_uses_terminal_columns() {
        assert_eq!(display_width("測試"), 4);

        let truncated = truncate_middle("feature/測試/worktree", 12, list_glyphs(true));

        assert!(truncated.contains("…"));
        assert!(display_width(&truncated) <= 12);
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
                warnings: Vec::new(),
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
                warnings: Vec::new(),
            },
            false,
        );

        assert!(output.starts_with("+ created  generated-handle"));
        assert!(output.contains("branch  detached"));
        assert!(output.contains("path    /worktrees/entry"));
    }

    #[test]
    fn created_output_renders_prune_warnings() {
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
                warnings: vec![
                    "created worktree, but failed to prune detached worktrees: permission denied"
                        .to_owned(),
                ],
            },
            false,
        );

        assert!(output.contains(
            "warning  created worktree, but failed to prune detached worktrees: permission denied"
        ));
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
                warnings: Vec::new(),
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
    fn handoff_output_renders_prune_warnings() {
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
                    name: "generated-handle".to_owned(),
                    path: PathBuf::from("/worktrees/entry"),
                },
                changed: true,
                warnings: vec![
                    "created worktree, but failed to prune detached worktrees: permission denied"
                        .to_owned(),
                ],
            },
            false,
        );

        assert!(output.contains(
            "warning  created worktree, but failed to prune detached worktrees: permission denied"
        ));
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
        let cli = Cli::try_parse_from([
            "hz",
            "rm",
            "-r",
            "/repo",
            "-j",
            "-d",
            "-f",
            "--no-cleanup",
            "target",
        ])
        .unwrap();

        match cli.command {
            Some(Command::Remove(args)) => {
                assert_eq!(args.targets, vec!["target".to_owned()]);
                assert_eq!(args.repo, Some(PathBuf::from("/repo")));
                assert!(args.json);
                assert!(args.debug);
                assert!(args.force);
                assert!(args.no_cleanup);
            }
            command => panic!("expected remove command, got {command:?}"),
        }
    }

    #[test]
    fn remove_accepts_multiple_targets() {
        let cli = Cli::try_parse_from(["hz", "rm", "cartesian-alpha", "archimedean-beta"]).unwrap();

        match cli.command {
            Some(Command::Remove(args)) => {
                assert_eq!(
                    args.targets,
                    vec!["cartesian-alpha".to_owned(), "archimedean-beta".to_owned()]
                );
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
                assert!(!args.create);
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

        let cli = Cli::try_parse_from([
            "hz",
            "handoff",
            "--new",
            "--max-detached",
            "3",
            "feature/ui",
        ])
        .unwrap();
        match cli.command {
            Some(Command::Handoff(args)) => {
                assert_eq!(args.target.as_deref(), Some("feature/ui"));
                assert!(args.create);
                assert_eq!(args.max_detached, Some(3));
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
            "--max-detached",
            "5",
            "-j",
            "-d",
            "--no-setup",
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
                assert_eq!(args.max_detached, Some(5));
                assert!(args.json);
                assert!(args.debug);
                assert!(args.no_setup);
            }
            command => panic!("expected new command, got {command:?}"),
        }

        let cli = Cli::try_parse_from([
            "hz",
            "diff",
            "-r",
            "/repo",
            "--unstaged",
            "--no-untracked",
            "--no-watch",
            "-s",
        ])
        .unwrap();
        match cli.command {
            Some(Command::Diff(args)) => {
                assert_eq!(args.repo, Some(PathBuf::from("/repo")));
                assert!(args.unstaged);
                assert!(args.no_untracked);
                assert!(args.no_watch);
                assert!(args.stat);
            }
            command => panic!("expected diff command, got {command:?}"),
        }

        let cli = Cli::try_parse_from(["hz", "diff", "-b", "main"]).unwrap();
        match cli.command {
            Some(Command::Diff(args)) => assert_eq!(args.base.as_deref(), Some("main")),
            command => panic!("expected diff command, got {command:?}"),
        }

        let cli = Cli::try_parse_from(["hz", "diff", "main", "feature"]).unwrap();
        match cli.command {
            Some(Command::Diff(args)) => assert_eq!(args.revs, ["main", "feature"]),
            command => panic!("expected diff command, got {command:?}"),
        }

        let cli = Cli::try_parse_from(["hz", "diff", "--patch", "changes.diff", "--stat"]).unwrap();
        match cli.command {
            Some(Command::Diff(args)) => {
                assert_eq!(args.patch, Some(PathBuf::from("changes.diff")));
                assert!(args.stat);
            }
            command => panic!("expected diff command, got {command:?}"),
        }
    }

    #[test]
    fn diff_scope_flags_conflict_with_revisions_at_parse_time() {
        assert!(Cli::try_parse_from(["hz", "diff", "--staged", "main"]).is_err());
        assert!(Cli::try_parse_from(["hz", "diff", "--unstaged", "main"]).is_err());
        assert!(Cli::try_parse_from(["hz", "diff", "--staged", "--base", "main"]).is_err());
        assert!(Cli::try_parse_from(["hz", "diff", "--unstaged", "--base", "main"]).is_err());
    }

    #[test]
    fn tree_sitter_commands_accept_language_args() {
        let cli = Cli::try_parse_from(["hz", "ts", "add", "rust", "mlir", "llvm"]).unwrap();
        match cli.command {
            Some(Command::TreeSitter {
                command: TreeSitterCommand::Add(args),
            }) => assert_eq!(args.languages, ["rust", "mlir", "llvm"]),
            command => panic!("expected tree-sitter add command, got {command:?}"),
        }

        let cli = Cli::try_parse_from(["hz", "tree-sitter", "rm", "rust"]).unwrap();
        match cli.command {
            Some(Command::TreeSitter {
                command: TreeSitterCommand::Rm(args),
            }) => assert_eq!(args.languages, ["rust"]),
            command => panic!("expected tree-sitter rm command, got {command:?}"),
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
    fn init_install_and_lifecycle_commands_parse() {
        let cli = Cli::try_parse_from(["hz", "init", "-r", "/repo"]).unwrap();
        match cli.command {
            Some(Command::Init(args)) => {
                assert_eq!(args.shell, None);
                assert_eq!(args.repo, Some(PathBuf::from("/repo")));
            }
            command => panic!("expected init command, got {command:?}"),
        }

        let cli = Cli::try_parse_from(["hz", "init", "zsh"]).unwrap();
        match cli.command {
            Some(Command::Init(args)) => assert_eq!(args.shell, Some(ShellArg::Zsh)),
            command => panic!("expected init command, got {command:?}"),
        }

        let cli = Cli::try_parse_from(["hz", "install", "fish"]).unwrap();
        match cli.command {
            Some(Command::Install(args)) => assert_eq!(args.shell, ShellArg::Fish),
            command => panic!("expected install command, got {command:?}"),
        }

        let cli = Cli::try_parse_from(["hz", "setup", "-r", "/repo", "target"]).unwrap();
        match cli.command {
            Some(Command::Setup(args)) => {
                assert_eq!(args.target.as_deref(), Some("target"));
                assert_eq!(args.repo, Some(PathBuf::from("/repo")));
            }
            command => panic!("expected setup command, got {command:?}"),
        }

        let cli = Cli::try_parse_from(["hz", "cleanup"]).unwrap();
        match cli.command {
            Some(Command::Cleanup(args)) => assert_eq!(args.target, None),
            command => panic!("expected cleanup command, got {command:?}"),
        }

        let cli = Cli::try_parse_from([
            "hz",
            "update",
            "--target-version",
            "0.1.1",
            "--install-dir",
            "/tmp/hz-bin",
        ])
        .unwrap();
        match cli.command {
            Some(Command::Update(args)) => {
                assert_eq!(args.version.as_deref(), Some("0.1.1"));
                assert_eq!(args.install_dir, Some(PathBuf::from("/tmp/hz-bin")));
            }
            command => panic!("expected update command, got {command:?}"),
        }
    }

    #[test]
    fn update_target_uses_invoked_binary_name_and_directory() {
        let cwd = env::current_dir().unwrap();

        assert_eq!(
            update_binary_name(OsStr::new("./target/debug/hz-dev")).unwrap(),
            OsString::from("hz-dev")
        );
        assert!(
            default_update_install_dir(OsStr::new("./target/debug/hz-dev"))
                .unwrap()
                .ends_with(Path::new("target/debug"))
        );
        assert_eq!(
            default_update_install_dir(OsStr::new("/usr/local/bin/hz-beta")).unwrap(),
            PathBuf::from("/usr/local/bin")
        );
        assert_eq!(
            absolute_path(PathBuf::from("bin")).unwrap(),
            cwd.join("bin")
        );
    }

    #[test]
    fn update_repo_respects_env_override() {
        assert_eq!(update_repo(None), OsString::from(RELEASE_REPO));
        assert_eq!(
            update_repo(Some(OsString::from("example/hz-fork"))),
            OsString::from("example/hz-fork")
        );
        assert_eq!(
            update_repo(Some(OsString::new())),
            OsString::from(RELEASE_REPO)
        );
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
    fn worktree_completion_candidates_use_display_targets() {
        let mut branched = test_entry(hz_command::WorktreeSource::Managed);
        branched.id = "45aa44e4-9dd5-4e74-b7ae-82db4b365e78".to_owned();
        branched.handle = "45aa44e4-9dd5-4e74-b7ae-82db4b365e78".to_owned();
        branched.branch = Some("feat(tui)/diff".to_owned());

        let mut detached = test_entry(hz_command::WorktreeSource::Managed);
        detached.id = "de625fc0-3962-4680-be9c-37fca7a57aaf".to_owned();
        detached.handle = "tw61".to_owned();
        detached.branch = None;

        let mut candidates = Vec::new();
        push_worktree_completion_candidate(&mut candidates, &branched);
        push_worktree_completion_candidate(&mut candidates, &detached);

        assert_eq!(candidates, vec!["feat(tui)/diff", "tw61"]);
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
    fn cleanup_runs_for_managed_removals_only() {
        assert!(should_run_cleanup_for_removal(&test_entry(
            hz_command::WorktreeSource::Managed
        )));
        assert!(!should_run_cleanup_for_removal(&test_entry(
            hz_command::WorktreeSource::Git
        )));
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

    #[test]
    fn single_target_json_keeps_object_shape() {
        let removed = vec![test_entry(hz_command::WorktreeSource::Managed)];

        let output = removed_worktrees_json(1, &removed).unwrap();

        assert!(output.trim_start().starts_with('{'));
    }

    #[test]
    fn multi_target_json_keeps_array_shape_when_one_is_removed() {
        let removed = vec![test_entry(hz_command::WorktreeSource::Managed)];

        let output = removed_worktrees_json(2, &removed).unwrap();

        assert!(output.trim_start().starts_with('['));
    }

    #[test]
    fn single_target_json_uses_array_when_nothing_was_removed() {
        let output = removed_worktrees_json(1, &[]).unwrap();

        assert_eq!(output, "[]");
    }

    fn remove_args(json: bool, force: bool) -> RemoveWorktreeArgs {
        RemoveWorktreeArgs {
            targets: vec!["target".to_owned()],
            repo: None,
            json,
            force,
            debug: false,
            no_cleanup: false,
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
