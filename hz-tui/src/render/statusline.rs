use ratatui::{
    Frame,
    layout::Rect,
    prelude::{Line, Modifier, Span, Style},
    widgets::Paragraph,
};
use unicode_width::UnicodeWidthStr;

use crate::{
    app::DiffApp,
    controls::{BranchMenu, DiffFilterKind},
    render::{
        menus::{diff_comparison_label, diff_selector_text},
        style::statusline_bg,
        text::{fit, fit_with_ellipsis, format_count, progress_label},
    },
    theme::{
        BRANCH_COMPARISON_SEPARATOR, STATUSLINE_ACCENT_BG, STATUSLINE_ACCENT_FG,
        STATUSLINE_INFO_BG, STATUSLINE_INFO_FG, STATUSLINE_SELECTOR_GAP,
    },
};

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
        .unwrap_or_else(|| {
            if app.pending_diff_load.is_some() {
                "loading diff"
            } else if app.live_reload_pending {
                "refreshing diff"
            } else {
                ""
            }
        });
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
