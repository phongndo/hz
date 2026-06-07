use crate::{
    BRANCH_COMPARISON_SEPARATOR, BranchMenu, DIFF_INDICATOR, DiffApp, DiffChoice, DiffFilterKind,
    DiffLayoutMode, DiffSide, DiffTheme, EMPTY_DIFF_FILL, EMPTY_DIFF_FILL_SPACING,
    FILE_SIDEBAR_MAX_WIDTH, FILE_SIDEBAR_MIN_DIFF_WIDTH, FILE_SIDEBAR_MIN_WIDTH, GUTTER_WIDTH,
    HELP_KEY_COLUMN_WIDTH, HELP_MENU_COLUMN_GAP, HELP_MENU_HORIZONTAL_PADDING, HELP_MENU_LEFT_ROWS,
    HELP_MENU_RIGHT_ROWS, HELP_MENU_TWO_COLUMN_MIN_WIDTH, HELP_MENU_VERTICAL_PADDING,
    HELP_MENU_WIDTH, HelpMenuRow, InlineRange, STATUSLINE_ACCENT_BG, STATUSLINE_ACCENT_FG,
    STATUSLINE_BG, STATUSLINE_INFO_BG, STATUSLINE_INFO_FG, STATUSLINE_SELECTOR_GAP, TextMatcher,
    UNIFIED_GUTTER_WIDTH, UiRow, diff_line_grep_prefix, line_gutter_bg, line_gutter_fg,
    split_cell_content_width, unified_content_width, unified_syntax_side,
};
use hz_diff::{DiffLine, DiffLineKind, DiffOptions, DiffScope, DiffSource, FileStatus};
use hz_syntax::{DiffBackground, DiffSignStyle, HighlightedLine, SyntaxClass};
use ratatui::{
    Frame,
    layout::Rect,
    prelude::{Color, Line, Modifier, Span, Style, Text},
    widgets::{Block, BorderType, Clear, Padding, Paragraph},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub(crate) fn draw(frame: &mut Frame<'_>, app: &mut DiffApp) {
    let area = frame.area();
    if area.height == 0 {
        return;
    }

    let header_area = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: 1,
    };
    let filter_bar_height = u16::from(filter_bar_visible(app) && area.height > 1);
    let body_height = area
        .height
        .saturating_sub(1)
        .saturating_sub(filter_bar_height);
    let body_area = Rect {
        x: area.x,
        y: area.y.saturating_add(1),
        width: area.width,
        height: body_height,
    };
    let filter_bar_area = (filter_bar_height > 0).then_some(Rect {
        x: area.x,
        y: area.y.saturating_add(area.height.saturating_sub(1)),
        width: area.width,
        height: 1,
    });

    let sidebar_width = file_sidebar_width(app, body_area.width);
    app.file_sidebar_render_width = sidebar_width;
    let (sidebar_area, diff_area) = if sidebar_width > 0 {
        (
            Some(Rect {
                x: body_area.x,
                y: body_area.y,
                width: sidebar_width,
                height: body_area.height,
            }),
            Rect {
                x: body_area.x.saturating_add(sidebar_width),
                y: body_area.y,
                width: body_area.width.saturating_sub(sidebar_width),
                height: body_area.height,
            },
        )
    } else {
        (None, body_area)
    };

    app.set_viewport_rows(diff_area.height as usize);
    app.set_viewport_width(diff_area.width as usize);
    draw_header(frame, app, header_area);
    if let Some(sidebar_area) = sidebar_area {
        draw_file_sidebar(frame, app, sidebar_area);
    }
    draw_diff(frame, app, diff_area);
    if let Some(filter_bar_area) = filter_bar_area {
        draw_filter_bar(frame, app, filter_bar_area);
    }
    draw_diff_menu(frame, app, area);
    draw_branch_menu(frame, app, area);
    draw_help_menu(frame, app, area);
}

pub(crate) fn draw_header(frame: &mut Frame<'_>, app: &DiffApp, area: Rect) {
    let line = statusline_header_line(app, area.width as usize);
    frame.render_widget(
        Paragraph::new(line).style(Style::default().bg(statusline_bg(app.theme))),
        area,
    );
}

pub(crate) fn draw_filter_bar(frame: &mut Frame<'_>, app: &DiffApp, area: Rect) {
    let line = filter_bar_line(app, area.width as usize);
    frame.render_widget(
        Paragraph::new(line).style(Style::default().bg(statusline_bg(app.theme))),
        area,
    );
}

pub(crate) fn filter_bar_visible(app: &DiffApp) -> bool {
    app.filter_input.is_some() || app.filters_active()
}

pub(crate) fn filter_bar_line(app: &DiffApp, width: usize) -> Line<'static> {
    if width == 0 {
        return Line::default();
    }

    if !filter_bar_visible(app) {
        return Line::from(Span::styled(
            " ".repeat(width),
            Style::default().bg(statusline_bg(app.theme)),
        ));
    }

    let mut remaining = width;
    let mut spans = Vec::new();
    let bg = statusline_bg(app.theme);

    if app.filter_input == Some(DiffFilterKind::File) || !app.file_filter.is_empty() {
        push_file_filter_bar_spans(app, &mut spans, &mut remaining);
    }

    if app.filter_input == Some(DiffFilterKind::Grep) || !app.grep_filter.is_empty() {
        if !spans.is_empty() {
            push_filter_bar_span(&mut spans, "  ", Style::default().bg(bg), &mut remaining);
        }
        push_grep_filter_bar_spans(app, &mut spans, &mut remaining);
    }

    if remaining > 0 {
        spans.push(Span::styled(" ".repeat(remaining), Style::default().bg(bg)));
    }

    Line::from(spans)
}

pub(crate) fn push_file_filter_bar_spans(
    app: &DiffApp,
    spans: &mut Vec<Span<'static>>,
    remaining: &mut usize,
) {
    let bg = statusline_bg(app.theme);
    let query = filter_bar_query(app, DiffFilterKind::File);
    push_filter_bar_span(
        spans,
        "filter: ",
        Style::default()
            .fg(app.theme.foreground)
            .bg(bg)
            .add_modifier(Modifier::BOLD),
        remaining,
    );
    if query.is_empty() {
        push_filter_bar_span(
            spans,
            "type to filter files",
            Style::default().fg(app.theme.muted).bg(bg),
            remaining,
        );
    } else {
        push_filter_bar_span(
            spans,
            query,
            Style::default().fg(app.theme.foreground).bg(bg),
            remaining,
        );
    }
}

pub(crate) fn push_grep_filter_bar_spans(
    app: &DiffApp,
    spans: &mut Vec<Span<'static>>,
    remaining: &mut usize,
) {
    let bg = statusline_bg(app.theme);
    let query = filter_bar_query(app, DiffFilterKind::Grep);
    push_filter_bar_span(
        spans,
        "/",
        Style::default()
            .fg(app.theme.foreground)
            .bg(bg)
            .add_modifier(Modifier::BOLD),
        remaining,
    );
    if query.is_empty() {
        push_filter_bar_span(
            spans,
            " type to grep diff",
            Style::default().fg(app.theme.muted).bg(bg),
            remaining,
        );
    } else {
        push_filter_bar_span(
            spans,
            query,
            Style::default().fg(app.theme.foreground).bg(bg),
            remaining,
        );
    }
}

pub(crate) fn filter_bar_query(app: &DiffApp, kind: DiffFilterKind) -> &str {
    if app.filter_input == Some(kind) {
        app.filter_input_query(kind)
    } else {
        app.filter_query(kind)
    }
}

pub(crate) fn push_filter_bar_span(
    spans: &mut Vec<Span<'static>>,
    text: &str,
    style: Style,
    remaining: &mut usize,
) {
    if *remaining == 0 {
        return;
    }

    let text = fit(text, *remaining);
    if text.is_empty() {
        return;
    }

    *remaining = (*remaining).saturating_sub(text.width());
    spans.push(Span::styled(text, style));
}

pub(crate) fn statusline_header_line(app: &DiffApp, width: usize) -> Line<'static> {
    if width == 0 {
        return Line::default();
    }

    let right_max_width = statusline_right_max_width(width);
    let right = statusline_file_label(app, right_max_width);
    let right_width = right.width();
    let mut left_width = width.saturating_sub(right_width);
    let mut spans = Vec::new();

    push_statusline_left_spans(&mut spans, app, &mut left_width);
    let left_used = width.saturating_sub(right_width).saturating_sub(left_width);
    let gap = width.saturating_sub(left_used).saturating_sub(right_width);
    if gap > 0 {
        spans.push(Span::styled(
            " ".repeat(gap),
            Style::default().bg(statusline_bg(app.theme)),
        ));
    }
    if right_width > 0 {
        spans.push(Span::styled(
            right,
            Style::default()
                .fg(STATUSLINE_INFO_FG)
                .bg(STATUSLINE_INFO_BG)
                .add_modifier(Modifier::BOLD),
        ));
    }

    Line::from(spans)
}

pub(crate) fn push_statusline_left_spans(
    spans: &mut Vec<Span<'static>>,
    app: &DiffApp,
    remaining: &mut usize,
) {
    push_fitted_statusline_span(
        spans,
        diff_selector_text(&app.options),
        Style::default()
            .fg(STATUSLINE_ACCENT_FG)
            .bg(STATUSLINE_ACCENT_BG)
            .add_modifier(Modifier::BOLD),
        remaining,
    );
    push_fitted_statusline_span(
        spans,
        STATUSLINE_SELECTOR_GAP,
        Style::default().bg(statusline_bg(app.theme)),
        remaining,
    );
    if app.is_branch_diff()
        && let (Some(head), Some(base)) = (
            app.branch_selector_text(BranchMenu::Head),
            app.branch_selector_text(BranchMenu::Base),
        )
    {
        push_fitted_statusline_span(
            spans,
            head,
            Style::default()
                .fg(app.theme.header)
                .bg(statusline_bg(app.theme))
                .add_modifier(Modifier::BOLD),
            remaining,
        );
        push_fitted_statusline_span(
            spans,
            BRANCH_COMPARISON_SEPARATOR,
            Style::default()
                .fg(app.theme.muted)
                .bg(statusline_bg(app.theme)),
            remaining,
        );
        push_fitted_statusline_span(
            spans,
            base,
            Style::default()
                .fg(app.theme.header)
                .bg(statusline_bg(app.theme))
                .add_modifier(Modifier::BOLD),
            remaining,
        );
    } else {
        push_fitted_statusline_span(
            spans,
            diff_comparison_label(&app.options),
            Style::default()
                .fg(app.theme.muted)
                .bg(statusline_bg(app.theme)),
            remaining,
        );
    }
    push_fitted_statusline_span(
        spans,
        "  ",
        Style::default().bg(statusline_bg(app.theme)),
        remaining,
    );
    push_fitted_statusline_span(
        spans,
        statusline_file_count_label(app),
        Style::default()
            .fg(app.theme.foreground)
            .bg(statusline_bg(app.theme)),
        remaining,
    );
    push_fitted_statusline_span(
        spans,
        "  ",
        Style::default().bg(statusline_bg(app.theme)),
        remaining,
    );
    push_fitted_statusline_span(
        spans,
        format!("+{}", format_count(app.stats.additions)),
        Style::default()
            .fg(app.theme.addition_fg)
            .bg(statusline_bg(app.theme))
            .add_modifier(Modifier::BOLD),
        remaining,
    );
    push_fitted_statusline_span(
        spans,
        " ",
        Style::default().bg(statusline_bg(app.theme)),
        remaining,
    );
    push_fitted_statusline_span(
        spans,
        format!("-{}", format_count(app.stats.deletions)),
        Style::default()
            .fg(app.theme.deletion_fg)
            .bg(statusline_bg(app.theme))
            .add_modifier(Modifier::BOLD),
        remaining,
    );
    let notice = app
        .notice
        .as_ref()
        .map(|notice| notice.text.as_str())
        .unwrap_or_default();
    if !notice.is_empty() {
        push_fitted_statusline_span(
            spans,
            "  ",
            Style::default().bg(statusline_bg(app.theme)),
            remaining,
        );
        push_fitted_statusline_span(
            spans,
            notice,
            Style::default()
                .fg(app.theme.notice)
                .bg(statusline_bg(app.theme)),
            remaining,
        );
    }
}

