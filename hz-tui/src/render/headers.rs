use hz_diff::DiffLineKind;
use ratatui::prelude::{Color, Line, Modifier, Span, Style};
use unicode_width::UnicodeWidthStr;

use crate::{
    controls::DiffLayoutMode,
    render::{
        style::{base_bg, diff_indicator_span, file_sidebar_style, focused_diff_indicator_span},
        text::{fit, fit_with_ellipsis, status_code},
    },
    theme::{DiffTheme, line_gutter_bg},
};

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
    hunk_header_line_with_focus(hunk, width, theme, false)
}

pub(crate) fn hunk_header_line_with_focus(
    hunk: &hz_diff::DiffHunk,
    width: usize,
    theme: DiffTheme,
    focused: bool,
) -> Line<'static> {
    if width == 0 {
        return Line::default();
    }

    let gutter_bg = line_gutter_bg(DiffLineKind::Meta, theme);
    let content_width = width.saturating_sub(1);
    let mut spans = Vec::new();
    spans.push(if focused {
        focused_diff_indicator_span(DiffLineKind::Meta, theme)
    } else {
        diff_indicator_span(DiffLineKind::Meta, theme)
    });
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
