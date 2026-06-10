use crate::render::{
    diff::{
        SplitCellRender, SplitLineRender, SplitSide, content_spans_at_scroll, context_hide_line,
        context_show_line, empty_diff_fill_from, inline_bg, render_row, render_row_with_focus,
        render_split_line_with_focus, render_unified_line_at_scroll, row_bg,
        split_cell_spans_at_scroll, syntax_fg,
    },
    grep::{grep_highlight_target_for_columns, highlighted_grep_text_line},
    headers::{file_header_line, file_separator_line, hunk_header_line, hunk_header_spans},
    menus::{
        diff_comparison_label, diff_selector_text, diff_selector_width, help_menu_bg,
        help_menu_content_rows, help_menu_lines, help_menu_row_spans, help_menu_title_color,
    },
    sidebar::file_sidebar_lines,
    statusline::{
        filter_bar_line, filter_bar_visible, statusline_file_count_label, statusline_header_line,
    },
    style::base_bg,
    text::{
        fit, fit_padded, fit_padded_from, fit_with_ellipsis, format_count, progress_label,
        skip_display_prefix,
    },
};
use crate::{app::*, controls::*, editor::*, live_diff::*, model::*, syntax::*, theme::*};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use hz_diff::{Changeset, DiffLine, DiffLineKind, DiffOptions, DiffScope, DiffSource, FileStatus};
use hz_syntax::{
    ColorOverrides, DiffContextExpansion, DiffSettings, HighlightedLine, SyntaxClass,
    SyntaxLanguageSet, SyntaxLimits, SyntaxThemeConfig, SyntaxThemeSource,
};
use ratatui::prelude::{Color, Line, Modifier, Span, Style};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant},
};
use unicode_width::UnicodeWidthStr;

#[test]
fn default_layout_uses_split_only_when_terminal_is_wide_enough() {
    assert_eq!(
        default_layout_for_width(MIN_SPLIT_WIDTH - 1),
        DiffLayoutMode::Unified
    );
    assert_eq!(
        default_layout_for_width(MIN_SPLIT_WIDTH),
        DiffLayoutMode::Split
    );
}

#[test]
fn max_scroll_stops_at_last_full_viewport() {
    assert_eq!(max_scroll_for_viewport(10, 1), 9);
    assert_eq!(max_scroll_for_viewport(10, 4), 6);
    assert_eq!(max_scroll_for_viewport(3, 10), 0);
    assert_eq!(max_scroll_for_viewport(10, 10), 0);
    assert_eq!(max_scroll_for_viewport(10, 0), 9);
}

#[test]
fn viewport_focus_offset_slides_between_edges_and_center() {
    assert_eq!(viewport_focus_offset(0, 100, 11), 0);
    assert_eq!(viewport_focus_offset(3, 100, 11), 3);
    assert_eq!(viewport_focus_offset(20, 100, 11), 5);
    assert_eq!(viewport_focus_offset(87, 100, 11), 8);
    assert_eq!(viewport_focus_offset(89, 100, 11), 10);
    assert_eq!(viewport_focus_offset(0, 5, 11), 2);
}

#[test]
fn app_clamps_scroll_to_last_full_viewport() {
    let changeset = changeset_with_context_lines(10);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);

    app.set_viewport_rows(5);
    app.set_scroll(usize::MAX);

    assert_eq!(app.scroll, app.model.len() - 5);
    assert_eq!(app.viewport_focus_row(), app.model.len() - 1);

    app.set_viewport_rows(usize::MAX);

    assert_eq!(app.scroll, 0);
}

#[test]
fn app_clamps_horizontal_scroll_to_diff_content() {
    let changeset = changeset_with_line_text("abcdefghijkl");
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);

    app.set_viewport_width(18);
    assert_eq!(diff_content_width(app.layout, app.viewport_width), 4);

    app.scroll_horizontally_by(HORIZONTAL_SCROLL_STEP as isize);
    assert_eq!(app.horizontal_scroll, 8);

    app.scroll_horizontally_by(HORIZONTAL_SCROLL_STEP as isize);
    assert_eq!(app.horizontal_scroll, 8);

    app.scroll_horizontally_by(-(HORIZONTAL_SCROLL_STEP as isize));
    assert_eq!(app.horizontal_scroll, 0);

    app.set_horizontal_scroll(8);
    app.set_viewport_width(80);
    assert_eq!(app.horizontal_scroll, 0);
}

#[test]
fn hunk_focus_uses_sliding_viewport_anchor() {
    let changeset = changeset_with_hunks_at(PathBuf::from("/repo"), &[1, 2, 3]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_rows(5);

    assert_eq!(app.focused_hunk_for_viewport(5), Some((0, 0)));

    app.set_scroll(1);
    assert_eq!(app.focused_hunk_for_viewport(5), Some((0, 1)));

    app.set_scroll(usize::MAX);
    assert_eq!(app.focused_hunk_for_viewport(5), Some((0, 2)));
    assert_eq!(app.scroll, app.max_scroll());
}

#[test]
fn hunk_focus_moves_between_hunks_when_diff_fits_viewport() {
    let changeset = changeset_with_hunks_at(PathBuf::from("/repo"), &[1, 2, 3]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_rows(20);

    assert_eq!(app.max_scroll(), 0);
    assert_eq!(app.focused_hunk_for_viewport(20), Some((0, 0)));

    app.next_hunk();
    assert_eq!(app.scroll, 0);
    assert_eq!(app.focused_hunk_for_viewport(20), Some((0, 1)));

    app.next_hunk();
    assert_eq!(app.scroll, 0);
    assert_eq!(app.focused_hunk_for_viewport(20), Some((0, 2)));

    app.previous_hunk();
    assert_eq!(app.focused_hunk_for_viewport(20), Some((0, 1)));

    app.previous_hunk();
    assert_eq!(app.scroll, 0);
    assert_eq!(app.focused_hunk_for_viewport(20), Some((0, 0)));
}

#[test]
fn j_and_k_move_hunk_focus_when_diff_fits_viewport() {
    let changeset = changeset_with_hunks_at(PathBuf::from("/repo"), &[1, 2, 3]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_rows(20);

    assert_eq!(app.max_scroll(), 0);
    assert_eq!(app.focused_hunk_for_viewport(20), Some((0, 0)));

    app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE))
        .expect("j should be handled");
    assert_eq!(app.scroll, 0);
    assert_eq!(app.focused_hunk_for_viewport(20), Some((0, 1)));

    app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE))
        .expect("j should be handled");
    assert_eq!(app.scroll, 0);
    assert_eq!(app.focused_hunk_for_viewport(20), Some((0, 2)));

    app.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE))
        .expect("k should be handled");
    assert_eq!(app.focused_hunk_for_viewport(20), Some((0, 1)));

    app.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE))
        .expect("k should be handled");
    assert_eq!(app.scroll, 0);
    assert_eq!(app.focused_hunk_for_viewport(20), Some((0, 0)));
}

#[test]
fn mouse_wheel_moves_hunk_focus_when_diff_fits_viewport() {
    let changeset = changeset_with_hunks_at(PathBuf::from("/repo"), &[1, 2, 3]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_rows(20);

    assert_eq!(app.max_scroll(), 0);
    assert_eq!(app.focused_hunk_for_viewport(20), Some((0, 0)));

    for _ in 0..2 {
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        })
        .expect("mouse wheel should be handled");
        assert_eq!(app.focused_hunk_for_viewport(20), Some((0, 0)));
    }
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 0,
        row: 0,
        modifiers: KeyModifiers::NONE,
    })
    .expect("mouse wheel should be handled");
    assert_eq!(app.scroll, 0);
    assert_eq!(app.focused_hunk_for_viewport(20), Some((0, 1)));

    for _ in 0..2 {
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        })
        .expect("mouse wheel should be handled");
        assert_eq!(app.focused_hunk_for_viewport(20), Some((0, 1)));
    }
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::ScrollUp,
        column: 0,
        row: 0,
        modifiers: KeyModifiers::NONE,
    })
    .expect("mouse wheel should be handled");
    assert_eq!(app.scroll, 0);
    assert_eq!(app.focused_hunk_for_viewport(20), Some((0, 0)));
}

#[test]
fn bracket_hunk_navigation_uses_focused_hunk_in_scrollable_diff() {
    let changeset = changeset_with_hunks_at(PathBuf::from("/repo"), &[1, 2, 3]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_rows(5);

    assert!(app.max_scroll() > 0);
    assert_eq!(app.focused_hunk_for_viewport(5), Some((0, 0)));

    app.next_hunk();
    assert_eq!(app.focused_hunk_for_viewport(5), Some((0, 1)));

    app.next_hunk();
    assert_eq!(app.focused_hunk_for_viewport(5), Some((0, 2)));

    app.previous_hunk();
    assert_eq!(app.focused_hunk_for_viewport(5), Some((0, 1)));
}

#[test]
fn bracket_hunk_navigation_focuses_visible_hunk_when_scroll_is_clamped() {
    let changeset = changeset_with_files(&[
        "a.rs", "b.rs", "c.rs", "d.rs", "e.rs", "f.rs", "g.rs", "h.rs", "i.rs", "j.rs",
    ]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_rows(20);

    assert!(app.max_scroll() > 0);
    assert_eq!(app.scroll, 0);
    assert_eq!(app.focused_hunk_for_viewport(20), Some((0, 0)));

    app.next_hunk();
    assert_eq!(app.scroll, 0);
    assert_eq!(app.focused_hunk_for_viewport(20), Some((1, 0)));

    app.next_hunk();
    assert_eq!(app.scroll, 0);
    assert_eq!(app.focused_hunk_for_viewport(20), Some((2, 0)));

    app.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE))
        .expect("k should be handled");
    assert_eq!(app.scroll, 0);
    assert_eq!(app.focused_hunk_for_viewport(20), Some((1, 0)));
}