pub(crate) fn statusline_file_count_label(app: &DiffApp) -> String {
    if app.filters_active() {
        format!(
            "{}/{} files",
            format_count(app.stats.files),
            format_count(app.total_stats.files)
        )
    } else {
        format!("{} files", format_count(app.stats.files))
    }
}

pub(crate) fn push_fitted_statusline_span(
    spans: &mut Vec<Span<'static>>,
    text: impl AsRef<str>,
    style: Style,
    remaining: &mut usize,
) {
    if *remaining == 0 {
        return;
    }

    let text = fit(text.as_ref(), *remaining);
    if text.is_empty() {
        return;
    }

    *remaining = (*remaining).saturating_sub(text.width());
    spans.push(Span::styled(text, style));
}

pub(crate) fn statusline_right_max_width(width: usize) -> usize {
    if width <= 24 {
        width
    } else {
        (width / 2).max(24).min(width)
    }
}

pub(crate) fn statusline_file_label(app: &DiffApp, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }

    let progress = progress_label(app.scroll, app.max_scroll());
    let file_count = app.model.visible_files().len();
    let file_number = app
        .model
        .visible_file_position(app.selected_file)
        .map(|position| position + 1)
        .unwrap_or_default();
    let position = format!("{file_number}/{file_count} {progress}");
    let fallback = "No file";
    let path = app
        .changeset
        .files
        .get(app.selected_file)
        .map(|file| file.display_path())
        .unwrap_or(fallback);

    let compact = format!(" {position} ");
    let compact_width = compact.width();
    if max_width <= compact_width {
        return fit(&compact, max_width);
    }

    let path_width = max_width.saturating_sub(position.width()).saturating_sub(3);
    let label = format!(" {} {} ", fit_with_ellipsis(path, path_width), position);
    if label.width() > max_width {
        fit(&label, max_width)
    } else {
        label
    }
}

pub(crate) fn draw_diff_menu(frame: &mut Frame<'_>, app: &DiffApp, area: Rect) {
    if !app.diff_menu_open || area.height <= 1 {
        return;
    }

    let choices = app.diff_menu_choices();
    if choices.is_empty() {
        return;
    }

    let width = diff_menu_width(&choices).min(area.width);
    let height = (choices.len() as u16).min(area.height - 1);
    if width == 0 || height == 0 {
        return;
    }

    let menu_area = Rect {
        x: area.x,
        y: area.y + 1,
        width,
        height,
    };
    let selected = diff_choice_from_options(&app.options);
    let lines: Vec<_> = choices
        .into_iter()
        .take(height as usize)
        .map(|choice| {
            let active = selected == Some(choice);
            let marker = if active { "✓" } else { " " };
            let text = fit_padded(&format!(" {marker} {}", choice.label()), width as usize);
            let style = if active {
                Style::default()
                    .fg(app.theme.header)
                    .bg(header_bg(app.theme))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
                    .fg(app.theme.foreground)
                    .bg(header_bg(app.theme))
            };
            Line::from(Span::styled(text, style))
        })
        .collect();

    frame.render_widget(Clear, menu_area);
    frame.render_widget(
        Paragraph::new(Text::from(lines)).style(Style::default().bg(header_bg(app.theme))),
        menu_area,
    );
}

pub(crate) fn draw_branch_menu(frame: &mut Frame<'_>, app: &DiffApp, area: Rect) {
    let Some(menu) = app.branch_menu_open else {
        return;
    };
    if area.height <= 1 || app.comparison_branches.is_empty() {
        return;
    }

    let x = app
        .branch_selector_start(menu)
        .unwrap_or_default()
        .min(area.width);
    let width = app.branch_menu_width().min(area.width.saturating_sub(x));
    let height = (app.branch_menu_height() as u16).min(area.height - 1);
    if width == 0 || height == 0 {
        return;
    }

    let menu_area = Rect {
        x: area.x + x,
        y: area.y + 1,
        width,
        height,
    };
    let selected = app.branch_ref(menu);
    let matches = app.filtered_branches();
    let lines: Vec<_> = if matches.is_empty() {
        vec![Line::from(Span::styled(
            fit_padded("   no matches", width as usize),
            Style::default()
                .fg(app.theme.muted)
                .bg(header_bg(app.theme)),
        ))]
    } else {
        matches
            .iter()
            .enumerate()
            .skip(app.branch_menu_scroll)
            .take(height as usize)
            .map(|(index, branch)| {
                let active = selected == Some(*branch);
                let highlighted = index == app.branch_menu_selected;
                let marker = if active {
                    "✓"
                } else if highlighted {
                    "›"
                } else {
                    " "
                };
                let branch_marker = app.branch_marker(menu, branch).unwrap_or(" ");
                let text = fit_padded(
                    &format!(" {marker} {branch_marker} {branch}"),
                    width as usize,
                );
                let mut style = if active || highlighted {
                    Style::default()
                        .fg(app.theme.header)
                        .bg(header_bg(app.theme))
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                        .fg(app.theme.foreground)
                        .bg(header_bg(app.theme))
                };
                if highlighted {
                    style = style.add_modifier(Modifier::REVERSED);
                }
                Line::from(Span::styled(text, style))
            })
            .collect()
    };

    frame.render_widget(Clear, menu_area);
    frame.render_widget(
        Paragraph::new(Text::from(lines)).style(Style::default().bg(header_bg(app.theme))),
        menu_area,
    );
}

pub(crate) fn draw_help_menu(frame: &mut Frame<'_>, app: &DiffApp, area: Rect) {
    if !app.help_menu_open || area.width < 4 || area.height < 3 {
        return;
    }

    let width = HELP_MENU_WIDTH.min(area.width);
    let content_width = width
        .saturating_sub(2)
        .saturating_sub(HELP_MENU_HORIZONTAL_PADDING.saturating_mul(2))
        as usize;
    let desired_height = (help_menu_content_rows(content_width) as u16)
        .saturating_add(2)
        .saturating_add(HELP_MENU_VERTICAL_PADDING.saturating_mul(2));
    let height = desired_height.min(area.height);
    if height == 0 {
        return;
    }

    let menu_area = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    };

    let block = help_menu_block(app.theme);
    let inner = block.inner(menu_area);

    frame.render_widget(Clear, menu_area);
    frame.render_widget(block, menu_area);
    frame.render_widget(
        Paragraph::new(Text::from(help_menu_lines(
            inner.width as usize,
            inner.height as usize,
            app.theme,
        )))
        .style(Style::default().bg(help_menu_bg(app.theme))),
        inner,
    );
}

pub(crate) fn help_menu_block(theme: DiffTheme) -> Block<'static> {
    let bg = help_menu_bg(theme);
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.muted).bg(bg))
        .style(Style::default().bg(bg))
        .padding(Padding::new(
            HELP_MENU_HORIZONTAL_PADDING,
            HELP_MENU_HORIZONTAL_PADDING,
            HELP_MENU_VERTICAL_PADDING,
            HELP_MENU_VERTICAL_PADDING,
        ))
        .title(Line::from(Span::styled(
            " keybindings ",
            Style::default()
                .fg(help_menu_title_color(theme))
                .bg(bg)
                .add_modifier(Modifier::BOLD),
        )))
}

pub(crate) fn help_menu_bg(theme: DiffTheme) -> Color {
    base_bg(theme)
}

pub(crate) fn help_menu_title_color(theme: DiffTheme) -> Color {
    theme.syntax.keyword.unwrap_or(theme.hunk)
}

pub(crate) fn help_menu_section_color(theme: DiffTheme) -> Color {
    theme.syntax.keyword.unwrap_or(theme.hunk)
}

pub(crate) fn help_menu_key_color(theme: DiffTheme) -> Color {
    theme.syntax.function.unwrap_or(theme.header)
}

pub(crate) fn help_menu_description_color(theme: DiffTheme) -> Color {
    theme.foreground
}

pub(crate) fn help_menu_lines(width: usize, height: usize, theme: DiffTheme) -> Vec<Line<'static>> {
    if help_menu_uses_two_columns(width) {
        return (0..height.min(help_menu_content_rows(width)))
            .map(|index| help_menu_columns_line(index, width, theme))
            .collect();
    }

    HELP_MENU_LEFT_ROWS
        .iter()
        .chain(HELP_MENU_RIGHT_ROWS)
        .take(height)
        .map(|row| Line::from(help_menu_row_spans(*row, width, theme)))
        .collect()
}

pub(crate) fn help_menu_columns_line(
    index: usize,
    width: usize,
    theme: DiffTheme,
) -> Line<'static> {
    let gap_width = HELP_MENU_COLUMN_GAP.min(width);
    let left_width = width.saturating_sub(gap_width) / 2;
    let right_width = width.saturating_sub(left_width).saturating_sub(gap_width);
    let bg = help_menu_bg(theme);

    let mut spans = help_menu_row_at(HELP_MENU_LEFT_ROWS, index)
        .map(|row| help_menu_row_spans(row, left_width, theme))
        .unwrap_or_else(|| help_menu_empty_spans(left_width, bg));
    spans.push(Span::styled(" ".repeat(gap_width), Style::default().bg(bg)));
    spans.extend(
        help_menu_row_at(HELP_MENU_RIGHT_ROWS, index)
            .map(|row| help_menu_row_spans(row, right_width, theme))
            .unwrap_or_else(|| help_menu_empty_spans(right_width, bg)),
    );

    Line::from(spans)
}

pub(crate) fn help_menu_row_at(rows: &[HelpMenuRow], index: usize) -> Option<HelpMenuRow> {
    rows.get(index).copied()
}

pub(crate) fn help_menu_content_rows(width: usize) -> usize {
    if help_menu_uses_two_columns(width) {
        HELP_MENU_LEFT_ROWS.len().max(HELP_MENU_RIGHT_ROWS.len())
    } else {
        HELP_MENU_LEFT_ROWS.len() + HELP_MENU_RIGHT_ROWS.len()
    }
}

pub(crate) fn help_menu_uses_two_columns(width: usize) -> bool {
    width >= HELP_MENU_TWO_COLUMN_MIN_WIDTH
}

