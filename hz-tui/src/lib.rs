use std::{
    io,
    time::{Duration, Instant},
};

use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers,
        MouseEvent, MouseEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use hz_core::HzResult;
use hz_diff::{Changeset, DiffLine, DiffLineKind, DiffOptions, DiffStats, FileStatus};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    prelude::{Color, Line, Modifier, Span, Style, Text},
    widgets::Paragraph,
};

const EVENT_POLL: Duration = Duration::from_millis(120);
const MAX_READY_EVENTS_PER_FRAME: usize = 64;
const MOUSE_SCROLL_HISTORY_SIZE: usize = 3;
const MOUSE_SCROLL_STREAK_TIMEOUT: Duration = Duration::from_millis(150);
const MOUSE_SCROLL_MIN_TICK_INTERVAL: Duration = Duration::from_millis(6);
const MOUSE_SCROLL_ACCEL_A: f64 = 0.4;
const MOUSE_SCROLL_ACCEL_TAU: f64 = 4.0;
const MOUSE_SCROLL_MAX_MULTIPLIER: f64 = 3.0;
const MOUSE_SCROLL_REFERENCE_INTERVAL_MS: f64 = 100.0;
const MIN_SPLIT_WIDTH: u16 = 120;
const GUTTER_WIDTH: usize = 7;
const UNIFIED_GUTTER_WIDTH: usize = 13;

fn muted() -> Color {
    Color::Rgb(125, 135, 148)
}

fn addition_bg() -> Color {
    Color::Rgb(16, 48, 27)
}

fn deletion_bg() -> Color {
    Color::Rgb(52, 25, 30)
}

fn addition_fg() -> Color {
    Color::Rgb(155, 214, 166)
}

fn deletion_fg() -> Color {
    Color::Rgb(232, 141, 141)
}

pub fn run() -> HzResult<()> {
    run_diff(DiffOptions::default())
}