#[test]
fn bracket_hunk_navigation_can_return_to_first_hunk_in_short_scrollable_diff() {
    let changeset = changeset_with_files(&["a.rs", "b.rs", "c.rs", "d.rs", "e.rs", "f.rs"]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_rows(20);

    assert!(app.max_scroll() > 0);
    assert_eq!(app.focused_hunk_for_viewport(20), Some((0, 0)));

    for _ in 0..10 {
        app.previous_hunk();
    }
    assert_eq!(app.scroll, 0);
    assert_eq!(app.focused_hunk_for_viewport(20), Some((0, 0)));

    app.next_hunk();
    assert_eq!(app.focused_hunk_for_viewport(20), Some((1, 0)));

    app.previous_hunk();
    assert_eq!(app.focused_hunk_for_viewport(20), Some((0, 0)));
}

#[test]
fn bracket_hunk_navigation_centers_hunk_that_fits_viewport() {
    let changeset =
        changeset_with_hunk_line_counts(PathBuf::from("/repo"), &[(1, 8), (20, 4), (40, 10)]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_rows(9);
    let range = app
        .model
        .hunk_row_range(0, 1)
        .expect("target hunk should have rows");

    app.next_hunk();

    let hunk_center = range
        .start
        .saturating_add(range.end.saturating_sub(range.start).saturating_sub(1) / 2);
    assert_eq!(
        app.scroll + viewport_center_offset(app.viewport_rows),
        hunk_center
    );
    assert_eq!(app.focused_hunk_for_viewport(9), Some((0, 1)));
}

#[test]
fn bracket_hunk_navigation_places_oversized_hunk_at_top() {
    let changeset =
        changeset_with_hunk_line_counts(PathBuf::from("/repo"), &[(1, 2), (20, 20), (60, 2)]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_rows(9);
    let range = app
        .model
        .hunk_row_range(0, 1)
        .expect("target hunk should have rows");

    app.next_hunk();

    assert_eq!(app.scroll, range.start - 1);
    assert!(matches!(
        app.model.row(app.scroll),
        Some(UiRow::Collapsed { .. })
    ));
    assert_eq!(
        app.model.row(app.scroll + 1),
        Some(UiRow::HunkHeader { file: 0, hunk: 1 })
    );
    assert_eq!(app.focused_hunk_for_viewport(9), Some((0, 1)));
}

#[test]
fn hunk_navigation_centers_with_surrounding_collapsed_context() {
    let changeset =
        changeset_with_hunk_line_counts(PathBuf::from("/repo"), &[(1, 2), (20, 2), (40, 2)]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_rows(9);

    let range = app
        .model
        .hunk_row_range(0, 1)
        .expect("target hunk should have rows");
    let range_start = range.start - 1;
    let range_end = range.end + 1;
    assert!(matches!(
        app.model.row(range_start),
        Some(UiRow::Collapsed { .. })
    ));
    assert!(matches!(
        app.model.row(range.end),
        Some(UiRow::Collapsed { .. })
    ));

    app.next_hunk();

    let center = range_start.saturating_add(range_end.saturating_sub(range_start + 1) / 2);
    assert_eq!(
        app.scroll + viewport_center_offset(app.viewport_rows),
        center
    );
    assert_eq!(app.focused_hunk_for_viewport(9), Some((0, 1)));
}

#[test]
fn hunk_navigation_keeps_adjacent_file_header_with_oversized_hunk() {
    let changeset = changeset_with_hunk_line_counts(PathBuf::from("/repo"), &[(1, 20)]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_rows(9);
    app.set_scroll(5);

    let hunk_row = app
        .model
        .hunk_start_row(0, 0)
        .expect("target hunk should have a header row");
    app.focus_hunk_row(hunk_row);

    assert_eq!(app.scroll, app.model.file_start_row(0).unwrap());
    assert_eq!(app.model.row(app.scroll), Some(UiRow::FileHeader(0)));
    assert_eq!(app.focused_hunk_for_viewport(9), Some((0, 0)));
}

#[test]
fn hunk_navigation_keeps_file_header_before_collapsed_context() {
    let changeset = changeset_with_hunk_line_counts(PathBuf::from("/repo"), &[(233, 20)]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_rows(9);
    app.set_scroll(5);

    let hunk_row = app
        .model
        .hunk_start_row(0, 0)
        .expect("target hunk should have a header row");
    app.focus_hunk_row(hunk_row);

    assert_eq!(app.scroll, app.model.file_start_row(0).unwrap());
    assert_eq!(app.model.row(app.scroll), Some(UiRow::FileHeader(0)));
    assert!(matches!(
        app.model.row(app.scroll + 1),
        Some(UiRow::Collapsed { .. })
    ));
    assert_eq!(
        app.model.row(app.scroll + 2),
        Some(UiRow::HunkHeader { file: 0, hunk: 0 })
    );
    assert_eq!(app.focused_hunk_for_viewport(9), Some((0, 0)));
}

#[test]
fn focused_hunk_editor_target_uses_new_path_and_visible_line() {
    let repo = PathBuf::from("/repo");
    let changeset = changeset_with_context_lines_at(repo.clone(), 1, 100);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_rows(11);
    app.set_scroll(40);

    assert_eq!(
        app.focused_hunk_editor_target(),
        Some(EditorTarget {
            path: repo.join("file.rs"),
            line: 44,
        })
    );
}

#[test]
fn focused_hunk_editor_target_uses_manual_focus_when_diff_fits_viewport() {
    let repo = PathBuf::from("/repo");
    let changeset = changeset_with_hunks_at(repo.clone(), &[10, 20, 30]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_rows(20);

    app.next_hunk();
    app.next_hunk();

    assert_eq!(
        app.focused_hunk_editor_target(),
        Some(EditorTarget {
            path: repo.join("file.rs"),
            line: 30,
        })
    );
}

#[test]
fn focused_hunk_editor_target_falls_back_to_hunk_start() {
    let repo = PathBuf::from("/repo");
    let changeset = changeset_with_hunks_at(repo.clone(), &[20, 40]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_rows(5);
    app.set_scroll(1);

    assert_eq!(
        app.focused_hunk_editor_target(),
        Some(EditorTarget {
            path: repo.join("file.rs"),
            line: 40,
        })
    );
}

#[test]
fn focused_hunk_editor_target_skips_deleted_files() {
    let mut changeset = changeset_with_hunk_at(PathBuf::from("/repo"), 20);
    changeset.files[0].status = FileStatus::Deleted;
    changeset.files[0].new_path = None;
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_rows(5);

    assert_eq!(app.focused_hunk_editor_target(), None);
}

#[test]
fn editor_command_helpers_choose_line_arguments() {
    assert_eq!(
        split_editor_command("nvim -f").unwrap(),
        vec!["nvim".to_owned(), "-f".to_owned()]
    );

    let quoted_code = split_editor_command(
        r#""/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code" -g"#,
    )
    .unwrap();
    assert_eq!(
        quoted_code,
        vec![
            "/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code".to_owned(),
            "-g".to_owned(),
        ]
    );
    assert!(editor_uses_goto_arg(&quoted_code[0]));

    assert_eq!(
        split_editor_command(r#"/Applications/Some\ Editor/bin/editor -g"#).unwrap(),
        vec![
            "/Applications/Some Editor/bin/editor".to_owned(),
            "-g".to_owned(),
        ]
    );
    assert_eq!(
        split_editor_command(r#"editor "--flag with spaces""#).unwrap(),
        vec!["editor".to_owned(), "--flag with spaces".to_owned()]
    );
    assert_eq!(split_editor_command(r#""unterminated"#), None);

    assert!(editor_uses_goto_arg("/usr/local/bin/code"));
    assert!(!editor_uses_goto_arg("vim"));
}

#[test]
fn ctrl_g_without_editable_target_does_not_scroll_to_top() {
    let mut changeset = changeset_with_hunk_at(PathBuf::from("/repo"), 20);
    changeset.files[0].new_path = None;
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_rows(1);
    app.set_scroll(1);

    let should_quit = app
        .handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL))
        .expect("Ctrl-G should be handled");

    assert!(!should_quit);
    assert_eq!(app.scroll, 1);
}

#[test]
fn ui_model_inserts_file_separator_between_files() {
    let changeset = changeset_with_files(&["a.rs", "b.rs"]);
    let model = UiModel::new(&changeset, DiffLayoutMode::Unified, &HashMap::new());

    assert_eq!(model.file_start_row(0), Some(0));
    assert_eq!(model.file_start_row(1), Some(4));
    assert_eq!(model.row(3), Some(UiRow::FileSeparator));
    assert_eq!(model.row(4), Some(UiRow::FileHeader(1)));
    assert_eq!(model.file_at_row(3), Some(0));
    assert_eq!(model.file_at_row(4), Some(1));
}

#[test]
fn file_separator_line_draws_rule_across_full_width() {
    let theme = DiffTheme::default();
    let line = file_separator_line(DiffLayoutMode::Unified, 24, theme);
    let text = line_text(&line);

    assert_eq!(text.width(), 24);
    assert_eq!(text, "────────────────────────");
    assert_eq!(line.spans[0].style.bg, Some(base_bg(theme)));
    assert_eq!(line.spans[0].style.fg, Some(theme.empty_diff));
}

#[test]
fn ui_model_expands_context_before_hunk_from_nearest_lines() {
    let step = default_context_expand_step();
    let changeset = changeset_with_hunk_at(PathBuf::from("/repo"), 50);
    let mut expansions = HashMap::new();

    let model = UiModel::new(&changeset, DiffLayoutMode::Unified, &expansions);
    assert_eq!(
        model.row(1),
        Some(UiRow::Collapsed {
            file: 0,
            hunk: 0,
            old_start: 1,
            new_start: 1,
            lines: 49,
            expanded: 0,
        })
    );

    expansions.insert(ContextKey { file: 0, hunk: 0 }, step);
    let model = UiModel::new(&changeset, DiffLayoutMode::Unified, &expansions);

    assert_eq!(
        model.row(1),
        Some(UiRow::Collapsed {
            file: 0,
            hunk: 0,
            old_start: 1,
            new_start: 1,
            lines: 29,
            expanded: step,
        })
    );
    assert_eq!(
        model.row(2),
        Some(UiRow::ContextLine {
            file: 0,
            old_line: 30,
            new_line: 30,
        })
    );
    assert_eq!(
        model.row(22),
        Some(UiRow::ContextHide {
            file: 0,
            hunk: 0,
            lines: step,
        })
    );
    assert_eq!(model.row(23), Some(UiRow::HunkHeader { file: 0, hunk: 0 }));
}

#[test]
fn ui_model_expands_context_after_previous_hunk_downward() {
    let step = default_context_expand_step();
    let changeset = changeset_with_hunks_at(PathBuf::from("/repo"), &[50, 100]);
    let mut expansions = HashMap::new();

    let model = UiModel::new(&changeset, DiffLayoutMode::Unified, &expansions);
    assert_eq!(
        model.row(4),
        Some(UiRow::Collapsed {
            file: 0,
            hunk: 1,
            old_start: 51,
            new_start: 51,
            lines: 49,
            expanded: 0,
        })
    );

    expansions.insert(ContextKey { file: 0, hunk: 1 }, step);
    let model = UiModel::new(&changeset, DiffLayoutMode::Unified, &expansions);

    assert_eq!(
        model.row(4),
        Some(UiRow::ContextHide {
            file: 0,
            hunk: 1,
            lines: step,
        })
    );
    assert_eq!(
        model.row(5),
        Some(UiRow::ContextLine {
            file: 0,
            old_line: 51,
            new_line: 51,
        })
    );
    assert_eq!(
        model.row(25),
        Some(UiRow::Collapsed {
            file: 0,
            hunk: 1,
            old_start: 51,
            new_start: 51,
            lines: 29,
            expanded: step,
        })
    );
    assert_eq!(model.row(26), Some(UiRow::HunkHeader { file: 0, hunk: 1 }));
}

#[test]
fn full_context_expansion_config_shows_all_remaining_lines() {
    let repo = temp_test_dir("full-context-expansion");
    fs::create_dir_all(&repo).expect("repo directory should be created");
    let text = (1..=80)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(repo.join("file.rs"), text).expect("context file should be written");
    let changeset = changeset_with_hunk_at(repo, 50);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.theme.diff.context_expansion = DiffContextExpansion::Full;

    assert_eq!(app.context_expand_count(49), 49);
    assert!(app.expand_context_at_row(1));
    assert_eq!(
        app.context_expansions.get(&ContextKey { file: 0, hunk: 0 }),
        Some(&49)
    );
    assert_eq!(
        app.model.row(1),
        Some(UiRow::ContextLine {
            file: 0,
            old_line: 1,
            new_line: 1,
        })
    );
    assert_eq!(
        app.model.row(50),
        Some(UiRow::ContextHide {
            file: 0,
            hunk: 0,
            lines: 49,
        })
    );
}

#[test]
fn clicking_collapsed_context_expands_more_on_each_click() {
    let step = default_context_expand_step();
    let repo = temp_test_dir("expand-context");
    fs::create_dir_all(&repo).expect("repo directory should be created");
    let text = (1..=80)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(repo.join("file.rs"), text).expect("context file should be written");
    let changeset = changeset_with_hunk_at(repo, 50);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);

    assert!(app.expand_context_at_row(1));
    assert_eq!(
        app.context_expansions.get(&ContextKey { file: 0, hunk: 0 }),
        Some(&step)
    );
    assert_eq!(
        app.model.row(1),
        Some(UiRow::Collapsed {
            file: 0,
            hunk: 0,
            old_start: 1,
            new_start: 1,
            lines: 29,
            expanded: step,
        })
    );
    let row = app.model.row(2).expect("expanded context row should exist");
    assert_eq!(
        row,
        UiRow::ContextLine {
            file: 0,
            old_line: 30,
            new_line: 30,
        }
    );
    let rendered = render_row(&mut app, 2, row, 80);
    assert!(line_text(&rendered).contains("line 30"));

    assert!(app.expand_context_at_row(1));
    assert_eq!(
        app.context_expansions.get(&ContextKey { file: 0, hunk: 0 }),
        Some(&(step * 2))
    );
    assert_eq!(
        app.model.row(2),
        Some(UiRow::ContextLine {
            file: 0,
            old_line: 10,
            new_line: 10,
        })
    );

    assert!(app.hide_context(0, 0));
    assert!(
        !app.context_expansions
            .contains_key(&ContextKey { file: 0, hunk: 0 })
    );
    assert_eq!(
        app.model.row(1),
        Some(UiRow::Collapsed {
            file: 0,
            hunk: 0,
            old_start: 1,
            new_start: 1,
            lines: 49,
            expanded: 0,
        })
    );
}

#[test]
fn clicking_collapsed_context_between_hunks_expands_downward() {
    let step = default_context_expand_step();
    let repo = temp_test_dir("expand-context-downward");
    fs::create_dir_all(&repo).expect("repo directory should be created");
    let text = (1..=120)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(repo.join("file.rs"), text).expect("context file should be written");
    let changeset = changeset_with_hunks_at(repo, &[50, 100]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);

    assert!(app.expand_context_at_row(4));
    assert_eq!(
        app.context_expansions.get(&ContextKey { file: 0, hunk: 1 }),
        Some(&step)
    );
    assert_eq!(
        app.model.row(4),
        Some(UiRow::ContextHide {
            file: 0,
            hunk: 1,
            lines: step,
        })
    );
    let row = app.model.row(5).expect("expanded context row should exist");
    assert_eq!(
        row,
        UiRow::ContextLine {
            file: 0,
            old_line: 51,
            new_line: 51,
        }
    );
    let rendered = render_row(&mut app, 5, row, 80);
    assert!(line_text(&rendered).contains("line 51"));

    assert!(app.expand_context_at_row(25));
    assert_eq!(
        app.context_expansions.get(&ContextKey { file: 0, hunk: 1 }),
        Some(&(step * 2))
    );
    assert_eq!(
        app.model.row(25),
        Some(UiRow::ContextLine {
            file: 0,
            old_line: 71,
            new_line: 71,
        })
    );

    assert!(app.hide_context(0, 1));
    assert!(
        !app.context_expansions
            .contains_key(&ContextKey { file: 0, hunk: 1 })
    );
    assert_eq!(
        app.model.row(4),
        Some(UiRow::Collapsed {
            file: 0,
            hunk: 1,
            old_start: 51,
            new_start: 51,
            lines: 49,
            expanded: 0,
        })
    );
}

#[test]
fn context_source_side_uses_loaded_fallback_side() {
    let repo = temp_test_dir("context-source-side-fallback");
    fs::create_dir_all(&repo).expect("repo directory should be created");
    git(&repo, &["init", "-q"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "Test"]);

    let text = (1..=10)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(repo.join("file.rs"), text).expect("old file should be written");
    git(&repo, &["add", "file.rs"]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    fs::remove_file(repo.join("file.rs")).expect("worktree file should be removed");

    let changeset = changeset_with_hunk_at(repo.clone(), 5);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);

    let (side, lines) = app
        .ensure_context_lines(0)
        .expect("old context should load after new side fails");
    assert_eq!(side, DiffSide::Old);
    assert_eq!(lines.first().map(String::as_str), Some("line 1"));
    assert_eq!(app.context_source_side(0), Some(DiffSide::Old));

    fs::remove_dir_all(repo).expect("repo directory should be removed");
}

#[test]
fn collapsed_context_control_is_minimal_and_readable() {
    let theme = DiffTheme::default();
    let line = context_show_line(20, false, 72, theme);
    let text = line_text(&line);

    assert_eq!(text.width(), 72);
    assert!(text.starts_with(DIFF_INDICATOR));
    assert!(text.contains("▾ show 20 lines"));
    assert_eq!(line.spans[1].style.fg, Some(theme.muted));
    assert_eq!(
        line.spans[0].style.bg,
        Some(line_gutter_bg(DiffLineKind::Meta, theme))
    );

    let hide = context_hide_line(20, 24, theme);
    let hide_text = line_text(&hide);
    assert!(hide_text.contains("▴ hide 20 lines"));
    assert_eq!(hide.spans[1].style.fg, Some(theme.muted));
}

#[test]
fn responsive_layout_preserves_valid_horizontal_scroll() {
    let long_line = "a".repeat(120);
    let changeset = changeset_with_line_text(&long_line);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_width(80);
    app.set_horizontal_scroll(40);

    app.apply_responsive_layout(MIN_SPLIT_WIDTH);

    assert_eq!(app.layout, DiffLayoutMode::Split);
    assert_eq!(app.horizontal_scroll, 40);
}

#[test]
fn responsive_layout_clamps_horizontal_scroll_without_layout_change() {
    let long_line = "a".repeat(100);
    let changeset = changeset_with_line_text(&long_line);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Split);

    app.apply_responsive_layout(MIN_SPLIT_WIDTH);
    assert_eq!(app.layout, DiffLayoutMode::Split);
    app.set_horizontal_scroll(usize::MAX);
    let previous_scroll = app.horizontal_scroll;

    app.apply_responsive_layout(MIN_SPLIT_WIDTH + 40);

    assert_eq!(app.layout, DiffLayoutMode::Split);
    assert!(app.max_horizontal_scroll() < previous_scroll);
    assert_eq!(app.horizontal_scroll, app.max_horizontal_scroll());
}

#[test]
fn b_key_toggles_file_sidebar() {
    let changeset = changeset_with_context_lines(1);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);

    assert!(!app.file_sidebar_open);

    let should_quit = app
        .handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE))
        .expect("b should be handled");
    assert!(!should_quit);
    assert!(app.file_sidebar_open);

    app.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE))
        .expect("b should be handled");
    assert!(!app.file_sidebar_open);
}

#[test]
fn b_key_clears_file_sidebar_resize_state() {
    let changeset = changeset_with_files(&["a.rs"]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.file_sidebar_open = true;
    app.file_sidebar_render_width = 30;
    app.viewport_width = 70;

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 29,
        row: 1,
        modifiers: KeyModifiers::NONE,
    })
    .expect("resize should start");

    assert!(app.file_sidebar_resizing);
    assert_eq!(app.file_sidebar_width, Some(30));

    app.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE))
        .expect("b should be handled");

    assert!(!app.file_sidebar_open);
    assert!(!app.file_sidebar_resizing);

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: 49,
        row: 1,
        modifiers: KeyModifiers::NONE,
    })
    .expect("drag should no longer resize after sidebar closes");

    assert_eq!(app.file_sidebar_width, Some(30));
}