pub(crate) fn help_menu_row_spans(
    row: HelpMenuRow,
    width: usize,
    theme: DiffTheme,
) -> Vec<Span<'static>> {
    let bg = help_menu_bg(theme);
    match row {
        HelpMenuRow::Section(section) => vec![Span::styled(
            fit_padded(&format!("  {section}"), width),
            Style::default()
                .fg(help_menu_section_color(theme))
                .bg(bg)
                .add_modifier(Modifier::BOLD),
        )],
        HelpMenuRow::Binding(keys, description) => {
            let key_width = HELP_KEY_COLUMN_WIDTH.min(width);
            let description_width = width.saturating_sub(key_width);
            vec![
                Span::styled(
                    fit_padded(&format!("  {keys}"), key_width),
                    Style::default()
                        .fg(help_menu_key_color(theme))
                        .bg(bg)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    fit_padded(description, description_width),
                    Style::default()
                        .fg(help_menu_description_color(theme))
                        .bg(bg),
                ),
            ]
        }
    }
}

pub(crate) fn help_menu_empty_spans(width: usize, bg: Color) -> Vec<Span<'static>> {
    vec![Span::styled(" ".repeat(width), Style::default().bg(bg))]
}

pub(crate) fn diff_selector_text(options: &DiffOptions) -> String {
    format!(" {} ", diff_type_label(options))
}

pub(crate) fn diff_selector_width(options: &DiffOptions) -> u16 {
    diff_selector_text(options).width() as u16
}

pub(crate) fn diff_type_label(options: &DiffOptions) -> &'static str {
    if let Some(choice) = diff_choice_from_options(options) {
        return choice.label();
    }

    match &options.source {
        DiffSource::Range { .. } => "Range",
        DiffSource::Patch(_) => "Patch",
        DiffSource::Worktree | DiffSource::Base(_) | DiffSource::Branch { .. } => "Diff",
    }
}

pub(crate) fn diff_choice_from_options(options: &DiffOptions) -> Option<DiffChoice> {
    match (&options.source, options.scope) {
        (DiffSource::Base(_) | DiffSource::Branch { .. }, DiffScope::All) => {
            Some(DiffChoice::Branch)
        }
        (DiffSource::Worktree, DiffScope::All) => Some(DiffChoice::All),
        (DiffSource::Worktree, DiffScope::Unstaged) => Some(DiffChoice::Unstaged),
        (DiffSource::Worktree, DiffScope::Staged) => Some(DiffChoice::Staged),
        _ => None,
    }
}

pub(crate) fn diff_comparison_label(options: &DiffOptions) -> String {
    match &options.source {
        DiffSource::Worktree => match options.scope {
            DiffScope::All => "HEAD → working tree".to_owned(),
            DiffScope::Staged => "HEAD → index".to_owned(),
            DiffScope::Unstaged => "index → working tree".to_owned(),
        },
        DiffSource::Base(base) => format!("HEAD → {base}"),
        DiffSource::Branch { base, head } => format!("{head} → {base}"),
        DiffSource::Range { left, right } => format!("{left} → {right}"),
        DiffSource::Patch(hz_diff::PatchSource::File(path)) => format!("patch {}", path.display()),
        DiffSource::Patch(hz_diff::PatchSource::Stdin(_)) => "patch stdin".to_owned(),
        DiffSource::Patch(hz_diff::PatchSource::Text { label, .. }) => label.clone(),
    }
}

pub(crate) fn diff_menu_width(choices: &[DiffChoice]) -> u16 {
    choices
        .iter()
        .map(|choice| choice.label().width() + 4)
        .max()
        .unwrap_or_default() as u16
}

pub(crate) fn branch_menu_width(branches: &[String]) -> u16 {
    branches
        .iter()
        .map(|branch| branch.width() + 6)
        .max()
        .unwrap_or_default() as u16
}

pub(crate) fn file_sidebar_width(app: &DiffApp, area_width: u16) -> u16 {
    if !app.file_sidebar_open {
        return 0;
    }

    let max_width = max_file_sidebar_width(area_width);
    if max_width == 0 {
        return 0;
    }

    app.file_sidebar_width
        .unwrap_or_else(|| file_sidebar_desired_width(app))
        .clamp(FILE_SIDEBAR_MIN_WIDTH, max_width)
}

pub(crate) fn max_file_sidebar_width(area_width: u16) -> u16 {
    let max_width = area_width.saturating_sub(FILE_SIDEBAR_MIN_DIFF_WIDTH);
    if max_width < FILE_SIDEBAR_MIN_WIDTH {
        0
    } else {
        max_width
    }
}

pub(crate) fn file_sidebar_desired_width(app: &DiffApp) -> u16 {
    let content_width = app
        .model
        .visible_files()
        .iter()
        .filter_map(|file| app.changeset.files.get(*file))
        .map(|file| {
            let stats = file_sidebar_stats(file);
            let stats_width = if stats.is_empty() {
                0
            } else {
                stats.width().saturating_add(2)
            };
            status_code(file.status)
                .width()
                .saturating_add(2)
                .saturating_add(file.display_path().width())
                .saturating_add(stats_width)
        })
        .max()
        .unwrap_or_else(|| " Files".width());
    let desired = content_width.saturating_add(1).min(usize::from(u16::MAX)) as u16;
    desired.clamp(FILE_SIDEBAR_MIN_WIDTH, FILE_SIDEBAR_MAX_WIDTH)
}

pub(crate) fn draw_file_sidebar(frame: &mut Frame<'_>, app: &mut DiffApp, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    app.clamp_file_sidebar_scroll(area.height as usize);
    frame.render_widget(
        Paragraph::new(Text::from(file_sidebar_lines(
            app,
            area.width as usize,
            area.height as usize,
        )))
        .style(Style::default().bg(base_bg(app.theme))),
        area,
    );
}

pub(crate) fn file_sidebar_lines(app: &DiffApp, width: usize, height: usize) -> Vec<Line<'static>> {
    if width == 0 || height == 0 {
        return Vec::new();
    }

    let theme = app.theme;
    let mut lines = Vec::with_capacity(height);
    let visible_files = height;
    let content_width = width.saturating_sub(1);
    for position in app.file_sidebar_scroll..app.file_sidebar_scroll.saturating_add(visible_files) {
        let Some(file_index) = app.model.visible_files().get(position).copied() else {
            lines.push(file_sidebar_line(
                "",
                Style::default().bg(base_bg(theme)),
                width,
                theme,
            ));
            continue;
        };
        let Some(file) = app.changeset.files.get(file_index) else {
            continue;
        };

        lines.push(file_sidebar_entry_line(
            file,
            file_index == app.selected_file,
            content_width,
            theme,
        ));
    }

    lines
}

pub(crate) fn file_sidebar_entry_line(
    file: &hz_diff::DiffFile,
    selected: bool,
    content_width: usize,
    theme: DiffTheme,
) -> Line<'static> {
    let width = content_width.saturating_add(1);
    if width == 0 {
        return Line::default();
    }

    let bg = if selected {
        header_bg(theme)
    } else {
        base_bg(theme)
    };
    let status_style = file_sidebar_status_style(file.status, bg, theme);
    let body_style = file_sidebar_body_style(selected, bg, theme);

    if file.is_binary || (file.additions == 0 && file.deletions == 0) {
        let stats = file_sidebar_stats(file);
        let stats_width = stats.width();
        let gap_width = usize::from(!stats.is_empty() && content_width > stats_width);
        let left_width = content_width
            .saturating_sub(stats_width)
            .saturating_sub(gap_width);
        let stats_width = content_width.saturating_sub(left_width + gap_width);

        let mut spans = file_sidebar_left_spans(file, left_width, status_style, body_style);
        if gap_width > 0 {
            spans.push(Span::styled(" ", body_style));
        }
        if stats_width > 0 {
            spans.push(Span::styled(fit(&stats, stats_width), body_style));
        }
        let used = spans_width(&spans);
        if used < content_width {
            spans.push(Span::styled(" ".repeat(content_width - used), body_style));
        }
        spans.push(file_sidebar_separator(theme));
        return Line::from(spans);
    }

    let additions = format!("+{}", file.additions);
    let deletions = format!("-{}", file.deletions);
    let stats_width = additions
        .width()
        .saturating_add(1)
        .saturating_add(deletions.width());
    let gap_width = usize::from(content_width > stats_width);
    let left_width = content_width
        .saturating_sub(stats_width)
        .saturating_sub(gap_width);

    let mut spans = Vec::new();
    if left_width > 0 {
        spans.extend(file_sidebar_left_spans(
            file,
            left_width,
            status_style,
            body_style,
        ));
    }
    if gap_width > 0 {
        spans.push(Span::styled(" ", body_style));
    }

    let mut remaining = content_width.saturating_sub(left_width + gap_width);
    push_sidebar_stat_span(
        &mut spans,
        &additions,
        sidebar_stat_style(theme.addition_fg, selected, bg),
        &mut remaining,
    );
    if remaining > 0 {
        spans.push(Span::styled(" ", body_style));
        remaining -= 1;
    }
    push_sidebar_stat_span(
        &mut spans,
        &deletions,
        sidebar_stat_style(theme.deletion_fg, selected, bg),
        &mut remaining,
    );
    if remaining > 0 {
        spans.push(Span::styled(" ".repeat(remaining), body_style));
    }
    spans.push(file_sidebar_separator(theme));

    Line::from(spans)
}

pub(crate) fn push_sidebar_stat_span(
    spans: &mut Vec<Span<'static>>,
    text: &str,
    style: Style,
    remaining: &mut usize,
) {
    if *remaining == 0 {
        return;
    }

    let text = fit(text, *remaining);
    if text.is_empty() {
        return;
    }

    *remaining = (*remaining).saturating_sub(text.width());
    spans.push(Span::styled(text, style));
}

pub(crate) fn spans_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(|span| span.content.as_ref().width()).sum()
}

pub(crate) fn sidebar_stat_style(color: Color, selected: bool, bg: Color) -> Style {
    let mut style = Style::default().fg(color).bg(bg);
    if selected {
        style = style.add_modifier(Modifier::BOLD);
    }
    style
}

pub(crate) fn file_sidebar_status_style(status: FileStatus, bg: Color, theme: DiffTheme) -> Style {
    file_sidebar_style(status, theme)
        .bg(bg)
        .add_modifier(Modifier::BOLD)
}

pub(crate) fn file_sidebar_body_style(selected: bool, bg: Color, theme: DiffTheme) -> Style {
    let mut style = Style::default()
        .fg(if selected {
            theme.header
        } else {
            theme.foreground
        })
        .bg(bg);
    if selected {
        style = style.add_modifier(Modifier::BOLD);
    }
    style
}

pub(crate) fn file_sidebar_left_spans(
    file: &hz_diff::DiffFile,
    width: usize,
    status_style: Style,
    body_style: Style,
) -> Vec<Span<'static>> {
    if width == 0 {
        return Vec::new();
    }

    let prefix = format!(" {} ", status_code(file.status));
    let prefix_width = prefix.width();
    if prefix_width >= width {
        return vec![Span::styled(fit_padded(&prefix, width), status_style)];
    }

    let path_width = width - prefix_width;
    vec![
        Span::styled(prefix, status_style),
        Span::styled(
            fit_padded(
                &fit_with_ellipsis(file.display_path(), path_width),
                path_width,
            ),
            body_style,
        ),
    ]
}