pub fn run_diff(options: DiffOptions) -> HzResult<()> {
    let changeset = hz_diff::load_review_ref(&options)?;

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let layout = default_layout_for_width(terminal.size()?.width);
    let mut app = DiffApp::new(options, changeset, layout);

    let result = run_loop(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;

    result
}

type CrosstermTerminal = Terminal<CrosstermBackend<io::Stdout>>;

fn default_layout_for_width(width: u16) -> DiffLayoutMode {
    if width >= MIN_SPLIT_WIDTH {
        DiffLayoutMode::Split
    } else {
        DiffLayoutMode::Unified
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiffLayoutMode {
    Split,
    Unified,
}

impl DiffLayoutMode {
    fn label(self) -> &'static str {
        match self {
            Self::Split => "split",
            Self::Unified => "unified",
        }
    }

    fn toggled(self) -> Self {
        match self {
            Self::Split => Self::Unified,
            Self::Unified => Self::Split,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UiRow {
    FileHeader(usize),
    BinaryFile(usize),
    Collapsed {
        lines: usize,
    },
    HunkHeader {
        file: usize,
        hunk: usize,
    },
    UnifiedLine {
        file: usize,
        hunk: usize,
        line: usize,
    },
    SplitLine {
        file: usize,
        hunk: usize,
        left: Option<usize>,
        right: Option<usize>,
    },
    MetaLine {
        file: usize,
        hunk: usize,
        line: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct UiModel {
    rows: Vec<UiRow>,
    file_start_rows: Vec<usize>,
    hunk_start_rows: Vec<usize>,
}

impl UiModel {
    fn new(changeset: &Changeset, layout: DiffLayoutMode) -> Self {
        let mut rows = Vec::new();
        let mut file_start_rows = Vec::with_capacity(changeset.files.len());
        let mut hunk_start_rows = Vec::new();

        for (file_index, file) in changeset.files.iter().enumerate() {
            file_start_rows.push(rows.len());
            rows.push(UiRow::FileHeader(file_index));

            if file.is_binary || file.hunks.is_empty() {
                rows.push(UiRow::BinaryFile(file_index));
                continue;
            }

            let mut next_old_line = 1;
            let mut next_new_line = 1;
            for (hunk_index, hunk) in file.hunks.iter().enumerate() {
                let collapsed_lines = hunk
                    .old_start
                    .saturating_sub(next_old_line)
                    .min(hunk.new_start.saturating_sub(next_new_line));
                if collapsed_lines > 0 {
                    rows.push(UiRow::Collapsed {
                        lines: collapsed_lines,
                    });
                }

                hunk_start_rows.push(rows.len());
                rows.push(UiRow::HunkHeader {
                    file: file_index,
                    hunk: hunk_index,
                });

                match layout {
                    DiffLayoutMode::Unified => {
                        for line_index in 0..hunk.lines.len() {
                            rows.push(UiRow::UnifiedLine {
                                file: file_index,
                                hunk: hunk_index,
                                line: line_index,
                            });
                        }
                    }
                    DiffLayoutMode::Split => push_split_hunk_rows(
                        &mut rows,
                        file_index,
                        hunk_index,
                        hunk.lines.as_slice(),
                    ),
                }

                next_old_line = hunk.old_start.saturating_add(hunk.old_count);
                next_new_line = hunk.new_start.saturating_add(hunk.new_count);
            }
        }

        Self {
            rows,
            file_start_rows,
            hunk_start_rows,
        }
    }

    fn len(&self) -> usize {
        self.rows.len()
    }

    fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    fn row(&self, index: usize) -> Option<UiRow> {
        self.rows.get(index).copied()
    }

    fn file_start_row(&self, file: usize) -> Option<usize> {
        self.file_start_rows.get(file).copied()
    }

    fn file_at_row(&self, row: usize) -> Option<usize> {
        if self.file_start_rows.is_empty() {
            return None;
        }
        match self.file_start_rows.binary_search(&row) {
            Ok(index) => Some(index),
            Err(0) => Some(0),
            Err(index) => Some(index - 1),
        }
    }

    fn next_hunk_row(&self, row: usize) -> Option<usize> {
        self.hunk_start_rows
            .iter()
            .copied()
            .find(|start| *start > row)
    }

    fn previous_hunk_row(&self, row: usize) -> Option<usize> {
        self.hunk_start_rows
            .iter()
            .rev()
            .copied()
            .find(|start| *start < row)
    }
}

fn push_split_hunk_rows(
    rows: &mut Vec<UiRow>,
    file_index: usize,
    hunk_index: usize,
    lines: &[DiffLine],
) {
    let mut index = 0;
    while index < lines.len() {
        match lines[index].kind {
            DiffLineKind::Context => {
                rows.push(UiRow::SplitLine {
                    file: file_index,
                    hunk: hunk_index,
                    left: Some(index),
                    right: Some(index),
                });
                index += 1;
            }
            DiffLineKind::Meta => {
                rows.push(UiRow::MetaLine {
                    file: file_index,
                    hunk: hunk_index,
                    line: index,
                });
                index += 1;
            }
            DiffLineKind::Deletion | DiffLineKind::Addition => {
                let mut deletions = Vec::new();
                let mut additions = Vec::new();
                while index < lines.len()
                    && matches!(
                        lines[index].kind,
                        DiffLineKind::Deletion | DiffLineKind::Addition
                    )
                {
                    match lines[index].kind {
                        DiffLineKind::Deletion => deletions.push(index),
                        DiffLineKind::Addition => additions.push(index),
                        DiffLineKind::Context | DiffLineKind::Meta => {}
                    }
                    index += 1;
                }

                let paired_rows = deletions.len().max(additions.len());
                for pair_index in 0..paired_rows {
                    rows.push(UiRow::SplitLine {
                        file: file_index,
                        hunk: hunk_index,
                        left: deletions.get(pair_index).copied(),
                        right: additions.get(pair_index).copied(),
                    });
                }
            }
        }
    }
}

fn run_loop(terminal: &mut CrosstermTerminal, app: &mut DiffApp) -> HzResult<()> {
    loop {
        if app.dirty {
            terminal.draw(|frame| draw(frame, app))?;
            app.dirty = false;
        }

        if !event::poll(EVENT_POLL)? {
            continue;
        }

        let mut should_quit = false;
        for _ in 0..MAX_READY_EVENTS_PER_FRAME {
            if handle_event(app, event::read()?)? {
                should_quit = true;
                break;
            }

            if !event::poll(Duration::ZERO)? {
                break;
            }
        }

        if should_quit {
            break;
        }
    }

    Ok(())
}

fn handle_event(app: &mut DiffApp, event: Event) -> HzResult<bool> {
    match event {
        Event::Key(key) if app.handle_key(key)? => Ok(true),
        Event::Mouse(mouse) => {
            app.handle_mouse(mouse);
            Ok(false)
        }
        Event::Resize(width, _) => {
            app.apply_responsive_layout(width);
            Ok(false)
        }
        _ => Ok(false),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MouseScrollDirection {
    Up,
    Down,
}

#[derive(Debug, Default)]
struct MouseScroll {
    last_tick: Option<Instant>,
    direction: Option<MouseScrollDirection>,
    intervals: Vec<Duration>,
    pending_lines: f64,
}

impl MouseScroll {
    fn scroll_delta(&mut self, direction: MouseScrollDirection, now: Instant) -> isize {
        let multiplier = self.multiplier(direction, now);
        self.pending_lines += multiplier;
        let lines = self.pending_lines.trunc() as isize;
        self.pending_lines -= lines as f64;

        match direction {
            MouseScrollDirection::Down => lines,
            MouseScrollDirection::Up => -lines,
        }
    }

    fn reset(&mut self) {
        self.last_tick = None;
        self.direction = None;
        self.intervals.clear();
        self.pending_lines = 0.0;
    }

    fn multiplier(&mut self, direction: MouseScrollDirection, now: Instant) -> f64 {
        let Some(last_tick) = self.last_tick else {
            self.start_streak(direction, now);
            return 1.0;
        };

        let elapsed = now.saturating_duration_since(last_tick);
        if self.direction != Some(direction) || elapsed > MOUSE_SCROLL_STREAK_TIMEOUT {
            self.start_streak(direction, now);
            return 1.0;
        }

        if elapsed < MOUSE_SCROLL_MIN_TICK_INTERVAL {
            return 1.0;
        }

        self.last_tick = Some(now);
        self.intervals.push(elapsed);
        if self.intervals.len() > MOUSE_SCROLL_HISTORY_SIZE {
            self.intervals.remove(0);
        }

        let average_interval_ms = self
            .intervals
            .iter()
            .map(|interval| interval.as_secs_f64() * 1000.0)
            .sum::<f64>()
            / self.intervals.len() as f64;
        let velocity = MOUSE_SCROLL_REFERENCE_INTERVAL_MS / average_interval_ms;
        let multiplier =
            1.0 + MOUSE_SCROLL_ACCEL_A * ((velocity / MOUSE_SCROLL_ACCEL_TAU).exp() - 1.0);

        multiplier.min(MOUSE_SCROLL_MAX_MULTIPLIER)
    }

    fn start_streak(&mut self, direction: MouseScrollDirection, now: Instant) {
        self.last_tick = Some(now);
        self.direction = Some(direction);
        self.intervals.clear();
        self.pending_lines = 0.0;
    }
}

#[derive(Debug)]
struct DiffApp {
    options: DiffOptions,
    changeset: Changeset,
    stats: DiffStats,
    model: UiModel,
    layout: DiffLayoutMode,
    scroll: usize,
    viewport_rows: usize,
    selected_file: usize,
    mouse_scroll: MouseScroll,
    notice: Option<String>,
    dirty: bool,
}

impl DiffApp {
    fn new(options: DiffOptions, changeset: Changeset, layout: DiffLayoutMode) -> Self {
        let model = UiModel::new(&changeset, layout);
        let stats = changeset.stats();
        Self {
            options,
            changeset,
            stats,
            model,
            layout,
            scroll: 0,
            viewport_rows: 1,
            selected_file: 0,
            mouse_scroll: MouseScroll::default(),
            notice: None,
            dirty: true,
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> HzResult<bool> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Ok(true);
        }

        self.mouse_scroll.reset();

        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return Ok(true),
            KeyCode::Down | KeyCode::Char('j') => self.scroll_by(1),
            KeyCode::Up | KeyCode::Char('k') => self.scroll_by(-1),
            KeyCode::PageDown | KeyCode::Char('d') => self.scroll_by(20),
            KeyCode::PageUp | KeyCode::Char('u') => self.scroll_by(-20),
            KeyCode::Home | KeyCode::Char('g') => self.set_scroll(0),
            KeyCode::End | KeyCode::Char('G') => self.set_scroll(self.max_scroll()),
            KeyCode::Char('n') | KeyCode::Char('J') => self.move_file(1),
            KeyCode::Char('p') | KeyCode::Char('K') => self.move_file(-1),
            KeyCode::Char(']') => self.next_hunk(),
            KeyCode::Char('[') => self.previous_hunk(),
            KeyCode::Char('s') => self.toggle_layout(),
            KeyCode::Char('r') => self.reload()?,
            _ => {}
        }

        Ok(false)
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) {
        match mouse.kind {
            MouseEventKind::ScrollDown => {
                let delta = self
                    .mouse_scroll
                    .scroll_delta(MouseScrollDirection::Down, Instant::now());
                self.scroll_by(delta);
            }
            MouseEventKind::ScrollUp => {
                let delta = self
                    .mouse_scroll
                    .scroll_delta(MouseScrollDirection::Up, Instant::now());
                self.scroll_by(delta);
            }
            _ => {}
        }
    }

    fn scroll_by(&mut self, delta: isize) {
        let next = if delta < 0 {
            self.scroll.saturating_sub(delta.unsigned_abs())
        } else {
            self.scroll.saturating_add(delta as usize)
        };
        self.set_scroll(next);
    }

    fn set_scroll(&mut self, scroll: usize) {
        let previous_scroll = self.scroll;
        let previous_file = self.selected_file;
        self.scroll = scroll.min(self.max_scroll());
        if let Some(file) = self.model.file_at_row(self.scroll) {
            self.selected_file = file;
        }
        if self.scroll != previous_scroll || self.selected_file != previous_file {
            self.dirty = true;
        }
    }

    fn max_scroll(&self) -> usize {
        max_scroll_for_viewport(self.model.len(), self.viewport_rows)
    }

    fn set_viewport_rows(&mut self, rows: usize) {
        let rows = rows.max(1);
        if self.viewport_rows == rows {
            return;
        }

        self.viewport_rows = rows;
        self.set_scroll(self.scroll);
    }

    fn move_file(&mut self, delta: isize) {
        if self.changeset.files.is_empty() {
            return;
        }

        let next = if delta < 0 {
            self.selected_file.saturating_sub(delta.unsigned_abs())
        } else {
            self.selected_file.saturating_add(delta as usize)
        }
        .min(self.changeset.files.len() - 1);

        self.selected_file = next;
        if let Some(row) = self.model.file_start_row(next) {
            self.set_scroll(row);
        } else {
            self.dirty = true;
        }
    }

    fn next_hunk(&mut self) {
        if let Some(row) = self.model.next_hunk_row(self.scroll) {
            self.set_scroll(row);
        }
    }

    fn previous_hunk(&mut self) {
        if let Some(row) = self.model.previous_hunk_row(self.scroll) {
            self.set_scroll(row);
        }
    }

    fn toggle_layout(&mut self) {
        self.set_layout(self.layout.toggled(), true);
    }

    fn apply_responsive_layout(&mut self, width: u16) {
        self.set_layout(default_layout_for_width(width), true);
        self.dirty = true;
    }

    fn set_layout(&mut self, layout: DiffLayoutMode, show_notice: bool) {
        if self.layout == layout {
            return;
        }

        self.layout = layout;
        self.model = UiModel::new(&self.changeset, self.layout);
        let scroll = self
            .model
            .file_start_row(self.selected_file)
            .unwrap_or_default();
        self.set_scroll(scroll);
        self.dirty = true;
        if show_notice {
            self.notice = Some(self.layout.label().to_owned());
        }
    }

    fn reload(&mut self) -> HzResult<()> {
        let selected_path = self
            .changeset
            .files
            .get(self.selected_file)
            .map(|file| file.display_path().to_owned());
        let changeset = hz_diff::load_review_ref(&self.options)?;
        let selected_file = selected_path
            .and_then(|path| {
                changeset
                    .files
                    .iter()
                    .position(|file| file.display_path() == path)
            })
            .unwrap_or(0);

        self.stats = changeset.stats();
        self.changeset = changeset;
        self.model = UiModel::new(&self.changeset, self.layout);
        self.selected_file = selected_file.min(self.changeset.files.len().saturating_sub(1));
        let scroll = self
            .model
            .file_start_row(self.selected_file)
            .unwrap_or_default();
        self.set_scroll(scroll);
        self.notice = Some("reloaded".to_owned());
        self.dirty = true;
        Ok(())
    }
}

fn max_scroll_for_viewport(row_count: usize, viewport_rows: usize) -> usize {
    row_count.saturating_sub(viewport_rows.max(1))
}

fn draw(frame: &mut Frame<'_>, app: &mut DiffApp) {
    let area = frame.area();
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(area);

    app.set_viewport_rows(vertical[1].height as usize);
    draw_header(frame, app, vertical[0]);
    draw_diff(frame, app, vertical[1]);
}

fn draw_header(frame: &mut Frame<'_>, app: &DiffApp, area: Rect) {
    let notice = app.notice.as_deref().unwrap_or("");
    let line = Line::from(vec![
        Span::styled(
            &app.changeset.title,
            Style::default()
                .fg(Color::Rgb(220, 225, 232))
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(app.layout.label(), Style::default().fg(muted())),
        Span::raw(format!(
            "  {} files  +{} -{}  {}",
            app.stats.files,
            app.stats.additions,
            app.stats.deletions,
            progress_label(app.scroll, app.max_scroll())
        )),
        Span::raw("  "),
        Span::styled(notice, Style::default().fg(Color::Green)),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn draw_diff(frame: &mut Frame<'_>, app: &DiffApp, area: Rect) {
    if app.model.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "No changes.",
                Style::default().fg(Color::DarkGray),
            ))),
            area,
        );
        return;
    }

    let visible_rows = area.height as usize;
    let lines: Vec<Line> = (0..visible_rows)
        .filter_map(|offset| app.model.row(app.scroll + offset))
        .map(|row| render_row(app, row, area.width as usize))
        .collect();

    frame.render_widget(Paragraph::new(Text::from(lines)), area);
}

fn render_row(app: &DiffApp, row: UiRow, width: usize) -> Line<'static> {
    match row {
        UiRow::FileHeader(file_index) => {
            let file = &app.changeset.files[file_index];
            let text = right_aligned(
                &format!("{} {}", status_code(file.status), file.display_path()),
                &format!("+{} -{}", file.additions, file.deletions),
                width,
            );
            Line::from(Span::styled(
                text,
                Style::default().fg(Color::Rgb(215, 218, 224)),
            ))
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
                Style::default().fg(muted()),
            ))
        }
        UiRow::Collapsed { lines } => {
            let label = format!("⋯ {lines} unchanged");
            Line::from(Span::styled(
                fit_padded(&label, width),
                Style::default().fg(muted()),
            ))
        }
        UiRow::HunkHeader { file, hunk } => {
            let hunk = &app.changeset.files[file].hunks[hunk];
            Line::from(Span::styled(
                fit_padded(&hunk.header, width),
                Style::default().fg(Color::Rgb(205, 130, 170)),
            ))
        }
        UiRow::UnifiedLine { file, hunk, line } | UiRow::MetaLine { file, hunk, line } => {
            render_unified_line(&app.changeset.files[file].hunks[hunk].lines[line], width)
        }
        UiRow::SplitLine {
            file,
            hunk,
            left,
            right,
        } => render_split_line(
            &app.changeset.files[file].hunks[hunk].lines,
            left,
            right,
            width,
        ),
    }
}

fn render_unified_line(line: &DiffLine, width: usize) -> Line<'static> {
    if width == 0 {
        return Line::default();
    }

    let sign = match line.kind {
        DiffLineKind::Context => " ",
        DiffLineKind::Addition => "+",
        DiffLineKind::Deletion => "-",
        DiffLineKind::Meta => " ",
    };
    let gutter_width = UNIFIED_GUTTER_WIDTH.min(width);
    let content_width = width.saturating_sub(gutter_width);
    let gutter = format!(
        "{:>5} {:>5} {sign}",
        line.old_line
            .map(|line| line.to_string())
            .unwrap_or_default(),
        line.new_line
            .map(|line| line.to_string())
            .unwrap_or_default()
    );
    let style = line_style(line.kind);
    Line::from(vec![
        Span::styled(
            fit_padded(&gutter, gutter_width),
            Style::default().fg(muted()).bg(row_bg(line.kind)),
        ),
        Span::styled(fit_padded(&line.text, content_width), style),
    ])
}

fn render_split_line(
    lines: &[DiffLine],
    left: Option<usize>,
    right: Option<usize>,
    width: usize,
) -> Line<'static> {
    if width == 0 {
        return Line::default();
    }

    let separator_width = usize::from(width > 1);
    let left_width = width.saturating_sub(separator_width) / 2;
    let right_width = width.saturating_sub(separator_width + left_width);
    let mut spans = split_cell_spans(
        left.and_then(|index| lines.get(index)),
        SplitSide::Old,
        left_width,
    );
    if separator_width > 0 {
        spans.push(Span::styled("│", Style::default().fg(muted())));
    }
    spans.extend(split_cell_spans(
        right.and_then(|index| lines.get(index)),
        SplitSide::New,
        right_width,
    ));
    Line::from(spans)
}

#[derive(Debug, Clone, Copy)]
enum SplitSide {
    Old,
    New,
}

fn split_cell_spans(line: Option<&DiffLine>, side: SplitSide, width: usize) -> Vec<Span<'static>> {
    if width == 0 {
        return Vec::new();
    }

    let Some(line) = line else {
        return vec![Span::raw(" ".repeat(width))];
    };

    let gutter_width = GUTTER_WIDTH.min(width);
    let content_width = width.saturating_sub(gutter_width);
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

    vec![
        Span::styled(
            fit_padded(&format!("{line_number:>5} {sign}"), gutter_width),
            Style::default().fg(muted()).bg(row_bg(line.kind)),
        ),
        Span::styled(fit_padded(&line.text, content_width), line_style(line.kind)),
    ]
}

fn row_bg(kind: DiffLineKind) -> Color {
    match kind {
        DiffLineKind::Addition => addition_bg(),
        DiffLineKind::Deletion => deletion_bg(),
        DiffLineKind::Meta => Color::Reset,
        DiffLineKind::Context => Color::Reset,
    }
}

fn line_style(kind: DiffLineKind) -> Style {
    match kind {
        DiffLineKind::Addition => Style::default().fg(addition_fg()).bg(addition_bg()),
        DiffLineKind::Deletion => Style::default().fg(deletion_fg()).bg(deletion_bg()),
        DiffLineKind::Meta => Style::default().fg(muted()),
        DiffLineKind::Context => Style::default(),
    }
}

fn status_code(status: FileStatus) -> &'static str {
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

fn progress_label(scroll: usize, max_scroll: usize) -> String {
    if max_scroll == 0 {
        return "100%".to_owned();
    }

    format!(
        "{}%",
        scroll.min(max_scroll).saturating_mul(100) / max_scroll
    )
}

fn fit_padded(text: &str, width: usize) -> String {
    let mut out = fit(text, width);
    let len = out.chars().count();
    if len < width {
        out.push_str(&" ".repeat(width - len));
    }
    out
}

fn right_aligned(left: &str, right: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }

    let left_len = left.chars().count();
    let right_len = right.chars().count();
    if left_len + right_len + 1 >= width {
        return fit(&format!("{left}  {right}"), width);
    }

    format!("{left}{}{right}", " ".repeat(width - left_len - right_len))
}

