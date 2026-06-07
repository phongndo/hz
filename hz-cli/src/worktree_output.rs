#![allow(unused_imports)]

use crate::*;
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

pub(crate) fn create_worktree(args: NewWorktreeArgs) -> HzResult<()> {
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

pub(crate) fn path_worktree(args: PathWorktreeArgs) -> HzResult<()> {
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

pub(crate) fn list_worktrees(args: ListWorktreeArgs) -> HzResult<()> {
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
pub(crate) fn render_worktree_list(worktrees: &[hz_command::WorktreeEntry]) -> String {
    render_worktree_list_with_style(worktrees, false)
}

#[cfg(test)]
pub(crate) fn render_worktree_list_with_style(
    worktrees: &[hz_command::WorktreeEntry],
    color: bool,
) -> String {
    render_worktree_rows(
        &worktree_rows(None, worktrees, None),
        color,
        list_glyphs(color),
        None,
    )
}

#[cfg(test)]
pub(crate) fn render_worktree_list_with_context(
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

pub(crate) fn render_worktree_list_with_options(
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
pub(crate) struct WorktreeListOptions {
    pub(crate) headers: hz_command::ListHeaders,
    pub(crate) columns: Vec<hz_command::ListColumn>,
    pub(crate) compact_columns: Vec<hz_command::ListColumn>,
    pub(crate) colors: ListColors,
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

pub(crate) fn list_options(
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

pub(crate) fn non_empty_columns(
    columns: Option<&[hz_command::ListColumn]>,
) -> Option<&[hz_command::ListColumn]> {
    columns.filter(|columns| !columns.is_empty())
}

pub(crate) fn default_list_columns() -> Vec<hz_command::ListColumn> {
    vec![
        hz_command::ListColumn::Marker,
        hz_command::ListColumn::Target,
        hz_command::ListColumn::Status,
        hz_command::ListColumn::Modified,
        hz_command::ListColumn::Path,
    ]
}

pub(crate) fn color_output_enabled(
    config: Option<&hz_command::ColorConfig>,
    terminal: bool,
) -> bool {
    match config.and_then(|config| config.mode) {
        Some(hz_command::ColorMode::Always) => true,
        Some(hz_command::ColorMode::Never) => false,
        Some(hz_command::ColorMode::Auto) | None => terminal,
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ListColors {
    pub(crate) header: StyleColor,
    pub(crate) target: StyleColor,
    pub(crate) branch: StyleColor,
    pub(crate) handle: StyleColor,
    pub(crate) base: StyleColor,
    pub(crate) modified: StyleColor,
    pub(crate) path: StyleColor,
    pub(crate) clean: StyleColor,
    pub(crate) dirty: StyleColor,
    pub(crate) unknown: StyleColor,
    pub(crate) current: StyleColor,
    pub(crate) local: StyleColor,
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

pub(crate) fn list_colors(config: Option<&hz_command::ColorConfig>) -> ListColors {
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

pub(crate) fn parse_style_color(value: Option<&str>) -> Option<StyleColor> {
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
pub(crate) struct WorktreeListRow {
    pub(crate) target: String,
    pub(crate) branch: Option<String>,
    pub(crate) handle: Option<String>,
    pub(crate) base: Option<String>,
    pub(crate) status: hz_command::WorktreeStatus,
    pub(crate) modified_at_unix: u64,
    pub(crate) path: PathBuf,
    pub(crate) local: bool,
    pub(crate) current: bool,
}

pub(crate) fn worktree_rows(
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
pub(crate) fn render_worktree_rows(
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

pub(crate) fn render_worktree_rows_with_options(
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

pub(crate) fn list_cell_value(
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

pub(crate) fn list_column_header(column: hz_command::ListColumn) -> &'static str {
    match column {
        hz_command::ListColumn::Marker => "",
        hz_command::ListColumn::Target => "target",
        hz_command::ListColumn::Branch => "branch",
        hz_command::ListColumn::Handle => "handle",
        hz_command::ListColumn::Status => "status",
        hz_command::ListColumn::Base => "base",
        hz_command::ListColumn::Modified => "modified",
        hz_command::ListColumn::Path => "path",
    }
}

pub(crate) fn list_column_min_width(column: hz_command::ListColumn) -> usize {
    match column {
        hz_command::ListColumn::Marker => 1,
        hz_command::ListColumn::Status => 4,
        hz_command::ListColumn::Base => 4,
        hz_command::ListColumn::Modified => 1,
        hz_command::ListColumn::Path => 4,
        hz_command::ListColumn::Target
        | hz_command::ListColumn::Branch
        | hz_command::ListColumn::Handle => 6,
    }
}

pub(crate) fn shrink_list_columns(
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

pub(crate) fn list_row_width(widths: &[usize]) -> usize {
    widths.iter().sum::<usize>() + widths.len().saturating_sub(1)
}

pub(crate) fn styled_list_cell(
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
        hz_command::ListColumn::Status => styled_centered_cell(
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
pub(crate) struct ListGlyphs {
    pub(crate) current: &'static str,
    pub(crate) local: &'static str,
    pub(crate) clean: &'static str,
    pub(crate) dirty: &'static str,
    pub(crate) unknown: &'static str,
    pub(crate) ellipsis: &'static str,
}

pub(crate) fn list_glyphs(unicode: bool) -> ListGlyphs {
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

pub(crate) fn ascii_output_requested() -> bool {
    env::var_os("HZ_ASCII").is_some()
}

pub(crate) fn terminal_width() -> Option<usize> {
    crossterm_terminal::size()
        .ok()
        .map(|(columns, _)| usize::from(columns))
        .filter(|columns| *columns > 0)
}

pub(crate) fn worktree_marker(row: &WorktreeListRow, glyphs: ListGlyphs) -> &'static str {
    if row.current {
        glyphs.current
    } else if row.local {
        glyphs.local
    } else {
        " "
    }
}

pub(crate) fn worktree_marker_color(row: &WorktreeListRow, colors: ListColors) -> StyleColor {
    if row.current {
        colors.current
    } else {
        colors.local
    }
}

pub(crate) fn same_path(left: &Path, right: &Path) -> bool {
    left == right
        || fs::canonicalize(left)
            .ok()
            .zip(fs::canonicalize(right).ok())
            .is_some_and(|(left, right)| left == right)
}

pub(crate) fn display_path(path: &Path) -> String {
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

pub(crate) fn home_relative_path(path: &Path, home: &Path) -> Option<String> {
    if home.as_os_str().is_empty() {
        return None;
    }

    if path == home {
        return Some("~".to_owned());
    }

    let relative = path.strip_prefix(home).ok()?;
    Some(format!("~/{}", relative.display()))
}

pub(crate) fn worktree_status_label(
    status: hz_command::WorktreeStatus,
    glyphs: ListGlyphs,
) -> &'static str {
    match status {
        hz_command::WorktreeStatus::Clean => glyphs.clean,
        hz_command::WorktreeStatus::Dirty => glyphs.dirty,
        hz_command::WorktreeStatus::Unknown => glyphs.unknown,
    }
}

pub(crate) fn worktree_status_color(
    status: hz_command::WorktreeStatus,
    colors: ListColors,
) -> StyleColor {
    match status {
        hz_command::WorktreeStatus::Clean => colors.clean,
        hz_command::WorktreeStatus::Dirty => colors.dirty,
        hz_command::WorktreeStatus::Unknown => colors.unknown,
    }
}

pub(crate) fn worktree_display_timestamp(worktree: &hz_command::WorktreeEntry) -> u64 {
    if worktree.modified_at_unix == 0 {
        worktree.created_at_unix
    } else {
        worktree.modified_at_unix
    }
}

pub(crate) fn format_modified_at(timestamp: u64) -> String {
    if timestamp == 0 {
        return "-".to_owned();
    }

    format_unix_timestamp(timestamp).unwrap_or_else(|| timestamp.to_string())
}

pub(crate) fn format_unix_timestamp(timestamp: u64) -> Option<String> {
    let timestamp = timestamp.to_string();
    let gnu_timestamp = format!("@{timestamp}");
    run_date_command(["-r", timestamp.as_str(), "+%b %e %H:%M"])
        .or_else(|| run_date_command(["-d", gnu_timestamp.as_str(), "+%b %e %H:%M"]))
}

pub(crate) fn run_date_command<const N: usize>(args: [&str; N]) -> Option<String> {
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
pub(crate) fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

pub(crate) fn render_created_worktree(
    created: &hz_command::CreatedWorktree,
    color: bool,
) -> String {
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

pub(crate) fn print_warnings(warnings: &[String], color: bool) {
    for warning in warnings {
        eprintln!(
            "{} {warning}",
            styled("warning:", StyleColor::Yellow, color)
        );
    }
}

pub(crate) fn render_removed_worktree(worktree: &hz_command::WorktreeEntry, color: bool) -> String {
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

pub(crate) fn render_handoff(handoff: &hz_command::WorktreeHandoff, color: bool) -> String {
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

pub(crate) fn handoff_mode_label(mode: hz_command::HandoffMode) -> &'static str {
    match mode {
        hz_command::HandoffMode::Patch => "patch",
        hz_command::HandoffMode::Branch => "branch",
    }
}

pub(crate) fn render_handoff_endpoint(
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

pub(crate) fn render_shell_init(shell: &str, init: &hz_command::ShellInit, color: bool) -> String {
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

pub(crate) fn render_repo_init(init: &hz_command::RepoInit, color: bool) -> String {
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

pub(crate) fn render_created_field(label: &str, path: &Path, created: bool, color: bool) -> String {
    let state = if created { "created" } else { "exists" };
    render_field(
        label,
        &format!("{} ({state})", path.display()),
        StyleColor::White,
        color,
    )
}

pub(crate) fn render_lifecycle_run(run: &hz_command::LifecycleRun, color: bool) -> String {
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

pub(crate) fn lifecycle_kind_label(kind: hz_command::LifecycleKind) -> &'static str {
    match kind {
        hz_command::LifecycleKind::Setup => "setup",
        hz_command::LifecycleKind::Cleanup => "cleanup",
    }
}

pub(crate) fn render_field(
    label: &str,
    value: &str,
    value_color: StyleColor,
    color: bool,
) -> String {
    format!(
        "  {}  {}\n",
        styled_cell(label, 6, StyleColor::Cyan, color),
        styled(value, value_color, color)
    )
}

pub(crate) fn display_width(value: &str) -> usize {
    UnicodeWidthStr::width(value)
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum StyleColor {
    Black,
    Green,
    Blue,
    Cyan,
    Magenta,
    Red,
    Yellow,
    White,
}

pub(crate) fn styled_cell(value: &str, width: usize, color: StyleColor, enabled: bool) -> String {
    styled(&plain_cell(value, width), color, enabled)
}

pub(crate) fn styled_centered_cell(
    value: &str,
    width: usize,
    color: StyleColor,
    enabled: bool,
) -> String {
    styled(&plain_centered_cell(value, width), color, enabled)
}

pub(crate) fn plain_cell(value: &str, width: usize) -> String {
    format!(
        "{value}{}",
        " ".repeat(width.saturating_sub(display_width(value)))
    )
}

pub(crate) fn plain_centered_cell(value: &str, width: usize) -> String {
    let padding = width.saturating_sub(display_width(value));
    let left = padding / 2;
    let right = padding - left;
    format!("{}{}{}", " ".repeat(left), value, " ".repeat(right))
}

pub(crate) fn truncate_middle(value: &str, width: usize, glyphs: ListGlyphs) -> String {
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

pub(crate) fn take_display_width(value: &str, width: usize) -> String {
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

pub(crate) fn take_display_width_from_end(value: &str, width: usize) -> String {
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

pub(crate) fn styled(value: &str, color: StyleColor, enabled: bool) -> String {
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