pub(crate) fn fit_with_ellipsis(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if text.width() <= width {
        return fit(text, width);
    }
    if width <= 3 {
        return fit("...", width);
    }

    format!("{}...", fit(text, width - 3))
}

pub(crate) fn file_sidebar_stats(file: &hz_diff::DiffFile) -> String {
    if file.is_binary {
        "binary".to_owned()
    } else if file.additions == 0 && file.deletions == 0 {
        String::new()
    } else {
        format!("+{} -{}", file.additions, file.deletions)
    }
}

pub(crate) fn file_sidebar_line(
    text: &str,
    style: Style,
    width: usize,
    theme: DiffTheme,
) -> Line<'static> {
    if width == 0 {
        return Line::default();
    }

    if width == 1 {
        return Line::from(file_sidebar_separator(theme));
    }

    Line::from(vec![
        Span::styled(fit_padded(text, width - 1), style),
        file_sidebar_separator(theme),
    ])
}

pub(crate) fn file_sidebar_separator(theme: DiffTheme) -> Span<'static> {
    Span::styled("│", Style::default().fg(theme.muted).bg(base_bg(theme)))
}

pub(crate) fn file_sidebar_style(status: FileStatus, theme: DiffTheme) -> Style {
    let color = match status {
        FileStatus::Added | FileStatus::Copied => theme.addition_fg,
        FileStatus::Deleted => theme.deletion_fg,
        FileStatus::Modified | FileStatus::Renamed | FileStatus::TypeChanged => theme.hunk,
        FileStatus::Unknown => theme.muted,
    };
    Style::default().fg(color)
}

pub(crate) fn format_count(count: usize) -> String {
    let digits = count.to_string();
    let mut formatted = String::with_capacity(digits.len() + digits.len() / 3);

    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            formatted.push(',');
        }
        formatted.push(digit);
    }

    formatted
}

pub(crate) fn draw_diff(frame: &mut Frame<'_>, app: &mut DiffApp, area: Rect) {
    if app.model.is_empty() {
        let message = if app.filters_active() && !app.base_changeset.files.is_empty() {
            "No files match filters."
        } else {
            "No changes."
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                message,
                Style::default().fg(app.theme.muted),
            )))
            .style(Style::default().bg(base_bg(app.theme))),
            area,
        );
        return;
    }

    let visible_rows = area.height as usize;
    if !app.syntax_updates_paused() {
        app.prepare_syntax_for_viewport(visible_rows);
    }

    let mut lines = Vec::with_capacity(visible_rows);
    for offset in 0..visible_rows {
        let Some(row) = app.model.row(app.scroll + offset) else {
            continue;
        };
        lines.push(render_row(
            app,
            app.scroll + offset,
            row,
            area.width as usize,
        ));
    }

    frame.render_widget(
        Paragraph::new(Text::from(lines)).style(Style::default().bg(base_bg(app.theme))),
        area,
    );
}

