use hz_diff::DiffLine;
use ratatui::prelude::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::{
    app::DiffApp,
    controls::{TextMatcher, diff_line_grep_prefix},
    model::UiRow,
    render::text::skip_display_prefix,
    theme::{DiffTheme, GUTTER_WIDTH, UNIFIED_GUTTER_WIDTH},
};

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
        UiRow::FileHeader(_) => Vec::new(),
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
