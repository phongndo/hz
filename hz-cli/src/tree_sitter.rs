use std::{
    io::{self, IsTerminal, Read},
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::{
    CliResult,
    args::{DiffArgs, TreeSitterAvailableArgs, TreeSitterCommand},
    worktree_output::{
        ListGlyphs, StyleColor, ascii_output_requested, display_width, list_glyphs, list_row_width,
        styled_cell, styled_centered_cell, terminal_width, truncate_middle,
    },
    write_stdout,
};
use hz_core::HzResult;

pub(crate) fn tree_sitter(command: TreeSitterCommand) -> CliResult<()> {
    match command {
        TreeSitterCommand::Add(args) => {
            let result = hz_command::syntax_add(&args.languages)?;
            print_tree_sitter_add_result(&result)?;
        }
        TreeSitterCommand::Update(args) => {
            let result = hz_command::syntax_update(&args.languages, args.all)?;
            print_tree_sitter_update_result(&result)?;
        }
        TreeSitterCommand::Rm(args) => {
            let result = hz_command::syntax_remove(&args.languages)?;
            print_tree_sitter_remove_result(&result)?;
        }
        TreeSitterCommand::List => {
            print_tree_sitter_statuses(&hz_command::syntax_statuses()?, false)?;
        }
        TreeSitterCommand::Available(args) => {
            for language in
                hz_command::syntax_available_languages(tree_sitter_available_filter(&args))?
            {
                write_stdout(format_args!("{language}\n"))?;
            }
        }
        TreeSitterCommand::Clean => {
            let result = hz_command::syntax_clean_cache()?;
            write_stdout(format_args!(
                "removed {} parser artifacts and {} checksum records\n",
                result.parser_artifacts_removed, result.artifact_records_removed
            ))?;
            write_stdout(format_args!(
                "kept {} enabled-language config entries\n",
                result.enabled_languages_kept
            ))?;
        }
        TreeSitterCommand::Path => {
            write_stdout(format_args!(
                "cache       {}\n",
                hz_command::syntax_cache_dir()?
            ))?;
            write_stdout(format_args!(
                "registry    {}\n",
                hz_command::syntax_config_path()?.display()
            ))?;
            write_stdout(format_args!(
                "config      {}\n",
                hz_command::syntax_settings_path()?.display()
            ))?;
            write_stdout(format_args!(
                "colorscheme {}\n",
                hz_command::syntax_colorscheme_dir()?.display()
            ))?;
        }
        TreeSitterCommand::Doctor => {
            let report = hz_command::syntax_doctor()?;
            print_tree_sitter_statuses(&report.statuses, true)?;
            if report.issues.is_empty() {
                write_stdout(format_args!("ok\n"))?;
            } else {
                for issue in report.issues {
                    write_stdout(format_args!(
                        "warning {}: {}\n",
                        issue.language, issue.message
                    ))?;
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn tree_sitter_available_filter(
    args: &TreeSitterAvailableArgs,
) -> hz_command::SyntaxAvailableFilter {
    if args.installed {
        hz_command::SyntaxAvailableFilter::Installed
    } else if args.enabled {
        hz_command::SyntaxAvailableFilter::Enabled
    } else {
        hz_command::SyntaxAvailableFilter::All
    }
}

pub(crate) fn diff_options(mut args: DiffArgs) -> HzResult<hz_command::DiffOptions> {
    if let Some(target) = args.pr.take() {
        return pr_diff_options(args, &target);
    }

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

pub(crate) fn pr_diff_options(args: DiffArgs, target: &str) -> HzResult<hz_command::DiffOptions> {
    if args.base.is_some() || !args.revs.is_empty() {
        return Err(hz_core::HzError::Usage(
            "use --pr without revisions or --base".to_owned(),
        ));
    }
    if args.staged || args.unstaged || args.no_untracked {
        return Err(hz_core::HzError::Usage(
            "--staged, --unstaged, and --no-untracked do not apply to hz diff --pr".to_owned(),
        ));
    }
    if args.patch.is_some() {
        return Err(hz_core::HzError::Usage(
            "--patch does not apply to hz diff --pr".to_owned(),
        ));
    }

    hz_command::github_pr_diff_options(args.repo, target, args.stat)
}

pub(crate) fn patch_source(path: PathBuf) -> HzResult<hz_command::DiffSource> {
    if path == Path::new("-") {
        let mut patch = Vec::new();
        io::stdin().read_to_end(&mut patch)?;
        return Ok(hz_command::DiffSource::Patch(
            hz_command::PatchSource::Stdin(Arc::from(patch.into_boxed_slice())),
        ));
    }

    Ok(hz_command::DiffSource::Patch(
        hz_command::PatchSource::File(path),
    ))
}

pub(crate) fn print_tree_sitter_add_result(result: &hz_command::SyntaxAddResult) -> CliResult<()> {
    for language in &result.added {
        write_stdout(format_args!("+ enabled {language}\n"))?;
    }
    for language in &result.already_enabled {
        write_stdout(format_args!("= enabled {language}\n"))?;
    }
    for language in &result.without_highlights {
        write_stdout(format_args!(
            "warning {language}: no bundled highlights query; diff will render plain text\n"
        ))?;
    }
    Ok(())
}

pub(crate) fn print_tree_sitter_update_result(
    result: &hz_command::SyntaxUpdateResult,
) -> CliResult<()> {
    if result.updated.is_empty()
        && result.bundled.is_empty()
        && result.not_installed.is_empty()
        && result.unavailable.is_empty()
    {
        write_stdout(format_args!("no parser caches to update\n"))?;
    }
    for language in &result.updated {
        write_stdout(format_args!("~ updated parser cache {language}\n"))?;
    }
    for language in &result.bundled {
        write_stdout(format_args!("= bundled parser {language}\n"))?;
    }
    for language in &result.not_installed {
        write_stdout(format_args!("= not installed {language}\n"))?;
    }
    for language in &result.unavailable {
        write_stdout(format_args!("warning {language}: language is not known\n"))?;
    }
    for language in &result.without_highlights {
        write_stdout(format_args!(
            "warning {language}: no bundled highlights query; diff will render plain text\n"
        ))?;
    }
    Ok(())
}

pub(crate) fn print_tree_sitter_remove_result(
    result: &hz_command::SyntaxRemoveResult,
) -> CliResult<()> {
    for language in &result.removed {
        write_stdout(format_args!("- disabled {language} in config\n"))?;
    }
    for language in &result.missing {
        write_stdout(format_args!("= not enabled in config {language}\n"))?;
    }
    for language in &result.cache_deleted {
        write_stdout(format_args!("- deleted parser cache {language}\n"))?;
    }
    for language in &result.cache_missing {
        write_stdout(format_args!("= no parser cache {language}\n"))?;
    }
    Ok(())
}

pub(crate) fn print_tree_sitter_statuses(
    statuses: &[hz_command::SyntaxLanguageStatus],
    detail: bool,
) -> CliResult<()> {
    if statuses.is_empty() {
        write_stdout(format_args!("no tree-sitter languages enabled\n"))?;
        return Ok(());
    }

    let terminal = io::stdout().is_terminal();
    let glyphs = list_glyphs(terminal && !ascii_output_requested());
    write_stdout(format_args!(
        "{}",
        render_tree_sitter_statuses(
            statuses,
            terminal,
            glyphs,
            terminal.then(terminal_width).flatten(),
        )
    ))?;

    if !detail {
        return Ok(());
    }

    for status in statuses {
        if let Some(artifact) = &status.artifact {
            write_stdout(format_args!(
                "  {} parser={} sha256={} source={} installed_at={}\n",
                status.language,
                artifact.path.display(),
                short_sha(&artifact.sha256),
                artifact.source,
                artifact.installed_at_unix
            ))?;
        }
    }
    Ok(())
}

pub(crate) fn render_tree_sitter_statuses(
    statuses: &[hz_command::SyntaxLanguageStatus],
    color: bool,
    glyphs: ListGlyphs,
    terminal_width: Option<usize>,
) -> String {
    let headers = ["language", "status", "source", "version"];
    let rows = statuses
        .iter()
        .map(|status| {
            [
                status.language.clone(),
                syntax_status_label(status, glyphs).to_owned(),
                syntax_source_label(status).to_owned(),
                syntax_version_label(status).to_owned(),
            ]
        })
        .collect::<Vec<_>>();
    let min_widths = [6, 4, 3, 1];
    let mut widths = headers
        .iter()
        .enumerate()
        .map(|(index, header)| {
            rows.iter()
                .map(|row| display_width(&row[index]))
                .chain([display_width(header), min_widths[index]])
                .max()
                .expect("width candidates should not be empty")
        })
        .collect::<Vec<_>>();

    shrink_tree_sitter_columns(&mut widths, min_widths, terminal_width);

    let mut output = String::new();
    for (index, header) in headers.iter().enumerate() {
        if index > 0 {
            output.push(' ');
        }
        output.push_str(&styled_cell(header, widths[index], StyleColor::Cyan, color));
    }
    output.push('\n');

    for (status, row) in statuses.iter().zip(rows) {
        for (index, value) in row.iter().enumerate() {
            if index > 0 {
                output.push(' ');
            }
            let value = truncate_middle(value, widths[index], glyphs);
            let color_for_cell = match index {
                0 => StyleColor::Magenta,
                1 => syntax_status_color(status),
                _ => StyleColor::White,
            };
            if index == 1 {
                output.push_str(&styled_centered_cell(
                    &value,
                    widths[index],
                    color_for_cell,
                    color,
                ));
            } else {
                output.push_str(&styled_cell(&value, widths[index], color_for_cell, color));
            }
        }
        output.push('\n');
    }

    output
}

pub(crate) fn shrink_tree_sitter_columns(
    widths: &mut [usize],
    min_widths: [usize; 4],
    terminal_width: Option<usize>,
) {
    let Some(terminal_width) = terminal_width else {
        return;
    };
    while list_row_width(widths) > terminal_width {
        let Some(index) = widths
            .iter()
            .enumerate()
            .filter(|(index, width)| **width > min_widths[*index])
            .max_by_key(|(_, width)| **width)
            .map(|(index, _)| index)
        else {
            break;
        };
        widths[index] -= 1;
    }
}

pub(crate) fn syntax_status_label(
    status: &hz_command::SyntaxLanguageStatus,
    glyphs: ListGlyphs,
) -> &'static str {
    match syntax_status_kind(status) {
        SyntaxStatusKind::Ready => glyphs.clean,
        SyntaxStatusKind::Warning => glyphs.dirty,
        SyntaxStatusKind::Error => glyphs.unknown,
        SyntaxStatusKind::Disabled => "-",
    }
}

pub(crate) fn syntax_status_color(status: &hz_command::SyntaxLanguageStatus) -> StyleColor {
    match syntax_status_kind(status) {
        SyntaxStatusKind::Ready => StyleColor::Green,
        SyntaxStatusKind::Warning => StyleColor::Yellow,
        SyntaxStatusKind::Error => StyleColor::Red,
        SyntaxStatusKind::Disabled => StyleColor::White,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SyntaxStatusKind {
    Ready,
    Warning,
    Error,
    Disabled,
}

pub(crate) fn syntax_status_kind(status: &hz_command::SyntaxLanguageStatus) -> SyntaxStatusKind {
    if !status.enabled {
        SyntaxStatusKind::Disabled
    } else if !status.installed || !status.trusted {
        SyntaxStatusKind::Error
    } else if !status.has_highlights {
        SyntaxStatusKind::Warning
    } else {
        SyntaxStatusKind::Ready
    }
}

pub(crate) fn syntax_source_label(status: &hz_command::SyntaxLanguageStatus) -> &'static str {
    if status.source.as_deref() == Some("bundled") {
        "bundled"
    } else if status.artifact.is_some() {
        "cache"
    } else {
        "-"
    }
}

pub(crate) fn syntax_version_label(status: &hz_command::SyntaxLanguageStatus) -> &str {
    status.version.as_deref().unwrap_or("-")
}

pub(crate) fn short_sha(sha: &str) -> &str {
    sha.get(..12).unwrap_or(sha)
}