pub(crate) fn render_row(
    app: &mut DiffApp,
    row_index: usize,
    row: UiRow,
    width: usize,
) -> Line<'static> {
    let theme = app.theme;
    let horizontal_scroll = app.horizontal_scroll;
    let mut line = match row {
        UiRow::FileSeparator => file_separator_line(app.layout, width, theme),
        UiRow::FileHeader(file_index) => {
            let file = &app.changeset.files[file_index];
            file_header_line(file, width, theme)
        }
        UiRow::BinaryFile(file_index) => {
            let file = &app.changeset.files[file_index];
            let message = if file.is_binary {
                "binary file"
            } else {
                "no textual changes"
            };
            Line::from(Span::styled(
                fit_padded(&format!("  {message}"), width),
                Style::default().fg(theme.muted),
            ))
        }
        UiRow::Collapsed {
            lines, expanded, ..
        } => context_show_line(app.context_expand_count(lines), expanded > 0, width, theme),
        UiRow::ContextLine {
            file,
            old_line,
            new_line,
        } => render_context_line(app, file, old_line, new_line, row_index, width),
        UiRow::ContextHide { lines, .. } => context_hide_line(lines, width, theme),
        UiRow::HunkHeader { file, hunk } => {
            let hunk = &app.changeset.files[file].hunks[hunk];
            hunk_header_line(hunk, width, theme)
        }
        UiRow::UnifiedLine { file, hunk, line } => {
            let diff_line = app.changeset.files[file].hunks[hunk].lines[line].clone();
            let syntax = unified_syntax_side(diff_line.kind)
                .and_then(|side| app.syntax_line(file, hunk, line, side));
            let inline = app.inline_ranges(file, hunk, line);
            render_unified_line_at_scroll(
                &diff_line,
                syntax.as_ref(),
                &inline,
                row_index,
                width,
                theme,
                horizontal_scroll,
            )
        }
        UiRow::MetaLine { file, hunk, line } => {
            let diff_line = app.changeset.files[file].hunks[hunk].lines[line].clone();
            render_unified_line_at_scroll(
                &diff_line,
                None,
                &[],
                row_index,
                width,
                theme,
                horizontal_scroll,
            )
        }
        UiRow::SplitLine {
            file,
            hunk,
            left,
            right,
        } => render_split_line(app, file, hunk, left, right, row_index, width),
    };

    if !app.grep_filter.is_empty() {
        let targets = grep_highlight_targets_for_row(app, row, &line, width);
        line = highlighted_grep_text_line(line, &app.grep_filter, targets, theme);
    }
    line
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GrepHighlightTarget {
    pub(crate) text: String,
    pub(crate) spans: Vec<GrepHighlightSpan>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GrepHighlightSpan {
    pub(crate) span_index: usize,
    pub(crate) text_byte_start: usize,
    pub(crate) span_byte_start: usize,
    pub(crate) span_byte_end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SpanColumnPosition {
    pub(crate) span_index: usize,
    pub(crate) byte_index: usize,
}

pub(crate) fn highlighted_grep_text_line(
    line: Line<'static>,
    query: &str,
    targets: Vec<GrepHighlightTarget>,
    theme: DiffTheme,
) -> Line<'static> {
    let Some(matcher) = TextMatcher::new(query) else {
        return line;
    };
    if targets.is_empty() {
        return line;
    }

    let ranges_by_span = grep_highlight_ranges_by_span(line.spans.len(), &targets, &matcher);
    if ranges_by_span.iter().all(Vec::is_empty) {
        return line;
    }

    let mut spans = Vec::with_capacity(line.spans.len());
    for (index, span) in line.spans.into_iter().enumerate() {
        push_highlighted_grep_spans(&mut spans, span, &ranges_by_span[index], theme);
    }
    Line::from(spans)
}

pub(crate) fn grep_highlight_ranges_by_span(
    span_count: usize,
    targets: &[GrepHighlightTarget],
    matcher: &TextMatcher,
) -> Vec<Vec<std::ops::Range<usize>>> {
    let mut ranges_by_span = vec![Vec::new(); span_count];
    for target in targets {
        let match_ranges = matcher.match_ranges(&target.text);
        if match_ranges.is_empty() {
            continue;
        }

        for span in &target.spans {
            if span.span_index >= ranges_by_span.len() || span.span_byte_start >= span.span_byte_end
            {
                continue;
            }

            let span_text_end = span.text_byte_start + (span.span_byte_end - span.span_byte_start);
            for range in &match_ranges {
                let start = range.start.max(span.text_byte_start);
                let end = range.end.min(span_text_end);
                if start < end {
                    let local_start = span.span_byte_start + (start - span.text_byte_start);
                    let local_end = span.span_byte_start + (end - span.text_byte_start);
                    ranges_by_span[span.span_index].push(local_start..local_end);
                }
            }
        }
    }

    for ranges in &mut ranges_by_span {
        merge_ranges(ranges);
    }
    ranges_by_span
}

pub(crate) fn merge_ranges(ranges: &mut Vec<std::ops::Range<usize>>) {
    if ranges.len() <= 1 {
        return;
    }

    ranges.sort_by_key(|range| (range.start, range.end));
    let mut merged: Vec<std::ops::Range<usize>> = Vec::with_capacity(ranges.len());
    for range in ranges.drain(..) {
        if let Some(previous) = merged.last_mut()
            && range.start <= previous.end
        {
            previous.end = previous.end.max(range.end);
            continue;
        }
        merged.push(range);
    }
    *ranges = merged;
}

pub(crate) fn grep_highlight_targets_for_row(
    app: &DiffApp,
    row: UiRow,
    line: &Line<'_>,
    width: usize,
) -> Vec<GrepHighlightTarget> {
    match row {
        UiRow::FileHeader(file) => app
            .changeset
            .files
            .get(file)
            .and_then(|file| {
                grep_highlight_target_for_columns(
                    file.display_path().to_owned(),
                    &line.spans,
                    status_code(file.status).width().saturating_add(1),
                    width,
                    0,
                )
            })
            .into_iter()
            .collect(),
        UiRow::BinaryFile(file) => app
            .changeset
            .files
            .get(file)
            .and_then(|file| {
                let message = if file.is_binary {
                    "binary file"
                } else {
                    "no textual changes"
                };
                grep_highlight_target_for_columns(
                    message.to_owned(),
                    &line.spans,
                    2.min(width),
                    width,
                    0,
                )
            })
            .into_iter()
            .collect(),
        UiRow::HunkHeader { file, hunk } => app
            .changeset
            .files
            .get(file)
            .and_then(|file| file.hunks.get(hunk))
            .and_then(|hunk| {
                grep_highlight_target_for_columns(
                    hunk.header.clone(),
                    &line.spans,
                    2.min(width),
                    width,
                    0,
                )
            })
            .into_iter()
            .collect(),
        UiRow::UnifiedLine {
            file,
            hunk,
            line: line_index,
        }
        | UiRow::MetaLine {
            file,
            hunk,
            line: line_index,
        } => app
            .changeset
            .files
            .get(file)
            .and_then(|file| file.hunks.get(hunk))
            .and_then(|hunk| hunk.lines.get(line_index))
            .and_then(|diff_line| {
                let content_start = unified_content_start_column(width);
                grep_highlight_target_for_columns(
                    diff_line_grep_highlight_text(diff_line),
                    &line.spans,
                    content_start,
                    width,
                    diff_line_grep_rendered_text_byte_start(diff_line, app.horizontal_scroll),
                )
            })
            .into_iter()
            .collect(),
        UiRow::SplitLine {
            file,
            hunk,
            left,
            right,
        } => {
            let Some(hunk) = app
                .changeset
                .files
                .get(file)
                .and_then(|file| file.hunks.get(hunk))
            else {
                return Vec::new();
            };

            let left_width = width / 2;
            let right_width = width.saturating_sub(left_width);
            let mut targets = Vec::with_capacity(2);
            if let Some(target) =
                left.and_then(|index| hunk.lines.get(index))
                    .and_then(|diff_line| {
                        split_diff_line_grep_highlight_target(
                            diff_line,
                            &line.spans,
                            0,
                            left_width,
                            app.horizontal_scroll,
                        )
                    })
            {
                targets.push(target);
            }
            if let Some(target) =
                right
                    .and_then(|index| hunk.lines.get(index))
                    .and_then(|diff_line| {
                        split_diff_line_grep_highlight_target(
                            diff_line,
                            &line.spans,
                            left_width,
                            right_width,
                            app.horizontal_scroll,
                        )
                    })
            {
                targets.push(target);
            }
            targets
        }
        UiRow::FileSeparator
        | UiRow::Collapsed { .. }
        | UiRow::ContextLine { .. }
        | UiRow::ContextHide { .. } => Vec::new(),
    }
}

pub(crate) fn split_diff_line_grep_highlight_target(
    line: &DiffLine,
    spans: &[Span<'_>],
    cell_start: usize,
    cell_width: usize,
    horizontal_scroll: usize,
) -> Option<GrepHighlightTarget> {
    let content_start = cell_start.saturating_add(split_content_start_column(cell_width));
    let content_end = cell_start.saturating_add(cell_width);
    grep_highlight_target_for_columns(
        diff_line_grep_highlight_text(line),
        spans,
        content_start,
        content_end,
        diff_line_grep_rendered_text_byte_start(line, horizontal_scroll),
    )
}

pub(crate) fn unified_content_start_column(width: usize) -> usize {
    let indicator_width = 1.min(width);
    let gutter_width = UNIFIED_GUTTER_WIDTH.min(width.saturating_sub(indicator_width));
    indicator_width + gutter_width
}

pub(crate) fn split_content_start_column(width: usize) -> usize {
    let indicator_width = 1.min(width);
    let gutter_width = GUTTER_WIDTH.min(width.saturating_sub(indicator_width));
    indicator_width + gutter_width
}

pub(crate) fn diff_line_grep_highlight_text(line: &DiffLine) -> String {
    let mut text = String::with_capacity(line.text.len().saturating_add(1));
    text.push(diff_line_grep_prefix(line.kind));
    text.push_str(&line.text);
    text
}

pub(crate) fn diff_line_grep_rendered_text_byte_start(
    line: &DiffLine,
    horizontal_scroll: usize,
) -> usize {
    1 + scrolled_text_byte_start(&line.text, horizontal_scroll)
}

pub(crate) fn scrolled_text_byte_start(text: &str, horizontal_scroll: usize) -> usize {
    text.len() - skip_display_prefix(text, horizontal_scroll).0.len()
}

pub(crate) fn grep_highlight_target_for_columns(
    text: String,
    spans: &[Span<'_>],
    start_column: usize,
    end_column: usize,
    text_byte_start: usize,
) -> Option<GrepHighlightTarget> {
    if text.is_empty() || start_column >= end_column || text_byte_start >= text.len() {
        return None;
    }

    let start = span_position_for_column(spans, start_column);
    let end = span_position_for_column(spans, end_column);
    if start.span_index >= spans.len() {
        return None;
    }

    let mut target = GrepHighlightTarget {
        text,
        spans: Vec::new(),
    };
    let mut current_text_byte = text_byte_start;
    for (index, span) in spans.iter().enumerate().skip(start.span_index) {
        if current_text_byte >= target.text.len() || index > end.span_index {
            break;
        }

        let span_text = span.content.as_ref();
        let span_byte_start = if index == start.span_index {
            start.byte_index
        } else {
            0
        };
        let span_byte_end = if index == end.span_index {
            end.byte_index
        } else {
            span_text.len()
        };
        if span_byte_start >= span_byte_end {
            if index == end.span_index {
                break;
            }
            continue;
        }

        let rendered = &span_text[span_byte_start..span_byte_end];
        let matched_len = common_prefix_byte_len(rendered, &target.text[current_text_byte..]);
        if matched_len > 0 {
            target.spans.push(GrepHighlightSpan {
                span_index: index,
                text_byte_start: current_text_byte,
                span_byte_start,
                span_byte_end: span_byte_start + matched_len,
            });
            current_text_byte += matched_len;
        }
        if matched_len < rendered.len() || index == end.span_index {
            break;
        }
    }

    (!target.spans.is_empty()).then_some(target)
}

pub(crate) fn span_position_for_column(spans: &[Span<'_>], column: usize) -> SpanColumnPosition {
    let mut used = 0usize;
    for (span_index, span) in spans.iter().enumerate() {
        if column <= used {
            return SpanColumnPosition {
                span_index,
                byte_index: 0,
            };
        }

        let text = span.content.as_ref();
        let width = text.width();
        if column < used + width {
            let visible = skip_display_prefix(text, column - used).0;
            return SpanColumnPosition {
                span_index,
                byte_index: text.len() - visible.len(),
            };
        }

        used += width;
    }

    SpanColumnPosition {
        span_index: spans.len(),
        byte_index: 0,
    }
}

pub(crate) fn common_prefix_byte_len(left: &str, right: &str) -> usize {
    let mut len = 0usize;
    let mut right_chars = right.chars();
    for (index, left_char) in left.char_indices() {
        let Some(right_char) = right_chars.next() else {
            break;
        };
        if left_char != right_char {
            break;
        }
        len = index + left_char.len_utf8();
    }
    len
}

pub(crate) fn push_highlighted_grep_spans(
    spans: &mut Vec<Span<'static>>,
    span: Span<'static>,
    ranges: &[std::ops::Range<usize>],
    theme: DiffTheme,
) {
    let text = span.content.as_ref();
    if ranges.is_empty() {
        spans.push(span);
        return;
    }

    let mut start = 0;
    for range in ranges {
        if range.start >= range.end
            || range.end > text.len()
            || !text.is_char_boundary(range.start)
            || !text.is_char_boundary(range.end)
        {
            continue;
        }
        if start < range.start {
            spans.push(Span::styled(
                text[start..range.start].to_owned(),
                span.style,
            ));
        }
        spans.push(Span::styled(
            text[range.start..range.end].to_owned(),
            span.style
                .fg(theme.search_match_fg)
                .bg(theme.search_match_bg),
        ));
        start = range.end;
    }
    if start < text.len() {
        spans.push(Span::styled(text[start..].to_owned(), span.style));
    }
}

pub(crate) fn context_show_line(
    lines: usize,
    more: bool,
    width: usize,
    theme: DiffTheme,
) -> Line<'static> {
    if width == 0 {
        return Line::default();
    }

    let suffix = if lines == 1 { "line" } else { "lines" };
    let label = if more {
        format!(" ▾ show {} more {suffix}", format_count(lines))
    } else {
        format!(" ▾ show {} {suffix}", format_count(lines))
    };
    context_action_line(&label, width, theme, theme.muted)
}

pub(crate) fn context_hide_line(lines: usize, width: usize, theme: DiffTheme) -> Line<'static> {
    let suffix = if lines == 1 { "line" } else { "lines" };
    context_action_line(
        &format!(" ▴ hide {} {suffix}", format_count(lines)),
        width,
        theme,
        theme.muted,
    )
}

pub(crate) fn context_action_line(
    label: &str,
    width: usize,
    theme: DiffTheme,
    text_color: Color,
) -> Line<'static> {
    if width == 0 {
        return Line::default();
    }

    let bg = base_bg(theme);
    let mut spans = Vec::new();
    let indicator_width = 1.min(width);
    if indicator_width > 0 {
        spans.push(diff_indicator_span(DiffLineKind::Meta, theme));
    }
    let content_width = width.saturating_sub(indicator_width);
    if content_width > 0 {
        spans.push(Span::styled(
            fit_padded(label, content_width),
            Style::default().fg(text_color).bg(bg),
        ));
    }
    Line::from(spans)
}

pub(crate) fn render_context_line(
    app: &mut DiffApp,
    file: usize,
    old_line: usize,
    new_line: usize,
    row_index: usize,
    width: usize,
) -> Line<'static> {
    let theme = app.theme;
    let horizontal_scroll = app.horizontal_scroll;
    let side = app.context_source_side(file);
    let syntax = side.and_then(|side| {
        let line_number = match side {
            DiffSide::Old => old_line,
            DiffSide::New => new_line,
        };
        app.syntax_file_line(file, side, line_number)
    });
    let diff_line = DiffLine {
        kind: DiffLineKind::Context,
        old_line: Some(old_line),
        new_line: Some(new_line),
        text: app.context_line_text(file, old_line, new_line),
    };

    match app.layout {
        DiffLayoutMode::Unified => render_unified_line_at_scroll(
            &diff_line,
            syntax.as_ref(),
            &[],
            row_index,
            width,
            theme,
            horizontal_scroll,
        ),
        DiffLayoutMode::Split => render_split_context_line(
            &diff_line,
            syntax.as_ref(),
            row_index,
            width,
            theme,
            horizontal_scroll,
        ),
    }
}

pub(crate) fn render_split_context_line(
    line: &DiffLine,
    syntax: Option<&HighlightedLine>,
    row_index: usize,
    width: usize,
    theme: DiffTheme,
    horizontal_scroll: usize,
) -> Line<'static> {
    if width == 0 {
        return Line::default();
    }

    let left_width = width / 2;
    let right_width = width.saturating_sub(left_width);
    let mut spans = split_cell_spans_at_scroll(
        Some(line),
        syntax,
        &[],
        SplitCellRender {
            side: SplitSide::Old,
            row_index,
            width: left_width,
            theme,
        },
        horizontal_scroll,
    );
    spans.extend(split_cell_spans_at_scroll(
        Some(line),
        syntax,
        &[],
        SplitCellRender {
            side: SplitSide::New,
            row_index,
            width: right_width,
            theme,
        },
        horizontal_scroll,
    ));
    Line::from(spans)
}

pub(crate) fn file_separator_line(
    _layout: DiffLayoutMode,
    width: usize,
    theme: DiffTheme,
) -> Line<'static> {
    if width == 0 {
        return Line::default();
    }

    Line::from(Span::styled(
        "─".repeat(width),
        Style::default().fg(theme.empty_diff).bg(base_bg(theme)),
    ))
}

pub(crate) fn file_header_line(
    file: &hz_diff::DiffFile,
    width: usize,
    theme: DiffTheme,
) -> Line<'static> {
    Line::from(file_header_spans(file, width, theme))
}

pub(crate) fn file_header_spans(
    file: &hz_diff::DiffFile,
    width: usize,
    theme: DiffTheme,
) -> Vec<Span<'static>> {
    let bg = base_bg(theme);
    header_spans(
        status_code(file.status),
        file.display_path(),
        &file_delta_parts(file.additions, file.deletions),
        width,
        HeaderStyles {
            prefix: file_sidebar_style(file.status, theme)
                .bg(bg)
                .add_modifier(Modifier::BOLD),
            body: Style::default().fg(theme.foreground).bg(bg),
            fill: Style::default().bg(bg),
            addition: Style::default().fg(theme.addition_fg).bg(bg),
            deletion: Style::default().fg(theme.deletion_fg).bg(bg),
        },
    )
}