#[test]
fn f_key_filters_files_and_escape_clears_filter() {
    let changeset = changeset_with_files(&["src/lib.rs", "README.md", "hz-tui/src/lib.rs"]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);

    app.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE))
        .expect("f should open file filter");
    assert!(line_text(&filter_bar_line(&app, 40)).starts_with("filter: type to filter files"));
    let generation_before_input = app.generation;
    for character in "tui".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE))
            .expect("file filter input should be handled");
    }

    assert_eq!(app.file_filter, "tui");
    assert_eq!(app.file_filter_input, "tui");
    assert_eq!(visible_paths(&app), vec!["hz-tui/src/lib.rs"]);
    assert_eq!(app.generation, generation_before_input);
    assert!(line_text(&filter_bar_line(&app, 40)).starts_with("filter: tui"));

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .expect("enter should keep file filter");

    assert_eq!(app.generation, generation_before_input);
    assert_eq!(app.file_filter, "tui");
    assert_eq!(visible_paths(&app), vec!["hz-tui/src/lib.rs"]);
    assert_eq!(statusline_file_count_label(&app), "1/3 files");
    assert!(line_text(&filter_bar_line(&app, 40)).starts_with("filter: tui"));
    assert!(!line_text(&statusline_header_line(&app, 120)).contains("f:tui"));
    assert!(app.filter_input.is_none());
    assert!(filter_bar_visible(&app));
    assert!(line_text(&filter_bar_line(&app, 40)).starts_with("filter: tui"));

    app.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE))
        .expect("f should reopen file filter");

    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
        .expect("escape should clear file filter");

    assert_eq!(app.file_filter, "");
    assert_eq!(
        visible_paths(&app),
        vec!["src/lib.rs", "README.md", "hz-tui/src/lib.rs"]
    );
    assert!(app.filter_input.is_none());
}

#[test]
fn slash_filters_files_by_diff_content_and_escape_clears_filter() {
    let changeset = changeset_with_files(&["a.rs", "b.rs", "c.rs"]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);

    app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE))
        .expect("slash should open grep filter");
    assert!(line_text(&filter_bar_line(&app, 40)).starts_with("/ type to grep diff"));
    for character in "line 1".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE))
            .expect("grep filter input should be handled");
    }

    assert_eq!(app.grep_filter, "line 1");
    assert_eq!(app.grep_filter_input, "line 1");
    assert_eq!(visible_paths(&app), vec!["b.rs"]);
    assert!(line_text(&filter_bar_line(&app, 40)).starts_with("/line 1"));

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .expect("enter should keep grep filter");

    assert_eq!(app.grep_filter, "line 1");
    assert_eq!(visible_paths(&app), vec!["b.rs"]);
    assert_eq!(app.grep_matches.len(), 1);
    assert_eq!(app.current_grep_match_row(), Some(2));
    assert!(line_text(&filter_bar_line(&app, 40)).starts_with("/line 1"));
    assert!(!line_text(&statusline_header_line(&app, 120)).contains("/:line 1"));
    assert!(app.filter_input.is_none());
    assert!(filter_bar_visible(&app));
    assert!(line_text(&filter_bar_line(&app, 40)).starts_with("/line 1"));

    app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE))
        .expect("slash should reopen grep filter");

    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
        .expect("escape should clear grep filter");

    assert_eq!(app.grep_filter, "");
    assert_eq!(visible_paths(&app), vec!["a.rs", "b.rs", "c.rs"]);
    assert!(app.filter_input.is_none());
}

#[test]
fn text_matcher_preserves_case_and_prefix_matching() {
    let lowercase = TextMatcher::new("line").expect("matcher should be created");
    assert!(lowercase.matches("LINE"));
    assert_eq!(lowercase.match_ranges("LINE").len(), 1);

    let uppercase = TextMatcher::new("Line").expect("matcher should be created");
    assert!(!uppercase.matches("line"));

    let unicode = TextMatcher::new("éclair").expect("matcher should be created");
    assert!(!unicode.case_sensitive);
    assert!(unicode.matches("éclair"));
    assert!(unicode.matches("éCLAIR"));
    assert!(!unicode.matches("Éclair"));

    let addition = DiffLine {
        kind: DiffLineKind::Addition,
        old_line: None,
        new_line: Some(1),
        text: "changed".to_owned(),
    };
    let prefixed = TextMatcher::new("+changed").expect("matcher should be created");
    assert!(diff_line_grep_text_matches(&addition, &prefixed));
}

#[test]
fn file_filter_and_grep_filter_compose_and_render_together() {
    let changeset = changeset_with_files(&["a.rs", "b.rs", "c.rs"]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);

    app.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE))
        .expect("f should open file filter");
    app.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE))
        .expect("file filter input should be handled");
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .expect("enter should keep file filter");

    app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE))
        .expect("slash should open grep filter");
    for character in "line 1".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE))
            .expect("grep filter input should be handled");
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .expect("enter should keep grep filter");

    assert_eq!(visible_paths(&app), vec!["b.rs"]);
    assert!(line_text(&filter_bar_line(&app, 80)).starts_with("filter: b  /line 1"));

    let should_quit = app
        .handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
        .expect("escape should clear active filters");
    assert!(!should_quit);
    assert_eq!(app.file_filter, "");
    assert_eq!(app.grep_filter, "");
    assert_eq!(visible_paths(&app), vec!["a.rs", "b.rs", "c.rs"]);
    assert!(!filter_bar_visible(&app));
}

#[test]
fn n_and_p_navigate_grep_matches_when_grep_filter_is_active() {
    let changeset = changeset_with_files(&["a.rs", "b.rs", "c.rs"]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_rows(5);
    app.grep_filter = "line".to_owned();
    app.apply_filters(true);

    assert_eq!(app.grep_matches.len(), 3);
    assert_eq!(app.current_grep_match_row(), Some(2));
    let row = app.model.row(2).unwrap();
    let rendered = render_row(&mut app, 2, row, 40);
    assert!(
        rendered
            .spans
            .iter()
            .any(|span| span.content.as_ref() == "line"
                && span.style.bg == Some(app.theme.search_match_bg)),
        "grep text should be highlighted"
    );
    assert!(
        rendered
            .spans
            .iter()
            .any(|span| span.content.as_ref() == "line"
                && span.style.fg == Some(app.theme.search_match_fg))
    );
    assert_ne!(rendered.spans[0].style.bg, Some(app.theme.search_match_bg));

    app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE))
        .expect("n should move to next grep match");
    assert_eq!(app.current_grep_match_row(), Some(6));
    assert_eq!(app.scroll, 4);

    app.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE))
        .expect("p should move to previous grep match");
    assert_eq!(app.current_grep_match_row(), Some(2));
}

#[test]
fn grep_highlight_uses_logical_text_across_rendered_spans() {
    let theme = DiffTheme::default();
    let line = Line::from(vec![
        Span::styled("fo", Style::default().add_modifier(Modifier::BOLD)),
        Span::styled("obar", Style::default()),
    ]);
    let target = grep_highlight_target_for_columns("foobar".to_owned(), &line.spans, 0, 6, 0)
        .expect("target should map rendered spans to logical text");

    let rendered = highlighted_grep_text_line(line, "foobar", vec![target], theme);

    assert_eq!(line_text(&rendered), "foobar");
    assert_eq!(rendered.spans[0].content.as_ref(), "fo");
    assert_eq!(rendered.spans[0].style.bg, Some(theme.search_match_bg));
    assert_eq!(rendered.spans[0].style.fg, Some(theme.search_match_fg));
    assert!(
        rendered.spans[0]
            .style
            .add_modifier
            .contains(Modifier::BOLD)
    );
    assert_eq!(rendered.spans[1].content.as_ref(), "obar");
    assert_eq!(rendered.spans[1].style.bg, Some(theme.search_match_bg));
}

#[test]
fn grep_highlight_ignores_unified_gutter_numbers() {
    let changeset = changeset_with_line_text("abc");
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.grep_filter = "1".to_owned();
    app.apply_filters(false);

    let row = app.model.row(2).expect("diff line should be visible");
    let rendered = render_row(&mut app, 2, row, 32);

    assert!(line_text(&rendered).contains('1'));
    assert!(
        rendered
            .spans
            .iter()
            .all(|span| span.style.bg != Some(app.theme.search_match_bg)),
        "grep should not highlight line numbers when only the gutter matches"
    );
}

#[test]
fn question_mark_key_toggles_help_menu() {
    let changeset = changeset_with_context_lines(1);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);

    assert!(!app.help_menu_open);

    let should_quit = app
        .handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::SHIFT))
        .expect("? should be handled");
    assert!(!should_quit);
    assert!(app.help_menu_open);

    app.handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE))
        .expect("? should be handled");
    assert!(!app.help_menu_open);
}

#[test]
fn question_mark_key_filters_branch_menu() {
    let options = DiffOptions {
        source: DiffSource::Base("main".to_owned()),
        ..DiffOptions::default()
    };
    let changeset = changeset_with_context_lines(1);
    let mut app = DiffApp::new(options, changeset, DiffLayoutMode::Unified);
    app.branch_menu_open = Some(BranchMenu::Head);
    app.comparison_branches = vec!["main".to_owned(), "feature/header".to_owned()];

    let should_quit = app
        .handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::SHIFT))
        .expect("? should be handled by branch filter");

    assert!(!should_quit);
    assert!(!app.help_menu_open);
    assert_eq!(app.branch_menu_input, "?");
}

#[test]
fn help_menu_esc_closes_without_quitting() {
    let changeset = changeset_with_context_lines(1);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.help_menu_open = true;
    app.dirty = false;

    let should_quit = app
        .handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
        .expect("Esc should close help");

    assert!(!should_quit);
    assert!(!app.help_menu_open);
    assert!(app.dirty);
}

#[test]
fn help_menu_lines_list_keybindings() {
    let width = 80;
    let lines = help_menu_lines(width, help_menu_content_rows(width), DiffTheme::default());
    let text: Vec<_> = lines.iter().map(line_text).collect();

    assert_eq!(lines.len(), help_menu_content_rows(width));
    assert!(text.iter().any(|line| line.contains("?")));
    assert!(text.iter().any(|line| line.contains("q")));
    assert!(text.iter().any(|line| line.contains("Ctrl-C")));
    assert!(text.iter().any(|line| line.contains("j/k")));
    assert!(text.iter().any(|line| line.contains("n/p")));
    assert!(text.iter().any(|line| line.contains("]/[")));
    assert!(text.iter().any(|line| line.contains("Ctrl-G")));
    assert!(text.iter().any(|line| line.contains("Backspace")));
    assert!(text.iter().any(|line| line.contains("Ctrl-U")));
}

