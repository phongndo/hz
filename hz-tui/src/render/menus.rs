use hz_diff::{DiffOptions, DiffScope, DiffSource};
use ratatui::{
    Frame,
    layout::Rect,
    prelude::{Color, Line, Modifier, Span, Style, Text},
    widgets::{Block, BorderType, Clear, Padding, Paragraph},
};
use unicode_width::UnicodeWidthStr;

use crate::{
    app::DiffApp,
    controls::DiffChoice,
    render::{
        style::{base_bg, header_bg},
        text::fit_padded,
    },
    theme::{
        DiffTheme, HELP_KEY_COLUMN_WIDTH, HELP_MENU_COLUMN_GAP, HELP_MENU_HORIZONTAL_PADDING,
        HELP_MENU_LEFT_ROWS, HELP_MENU_RIGHT_ROWS, HELP_MENU_TWO_COLUMN_MIN_WIDTH,
        HELP_MENU_VERTICAL_PADDING, HELP_MENU_WIDTH, HelpMenuRow,
    },
};

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