pub(crate) fn hunk_header_line(
    hunk: &hz_diff::DiffHunk,
    width: usize,
    theme: DiffTheme,
) -> Line<'static> {
    if width == 0 {
        return Line::default();
    }

    let gutter_bg = line_gutter_bg(DiffLineKind::Meta, theme);
    let content_width = width.saturating_sub(1);
    let mut spans = Vec::new();
    spans.push(diff_indicator_span(DiffLineKind::Meta, theme));
    if content_width > 0 {
        spans.push(Span::styled(" ", Style::default().bg(gutter_bg)));
        if content_width > 1 {
            spans.extend(hunk_header_spans(hunk, content_width - 1, theme, gutter_bg));
        }
    }

    Line::from(spans)
}

pub(crate) fn hunk_header_spans(
    hunk: &hz_diff::DiffHunk,
    width: usize,
    theme: DiffTheme,
    bg: Color,
) -> Vec<Span<'static>> {
    let (additions, deletions) = hunk_change_counts(hunk);
    hunk_header_spans_with_delta(
        &hunk_header_location_parts(&hunk.header, theme, bg),
        hunk_header_context(&hunk.header),
        &compact_delta_parts(additions, deletions),
        width,
        HeaderStyles {
            prefix: Style::default().fg(theme.muted).bg(bg),
            body: Style::default().fg(theme.foreground).bg(bg),
            fill: Style::default().bg(bg),
            addition: Style::default().fg(theme.addition_fg).bg(bg),
            deletion: Style::default().fg(theme.deletion_fg).bg(bg),
        },
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HeaderSpanPart {
    pub(crate) text: String,
    pub(crate) style: Style,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeltaKind {
    Addition,
    Deletion,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeltaPart {
    pub(crate) text: String,
    pub(crate) kind: DeltaKind,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct HeaderStyles {
    pub(crate) prefix: Style,
    pub(crate) body: Style,
    pub(crate) fill: Style,
    pub(crate) addition: Style,
    pub(crate) deletion: Style,
}

pub(crate) fn file_delta_parts(additions: usize, deletions: usize) -> Vec<DeltaPart> {
    vec![
        DeltaPart {
            text: format!("+{additions}"),
            kind: DeltaKind::Addition,
        },
        DeltaPart {
            text: format!("-{deletions}"),
            kind: DeltaKind::Deletion,
        },
    ]
}

pub(crate) fn compact_delta_parts(additions: usize, deletions: usize) -> Vec<DeltaPart> {
    let mut parts = Vec::with_capacity(2);
    if additions > 0 {
        parts.push(DeltaPart {
            text: format!("+{additions}"),
            kind: DeltaKind::Addition,
        });
    }
    if deletions > 0 {
        parts.push(DeltaPart {
            text: format!("-{deletions}"),
            kind: DeltaKind::Deletion,
        });
    }
    parts
}

pub(crate) fn header_spans(
    prefix: &str,
    body: &str,
    delta_parts: &[DeltaPart],
    width: usize,
    styles: HeaderStyles,
) -> Vec<Span<'static>> {
    if width == 0 {
        return Vec::new();
    }

    let delta_width = delta_parts_width(delta_parts);
    if delta_width >= width {
        let mut spans = Vec::new();
        push_fitted_delta_spans(&mut spans, delta_parts, width, styles);
        return spans;
    }

    let delta_gap = usize::from(delta_width > 0);
    let left_width = width.saturating_sub(delta_width).saturating_sub(delta_gap);
    let fitted = fitted_prefixed_parts(prefix, body, left_width);
    let mut spans = Vec::new();
    let left_used = push_prefixed_spans(&mut spans, fitted, styles);
    let gap = width.saturating_sub(left_used).saturating_sub(delta_width);
    if gap > 0 {
        spans.push(Span::styled(" ".repeat(gap), styles.fill));
    }
    push_delta_spans(&mut spans, delta_parts, styles);
    spans
}

pub(crate) fn hunk_header_spans_with_delta(
    prefix_parts: &[HeaderSpanPart],
    body: &str,
    delta_parts: &[DeltaPart],
    width: usize,
    styles: HeaderStyles,
) -> Vec<Span<'static>> {
    if width == 0 {
        return Vec::new();
    }

    let delta_width = delta_parts_width(delta_parts);
    if delta_width >= width {
        let mut spans = Vec::new();
        push_fitted_delta_spans(&mut spans, delta_parts, width, styles);
        return spans;
    }

    let delta_gap = usize::from(delta_width > 0);
    let left_width = width.saturating_sub(delta_width).saturating_sub(delta_gap);
    let mut spans = Vec::new();
    let left_used =
        push_header_prefix_and_body_spans(&mut spans, prefix_parts, body, left_width, styles);
    let gap = width.saturating_sub(left_used).saturating_sub(delta_width);
    if gap > 0 {
        spans.push(Span::styled(" ".repeat(gap), styles.fill));
    }
    push_delta_spans(&mut spans, delta_parts, styles);
    spans
}

pub(crate) fn push_header_prefix_and_body_spans(
    spans: &mut Vec<Span<'static>>,
    prefix_parts: &[HeaderSpanPart],
    body: &str,
    width: usize,
    styles: HeaderStyles,
) -> usize {
    if width == 0 {
        return 0;
    }

    let prefix_width = header_span_parts_width(prefix_parts);
    if prefix_width >= width {
        return push_fitted_header_span_parts(spans, prefix_parts, width, true);
    }

    let mut used = push_header_span_parts(spans, prefix_parts);
    if body.is_empty() {
        return used;
    }

    let body_width = width.saturating_sub(used).saturating_sub(1);
    if body_width == 0 {
        return used;
    }

    spans.push(Span::styled(" ", styles.body));
    used += 1;
    let body = fit_with_ellipsis(body, body_width);
    used += body.width();
    spans.push(Span::styled(body, styles.body));
    used
}

pub(crate) fn header_span_parts_width(parts: &[HeaderSpanPart]) -> usize {
    parts.iter().map(|part| part.text.width()).sum()
}

pub(crate) fn push_header_span_parts(
    spans: &mut Vec<Span<'static>>,
    parts: &[HeaderSpanPart],
) -> usize {
    let mut used = 0;
    for part in parts {
        used += part.text.width();
        spans.push(Span::styled(part.text.clone(), part.style));
    }
    used
}

pub(crate) fn push_fitted_header_span_parts(
    spans: &mut Vec<Span<'static>>,
    parts: &[HeaderSpanPart],
    width: usize,
    ellipsis: bool,
) -> usize {
    if width == 0 {
        return 0;
    }

    let source_width = header_span_parts_width(parts);
    if !ellipsis || source_width <= width {
        return push_fitted_header_span_part_prefix(spans, parts, width);
    }

    let ellipsis_width = "...".width();
    if width <= ellipsis_width {
        let text = fit("...", width);
        let used = text.width();
        if !text.is_empty() {
            spans.push(Span::styled(
                text,
                parts.first().map(|part| part.style).unwrap_or_default(),
            ));
        }
        return used;
    }

    let prefix_width = width.saturating_sub(ellipsis_width);
    let used = push_fitted_header_span_part_prefix(spans, parts, prefix_width);
    let ellipsis_style = spans
        .last()
        .map(|span| span.style)
        .or_else(|| parts.first().map(|part| part.style))
        .unwrap_or_default();
    spans.push(Span::styled("...", ellipsis_style));
    used + ellipsis_width
}

pub(crate) fn push_fitted_header_span_part_prefix(
    spans: &mut Vec<Span<'static>>,
    parts: &[HeaderSpanPart],
    width: usize,
) -> usize {
    let mut used = 0;
    for part in parts {
        if used >= width {
            break;
        }

        let remaining = width - used;
        let part_width = part.text.width();
        if part_width <= remaining {
            if !part.text.is_empty() {
                spans.push(Span::styled(part.text.clone(), part.style));
            }
            used += part_width;
            continue;
        }

        let text = fit(&part.text, remaining);
        used += text.width();
        if !text.is_empty() {
            spans.push(Span::styled(text, part.style));
        }
        break;
    }
    used
}

pub(crate) fn delta_parts_width(parts: &[DeltaPart]) -> usize {
    parts
        .iter()
        .map(|part| part.text.width())
        .sum::<usize>()
        .saturating_add(parts.len().saturating_sub(1))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FittedPrefixedParts {
    pub(crate) prefix: String,
    pub(crate) gap: bool,
    pub(crate) body: String,
}

pub(crate) fn fitted_prefixed_parts(prefix: &str, body: &str, width: usize) -> FittedPrefixedParts {
    if width == 0 {
        return FittedPrefixedParts {
            prefix: String::new(),
            gap: false,
            body: String::new(),
        };
    }
    if prefix.is_empty() {
        return FittedPrefixedParts {
            prefix: String::new(),
            gap: false,
            body: fit_with_ellipsis(body, width),
        };
    }
    if body.is_empty() {
        return FittedPrefixedParts {
            prefix: fit_with_ellipsis(prefix, width),
            gap: false,
            body: String::new(),
        };
    }

    let prefix_width = prefix.width();
    if prefix_width >= width {
        return FittedPrefixedParts {
            prefix: fit_with_ellipsis(prefix, width),
            gap: false,
            body: String::new(),
        };
    }

    let body_width = width.saturating_sub(prefix_width).saturating_sub(1);
    if body_width == 0 {
        return FittedPrefixedParts {
            prefix: fit(prefix, width),
            gap: false,
            body: String::new(),
        };
    }

    FittedPrefixedParts {
        prefix: prefix.to_owned(),
        gap: true,
        body: fit_with_ellipsis(body, body_width),
    }
}

pub(crate) fn push_prefixed_spans(
    spans: &mut Vec<Span<'static>>,
    fitted: FittedPrefixedParts,
    styles: HeaderStyles,
) -> usize {
    let mut used = 0;
    if !fitted.prefix.is_empty() {
        used += fitted.prefix.width();
        spans.push(Span::styled(fitted.prefix, styles.prefix));
    }
    if fitted.gap {
        used += 1;
        spans.push(Span::styled(" ", styles.body));
    }
    if !fitted.body.is_empty() {
        used += fitted.body.width();
        spans.push(Span::styled(fitted.body, styles.body));
    }
    used
}

pub(crate) fn push_delta_spans(
    spans: &mut Vec<Span<'static>>,
    delta_parts: &[DeltaPart],
    styles: HeaderStyles,
) {
    for (index, part) in delta_parts.iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled(" ", styles.fill));
        }
        spans.push(Span::styled(
            part.text.clone(),
            delta_style(part.kind, styles),
        ));
    }
}

pub(crate) fn push_fitted_delta_spans(
    spans: &mut Vec<Span<'static>>,
    delta_parts: &[DeltaPart],
    width: usize,
    styles: HeaderStyles,
) {
    let mut remaining = width;
    for (index, part) in delta_parts.iter().enumerate() {
        if remaining == 0 {
            return;
        }
        if index > 0 {
            spans.push(Span::styled(" ", styles.fill));
            remaining = remaining.saturating_sub(1);
        }
        if remaining == 0 {
            return;
        }

        let text = fit(&part.text, remaining);
        remaining = remaining.saturating_sub(text.width());
        if !text.is_empty() {
            spans.push(Span::styled(text, delta_style(part.kind, styles)));
        }
    }

    if remaining > 0 {
        spans.push(Span::styled(" ".repeat(remaining), styles.fill));
    }
}