#[test]
fn help_menu_uses_diff_theme_colors() {
    let default_theme = DiffTheme::default();
    let section_color = Color::Rgb(10, 11, 12);
    let key_color = Color::Rgb(13, 14, 15);
    let theme = DiffTheme {
        background: Color::Rgb(1, 2, 3),
        header: Color::Rgb(4, 5, 6),
        foreground: Color::Rgb(7, 8, 9),
        syntax: SyntaxPalette {
            keyword: Some(section_color),
            function: Some(key_color),
            ..default_theme.syntax
        },
        ..default_theme
    };

    assert_eq!(help_menu_bg(theme), theme.background);
    assert_eq!(help_menu_title_color(theme), section_color);

    let section = help_menu_row_spans(HelpMenuRow::Section("Section"), 20, theme);
    assert_eq!(section[0].style.fg, Some(section_color));
    assert_eq!(section[0].style.bg, Some(theme.background));

    let binding = help_menu_row_spans(HelpMenuRow::Binding("?", "help"), 20, theme);
    assert_eq!(binding[0].style.fg, Some(key_color));
    assert_eq!(binding[0].style.bg, Some(theme.background));
    assert_eq!(binding[1].style.fg, Some(theme.foreground));
    assert_eq!(binding[1].style.bg, Some(theme.background));
}

#[test]
fn file_sidebar_tracks_selected_file() {
    let changeset = changeset_with_files(&["a.rs", "b.rs", "c.rs", "d.rs", "e.rs"]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.file_sidebar_open = true;
    app.set_viewport_rows(4);

    app.selected_file = 4;
    app.ensure_file_sidebar_selection_visible(app.visible_file_sidebar_rows());

    assert_eq!(app.selected_file, 4);
    assert_eq!(app.file_sidebar_scroll, 1);

    app.selected_file = 1;
    app.ensure_file_sidebar_selection_visible(app.visible_file_sidebar_rows());

    assert_eq!(app.selected_file, 1);
    assert_eq!(app.file_sidebar_scroll, 1);
}

#[test]
fn diff_scroll_does_not_move_file_sidebar_scroll() {
    let changeset = changeset_with_files(&["a.rs", "b.rs", "c.rs", "d.rs", "e.rs"]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.file_sidebar_open = true;
    app.set_viewport_rows(4);
    app.set_file_sidebar_scroll(1);

    app.set_scroll(0);

    assert_eq!(app.selected_file, 0);
    assert_eq!(app.file_sidebar_scroll, 1);
}

#[test]
fn replace_changeset_keeps_remapped_file_sidebar_selection_visible() {
    let changeset = changeset_with_files(&["a.rs", "b.rs", "c.rs", "d.rs", "e.rs"]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.file_sidebar_open = true;
    app.set_viewport_rows(2);
    app.selected_file = 4;
    app.ensure_file_sidebar_selection_visible(app.visible_file_sidebar_rows());

    assert_eq!(app.file_sidebar_scroll, 3);

    app.replace_changeset(
        changeset_with_files(&["new.rs", "other.rs", "third.rs", "fourth.rs", "fifth.rs"]),
        None,
    );

    assert_eq!(app.selected_file, 0);
    assert_eq!(app.file_sidebar_scroll, 0);
}

#[test]
fn mouse_wheel_over_file_sidebar_scrolls_sidebar_only() {
    let changeset = changeset_with_files(&["a.rs", "b.rs", "c.rs", "d.rs", "e.rs"]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.file_sidebar_open = true;
    app.file_sidebar_render_width = 20;
    app.set_viewport_rows(4);

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 1,
        row: 1,
        modifiers: KeyModifiers::NONE,
    })
    .expect("sidebar scroll should be handled");

    assert_eq!(app.scroll, 0);
    assert_eq!(app.file_sidebar_scroll, 1);
}

#[test]
fn horizontal_mouse_wheel_over_file_sidebar_is_ignored() {
    let changeset = changeset_with_line_text("abcdefghijkl");
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.file_sidebar_open = true;
    app.file_sidebar_render_width = 20;
    app.set_viewport_width(18);

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::ScrollRight,
        column: 1,
        row: 1,
        modifiers: KeyModifiers::NONE,
    })
    .expect("sidebar horizontal scroll should be ignored");

    assert_eq!(app.horizontal_scroll, 0);

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::ScrollRight,
        column: 21,
        row: 1,
        modifiers: KeyModifiers::NONE,
    })
    .expect("diff horizontal scroll should still work");

    assert_eq!(app.horizontal_scroll, HORIZONTAL_SCROLL_STEP);
}

#[test]
fn file_sidebar_renders_changed_file_summary() {
    let mut changeset = changeset_with_files(&["src/lib.rs", "README.md"]);
    changeset.files[1].status = FileStatus::Added;
    changeset.files[1].additions = 12;
    changeset.files[1].deletions = 0;
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.file_sidebar_open = true;
    app.selected_file = 1;

    let lines = file_sidebar_lines(&app, 24, 2);
    let additions = lines[1]
        .spans
        .iter()
        .find(|span| span.content.as_ref() == "+12")
        .expect("additions should render as their own span");
    let deletions = lines[1]
        .spans
        .iter()
        .find(|span| span.content.as_ref() == "-0")
        .expect("deletions should render as their own span");

    assert!(line_text(&lines[0]).contains(" M src/lib.rs"));
    assert!(line_text(&lines[1]).contains(" A README.md"));
    assert!(line_text(&lines[1]).contains("+12 -0"));
    assert_eq!(lines[0].spans[0].content.as_ref(), " M ");
    assert_eq!(lines[0].spans[0].style.fg, Some(DiffTheme::default().hunk));
    assert!(
        lines[0].spans[0]
            .style
            .add_modifier
            .contains(Modifier::BOLD)
    );
    assert_eq!(
        lines[0].spans[1].style.fg,
        Some(DiffTheme::default().foreground)
    );
    assert_eq!(additions.style.fg, Some(DiffTheme::default().addition_fg));
    assert_eq!(deletions.style.fg, Some(DiffTheme::default().deletion_fg));
    assert!(
        lines[1].spans[0]
            .style
            .add_modifier
            .contains(Modifier::BOLD)
    );
}

#[test]
fn file_sidebar_truncates_long_paths_before_stats() {
    let mut changeset =
        changeset_with_files(&["src/runtime/test_runner/expect/toMatchInlineSnapshot.rs"]);
    changeset.files[0].status = FileStatus::Added;
    changeset.files[0].additions = 1290;
    changeset.files[0].deletions = 3910;
    let app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);

    let lines = file_sidebar_lines(&app, 32, 1);
    let text = line_text(&lines[0]);

    assert_eq!(text.width(), 32);
    assert!(text.contains("..."));
    assert!(text.contains("+1290 -3910"));
}

#[test]
fn file_sidebar_separator_drag_resizes_sidebar() {
    let changeset = changeset_with_files(&["a.rs"]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.file_sidebar_open = true;
    app.file_sidebar_render_width = 30;
    app.viewport_width = 70;

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 29,
        row: 1,
        modifiers: KeyModifiers::NONE,
    })
    .expect("resize should start");

    assert!(app.file_sidebar_resizing);

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: 49,
        row: 1,
        modifiers: KeyModifiers::NONE,
    })
    .expect("drag should resize");

    assert_eq!(app.file_sidebar_width, Some(50));
    assert!(app.dirty);

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: 49,
        row: 1,
        modifiers: KeyModifiers::NONE,
    })
    .expect("resize should end");

    assert!(!app.file_sidebar_resizing);
}

#[test]
fn notices_expire_after_ttl() {
    let changeset = changeset_with_context_lines(1);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);

    app.set_notice("reloaded");
    let expires_at = app.notice.as_ref().unwrap().expires_at;
    app.dirty = false;

    app.expire_notice(expires_at - Duration::from_millis(1));
    assert!(app.notice.is_some());
    assert!(!app.dirty);

    app.expire_notice(expires_at);
    assert!(app.notice.is_none());
    assert!(app.dirty);
}

#[test]
fn progress_label_is_bounded() {
    assert_eq!(progress_label(0, 0), "100%");
    assert_eq!(progress_label(0, 20), "0%");
    assert_eq!(progress_label(10, 20), "50%");
    assert_eq!(progress_label(100, 20), "100%");
}

#[test]
fn diff_header_labels_describe_selected_scope() {
    let mut options = DiffOptions::default();

    assert_eq!(diff_selector_text(&options), " All changes ");
    assert_eq!(diff_comparison_label(&options), "HEAD → working tree");

    options.scope = DiffScope::Unstaged;
    assert_eq!(diff_selector_text(&options), " Unstaged ");
    assert_eq!(diff_comparison_label(&options), "index → working tree");

    options.scope = DiffScope::Staged;
    assert_eq!(diff_selector_text(&options), " Staged ");
    assert_eq!(diff_comparison_label(&options), "HEAD → index");

    options.source = DiffSource::Base("origin/main".to_owned());
    options.scope = DiffScope::All;
    assert_eq!(diff_selector_text(&options), " Branch ");
    assert_eq!(diff_comparison_label(&options), "HEAD → origin/main");

    options.source = DiffSource::Branch {
        base: "origin/main".to_owned(),
        head: "feature/ui".to_owned(),
    };
    assert_eq!(diff_comparison_label(&options), "feature/ui → origin/main");
}