fn fit(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let mut out = String::new();
    for ch in text.chars().take(width) {
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

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
        assert_eq!(max_scroll_for_viewport(10, 0), 9);
    }

    #[test]
    fn app_clamps_scroll_to_last_full_viewport() {
        let changeset = changeset_with_context_lines(10);
        let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);

        app.set_viewport_rows(5);
        app.set_scroll(usize::MAX);

        assert_eq!(app.scroll, app.model.len() - 5);

        app.set_viewport_rows(usize::MAX);

        assert_eq!(app.scroll, 0);
    }

    #[test]
    fn progress_label_is_bounded() {
        assert_eq!(progress_label(0, 0), "100%");
        assert_eq!(progress_label(0, 20), "0%");
        assert_eq!(progress_label(10, 20), "50%");
        assert_eq!(progress_label(100, 20), "100%");
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

    fn changeset_with_context_lines(line_count: usize) -> Changeset {
        let lines = (1..=line_count)
            .map(|line| DiffLine {
                kind: DiffLineKind::Context,
                old_line: Some(line),
                new_line: Some(line),
                text: format!("line {line}"),
            })
            .collect();

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
                    old_count: line_count,
                    new_start: 1,
                    new_count: line_count,
                    lines,
                }],
                additions: 0,
                deletions: 0,
                is_binary: false,
            }],
            raw_patch: String::new(),
        }
    }
}