pub(crate) fn delta_style(kind: DeltaKind, styles: HeaderStyles) -> Style {
    match kind {
        DeltaKind::Addition => styles.addition,
        DeltaKind::Deletion => styles.deletion,
    }
}

pub(crate) fn hunk_header_context(header: &str) -> &str {
    header
        .splitn(3, "@@")
        .nth(2)
        .map(str::trim)
        .unwrap_or_default()
}

pub(crate) fn hunk_header_location_parts(
    header: &str,
    theme: DiffTheme,
    bg: Color,
) -> Vec<HeaderSpanPart> {
    let mut parts = header.splitn(3, "@@");
    let Some("") = parts.next() else {
        return vec![HeaderSpanPart {
            text: header.trim().to_owned(),
            style: Style::default().fg(theme.muted).bg(bg),
        }];
    };
    let Some(location) = parts.next() else {
        return vec![HeaderSpanPart {
            text: header.trim().to_owned(),
            style: Style::default().fg(theme.muted).bg(bg),
        }];
    };

    let mut coordinates = location.split_whitespace();
    let old_range = coordinates.next().unwrap_or_default();
    let new_range = coordinates.next().unwrap_or_default();
    if old_range.is_empty() || new_range.is_empty() {
        return vec![HeaderSpanPart {
            text: format!("@@{location}@@"),
            style: Style::default().fg(theme.muted).bg(bg),
        }];
    }

    vec![
        HeaderSpanPart {
            text: "@@ ".to_owned(),
            style: Style::default().fg(theme.muted).bg(bg),
        },
        HeaderSpanPart {
            text: old_range.to_owned(),
            style: Style::default().fg(theme.deletion_fg).bg(bg),
        },
        HeaderSpanPart {
            text: " ".to_owned(),
            style: Style::default().fg(theme.muted).bg(bg),
        },
        HeaderSpanPart {
            text: new_range.to_owned(),
            style: Style::default().fg(theme.addition_fg).bg(bg),
        },
        HeaderSpanPart {
            text: " @@".to_owned(),
            style: Style::default().fg(theme.muted).bg(bg),
        },
    ]
}

pub(crate) fn hunk_change_counts(hunk: &hz_diff::DiffHunk) -> (usize, usize) {
    hunk.lines.iter().fold(
        (0usize, 0usize),
        |(additions, deletions), line| match line.kind {
            DiffLineKind::Addition => (additions + 1, deletions),
            DiffLineKind::Deletion => (additions, deletions + 1),
            DiffLineKind::Context | DiffLineKind::Meta => (additions, deletions),
        },
    )
}

pub(crate) fn render_unified_line_at_scroll(
    line: &DiffLine,
    syntax: Option<&HighlightedLine>,
    inline: &[InlineRange],
    _row_index: usize,
    width: usize,
    theme: DiffTheme,
    horizontal_scroll: usize,
) -> Line<'static> {
    if width == 0 {
        return Line::default();
    }

    let sign = match line.kind {
        DiffLineKind::Context => " ",
        DiffLineKind::Addition => "+",
        DiffLineKind::Deletion => "-",
        DiffLineKind::Meta => " ",
    };
    let indicator_width = 1.min(width);
    let gutter_width = UNIFIED_GUTTER_WIDTH.min(width.saturating_sub(indicator_width));
    let content_width = unified_content_width(width);
    let gutter = format!(
        "{:>5} {:>5} ",
        unified_line_number(line.old_line, line.kind),
        unified_line_number(line.new_line, line.kind)
    );
    let mut spans = Vec::new();
    if indicator_width > 0 {
        spans.push(diff_indicator_span(line.kind, theme));
    }
    if gutter_width > 0 {
        spans.extend(gutter_spans(&gutter, sign, gutter_width, line.kind, theme));
    }
    spans.extend(content_spans_at_scroll(
        &line.text,
        syntax,
        inline,
        line.kind,
        content_width,
        theme,
        horizontal_scroll,
    ));
    Line::from(spans)
}

pub(crate) fn unified_line_number(line: Option<usize>, _kind: DiffLineKind) -> String {
    match line {
        Some(line) => line.to_string(),
        None => String::new(),
    }
}

pub(crate) fn gutter_spans(
    body: &str,
    sign: &str,
    width: usize,
    kind: DiffLineKind,
    theme: DiffTheme,
) -> Vec<Span<'static>> {
    if width == 0 {
        return Vec::new();
    }

    let body_style = Style::default()
        .fg(line_gutter_fg(kind, theme))
        .bg(line_gutter_bg(kind, theme));
    if sign.trim().is_empty() || width == 1 {
        return vec![Span::styled(
            fit_padded(&format!("{body}{sign}"), width),
            body_style,
        )];
    }

    let sign_width = 1;
    let body_width = width.saturating_sub(sign_width);
    vec![
        Span::styled(fit_padded(body, body_width), body_style),
        Span::styled(fit(sign, sign_width), diff_sign_style(kind, theme)),
    ]
}

pub(crate) fn diff_sign_style(kind: DiffLineKind, theme: DiffTheme) -> Style {
    let mut style = Style::default()
        .fg(diff_indicator_fg(kind, theme))
        .bg(line_gutter_bg(kind, theme));
    if theme.diff.sign_style == DiffSignStyle::Bold
        && matches!(kind, DiffLineKind::Addition | DiffLineKind::Deletion)
    {
        style = style.add_modifier(Modifier::BOLD);
    }
    style
}

pub(crate) fn diff_indicator_span(kind: DiffLineKind, theme: DiffTheme) -> Span<'static> {
    Span::styled(DIFF_INDICATOR, diff_indicator_style(kind, theme))
}

pub(crate) fn diff_indicator_style(kind: DiffLineKind, theme: DiffTheme) -> Style {
    Style::default()
        .fg(diff_indicator_fg(kind, theme))
        .bg(line_gutter_bg(kind, theme))
}

pub(crate) fn diff_indicator_fg(kind: DiffLineKind, theme: DiffTheme) -> Color {
    match kind {
        DiffLineKind::Addition => theme.addition_fg,
        DiffLineKind::Deletion => theme.deletion_fg,
        DiffLineKind::Context | DiffLineKind::Meta => theme.muted,
    }
}

pub(crate) fn base_bg(theme: DiffTheme) -> Color {
    if theme.transparent_background {
        Color::Reset
    } else {
        theme.background
    }
}

pub(crate) fn header_bg(theme: DiffTheme) -> Color {
    if theme.transparent_background {
        Color::Reset
    } else {
        theme.gutter_bg
    }
}

pub(crate) fn statusline_bg(theme: DiffTheme) -> Color {
    if theme.transparent_background {
        Color::Reset
    } else {
        STATUSLINE_BG
    }
}

pub(crate) fn empty_diff_fill_from(width: usize, row_index: usize, column_offset: usize) -> String {
    (0..width)
        .map(|column| {
            if (column + column_offset + row_index) % EMPTY_DIFF_FILL_SPACING == 0 {
                EMPTY_DIFF_FILL
            } else {
                ' '
            }
        })
        .collect()
}

pub(crate) fn content_spans_at_scroll(
    text: &str,
    syntax: Option<&HighlightedLine>,
    inline: &[InlineRange],
    kind: DiffLineKind,
    width: usize,
    theme: DiffTheme,
    horizontal_scroll: usize,
) -> Vec<Span<'static>> {
    if width == 0 {
        return Vec::new();
    }

    let inline = valid_inline_ranges(text, inline);
    let syntax = syntax.filter(|syntax| syntax_line_matches_text(syntax, text));
    if syntax.is_none() && inline.is_empty() {
        return vec![Span::styled(
            fit_padded_from(text, horizontal_scroll, width),
            line_style(kind, theme),
        )];
    }

    let mut writer = ContentSpanWriter::new(&inline, kind, width, theme, horizontal_scroll);

    if let Some(syntax) = syntax {
        let mut byte_start = 0usize;
        for segment in &syntax.segments {
            if !writer.push_segment(
                &segment.text,
                byte_start,
                syntax_style(segment.class, kind, theme),
            ) {
                break;
            }
            byte_start += segment.text.len();
        }
    } else {
        writer.push_segment(text, 0, line_style(kind, theme));
    }

    writer.finish()
}

pub(crate) fn valid_inline_ranges(text: &str, ranges: &[InlineRange]) -> Vec<InlineRange> {
    let mut valid = ranges
        .iter()
        .filter_map(|range| {
            let byte_start = range.byte_start.min(text.len());
            let byte_end = range.byte_end.min(text.len());
            (byte_start < byte_end
                && text.is_char_boundary(byte_start)
                && text.is_char_boundary(byte_end))
            .then_some(InlineRange {
                byte_start,
                byte_end,
            })
        })
        .collect::<Vec<_>>();
    valid.sort_by_key(|range| (range.byte_start, range.byte_end));
    valid
}

pub(crate) struct ContentSpanWriter<'a> {
    spans: Vec<Span<'static>>,
    inline: &'a [InlineRange],
    kind: DiffLineKind,
    width: usize,
    skip: usize,
    used: usize,
    theme: DiffTheme,
}

impl<'a> ContentSpanWriter<'a> {
    pub(crate) fn new(
        inline: &'a [InlineRange],
        kind: DiffLineKind,
        width: usize,
        theme: DiffTheme,
        horizontal_scroll: usize,
    ) -> Self {
        Self {
            spans: Vec::new(),
            inline,
            kind,
            width,
            skip: horizontal_scroll,
            used: 0,
            theme,
        }
    }

    pub(crate) fn push_segment(
        &mut self,
        segment_text: &str,
        segment_byte_start: usize,
        style: Style,
    ) -> bool {
        let segment_byte_end = segment_byte_start + segment_text.len();
        let mut cursor = segment_byte_start;

        for range in self.inline {
            if self.used >= self.width {
                return false;
            }
            if range.byte_end <= cursor {
                continue;
            }
            if range.byte_start >= segment_byte_end {
                break;
            }

            let normal_end = range.byte_start.min(segment_byte_end);
            if !self.push_piece(segment_text, segment_byte_start, cursor, normal_end, style) {
                return false;
            }

            let inline_start = range.byte_start.max(cursor).min(segment_byte_end);
            let inline_end = range.byte_end.min(segment_byte_end);
            if !self.push_piece(
                segment_text,
                segment_byte_start,
                inline_start,
                inline_end,
                inline_style(style, self.kind, self.theme),
            ) {
                return false;
            }
            cursor = inline_end;
        }

        self.push_piece(
            segment_text,
            segment_byte_start,
            cursor,
            segment_byte_end,
            style,
        )
    }