#[test]
fn statusline_header_right_aligns_current_file() {
    let changeset = changeset_with_files(&["src/lib.rs", "README.md", "docs/guide.md"]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_rows(3);
    app.select_file(1);

    let line = statusline_header_line(&app, 80);
    let text = line_text(&line);

    assert_eq!(text.width(), 80);
    assert!(text.starts_with(" All changes  HEAD → working tree"));
    assert!(text.contains("README.md 2/3"));
    assert!(text.ends_with("% "));

    let selector = line.spans.first().expect("selector block should render");
    assert_eq!(selector.style.fg, Some(STATUSLINE_ACCENT_FG));
    assert_eq!(selector.style.bg, Some(STATUSLINE_ACCENT_BG));

    let file = line.spans.last().expect("file block should render");
    assert_eq!(file.style.fg, Some(STATUSLINE_INFO_FG));
    assert_eq!(file.style.bg, Some(STATUSLINE_INFO_BG));
    assert!(file.style.add_modifier.contains(Modifier::BOLD));
}

#[test]
fn diff_menu_lists_all_changes_first() {
    let mut app = DiffApp::new(
        DiffOptions::default(),
        changeset_with_context_lines(1),
        DiffLayoutMode::Unified,
    );
    app.branch_base = Some("origin/main".to_owned());

    assert_eq!(
        app.diff_menu_choices(),
        vec![
            DiffChoice::All,
            DiffChoice::Unstaged,
            DiffChoice::Staged,
            DiffChoice::Branch,
        ]
    );
}

#[test]
fn diff_menu_options_preserve_repo_and_untracked_setting() {
    let options = DiffOptions {
        repo: Some(PathBuf::from("/repo")),
        include_untracked: false,
        ..DiffOptions::default()
    };
    let mut app = DiffApp::new(
        options.clone(),
        changeset_with_context_lines(1),
        DiffLayoutMode::Unified,
    );
    app.branch_base = Some("origin/main".to_owned());
    app.branch_head = Some("feature/ui".to_owned());
    app.current_head = Some("feature/ui".to_owned());

    let staged = app.options_for_choice(DiffChoice::Staged).unwrap();
    assert_eq!(staged.repo, options.repo);
    assert!(!staged.include_untracked);
    assert_eq!(staged.source, DiffSource::Worktree);
    assert_eq!(staged.scope, DiffScope::Staged);

    let branch = app.options_for_choice(DiffChoice::Branch).unwrap();
    assert_eq!(branch.source, DiffSource::Base("origin/main".to_owned()));
    assert_eq!(branch.scope, DiffScope::All);
}

#[test]
fn branch_choice_survives_switching_to_worktree_scope() {
    let options = DiffOptions {
        source: DiffSource::Base("origin/main".to_owned()),
        ..DiffOptions::default()
    };
    let mut app = DiffApp::new(
        options,
        changeset_with_context_lines(1),
        DiffLayoutMode::Unified,
    );
    app.branch_base = Some("origin/main".to_owned());
    app.branch_head = Some("feature/header".to_owned());

    app.replace_loaded_diff(
        DiffOptions::default(),
        changeset_with_context_lines(1),
        None,
    );

    assert_eq!(app.branch_base.as_deref(), Some("origin/main"));
    assert_eq!(app.branch_head.as_deref(), Some("feature/header"));
    assert_eq!(
        app.options_for_choice(DiffChoice::Branch)
            .map(|options| options.source),
        Some(DiffSource::Branch {
            base: "origin/main".to_owned(),
            head: "feature/header".to_owned(),
        })
    );
}

#[test]
fn branch_header_exposes_head_and_base_selectors() {
    let options = DiffOptions {
        source: DiffSource::Base("origin/main".to_owned()),
        ..DiffOptions::default()
    };
    let mut app = DiffApp::new(
        options,
        changeset_with_context_lines(1),
        DiffLayoutMode::Unified,
    );
    app.branch_head = Some("feature/ui".to_owned());
    app.branch_base = Some("origin/main".to_owned());
    app.current_head = Some("feature/ui".to_owned());

    assert_eq!(
        app.branch_selector_text(BranchMenu::Head).as_deref(),
        Some("● feature/ui ▾")
    );
    assert_eq!(
        app.branch_selector_text(BranchMenu::Base).as_deref(),
        Some("⌂ origin/main ▾")
    );
    assert_eq!(
        app.branch_selector_at(diff_selector_width(&app.options)),
        None
    );
    assert_eq!(
        app.branch_selector_at(
            diff_selector_width(&app.options) + STATUSLINE_SELECTOR_GAP.width() as u16
        ),
        Some(BranchMenu::Head)
    );

    app.toggle_branch_menu(BranchMenu::Head);
    let empty_input = app.branch_selector_text(BranchMenu::Head).unwrap();
    assert_eq!(empty_input.width(), "● feature/ui ▾".width());
    assert!(empty_input.trim_start().starts_with('▾'));
    app.push_branch_input('f');
    let typed_input = app.branch_selector_text(BranchMenu::Head).unwrap();
    assert_eq!(typed_input.width(), "● feature/ui ▾".width());
    assert!(typed_input.starts_with('f'));
    app.close_branch_menu();
    assert_eq!(
        app.branch_selector_text(BranchMenu::Head).as_deref(),
        Some("● feature/ui ▾")
    );
}

#[test]
fn branch_menu_scrolls_visible_branch_window() {
    let options = DiffOptions {
        source: DiffSource::Base("branch-00".to_owned()),
        ..DiffOptions::default()
    };
    let mut app = DiffApp::new(
        options,
        changeset_with_context_lines(1),
        DiffLayoutMode::Unified,
    );
    app.comparison_branches = (0..12).map(|index| format!("branch-{index:02}")).collect();

    assert_eq!(app.visible_branch_menu_rows(), MAX_BRANCH_MENU_ROWS);
    assert_eq!(app.max_branch_menu_scroll(), 2);

    app.move_branch_selection(99);
    assert_eq!(app.branch_menu_selected, 11);
    assert_eq!(app.branch_menu_scroll, 2);

    app.move_branch_selection(-1);
    assert_eq!(app.branch_menu_selected, 10);
    assert_eq!(app.branch_menu_scroll, 2);
}

#[test]
fn branch_combo_input_filters_and_completes() {
    let options = DiffOptions {
        source: DiffSource::Base("main".to_owned()),
        ..DiffOptions::default()
    };
    let mut app = DiffApp::new(
        options,
        changeset_with_context_lines(1),
        DiffLayoutMode::Unified,
    );
    app.comparison_branches = vec![
        "main".to_owned(),
        "feature/header".to_owned(),
        "fix/footer".to_owned(),
    ];

    app.push_branch_input('h');
    assert_eq!(app.filtered_branches(), vec!["feature/header"]);

    app.clear_branch_input();
    app.push_branch_input('f');
    app.push_branch_input('h');
    assert_eq!(app.filtered_branches(), vec!["feature/header"]);

    app.branch_menu_open = Some(BranchMenu::Head);
    app.cycle_branch_completion(1);
    assert_eq!(app.branch_menu_selected, 0);
    assert_eq!(app.branch_menu_input, "fh");

    app.clear_branch_input();
    app.push_branch_input('f');
    assert_eq!(
        app.filtered_branches(),
        vec!["fix/footer", "feature/header"]
    );
    app.cycle_branch_completion(1);
    assert_eq!(app.branch_menu_selected, 1);
    app.cycle_branch_completion(-1);
    assert_eq!(app.branch_menu_selected, 0);

    app.clear_branch_input();
    assert!(app.branch_menu_input.is_empty());
}

#[test]
fn branch_combo_pins_current_head_and_base_before_recent_order() {
    let options = DiffOptions {
        source: DiffSource::Base("release".to_owned()),
        ..DiffOptions::default()
    };
    let mut app = DiffApp::new(
        options,
        changeset_with_context_lines(1),
        DiffLayoutMode::Unified,
    );
    app.branch_head = Some("feature/header".to_owned());
    app.current_head = Some("feature/header".to_owned());
    app.branch_base = Some("release".to_owned());
    app.comparison_branches = vec![
        "recent".to_owned(),
        "old".to_owned(),
        "origin/main".to_owned(),
        "release".to_owned(),
        "feature/header".to_owned(),
    ];

    app.branch_menu_open = Some(BranchMenu::Base);
    assert_eq!(
        app.filtered_branches(),
        vec!["release", "feature/header", "recent", "old", "origin/main"]
    );

    app.branch_menu_open = Some(BranchMenu::Head);
    assert_eq!(
        app.filtered_branches(),
        vec!["feature/header", "release", "recent", "old", "origin/main"]
    );
}

#[test]
fn branch_combo_close_clears_input_without_changing_selection() {
    let options = DiffOptions {
        source: DiffSource::Base("main".to_owned()),
        ..DiffOptions::default()
    };
    let mut app = DiffApp::new(
        options,
        changeset_with_context_lines(1),
        DiffLayoutMode::Unified,
    );
    app.branch_base = Some("main".to_owned());
    app.branch_head = Some("feature/header".to_owned());
    app.comparison_branches = vec!["main".to_owned(), "feature/header".to_owned()];

    app.toggle_branch_menu(BranchMenu::Base);
    app.push_branch_input('f');
    app.close_branch_menu();

    assert!(app.branch_menu_open.is_none());
    assert!(app.branch_menu_input.is_empty());
    assert_eq!(app.branch_base.as_deref(), Some("main"));
    assert_eq!(app.branch_head.as_deref(), Some("feature/header"));
    assert_eq!(app.options.source, DiffSource::Base("main".to_owned()));
}

#[test]
fn format_count_groups_thousands() {
    assert_eq!(format_count(0), "0");
    assert_eq!(format_count(42), "42");
    assert_eq!(format_count(999), "999");
    assert_eq!(format_count(1_000), "1,000");
    assert_eq!(format_count(1_009_257), "1,009,257");
}

#[test]
fn live_diff_filter_ignores_non_state_git_paths() {
    let repo = std::env::temp_dir().join("hz-tui-live-filter-repo");
    let other = std::env::temp_dir().join("hz-tui-live-filter-other");
    let filter = LiveDiffFilter {
        repo: repo.clone(),
        git_state_paths: vec![
            repo.join(".git/index"),
            repo.join(".git/index.lock"),
            repo.join(".git/refs"),
        ],
    };

    assert!(filter.is_relevant_path(Path::new("src/lib.rs")));
    assert!(filter.is_relevant_path(&repo.join("src/lib.rs")));
    assert!(filter.is_relevant_path(&repo.join(".git/index")));
    assert!(filter.is_relevant_path(&repo.join(".git/index.lock")));
    assert!(filter.is_relevant_path(&repo.join(".git/refs/heads/main")));
    assert!(!filter.is_relevant_path(&repo.join(".git/logs/HEAD")));
    assert!(!filter.is_relevant_path(&other.join("file.rs")));
}

#[test]
fn live_diff_watch_paths_upgrade_to_recursive() {
    let mut spec = LiveDiffWatchSpec::new(Path::new("repo"));

    spec.add_watch_path(PathBuf::from("repo/.git"), false);
    spec.add_watch_path(PathBuf::from("repo/.git"), true);

    let watch_path = spec
        .watch_paths
        .iter()
        .find(|watch_path| watch_path.path == Path::new("repo/.git"))
        .unwrap();
    assert!(watch_path.recursive);
}

#[test]
fn fit_helpers_use_terminal_display_width() {
    assert_eq!(fit("界a", 2), "界");
    assert_eq!(fit_padded("e\u{301}", 2), "e\u{301} ");
    assert_eq!(fit_padded_from("abcdef", 2, 3), "cde");
    assert_eq!(skip_display_prefix("abcdef", 2), ("cdef", 2));
    assert_eq!(skip_display_prefix("e\u{301}f", 1), ("f", 1));
    assert_eq!(fit_with_ellipsis("abcdef", 5), "ab...");
}

#[test]
fn file_header_truncates_path_before_delta() {
    let file = hz_diff::DiffFile {
        old_path: Some("src/runtime/test_runner/expect/toMatchInlineSnapshot.rs".to_owned()),
        new_path: Some("src/runtime/test_runner/expect/toMatchInlineSnapshot.rs".to_owned()),
        status: FileStatus::Modified,
        hunks: Vec::new(),
        additions: 1290,
        deletions: 3910,
        is_binary: false,
    };

    let theme = DiffTheme::default();
    let line = file_header_line(&file, 32, theme);
    let text = line_text(&line);

    assert_eq!(text.width(), 32);
    assert!(text.starts_with("M "));
    assert!(text.contains("..."));
    assert!(text.ends_with("+1290 -3910"));
    assert_eq!(line.spans[0].content.as_ref(), "M");
    assert_eq!(line.spans[0].style.fg, Some(theme.hunk));
    assert!(line.spans[0].style.add_modifier.contains(Modifier::BOLD));
    assert_eq!(line.spans[2].style.fg, Some(theme.foreground));

    let additions = line
        .spans
        .iter()
        .find(|span| span.content.as_ref() == "+1290")
        .expect("additions should render as a separate span");
    let deletions = line
        .spans
        .iter()
        .find(|span| span.content.as_ref() == "-3910")
        .expect("deletions should render as a separate span");

    assert_eq!(additions.style.fg, Some(theme.addition_fg));
    assert_eq!(deletions.style.fg, Some(theme.deletion_fg));
}

#[test]
fn hunk_header_uses_raw_location_context_and_delta() {
    let hunk = hz_diff::DiffHunk {
        header: "@@ -200,2 +211,3 @@ render_diff_hunk".to_owned(),
        old_start: 200,
        old_count: 2,
        new_start: 211,
        new_count: 3,
        lines: vec![
            DiffLine {
                kind: DiffLineKind::Context,
                old_line: Some(200),
                new_line: Some(211),
                text: "context".to_owned(),
            },
            DiffLine {
                kind: DiffLineKind::Deletion,
                old_line: Some(201),
                new_line: None,
                text: "old".to_owned(),
            },
            DiffLine {
                kind: DiffLineKind::Addition,
                old_line: None,
                new_line: Some(212),
                text: "new".to_owned(),
            },
            DiffLine {
                kind: DiffLineKind::Addition,
                old_line: None,
                new_line: Some(213),
                text: "again".to_owned(),
            },
        ],
    };

    let theme = DiffTheme::default();
    let text = line_text(&Line::from(hunk_header_spans(
        &hunk,
        48,
        theme,
        line_gutter_bg(DiffLineKind::Meta, theme),
    )));

    assert_eq!(text.width(), 48);
    assert!(text.starts_with("@@ -200,2 +211,3 @@ render_diff_hunk"));
    assert!(text.ends_with("+2 -1"));
}

#[test]
fn hunk_header_line_matches_unified_gutter() {
    let hunk = hz_diff::DiffHunk {
        header: "@@ -200,2 +211,3 @@ render_diff_hunk".to_owned(),
        old_start: 200,
        old_count: 2,
        new_start: 211,
        new_count: 3,
        lines: vec![DiffLine {
            kind: DiffLineKind::Addition,
            old_line: None,
            new_line: Some(211),
            text: "new".to_owned(),
        }],
    };

    let theme = DiffTheme::default();
    let line = hunk_header_line(&hunk, 64, theme);
    let text = line_text(&line);

    assert_eq!(text.width(), 64);
    assert!(text.starts_with(&format!("{DIFF_INDICATOR} @@ -200,2 +211,3 @@")));
    assert!(text.contains("@@ -200,2 +211,3 @@ render_diff_hunk"));
    assert!(text.ends_with("+1"));
    let old_range = line
        .spans
        .iter()
        .find(|span| span.content.as_ref() == "-200,2")
        .expect("old range should render as a separate span");
    let new_range = line
        .spans
        .iter()
        .find(|span| span.content.as_ref() == "+211,3")
        .expect("new range should render as a separate span");
    let context = line
        .spans
        .iter()
        .find(|span| span.content.as_ref().contains("render_diff_hunk"))
        .expect("context should render as a separate span");
    let additions = line
        .spans
        .iter()
        .find(|span| span.content.as_ref() == "+1")
        .expect("additions should render as a separate span");

    assert_eq!(old_range.style.fg, Some(theme.deletion_fg));
    assert_eq!(new_range.style.fg, Some(theme.addition_fg));
    assert_eq!(context.style.fg, Some(theme.foreground));
    assert_eq!(additions.style.fg, Some(theme.addition_fg));
    assert!(
        line.spans
            .iter()
            .all(|span| span.style.bg == Some(line_gutter_bg(DiffLineKind::Meta, theme)))
    );
}

#[test]
fn hunk_header_truncates_context_before_delta() {
    let hunk = hz_diff::DiffHunk {
        header: "@@ -1 +1 @@ render_diff_hunk_with_a_really_long_name".to_owned(),
        old_start: 1,
        old_count: 1,
        new_start: 1,
        new_count: 1,
        lines: vec![
            DiffLine {
                kind: DiffLineKind::Deletion,
                old_line: Some(1),
                new_line: None,
                text: "old".to_owned(),
            },
            DiffLine {
                kind: DiffLineKind::Addition,
                old_line: None,
                new_line: Some(1),
                text: "new".to_owned(),
            },
        ],
    };

    let theme = DiffTheme::default();
    let text = line_text(&Line::from(hunk_header_spans(
        &hunk,
        32,
        theme,
        line_gutter_bg(DiffLineKind::Meta, theme),
    )));

    assert_eq!(text.width(), 32);
    assert!(text.starts_with("@@ -1 +1 @@ render"));
    assert!(text.contains("..."));
    assert!(text.ends_with("+1 -1"));
}

#[test]
fn hunk_header_truncates_location_without_collapsing_range_styles() {
    let hunk = hz_diff::DiffHunk {
        header: "@@ -200,2 +211,3 @@ render_diff_hunk".to_owned(),
        old_start: 200,
        old_count: 2,
        new_start: 211,
        new_count: 3,
        lines: vec![DiffLine {
            kind: DiffLineKind::Context,
            old_line: Some(200),
            new_line: Some(211),
            text: "context".to_owned(),
        }],
    };

    let theme = DiffTheme::default();
    let line = Line::from(hunk_header_spans(
        &hunk,
        17,
        theme,
        line_gutter_bg(DiffLineKind::Meta, theme),
    ));
    let text = line_text(&line);

    assert_eq!(text, "@@ -200,2 +211...");
    assert_eq!(text.width(), 17);

    let old_range = line
        .spans
        .iter()
        .find(|span| span.content.as_ref() == "-200,2")
        .expect("old range should keep its own span when truncated");
    let new_range = line
        .spans
        .iter()
        .find(|span| span.content.as_ref() == "+211")
        .expect("new range should keep its own span when truncated");

    assert_eq!(old_range.style.fg, Some(theme.deletion_fg));
    assert_eq!(new_range.style.fg, Some(theme.addition_fg));
}

#[test]
fn content_spans_fall_back_when_syntax_text_mismatches_diff_text() {
    let syntax = HighlightedLine {
        segments: vec![hz_syntax::SyntaxSegment {
            byte_start: 0,
            byte_end: 5,
            text: "wrong".to_owned(),
            class: Some(SyntaxClass::Keyword),
        }],
    };

    let spans = content_spans_at_scroll(
        "right",
        Some(&syntax),
        &[],
        DiffLineKind::Addition,
        8,
        DiffTheme::default(),
        0,
    );
    let text = spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();

    assert_eq!(text, "right   ");
    assert_eq!(spans.len(), 1);
}

#[test]
fn empty_diff_fill_draws_shifted_diagonal_pattern() {
    assert_eq!(empty_diff_fill_from(8, 0, 0), "╱  ╱  ╱ ");
    assert_eq!(empty_diff_fill_from(8, 1, 0), "  ╱  ╱  ");
    assert_eq!(empty_diff_fill_from(8, 2, 0), " ╱  ╱  ╱");
}

#[test]
fn split_empty_cells_use_default_gutter_and_hatched_fill() {
    let spans = split_cell_spans_at_scroll(
        None,
        None,
        &[],
        SplitCellRender {
            side: SplitSide::Old,
            row_index: 0,
            width: 12,
            theme: DiffTheme::default(),
        },
        0,
    );

    assert_eq!(span_text(&spans), "▌        ╱  ");
    assert_eq!(spans[0].content.as_ref(), DIFF_INDICATOR);
    assert_eq!(spans[0].style.fg, Some(DiffTheme::default().muted));
    assert_eq!(spans[0].style.bg, Some(DiffTheme::default().gutter_bg));
    assert_eq!(spans[1].content.as_ref(), "       ");
    assert_eq!(spans[1].style.bg, Some(DiffTheme::default().gutter_bg));
    assert_eq!(spans[2].style.fg, Some(DiffTheme::default().empty_diff));
}

#[test]
fn line_gutters_use_theme_background() {
    let theme = DiffTheme::default();
    let line = DiffLine {
        kind: DiffLineKind::Context,
        old_line: Some(7),
        new_line: Some(7),
        text: "same".to_owned(),
    };

    let rendered = render_unified_line_at_scroll(&line, None, &[], 0, 24, theme, 0);

    assert_eq!(rendered.spans[0].style.fg, Some(theme.muted));
    assert_eq!(rendered.spans[0].style.bg, Some(theme.gutter_bg));
    assert_eq!(rendered.spans[1].style.fg, Some(theme.foreground));
    assert_eq!(rendered.spans[1].style.bg, Some(theme.gutter_bg));
}

#[test]
fn changed_line_gutters_use_delta_colors_and_bold_signs() {
    let theme = DiffTheme::default();
    let line = DiffLine {
        kind: DiffLineKind::Addition,
        old_line: None,
        new_line: Some(7),
        text: "added".to_owned(),
    };

    let rendered = render_unified_line_at_scroll(&line, None, &[], 0, 24, theme, 0);

    assert_eq!(rendered.spans[0].style.bg, Some(theme.addition_gutter_bg));
    assert_eq!(rendered.spans[1].style.fg, Some(theme.addition_fg));
    assert_eq!(rendered.spans[1].style.bg, Some(theme.addition_gutter_bg));
    assert_eq!(rendered.spans[2].content.as_ref(), "+");
    assert_eq!(rendered.spans[2].style.fg, Some(theme.addition_fg));
    assert_eq!(rendered.spans[2].style.bg, Some(theme.addition_gutter_bg));
    assert!(
        rendered.spans[2]
            .style
            .add_modifier
            .contains(Modifier::BOLD)
    );
    assert_eq!(rendered.spans[3].style.fg, Some(theme.foreground));
    assert_eq!(rendered.spans[3].style.bg, Some(theme.addition_bg));
}

#[test]
fn split_view_uses_right_indicator_as_separator() {
    let changeset = changeset_with_context_lines(1);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Split);

    let rendered = render_split_line_with_focus(
        &mut app,
        SplitLineRender {
            file: 0,
            hunk: 0,
            left: Some(0),
            right: Some(0),
            row_index: 0,
            width: 24,
            focused: false,
        },
    );
    let text = line_text(&rendered);

    assert!(!text.contains('│'));
    assert_eq!(text.chars().nth(12), Some('▌'));
}

