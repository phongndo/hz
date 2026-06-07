use hz_diff::FileStatus;
use ratatui::{
    Frame,
    layout::Rect,
    prelude::{Color, Line, Modifier, Span, Style, Text},
    widgets::Paragraph,
};
use unicode_width::UnicodeWidthStr;

use crate::{
    app::DiffApp,
    render::{
        style::{base_bg, file_sidebar_style, header_bg},
        text::{fit, fit_padded, fit_with_ellipsis, status_code},
    },
    theme::{
        DiffTheme, FILE_SIDEBAR_MAX_WIDTH, FILE_SIDEBAR_MIN_DIFF_WIDTH, FILE_SIDEBAR_MIN_WIDTH,
    },
};

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