    pub(crate) fn push_piece(
        &mut self,
        segment_text: &str,
        segment_byte_start: usize,
        byte_start: usize,
        byte_end: usize,
        style: Style,
    ) -> bool {
        if byte_start >= byte_end {
            return true;
        }
        let remaining = self.width.saturating_sub(self.used);
        if remaining == 0 {
            return false;
        }

        let relative_start = byte_start.saturating_sub(segment_byte_start);
        let relative_end = byte_end.saturating_sub(segment_byte_start);
        let mut piece = &segment_text[relative_start..relative_end];
        if self.skip > 0 {
            let (visible, skipped) = skip_display_prefix(piece, self.skip);
            self.skip = self.skip.saturating_sub(skipped);
            piece = visible;
            if piece.is_empty() {
                return true;
            }
        }
        let fitted = fit(piece, remaining);
        if fitted.is_empty() {
            return false;
        }

        let fitted_len = fitted.len();
        self.used += UnicodeWidthStr::width(fitted.as_str());
        self.spans.push(Span::styled(fitted, style));
        fitted_len == piece.len()
    }

    pub(crate) fn finish(mut self) -> Vec<Span<'static>> {
        if self.used < self.width {
            self.spans.push(Span::styled(
                " ".repeat(self.width - self.used),
                line_style(self.kind, self.theme),
            ));
        }
        self.spans
    }
}

pub(crate) fn syntax_line_matches_text(syntax: &HighlightedLine, text: &str) -> bool {
    let mut remaining = text;
    for segment in &syntax.segments {
        if !remaining.starts_with(&segment.text) {
            return false;
        }
        remaining = &remaining[segment.text.len()..];
    }
    remaining.is_empty()
}

pub(crate) fn syntax_style(
    class: Option<SyntaxClass>,
    kind: DiffLineKind,
    theme: DiffTheme,
) -> Style {
    let mut style = line_style(kind, theme);
    if let Some(color) = class.and_then(|class| syntax_fg(class, theme)) {
        style = style.fg(color);
    }
    style
}

pub(crate) fn inline_style(style: Style, kind: DiffLineKind, theme: DiffTheme) -> Style {
    if theme.transparent_background || theme.diff.inline_background == DiffBackground::None {
        return match kind {
            DiffLineKind::Addition | DiffLineKind::Deletion => style.add_modifier(Modifier::BOLD),
            DiffLineKind::Context | DiffLineKind::Meta => style,
        };
    }

    match kind {
        DiffLineKind::Addition => style
            .bg(inline_bg(kind, theme))
            .add_modifier(Modifier::BOLD),
        DiffLineKind::Deletion => style
            .bg(inline_bg(kind, theme))
            .add_modifier(Modifier::BOLD),
        DiffLineKind::Context | DiffLineKind::Meta => style,
    }
}

pub(crate) fn inline_bg(kind: DiffLineKind, theme: DiffTheme) -> Color {
    match (theme.diff.inline_background, kind) {
        (DiffBackground::Subtle, DiffLineKind::Addition) => theme.addition_bg,
        (DiffBackground::Subtle, DiffLineKind::Deletion) => theme.deletion_bg,
        (_, DiffLineKind::Addition) => theme.addition_inline_bg,
        (_, DiffLineKind::Deletion) => theme.deletion_inline_bg,
        _ => Color::Reset,
    }
}

pub(crate) fn syntax_fg(class: SyntaxClass, theme: DiffTheme) -> Option<Color> {
    theme.syntax.color(class)
}

pub(crate) fn render_split_line(
    app: &mut DiffApp,
    file: usize,
    hunk: usize,
    left: Option<usize>,
    right: Option<usize>,
    row_index: usize,
    width: usize,
) -> Line<'static> {
    if width == 0 {
        return Line::default();
    }
    let theme = app.theme;
    let horizontal_scroll = app.horizontal_scroll;

    let (left_line, right_line) = {
        let lines = &app.changeset.files[file].hunks[hunk].lines;
        (
            left.and_then(|index| lines.get(index)).cloned(),
            right.and_then(|index| lines.get(index)).cloned(),
        )
    };
    let left_syntax = left.and_then(|index| app.syntax_line(file, hunk, index, DiffSide::Old));
    let right_syntax = right.and_then(|index| app.syntax_line(file, hunk, index, DiffSide::New));
    let left_inline = left
        .map(|index| app.inline_ranges(file, hunk, index))
        .unwrap_or_default();
    let right_inline = right
        .map(|index| app.inline_ranges(file, hunk, index))
        .unwrap_or_default();

    let left_width = width / 2;
    let right_width = width.saturating_sub(left_width);
    let mut spans = split_cell_spans_at_scroll(
        left_line.as_ref(),
        left_syntax.as_ref(),
        &left_inline,
        SplitCellRender {
            side: SplitSide::Old,
            row_index,
            width: left_width,
            theme,
        },
        horizontal_scroll,
    );
    spans.extend(split_cell_spans_at_scroll(
        right_line.as_ref(),
        right_syntax.as_ref(),
        &right_inline,
        SplitCellRender {
            side: SplitSide::New,
            row_index,
            width: right_width,
            theme,
        },
        horizontal_scroll,
    ));
    Line::from(spans)
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum SplitSide {
    Old,
    New,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct SplitCellRender {
    pub(crate) side: SplitSide,
    pub(crate) row_index: usize,
    pub(crate) width: usize,
    pub(crate) theme: DiffTheme,
}

pub(crate) fn split_cell_spans_at_scroll(
    line: Option<&DiffLine>,
    syntax: Option<&HighlightedLine>,
    inline: &[InlineRange],
    render: SplitCellRender,
    horizontal_scroll: usize,
) -> Vec<Span<'static>> {
    let SplitCellRender {
        side,
        row_index,
        width,
        theme,
    } = render;

    if width == 0 {
        return Vec::new();
    }

    let Some(line) = line else {
        let empty_kind = DiffLineKind::Context;
        let indicator_width = 1.min(width);
        let gutter_width = GUTTER_WIDTH.min(width.saturating_sub(indicator_width));
        let content_width = split_cell_content_width(width);
        let mut spans = Vec::new();
        if indicator_width > 0 {
            spans.push(diff_indicator_span(empty_kind, theme));
        }
        if gutter_width > 0 {
            spans.push(Span::styled(
                " ".repeat(gutter_width),
                Style::default().bg(line_gutter_bg(empty_kind, theme)),
            ));
        }
        if content_width > 0 {
            spans.push(Span::styled(
                empty_diff_fill_from(
                    content_width,
                    row_index,
                    indicator_width + gutter_width + horizontal_scroll,
                ),
                Style::default().fg(theme.empty_diff).bg(base_bg(theme)),
            ));
        }
        return spans;
    };

    let indicator_width = 1.min(width);
    let gutter_width = GUTTER_WIDTH.min(width.saturating_sub(indicator_width));
    let content_width = split_cell_content_width(width);
    let line_number = match side {
        SplitSide::Old => line.old_line,
        SplitSide::New => line.new_line,
    }
    .map(|line| line.to_string())
    .unwrap_or_default();
    let sign = match (side, line.kind) {
        (SplitSide::Old, DiffLineKind::Deletion) => "-",
        (SplitSide::New, DiffLineKind::Addition) => "+",
        _ => " ",
    };

    let mut spans = Vec::new();
    if indicator_width > 0 {
        spans.push(diff_indicator_span(line.kind, theme));
    }
    if gutter_width > 0 {
        spans.extend(gutter_spans(
            &format!("{line_number:>5} "),
            sign,
            gutter_width,
            line.kind,
            theme,
        ));
    }
    spans.extend(content_spans_at_scroll(
        &line.text,
        syntax,
        inline,
        line.kind,
        content_width,
        theme,
        horizontal_scroll,
    ));
    spans
}

pub(crate) fn row_bg(kind: DiffLineKind, theme: DiffTheme) -> Color {
    if theme.transparent_background {
        return Color::Reset;
    }

    match (theme.diff.line_background, kind) {
        (DiffBackground::None, _) => theme.background,
        (DiffBackground::Subtle, DiffLineKind::Addition) => theme.addition_bg,
        (DiffBackground::Subtle, DiffLineKind::Deletion) => theme.deletion_bg,
        (DiffBackground::Strong, DiffLineKind::Addition) => theme.addition_inline_bg,
        (DiffBackground::Strong, DiffLineKind::Deletion) => theme.deletion_inline_bg,
        _ => theme.background,
    }
}

pub(crate) fn line_style(kind: DiffLineKind, theme: DiffTheme) -> Style {
    match kind {
        DiffLineKind::Addition => Style::default()
            .fg(theme.foreground)
            .bg(row_bg(kind, theme)),
        DiffLineKind::Deletion => Style::default()
            .fg(theme.foreground)
            .bg(row_bg(kind, theme)),
        DiffLineKind::Meta => Style::default().fg(theme.muted).bg(base_bg(theme)),
        DiffLineKind::Context => Style::default().fg(theme.foreground).bg(base_bg(theme)),
    }
}

pub(crate) fn status_code(status: FileStatus) -> &'static str {
    match status {
        FileStatus::Modified => "M",
        FileStatus::Added => "A",
        FileStatus::Deleted => "D",
        FileStatus::Renamed => "R",
        FileStatus::Copied => "C",
        FileStatus::TypeChanged => "T",
        FileStatus::Unknown => "?",
    }
}

pub(crate) fn progress_label(scroll: usize, max_scroll: usize) -> String {
    if max_scroll == 0 {
        return "100%".to_owned();
    }

    format!(
        "{}%",
        scroll.min(max_scroll).saturating_mul(100) / max_scroll
    )
}

pub(crate) fn fit_padded(text: &str, width: usize) -> String {
    fit_padded_from(text, 0, width)
}

pub(crate) fn fit_padded_from(text: &str, horizontal_scroll: usize, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let visible = if horizontal_scroll > 0 {
        skip_display_prefix(text, horizontal_scroll).0
    } else {
        text
    };
    if is_single_width_ascii(visible) {
        let used = visible.len().min(width);
        let mut out = String::with_capacity(width);
        out.push_str(&visible[..used]);
        if used < width {
            out.extend(std::iter::repeat_n(' ', width - used));
        }
        return out;
    }

    let mut out = fit(visible, width);
    let len = UnicodeWidthStr::width(out.as_str());
    if len < width {
        out.reserve(width - len);
        out.extend(std::iter::repeat_n(' ', width - len));
    }
    out
}

pub(crate) fn skip_display_prefix(text: &str, columns: usize) -> (&str, usize) {
    if columns == 0 {
        return (text, 0);
    }
    if is_single_width_ascii(text) {
        let skipped = columns.min(text.len());
        return (&text[skipped..], skipped);
    }

    let mut skipped = 0usize;
    let mut byte_index = 0usize;
    for (index, ch) in text.char_indices() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if skipped >= columns {
            if ch_width == 0 {
                byte_index = index + ch.len_utf8();
                continue;
            }
            break;
        }

        skipped = skipped.saturating_add(ch_width);
        byte_index = index + ch.len_utf8();
    }

    (&text[byte_index..], skipped)
}

pub(crate) fn fit(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if is_single_width_ascii(text) {
        return text[..text.len().min(width)].to_owned();
    }

    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + ch_width > width {
            break;
        }
        used += ch_width;
        out.push(ch);
    }
    out
}

pub(crate) fn is_single_width_ascii(text: &str) -> bool {
    text.bytes().all(|byte| (b' '..=b'~').contains(&byte))
}