#[test]
fn unified_diff_content_scrolls_horizontally() {
    let line = DiffLine {
        kind: DiffLineKind::Context,
        old_line: Some(1),
        new_line: Some(1),
        text: "abcdef".to_owned(),
    };

    let rendered = render_unified_line_at_scroll(&line, None, &[], 0, 18, DiffTheme::default(), 2);

    assert!(line_text(&rendered).ends_with("cdef"));
}

#[test]
fn split_diff_content_scrolls_horizontally() {
    let changeset = changeset_with_line_text("abcdef");
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Split);
    app.horizontal_scroll = 2;

    let rendered = render_split_line_with_focus(
        &mut app,
        SplitLineRender {
            file: 0,
            hunk: 0,
            left: Some(0),
            right: Some(0),
            row_index: 0,
            width: 24,
            focused: false,
        },
    );

    assert_eq!(line_text(&rendered), "▌    1  cdef▌    1  cdef");
}

#[test]
fn diff_lines_start_with_change_indicator() {
    let line = DiffLine {
        kind: DiffLineKind::Addition,
        old_line: None,
        new_line: Some(3),
        text: "new".to_owned(),
    };

    let rendered = render_unified_line_at_scroll(&line, None, &[], 0, 24, DiffTheme::default(), 0);

    assert_eq!(rendered.spans[0].content.as_ref(), DIFF_INDICATOR);
    assert_eq!(
        rendered.spans[0].style.fg,
        Some(DiffTheme::default().addition_fg)
    );
    assert!(!line_text(&rendered).contains(EMPTY_DIFF_FILL));
}

#[test]
fn focused_hunk_highlights_diff_indicators() {
    let changeset = changeset_with_context_lines(1);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    let theme = app.theme;

    let header = render_row_with_focus(
        &mut app,
        1,
        UiRow::HunkHeader { file: 0, hunk: 0 },
        24,
        Some((0, 0)),
    );
    assert_eq!(header.spans[0].style.fg, Some(theme.hunk));
    assert!(header.spans[0].style.add_modifier.contains(Modifier::BOLD));

    let row = app.model.row(2).expect("diff line should be visible");
    let focused = render_row_with_focus(&mut app, 2, row, 24, Some((0, 0)));
    let unfocused = render_row_with_focus(&mut app, 2, row, 24, Some((0, 1)));

    assert_eq!(focused.spans[0].style.fg, Some(theme.hunk));
    assert!(focused.spans[0].style.add_modifier.contains(Modifier::BOLD));
    assert_eq!(unfocused.spans[0].style.fg, Some(theme.muted));
    assert!(
        !unfocused.spans[0]
            .style
            .add_modifier
            .contains(Modifier::BOLD)
    );
}

#[test]
fn ansi_theme_uses_terminal_palette_indices() {
    let theme = diff_theme_from_config(&SyntaxThemeConfig {
        source: SyntaxThemeSource::Ansi,
        name: None,
        path: None,
    })
    .expect("ansi theme should load");

    assert_eq!(theme.addition_fg, Color::Indexed(2));
    assert_eq!(
        theme.syntax.color(SyntaxClass::Keyword),
        Some(Color::Indexed(13))
    );
}

#[test]
fn system_theme_preserves_terminal_base_and_uses_owned_diff_colors() {
    let theme = builtin_diff_theme(Some("system")).expect("system theme should load");

    assert_eq!(theme.foreground, Color::Reset);
    assert_eq!(theme.background, Color::Reset);
    assert_eq!(theme.file, Color::Reset);
    assert_ne!(theme.addition_fg, Color::Indexed(2));
    assert_ne!(theme.deletion_fg, Color::Indexed(1));
    assert_eq!(row_bg(DiffLineKind::Addition, theme), theme.addition_bg);
    assert_eq!(
        inline_bg(DiffLineKind::Addition, theme),
        theme.addition_inline_bg
    );
    assert_eq!(
        line_gutter_bg(DiffLineKind::Addition, theme),
        theme.addition_gutter_bg
    );
    assert_eq!(
        theme.syntax.color(SyntaxClass::String),
        SyntaxPalette::ansi().color(SyntaxClass::String)
    );
}

#[test]
fn default_theme_alias_uses_system_theme() {
    let theme = builtin_diff_theme(Some("default")).expect("default theme should load");

    assert_eq!(theme, DiffTheme::system());
}

#[test]
fn color_overrides_layer_on_colorscheme() {
    let theme = DiffTheme::system()
        .with_color_overrides(&ColorOverrides {
            bg: Some("#010203".to_owned()),
            addition_bg: Some("#123456".to_owned()),
            deletion_fg: Some("bright-red".to_owned()),
            search_match_fg: Some("#112233".to_owned()),
            search_match_bg: Some("#223344".to_owned()),
            keyword: Some("ansi-13".to_owned()),
            ..ColorOverrides::default()
        })
        .expect("color overrides should parse");

    assert_eq!(theme.background, Color::Rgb(1, 2, 3));
    assert_eq!(
        row_bg(DiffLineKind::Addition, theme),
        Color::Rgb(0x12, 0x34, 0x56)
    );
    assert_eq!(theme.deletion_fg, Color::LightRed);
    assert_eq!(theme.search_match_fg, Color::Rgb(0x11, 0x22, 0x33));
    assert_eq!(theme.search_match_bg, Color::Rgb(0x22, 0x33, 0x44));
    assert_eq!(
        theme.syntax.color(SyntaxClass::Keyword),
        Some(Color::Indexed(13))
    );
}

#[test]
fn packaged_popular_themes_are_available() {
    for name in ["catppuccin-mocha", "gruvbox-dark", "tokyonight", "dracula"] {
        let theme = builtin_diff_theme(Some(name)).expect("built-in theme should load");

        assert_ne!(
            theme.file,
            Color::Reset,
            "{name} should set file foreground"
        );
        assert!(
            theme.syntax.color(SyntaxClass::Keyword).is_some(),
            "{name} should set syntax keyword foreground"
        );
    }
}

#[test]
fn transparent_background_resets_diff_and_inline_backgrounds() {
    let theme = DiffTheme::catppuccin_mocha().with_transparent_background(true);
    let spans = content_spans_at_scroll(
        "changed",
        None,
        &[InlineRange {
            byte_start: 0,
            byte_end: 7,
        }],
        DiffLineKind::Addition,
        8,
        theme,
        0,
    );

    assert_eq!(row_bg(DiffLineKind::Addition, theme), Color::Reset);
    assert_eq!(spans[0].style.bg, Some(Color::Reset));
    assert!(spans[0].style.add_modifier.contains(Modifier::BOLD));
}

#[test]
fn base16_theme_parser_accepts_yaml_or_toml_lines() {
    let scheme = parse_base16_scheme(
        r##"
base00: "#000000"
base01: "111111"
base02: "222222"
base03: "333333"
base04 = "444444"
base05 = "555555"
base06 = "666666"
base07 = "777777"
base08 = "888888"
base09 = "999999"
base0A = "aaaaaa"
base0B = "bbbbbb"
base0C = "cccccc"
base0D = "dddddd"
base0E = "eeeeee"
base0F = "ffffff"
"##,
    )
    .expect("base16 scheme should parse");
    let theme = DiffTheme::base16(scheme);

    assert_eq!(theme.muted, Color::Rgb(51, 51, 51));
    assert_eq!(
        theme.syntax.color(SyntaxClass::String),
        Some(Color::Rgb(187, 187, 187))
    );
}

#[test]
fn inline_emphasis_marks_changed_tokens_in_paired_lines() {
    let lines = vec![
        DiffLine {
            kind: DiffLineKind::Deletion,
            old_line: Some(1),
            new_line: None,
            text: "let count = 1;".to_owned(),
        },
        DiffLine {
            kind: DiffLineKind::Addition,
            old_line: None,
            new_line: Some(1),
            text: "let total = 2;".to_owned(),
        },
    ];

    let emphasis = compute_hunk_inline_emphasis(&lines);

    assert_eq!(
        range_texts(&lines[0].text, &emphasis[0].ranges),
        ["count", "1"]
    );
    assert_eq!(
        range_texts(&lines[1].text, &emphasis[1].ranges),
        ["total", "2"]
    );
}

#[test]
fn inline_emphasis_leaves_unpaired_changed_lines_to_line_style() {
    let lines = vec![DiffLine {
        kind: DiffLineKind::Deletion,
        old_line: Some(1),
        new_line: None,
        text: "removed line".to_owned(),
    }];

    let emphasis = compute_hunk_inline_emphasis(&lines);

    assert!(emphasis[0].ranges.is_empty());
}

#[test]
fn lazy_inline_emphasis_matches_eager_emphasis() {
    let lines = vec![
        DiffLine {
            kind: DiffLineKind::Deletion,
            old_line: Some(1),
            new_line: None,
            text: "let count = 1;".to_owned(),
        },
        DiffLine {
            kind: DiffLineKind::Addition,
            old_line: None,
            new_line: Some(1),
            text: "let total = 2;".to_owned(),
        },
        DiffLine {
            kind: DiffLineKind::Deletion,
            old_line: Some(2),
            new_line: None,
            text: "removed only".to_owned(),
        },
        DiffLine {
            kind: DiffLineKind::Context,
            old_line: Some(3),
            new_line: Some(2),
            text: "context".to_owned(),
        },
        DiffLine {
            kind: DiffLineKind::Addition,
            old_line: None,
            new_line: Some(3),
            text: "added only".to_owned(),
        },
        DiffLine {
            kind: DiffLineKind::Deletion,
            old_line: Some(4),
            new_line: None,
            text: "alpha beta".to_owned(),
        },
        DiffLine {
            kind: DiffLineKind::Deletion,
            old_line: Some(5),
            new_line: None,
            text: "gamma".to_owned(),
        },
        DiffLine {
            kind: DiffLineKind::Addition,
            old_line: None,
            new_line: Some(4),
            text: "alpha zeta".to_owned(),
        },
        DiffLine {
            kind: DiffLineKind::Addition,
            old_line: None,
            new_line: Some(5),
            text: "delta".to_owned(),
        },
    ];
    let expected = compute_hunk_inline_emphasis(&lines);
    let mut cache = InlineHunkEmphasisCache::new(&lines);

    for index in [7, 5, 2, 4, 0, 1, 8, 6, 3] {
        assert_eq!(
            cache.ranges_for_line(&lines, index),
            expected[index].ranges,
            "lazy emphasis should match eager emphasis for line {index}"
        );
        assert_eq!(
            cache.ranges_for_line(&lines, index),
            expected[index].ranges,
            "cached emphasis should be stable for line {index}"
        );
    }
}

#[test]
fn inline_ascii_tokenizer_treats_vertical_tab_as_whitespace() {
    let tokens = inline_tokens("a \x0B\tb");

    assert_eq!(tokens.len(), 3);
    assert_eq!(tokens[1].byte_start, 1);
    assert_eq!(tokens[1].byte_end, 4);
    assert_eq!(inline_ascii_class(0x0B), InlineCharClass::Whitespace);
}

#[test]
fn inline_diff_skips_expensive_long_line_pairs() {
    let lines = vec![
        DiffLine {
            kind: DiffLineKind::Deletion,
            old_line: Some(1),
            new_line: None,
            text: "a".repeat(MAX_INLINE_DIFF_LINE_BYTES + 1),
        },
        DiffLine {
            kind: DiffLineKind::Addition,
            old_line: None,
            new_line: Some(1),
            text: "b".repeat(MAX_INLINE_DIFF_LINE_BYTES + 1),
        },
    ];

    let emphasis = compute_hunk_inline_emphasis(&lines);

    assert!(emphasis[0].ranges.is_empty());
    assert!(emphasis[1].ranges.is_empty());
}

#[test]
fn content_spans_layers_inline_emphasis_over_syntax() {
    let text = "let value = 2;";
    let number_start = text.find('2').unwrap();
    let syntax = HighlightedLine {
        segments: vec![
            hz_syntax::SyntaxSegment {
                byte_start: 0,
                byte_end: 12,
                text: "let value = ".to_owned(),
                class: Some(SyntaxClass::Keyword),
            },
            hz_syntax::SyntaxSegment {
                byte_start: 12,
                byte_end: 13,
                text: "2".to_owned(),
                class: Some(SyntaxClass::Number),
            },
            hz_syntax::SyntaxSegment {
                byte_start: 13,
                byte_end: 14,
                text: ";".to_owned(),
                class: Some(SyntaxClass::Punctuation),
            },
        ],
    };

    let spans = content_spans_at_scroll(
        text,
        Some(&syntax),
        &[InlineRange {
            byte_start: number_start,
            byte_end: number_start + 1,
        }],
        DiffLineKind::Addition,
        20,
        DiffTheme::default(),
        0,
    );
    let number = spans
        .iter()
        .find(|span| span.content.as_ref() == "2")
        .expect("number span should be split out for inline emphasis");

    assert_eq!(
        number.style.fg,
        syntax_fg(SyntaxClass::Number, DiffTheme::default())
    );
    assert_eq!(
        number.style.bg,
        Some(DiffTheme::default().addition_inline_bg)
    );
    assert!(number.style.add_modifier.contains(Modifier::BOLD));
}

#[test]
fn mouse_scroll_starts_precise_then_accelerates_sustained_bursts() {
    let start = Instant::now();
    let mut scroll = MouseScroll::default();

    assert_eq!(scroll.scroll_delta(MouseScrollDirection::Down, start), 1);

    let mut total = 1;
    for tick in 1..10 {
        total += scroll.scroll_delta(
            MouseScrollDirection::Down,
            start + Duration::from_millis(tick * 20),
        );
    }

    assert!(total > 10, "sustained wheel bursts should accelerate");
    assert!(
        total <= 30,
        "acceleration should stay capped at three rows per tick"
    );
}

#[test]
fn mouse_scroll_resets_after_pause_or_direction_change() {
    let start = Instant::now();
    let mut scroll = MouseScroll::default();

    assert_eq!(scroll.scroll_delta(MouseScrollDirection::Down, start), 1);
    assert!(
        scroll.scroll_delta(
            MouseScrollDirection::Down,
            start + Duration::from_millis(20)
        ) >= 1
    );
    assert_eq!(
        scroll.scroll_delta(
            MouseScrollDirection::Down,
            start + Duration::from_millis(400)
        ),
        1
    );
    assert_eq!(
        scroll.scroll_delta(MouseScrollDirection::Up, start + Duration::from_millis(420)),
        -1
    );
}

#[test]
fn highlight_cache_evicts_least_recently_used_entry() {
    let mut cache = LruCache::new(2);
    let first = syntax_key(0);
    let second = syntax_key(1);
    let third = syntax_key(2);

    cache.insert(first, 1);
    cache.insert(second, 2);
    assert_eq!(cache.get(&first), Some(&1));

    cache.insert(third, 3);

    assert_eq!(cache.get(&second), None);
    assert_eq!(cache.get(&first), Some(&1));
    assert_eq!(cache.get(&third), Some(&3));
}

#[test]
fn highlight_queue_runs_visible_jobs_before_prefetch_jobs() {
    let queue = SyntaxWorkerQueue::new(8, 0);
    let prefetch = syntax_key(1);
    let visible = syntax_key(2);

    queue
        .try_push(syntax_job(prefetch), SyntaxPriority::Prefetch)
        .unwrap();
    queue
        .try_push(syntax_job(visible), SyntaxPriority::Visible)
        .unwrap();

    assert_eq!(queue.try_pop().map(|job| job.key), Some(visible));
    assert_eq!(queue.try_pop().map(|job| job.key), Some(prefetch));
}

#[test]
fn visible_highlight_job_can_evict_prefetch_when_queue_is_full() {
    let queue = SyntaxWorkerQueue::new(1, 0);
    let prefetch = syntax_key(1);
    let visible = syntax_key(2);

    queue
        .try_push(syntax_job(prefetch), SyntaxPriority::Prefetch)
        .unwrap();
    let pushed = queue
        .try_push(syntax_job(visible), SyntaxPriority::Visible)
        .unwrap();

    assert_eq!(pushed.dropped, Some(prefetch));
    assert_eq!(queue.len(), 1);
    assert_eq!(queue.try_pop().map(|job| job.key), Some(visible));
}

#[test]
fn stale_highlight_jobs_are_dropped_on_generation_change() {
    let queue = SyntaxWorkerQueue::new(8, 0);

    queue
        .try_push(syntax_job(syntax_key(1)), SyntaxPriority::Prefetch)
        .unwrap();
    queue.set_generation(1);

    assert_eq!(queue.len(), 0);
    assert_eq!(
        queue.try_push(syntax_job(syntax_key(2)), SyntaxPriority::Visible),
        Err(SyntaxQueueError::Stale)
    );

    let fresh = syntax_key_with_generation(1, 0);
    queue
        .try_push(syntax_job(fresh), SyntaxPriority::Visible)
        .unwrap();
    assert_eq!(queue.try_pop().map(|job| job.key), Some(fresh));
}

#[test]
fn closed_queue_marks_full_file_source_skipped() {
    let queue = SyntaxWorkerQueue::new(8, 0);
    queue.close();
    let mut syntax = syntax_runtime_with_queue(queue);
    let source_id = SyntaxSourceId {
        generation: 0,
        file: 0,
        side: DiffSide::New,
        kind: SyntaxSourceKind::FullFile,
    };
    let key = SyntaxKey {
        source: source_id,
        language_hash: 1,
        theme_id: SYNTAX_THEME_ID,
    };

    assert!(!syntax.queue_job(
        key,
        "rust".to_owned(),
        full_file_syntax_job_source(),
        SyntaxPriority::Visible,
        None,
    ));

    assert!(syntax.skipped_sources.contains(&source_id));
    assert_eq!(syntax.stats.jobs_skipped, 1);
    assert_eq!(syntax.stats.jobs_rejected, 0);
}

#[test]
fn oversized_hunks_fall_back_to_plain_diff_text() {
    let limits = SyntaxLimits::default();
    let text = "x".repeat(limits.max_line_bytes);
    let line_count = (limits.max_source_bytes / limits.max_line_bytes) + 2;
    let lines = (0..line_count)
        .map(|index| DiffLine {
            kind: DiffLineKind::Context,
            old_line: Some(index + 1),
            new_line: Some(index + 1),
            text: text.clone(),
        })
        .collect::<Vec<_>>();

    assert_eq!(
        build_hunk_source(&lines, DiffSide::New, limits).unwrap_err(),
        SyntaxSkipReason::TooLarge
    );
}

#[test]
fn oversized_lines_disable_hunk_highlighting() {
    let limits = SyntaxLimits::default();
    let lines = vec![
        DiffLine {
            kind: DiffLineKind::Context,
            old_line: Some(1),
            new_line: Some(1),
            text: "x".repeat(limits.max_line_bytes + 1),
        },
        DiffLine {
            kind: DiffLineKind::Context,
            old_line: Some(2),
            new_line: Some(2),
            text: "let value = 1;".to_owned(),
        },
    ];

    assert_eq!(
        build_hunk_source(&lines, DiffSide::New, limits).unwrap_err(),
        SyntaxSkipReason::TooLarge
    );
}

#[test]
fn hunk_source_excludes_diff_meta_lines_and_preserves_empty_lines() {
    let lines = vec![
        DiffLine {
            kind: DiffLineKind::Context,
            old_line: Some(1),
            new_line: Some(1),
            text: "let a = 1;".to_owned(),
        },
        DiffLine {
            kind: DiffLineKind::Meta,
            old_line: None,
            new_line: None,
            text: "\\ No newline at end of file".to_owned(),
        },
        DiffLine {
            kind: DiffLineKind::Addition,
            old_line: None,
            new_line: Some(2),
            text: String::new(),
        },
    ];

    let source = build_hunk_source(&lines, DiffSide::New, SyntaxLimits::default()).unwrap();

    assert_eq!(source.text, "let a = 1;\n");
    assert_eq!(source.line_map, vec![Some(0), None, Some(1)]);
    assert_eq!(source.source_lines, 2);
}

#[test]
fn hunk_source_keeps_single_line_without_trailing_newline_marker() {
    let lines = vec![DiffLine {
        kind: DiffLineKind::Addition,
        old_line: None,
        new_line: Some(1),
        text: "let value = 1;".to_owned(),
    }];

    let source = build_hunk_source(&lines, DiffSide::New, SyntaxLimits::default()).unwrap();

    assert_eq!(source.text, "let value = 1;");
    assert_eq!(source.line_map, vec![Some(0)]);
    assert_eq!(source.source_lines, 1);
}

#[test]
fn hunk_source_preserves_leading_empty_lines() {
    let lines = vec![
        DiffLine {
            kind: DiffLineKind::Addition,
            old_line: None,
            new_line: Some(1),
            text: String::new(),
        },
        DiffLine {
            kind: DiffLineKind::Addition,
            old_line: None,
            new_line: Some(2),
            text: "let value = 1;".to_owned(),
        },
    ];

    let source = build_hunk_source(&lines, DiffSide::New, SyntaxLimits::default()).unwrap();

    assert_eq!(source.text, "\nlet value = 1;");
    assert_eq!(source.line_map, vec![Some(0), Some(1)]);
    assert_eq!(source.source_lines, 2);
}

#[test]
fn full_file_line_map_uses_absolute_line_numbers() {
    let lines = vec![
        DiffLine {
            kind: DiffLineKind::Deletion,
            old_line: Some(10),
            new_line: None,
            text: "old".to_owned(),
        },
        DiffLine {
            kind: DiffLineKind::Addition,
            old_line: None,
            new_line: Some(11),
            text: "new".to_owned(),
        },
        DiffLine {
            kind: DiffLineKind::Context,
            old_line: Some(12),
            new_line: Some(12),
            text: "same".to_owned(),
        },
    ];

    assert_eq!(
        build_full_file_line_map(&lines, DiffSide::Old).unwrap(),
        vec![Some(9), None, Some(11)]
    );
    assert_eq!(
        build_full_file_line_map(&lines, DiffSide::New).unwrap(),
        vec![None, Some(10), Some(11)]
    );
}

#[test]
fn full_file_sources_cover_diff_modes_and_statuses() {
    let repo = std::env::temp_dir();
    let file = hz_diff::DiffFile {
        old_path: Some("old.rs".to_owned()),
        new_path: Some("new.rs".to_owned()),
        status: hz_diff::FileStatus::Renamed,
        hunks: Vec::new(),
        additions: 0,
        deletions: 0,
        is_binary: false,
    };

    assert_eq!(
        full_file_source(&repo, &DiffOptions::default(), &file, DiffSide::Old)
            .unwrap()
            .kind,
        FullFileSourceKind::GitRevision {
            rev: "HEAD".to_owned(),
            path: "old.rs".to_owned(),
        }
    );
    assert_eq!(
        full_file_source(&repo, &DiffOptions::default(), &file, DiffSide::New)
            .unwrap()
            .kind,
        FullFileSourceKind::Worktree {
            path: "new.rs".to_owned(),
        }
    );

    let staged = DiffOptions {
        scope: DiffScope::Staged,
        ..DiffOptions::default()
    };
    assert_eq!(
        full_file_source(&repo, &staged, &file, DiffSide::New)
            .unwrap()
            .kind,
        FullFileSourceKind::GitIndex {
            path: "new.rs".to_owned(),
        }
    );

    let unstaged = DiffOptions {
        scope: DiffScope::Unstaged,
        ..DiffOptions::default()
    };
    assert_eq!(
        full_file_source(&repo, &unstaged, &file, DiffSide::Old)
            .unwrap()
            .kind,
        FullFileSourceKind::GitIndex {
            path: "old.rs".to_owned(),
        }
    );

    let base = DiffOptions {
        source: DiffSource::Base("main".to_owned()),
        ..DiffOptions::default()
    };
    assert_eq!(
        full_file_source(&repo, &base, &file, DiffSide::Old)
            .unwrap()
            .kind,
        FullFileSourceKind::GitMergeBase {
            base: "main".to_owned(),
            head: "HEAD".to_owned(),
            path: "old.rs".to_owned(),
        }
    );

    let range = DiffOptions {
        source: DiffSource::Range {
            left: "left".to_owned(),
            right: "right".to_owned(),
        },
        ..DiffOptions::default()
    };
    assert_eq!(
        full_file_source(&repo, &range, &file, DiffSide::New)
            .unwrap()
            .kind,
        FullFileSourceKind::GitRevision {
            rev: "right".to_owned(),
            path: "new.rs".to_owned(),
        }
    );

    let patch = DiffOptions {
        source: DiffSource::Patch(hz_diff::PatchSource::Stdin(Arc::from(&b""[..]))),
        ..DiffOptions::default()
    };
    assert!(full_file_source(&repo, &patch, &file, DiffSide::New).is_none());

    let deleted = hz_diff::DiffFile {
        new_path: None,
        status: hz_diff::FileStatus::Deleted,
        ..file.clone()
    };
    assert!(full_file_source(&repo, &DiffOptions::default(), &deleted, DiffSide::New).is_none());
}

#[test]
fn branch_full_file_source_uses_merge_base_and_head_revision() {
    let repo = std::env::temp_dir();
    let file = hz_diff::DiffFile {
        old_path: Some("old.rs".to_owned()),
        new_path: Some("new.rs".to_owned()),
        status: hz_diff::FileStatus::Modified,
        hunks: Vec::new(),
        additions: 0,
        deletions: 0,
        is_binary: false,
    };
    let base = "origin/main".to_owned();
    let head = "feature/full-file".to_owned();
    let branch = DiffOptions {
        source: DiffSource::Branch {
            base: base.clone(),
            head: head.clone(),
        },
        scope: DiffScope::All,
        ..DiffOptions::default()
    };

    assert_eq!(
        full_file_source(&repo, &branch, &file, DiffSide::Old)
            .unwrap()
            .kind,
        FullFileSourceKind::GitMergeBase {
            base,
            head: head.clone(),
            path: "old.rs".to_owned(),
        }
    );
    assert_eq!(
        full_file_source(&repo, &branch, &file, DiffSide::New)
            .unwrap()
            .kind,
        FullFileSourceKind::GitRevision {
            rev: head,
            path: "new.rs".to_owned(),
        }
    );
}

#[test]
fn full_file_source_loads_worktree_index_and_revision_contents() {
    let repo = temp_test_dir("full-file-source");
    fs::create_dir_all(&repo).expect("repo directory should be created");
    git(&repo, &["init", "-q"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "Test"]);

    fs::write(repo.join("file.rs"), "fn old() {}\n").expect("old file should be written");
    git(&repo, &["add", "file.rs"]);
    git(&repo, &["commit", "-q", "-m", "init"]);

    fs::write(repo.join("file.rs"), "fn new() {}\n").expect("new file should be written");
    assert_eq!(
        load_full_file_source(&FullFileSource {
            repo: repo.clone(),
            kind: FullFileSourceKind::GitRevision {
                rev: "HEAD".to_owned(),
                path: "file.rs".to_owned(),
            },
        })
        .unwrap(),
        "fn old() {}\n"
    );
    assert_eq!(
        load_full_file_source(&FullFileSource {
            repo: repo.clone(),
            kind: FullFileSourceKind::Worktree {
                path: "file.rs".to_owned(),
            },
        })
        .unwrap(),
        "fn new() {}\n"
    );

    git(&repo, &["add", "file.rs"]);
    assert_eq!(
        load_full_file_source(&FullFileSource {
            repo: repo.clone(),
            kind: FullFileSourceKind::GitIndex {
                path: "file.rs".to_owned(),
            },
        })
        .unwrap(),
        "fn new() {}\n"
    );

    fs::remove_dir_all(repo).expect("repo directory should be removed");
}

#[test]
fn git_full_file_helpers_do_not_treat_revisions_as_options() {
    let repo = temp_test_dir("git-option-boundary");
    fs::create_dir_all(&repo).expect("repo directory should be created");
    git(&repo, &["init", "-q"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "Test"]);
    fs::write(repo.join("file.rs"), "fn main() {}\n").expect("file should be written");
    git(&repo, &["add", "file.rs"]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    let output_path = repo.join("poc.txt");
    let output_arg = format!("--output={}", output_path.display());

    assert!(git_blob(&repo, &output_arg).is_err());
    assert!(!output_path.exists());
    assert!(git_merge_base(&repo, &output_arg, "HEAD").is_err());
    assert!(!output_path.exists());

    fs::remove_dir_all(repo).expect("repo directory should be removed");
}

#[test]
fn queue_close_wakes_blocked_pop() {
    let queue = SyntaxWorkerQueue::new(8, 0);
    let worker_queue = queue.clone();
    let worker = thread::spawn(move || worker_queue.pop());

    queue.close();

    assert!(worker.join().unwrap().is_none());
}

fn changeset_with_context_lines(line_count: usize) -> Changeset {
    changeset_with_context_lines_at(PathBuf::from("/repo"), 1, line_count)
}

fn changeset_with_context_lines_at(repo: PathBuf, start: usize, line_count: usize) -> Changeset {
    let lines = (1..=line_count)
        .map(|line| DiffLine {
            kind: DiffLineKind::Context,
            old_line: Some(start.saturating_add(line - 1)),
            new_line: Some(start.saturating_add(line - 1)),
            text: format!("line {line}"),
        })
        .collect();

    Changeset {
        repo,
        title: "test".to_owned(),
        files: vec![hz_diff::DiffFile {
            old_path: Some("file.rs".to_owned()),
            new_path: Some("file.rs".to_owned()),
            status: hz_diff::FileStatus::Modified,
            hunks: vec![hz_diff::DiffHunk {
                header: format!("@@ -{start} +{start} @@"),
                old_start: start,
                old_count: line_count,
                new_start: start,
                new_count: line_count,
                lines,
            }],
            additions: 0,
            deletions: 0,
            is_binary: false,
        }],
        raw_patch: Vec::new(),
    }
}

fn changeset_with_line_text(text: &str) -> Changeset {
    Changeset {
        repo: PathBuf::from("/repo"),
        title: "test".to_owned(),
        files: vec![hz_diff::DiffFile {
            old_path: Some("file.rs".to_owned()),
            new_path: Some("file.rs".to_owned()),
            status: hz_diff::FileStatus::Modified,
            hunks: vec![hz_diff::DiffHunk {
                header: "@@ -1 +1 @@".to_owned(),
                old_start: 1,
                old_count: 1,
                new_start: 1,
                new_count: 1,
                lines: vec![DiffLine {
                    kind: DiffLineKind::Context,
                    old_line: Some(1),
                    new_line: Some(1),
                    text: text.to_owned(),
                }],
            }],
            additions: 0,
            deletions: 0,
            is_binary: false,
        }],
        raw_patch: Vec::new(),
    }
}

fn changeset_with_hunk_at(repo: PathBuf, line_number: usize) -> Changeset {
    changeset_with_hunks_at(repo, &[line_number])
}

fn changeset_with_hunks_at(repo: PathBuf, line_numbers: &[usize]) -> Changeset {
    let hunks = line_numbers
        .iter()
        .map(|line_number| hz_diff::DiffHunk {
            header: format!("@@ -{line_number} +{line_number} @@"),
            old_start: *line_number,
            old_count: 1,
            new_start: *line_number,
            new_count: 1,
            lines: vec![DiffLine {
                kind: DiffLineKind::Context,
                old_line: Some(*line_number),
                new_line: Some(*line_number),
                text: format!("line {line_number}"),
            }],
        })
        .collect();

    Changeset {
        repo,
        title: "test".to_owned(),
        files: vec![hz_diff::DiffFile {
            old_path: Some("file.rs".to_owned()),
            new_path: Some("file.rs".to_owned()),
            status: hz_diff::FileStatus::Modified,
            hunks,
            additions: 0,
            deletions: 0,
            is_binary: false,
        }],
        raw_patch: Vec::new(),
    }
}

fn changeset_with_hunk_line_counts(repo: PathBuf, hunks: &[(usize, usize)]) -> Changeset {
    let hunks = hunks
        .iter()
        .map(|(line_number, line_count)| hz_diff::DiffHunk {
            header: format!("@@ -{line_number},{line_count} +{line_number},{line_count} @@"),
            old_start: *line_number,
            old_count: *line_count,
            new_start: *line_number,
            new_count: *line_count,
            lines: (0..*line_count)
                .map(|offset| DiffLine {
                    kind: DiffLineKind::Context,
                    old_line: Some(line_number + offset),
                    new_line: Some(line_number + offset),
                    text: format!("line {}", line_number + offset),
                })
                .collect(),
        })
        .collect();

    Changeset {
        repo,
        title: "test".to_owned(),
        files: vec![hz_diff::DiffFile {
            old_path: Some("file.rs".to_owned()),
            new_path: Some("file.rs".to_owned()),
            status: hz_diff::FileStatus::Modified,
            hunks,
            additions: 0,
            deletions: 0,
            is_binary: false,
        }],
        raw_patch: Vec::new(),
    }
}

fn changeset_with_files(paths: &[&str]) -> Changeset {
    let files = paths
        .iter()
        .enumerate()
        .map(|(index, path)| hz_diff::DiffFile {
            old_path: Some((*path).to_owned()),
            new_path: Some((*path).to_owned()),
            status: hz_diff::FileStatus::Modified,
            hunks: vec![hz_diff::DiffHunk {
                header: "@@ -1 +1 @@".to_owned(),
                old_start: 1,
                old_count: 1,
                new_start: 1,
                new_count: 1,
                lines: vec![DiffLine {
                    kind: DiffLineKind::Context,
                    old_line: Some(1),
                    new_line: Some(1),
                    text: format!("line {index}"),
                }],
            }],
            additions: index + 1,
            deletions: index,
            is_binary: false,
        })
        .collect();

    Changeset {
        repo: PathBuf::from("/repo"),
        title: "test".to_owned(),
        files,
        raw_patch: Vec::new(),
    }
}

fn syntax_key(file: usize) -> SyntaxKey {
    syntax_key_with_generation(0, file)
}

fn syntax_key_with_generation(generation: u64, file: usize) -> SyntaxKey {
    SyntaxKey {
        source: SyntaxSourceId {
            generation,
            file,
            side: DiffSide::New,
            kind: SyntaxSourceKind::HunkSide { hunk: 0 },
        },
        language_hash: 1,
        theme_id: SYNTAX_THEME_ID,
    }
}

fn syntax_job(key: SyntaxKey) -> SyntaxJob {
    SyntaxJob {
        key,
        language: "rust".to_owned(),
        source: SyntaxJobSource::Hunk(HunkSource {
            text: "fn main() {}".to_owned(),
            line_map: vec![Some(0)],
            source_lines: 1,
        }),
        limits: SyntaxLimits::default(),
    }
}

fn full_file_syntax_job_source() -> SyntaxJobSource {
    SyntaxJobSource::FullFile(FullFileSource {
        repo: PathBuf::from("/repo"),
        kind: FullFileSourceKind::Worktree {
            path: "file.rs".to_owned(),
        },
    })
}

fn syntax_runtime_with_queue(queue: SyntaxWorkerQueue) -> SyntaxRuntime {
    let (_result_tx, result_rx) = mpsc::channel();
    SyntaxRuntime {
        languages: SyntaxLanguageSet::from_enabled_languages(&[]),
        limits: SyntaxLimits::default(),
        result_rx,
        queue,
        cache: LruCache::new(8),
        pending: HashSet::new(),
        source_keys: HashMap::new(),
        position_keys: HashMap::new(),
        line_maps: HashMap::new(),
        skipped: HashMap::new(),
        skipped_sources: HashSet::new(),
        unavailable_full_files: HashSet::new(),
        failed: HashSet::new(),
        stats: SyntaxBenchmarkReport::default(),
        worker: None,
    }
}

fn range_texts(text: &str, ranges: &[InlineRange]) -> Vec<String> {
    ranges
        .iter()
        .map(|range| text[range.byte_start..range.byte_end].to_owned())
        .collect()
}

fn line_text(line: &Line<'_>) -> String {
    span_text(&line.spans)
}

fn visible_paths(app: &DiffApp) -> Vec<&str> {
    app.model
        .visible_files()
        .iter()
        .filter_map(|file| app.changeset.files.get(*file))
        .map(|file| file.display_path())
        .collect()
}

fn span_text(spans: &[Span<'_>]) -> String {
    spans.iter().map(|span| span.content.as_ref()).collect()
}

fn default_context_expand_step() -> usize {
    match DiffSettings::default().context_expansion {
        DiffContextExpansion::Lines(lines) => lines,
        DiffContextExpansion::Full => panic!("default context expansion should be bounded"),
    }
}

fn temp_test_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "hz-tui-{name}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos()
    ))
}

fn git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .expect("git should run");
    assert!(
        output.status.success(),
        "git failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
