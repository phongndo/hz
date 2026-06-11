use std::{
    collections::{HashMap, HashSet},
    fs,
    ops::Range,
    path::Path,
    sync::{Arc, mpsc::Receiver},
    time::{Duration, Instant, SystemTime},
};

use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use hz_core::HzResult;
use hz_diff::{Changeset, DiffOptions, DiffScope, DiffSource, DiffStats};
use hz_syntax::{HighlightedLine, SyntaxLimits, SyntaxSettings};
use unicode_width::UnicodeWidthStr;

use crate::{
    controls::{
        BranchMenu, CrosstermTerminal, DiffChoice, DiffFilterKind, DiffLayoutMode,
        WORKTREE_DIFF_CHOICES, branch_base_from_options, branch_head_from_options,
        branch_match_score, comparison_branches, current_head_label, default_branch_base,
        default_layout_for_width, diff_stats_for_files, filtered_file_indices, grep_match_rows,
    },
    editor::{EditorTarget, configured_editor, open_editor, repo_file_path},
    live_diff::{LiveDiff, LiveDiffReload, live_diff_supported},
    model::{
        ContextExpansionDirection, ContextKey, ContextSourceEntry, ContextSourceKey, UiModel,
        UiRow, context_expansion_direction,
    },
    render::{
        draw,
        menus::{branch_menu_width, diff_menu_width, diff_selector_width},
        sidebar::max_file_sidebar_width,
        text::fit_padded,
    },
    syntax::{
        DiffSide, InlineHunkEmphasisCache, InlineHunkKey, InlineRange, LruCache, SyntaxPosition,
        SyntaxPriority, SyntaxRuntime, available_context_lines, full_file_source,
        load_full_file_source, split_context_source_lines, unified_syntax_side,
    },
    theme::{
        BASE_BRANCH_MARKER, BRANCH_COMPARISON_SEPARATOR, CURRENT_BRANCH_MARKER, DiffTheme,
        EVENT_POLL, FILE_SIDEBAR_MIN_WIDTH, GUTTER_WIDTH, HORIZONTAL_SCROLL_STEP,
        MAX_BRANCH_MENU_ROWS, MAX_INLINE_DIFF_CACHE_ENTRIES, MAX_READY_EVENTS_PER_FRAME,
        MAX_SYNTAX_RESULTS_PER_FRAME, MOUSE_SCROLL_ACCEL_A, MOUSE_SCROLL_ACCEL_TAU,
        MOUSE_SCROLL_HISTORY_SIZE, MOUSE_SCROLL_MAX_MULTIPLIER, MOUSE_SCROLL_MIN_TICK_INTERVAL,
        MOUSE_SCROLL_REFERENCE_INTERVAL_MS, MOUSE_SCROLL_STREAK_TIMEOUT, NOTICE_TTL,
        STATUSLINE_SELECTOR_GAP, SyntaxBenchmarkReport, UNIFIED_GUTTER_WIDTH,
        diff_theme_from_config,
    },
};

const MOUSE_HUNK_FOCUS_SCROLL_TICKS: isize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HunkFocusScrollBehavior {
    Preserve,
    ClearOnScroll,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HunkFocusModelBehavior {
    PreserveIfValid,
    Clear,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditorReloadBehavior {
    None,
    Live,
    Sync,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FileFingerprint {
    len: u64,
    modified: Option<SystemTime>,
}

impl FileFingerprint {
    pub(crate) fn read(path: &Path) -> Option<Self> {
        let metadata = fs::metadata(path).ok()?;
        Some(Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
        })
    }
}

pub(crate) fn file_changed_since(path: &Path, before: Option<FileFingerprint>) -> bool {
    let after = FileFingerprint::read(path);
    match (before, after) {
        (Some(before), Some(after)) => before != after,
        (None, None) => false,
        _ => true,
    }
}

pub(crate) fn run_loop(
    terminal: &mut CrosstermTerminal,
    app: &mut DiffApp,
    live_updates: bool,
    live_diff: &mut Option<LiveDiff>,
) -> HzResult<()> {
    loop {
        sync_live_diff(live_diff, app, live_updates);
        app.expire_notice(Instant::now());
        drain_live_reloads(
            app,
            live_diff.as_ref().map(|live_diff| &live_diff.reload_rx),
        );
        app.drain_syntax();
        if app.dirty {
            if app.terminal_clear_requested {
                terminal.clear()?;
                app.terminal_clear_requested = false;
            }
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

pub(crate) fn sync_live_diff(
    live_diff: &mut Option<LiveDiff>,
    app: &mut DiffApp,
    live_updates: bool,
) {
    if !live_updates || !live_diff_supported(&app.options) {
        *live_diff = None;
        app.live_diff_failed_options = None;
        return;
    }

    if live_diff
        .as_ref()
        .is_some_and(|live_diff| live_diff.options == app.options)
    {
        return;
    }
    if app.live_diff_failed_options.as_ref() == Some(&app.options) {
        return;
    }

    match LiveDiff::start(app.options.clone(), &app.changeset.repo) {
        Ok(next_live_diff) => {
            app.live_diff_failed_options = None;
            *live_diff = Some(next_live_diff);
        }
        Err(error) => {
            *live_diff = None;
            app.live_diff_failed_options = Some(app.options.clone());
            app.set_notice(format!("live reload unavailable: {error}"));
        }
    }
}

pub(crate) fn drain_live_reloads(
    app: &mut DiffApp,
    live_reload_rx: Option<&Receiver<LiveDiffReload>>,
) {
    let Some(live_reload_rx) = live_reload_rx else {
        return;
    };

    while let Ok(reload) = live_reload_rx.try_recv() {
        match reload {
            LiveDiffReload::Loaded(Ok(changeset)) => app.replace_changeset(changeset, None),
            LiveDiffReload::Loaded(Err(error)) => {
                app.set_notice(format!("live reload failed: {error}"));
            }
        }
    }
}

pub(crate) fn handle_event(app: &mut DiffApp, event: Event) -> HzResult<bool> {
    match event {
        Event::Key(key) if app.handle_key(key)? => Ok(true),
        Event::Mouse(mouse) => {
            app.handle_mouse(mouse)?;
            Ok(false)
        }
        Event::Resize(width, _) => {
            app.apply_responsive_layout(width);
            Ok(false)
        }
        _ => Ok(false),
    }
}

pub(crate) fn is_plain_char_key(key: KeyEvent, character: char) -> bool {
    key.code == KeyCode::Char(character)
        && !key.modifiers.contains(KeyModifiers::CONTROL)
        && !key.modifiers.contains(KeyModifiers::ALT)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MouseScrollDirection {
    Up,
    Down,
}

#[derive(Debug, Default)]
pub(crate) struct MouseScroll {
    pub(crate) last_tick: Option<Instant>,
    pub(crate) direction: Option<MouseScrollDirection>,
    pub(crate) intervals: Vec<Duration>,
    pub(crate) pending_lines: f64,
    pub(crate) pending_hunk_focus_ticks: isize,
}

impl MouseScroll {
    pub(crate) fn scroll_delta(&mut self, direction: MouseScrollDirection, now: Instant) -> isize {
        let multiplier = self.multiplier(direction, now);
        self.pending_lines += multiplier;
        let lines = self.pending_lines.trunc() as isize;
        self.pending_lines -= lines as f64;

        match direction {
            MouseScrollDirection::Down => lines,
            MouseScrollDirection::Up => -lines,
        }
    }

    pub(crate) fn reset(&mut self) {
        self.last_tick = None;
        self.direction = None;
        self.intervals.clear();
        self.pending_lines = 0.0;
        self.pending_hunk_focus_ticks = 0;
    }

    pub(crate) fn reset_hunk_focus_ticks(&mut self) {
        self.pending_hunk_focus_ticks = 0;
    }

    pub(crate) fn hunk_focus_delta(&mut self, direction: MouseScrollDirection) -> isize {
        match direction {
            MouseScrollDirection::Down => self.pending_hunk_focus_ticks += 1,
            MouseScrollDirection::Up => self.pending_hunk_focus_ticks -= 1,
        }

        if self.pending_hunk_focus_ticks >= MOUSE_HUNK_FOCUS_SCROLL_TICKS {
            self.pending_hunk_focus_ticks -= MOUSE_HUNK_FOCUS_SCROLL_TICKS;
            1
        } else if self.pending_hunk_focus_ticks <= -MOUSE_HUNK_FOCUS_SCROLL_TICKS {
            self.pending_hunk_focus_ticks += MOUSE_HUNK_FOCUS_SCROLL_TICKS;
            -1
        } else {
            0
        }
    }

    pub(crate) fn multiplier(&mut self, direction: MouseScrollDirection, now: Instant) -> f64 {
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

    pub(crate) fn start_streak(&mut self, direction: MouseScrollDirection, now: Instant) {
        self.last_tick = Some(now);
        self.direction = Some(direction);
        self.intervals.clear();
        self.pending_lines = 0.0;
        self.pending_hunk_focus_ticks = 0;
    }
}

#[derive(Debug)]
pub(crate) struct Notice {
    pub(crate) text: String,
    pub(crate) expires_at: Instant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SyntaxStartupMode {
    Config,
    Disabled,
    Languages(Vec<String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HunkFocusSearch {
    FirstVisible,
    NearestTo(usize),
}

#[derive(Debug)]
pub(crate) struct DiffApp {
    pub(crate) options: DiffOptions,
    pub(crate) base_changeset: Changeset,
    pub(crate) changeset: Changeset,
    pub(crate) total_stats: DiffStats,
    pub(crate) stats: DiffStats,
    pub(crate) model: UiModel,
    pub(crate) layout: DiffLayoutMode,
    pub(crate) scroll: usize,
    pub(crate) horizontal_scroll: usize,
    pub(crate) viewport_rows: usize,
    pub(crate) viewport_width: usize,
    pub(crate) max_line_width: usize,
    pub(crate) manual_hunk_focus: Option<(usize, usize)>,
    pub(crate) selected_file: usize,
    pub(crate) file_sidebar_open: bool,
    pub(crate) file_sidebar_scroll: usize,
    pub(crate) file_sidebar_width: Option<u16>,
    pub(crate) file_sidebar_render_width: u16,
    pub(crate) file_sidebar_resizing: bool,
    pub(crate) help_menu_open: bool,
    pub(crate) diff_menu_open: bool,
    pub(crate) filter_input: Option<DiffFilterKind>,
    pub(crate) file_filter: String,
    pub(crate) file_filter_input: String,
    pub(crate) grep_filter: String,
    pub(crate) grep_filter_input: String,
    pub(crate) grep_matches: Vec<usize>,
    pub(crate) selected_grep_match: Option<usize>,
    pub(crate) branch_menu_open: Option<BranchMenu>,
    pub(crate) branch_menu_input: String,
    pub(crate) branch_menu_scroll: usize,
    pub(crate) branch_menu_selected: usize,
    pub(crate) branch_base: Option<String>,
    pub(crate) branch_head: Option<String>,
    pub(crate) current_head: Option<String>,
    pub(crate) comparison_branches: Vec<String>,
    pub(crate) live_diff_failed_options: Option<DiffOptions>,
    pub(crate) live_updates_enabled: bool,
    pub(crate) mouse_scroll: MouseScroll,
    pub(crate) notice: Option<Notice>,
    pub(crate) theme: DiffTheme,
    pub(crate) context_expansions: HashMap<ContextKey, usize>,
    pub(crate) context_cache: HashMap<ContextSourceKey, ContextSourceEntry>,
    pub(crate) syntax_limits: SyntaxLimits,
    pub(crate) syntax: Option<SyntaxRuntime>,
    pub(crate) inline_cache: LruCache<InlineHunkKey, InlineHunkEmphasisCache>,
    pub(crate) generation: u64,
    pub(crate) terminal_clear_requested: bool,
    pub(crate) dirty: bool,
}

pub(crate) fn load_syntax_settings_for_diff(
    load_user_settings: bool,
) -> (SyntaxSettings, Option<Notice>) {
    if !load_user_settings {
        return (SyntaxSettings::default(), None);
    }

    match hz_syntax::load_settings() {
        Ok(settings) => (settings, None),
        Err(error) => (
            SyntaxSettings::default(),
            Some(Notice {
                text: format!("syntax settings ignored: {error}"),
                expires_at: Instant::now() + NOTICE_TTL,
            }),
        ),
    }
}

impl DiffApp {
    #[cfg(test)]
    pub(crate) fn new(options: DiffOptions, changeset: Changeset, layout: DiffLayoutMode) -> Self {
        Self::new_with_syntax(options, changeset, layout, SyntaxStartupMode::Config)
    }

    pub(crate) fn new_with_syntax(
        options: DiffOptions,
        changeset: Changeset,
        layout: DiffLayoutMode,
        syntax_mode: SyntaxStartupMode,
    ) -> Self {
        let context_expansions = HashMap::new();
        let context_cache = HashMap::new();
        let model = UiModel::new(&changeset, layout, &context_expansions);
        let manual_hunk_focus = model
            .hunk_start_rows
            .first()
            .and_then(|row| model.row(*row).and_then(UiRow::hunk_key));
        let stats = changeset.stats();
        let total_stats = stats.clone();
        let branch_base = default_branch_base(&options, &changeset.repo);
        let current_head = current_head_label(&changeset.repo);
        let branch_head = branch_head_from_options(&options, current_head.as_deref());
        let comparison_branches = comparison_branches(
            &changeset.repo,
            &[
                current_head.as_deref(),
                branch_head.as_deref(),
                branch_base.as_deref(),
            ],
        );
        let (settings, mut notice) = load_syntax_settings_for_diff(matches!(
            syntax_mode,
            SyntaxStartupMode::Config | SyntaxStartupMode::Disabled
        ));
        let theme = match diff_theme_from_config(&settings.theme).and_then(|theme| {
            theme
                .with_color_overrides(&settings.colors)
                .map(|theme| theme.with_transparent_background(settings.transparent_background))
        }) {
            Ok(theme) => theme.with_diff_settings(settings.diff),
            Err(error) => {
                notice = Some(Notice {
                    text: format!("colorscheme ignored: {error}"),
                    expires_at: Instant::now() + NOTICE_TTL,
                });
                DiffTheme::default()
                    .with_color_overrides(&settings.colors)
                    .unwrap_or_else(|_| DiffTheme::default())
                    .with_transparent_background(settings.transparent_background)
                    .with_diff_settings(settings.diff)
            }
        };
        let syntax_limits = settings.limits;
        let syntax = match syntax_mode {
            SyntaxStartupMode::Config => match SyntaxRuntime::start(&settings) {
                Ok(syntax) => syntax,
                Err(error) => {
                    notice = Some(Notice {
                        text: format!("syntax disabled: {error}"),
                        expires_at: Instant::now() + NOTICE_TTL,
                    });
                    None
                }
            },
            SyntaxStartupMode::Disabled => None,
            SyntaxStartupMode::Languages(languages) => {
                SyntaxRuntime::start_with_languages(languages, syntax_limits)
            }
        };
        let max_line_width = changeset_max_line_width(&changeset);
        Self {
            options,
            base_changeset: changeset.clone(),
            changeset,
            total_stats,
            stats,
            model,
            layout,
            scroll: 0,
            horizontal_scroll: 0,
            viewport_rows: 1,
            viewport_width: 1,
            max_line_width,
            manual_hunk_focus,
            selected_file: 0,
            file_sidebar_open: false,
            file_sidebar_scroll: 0,
            file_sidebar_width: None,
            file_sidebar_render_width: 0,
            file_sidebar_resizing: false,
            help_menu_open: false,
            diff_menu_open: false,
            filter_input: None,
            file_filter: String::new(),
            file_filter_input: String::new(),
            grep_filter: String::new(),
            grep_filter_input: String::new(),
            grep_matches: Vec::new(),
            selected_grep_match: None,
            branch_menu_open: None,
            branch_menu_input: String::new(),
            branch_menu_scroll: 0,
            branch_menu_selected: 0,
            branch_base,
            branch_head,
            current_head,
            comparison_branches,
            live_diff_failed_options: None,
            live_updates_enabled: false,
            mouse_scroll: MouseScroll::default(),
            notice,
            theme,
            context_expansions,
            context_cache,
            syntax_limits,
            syntax,
            inline_cache: LruCache::new(MAX_INLINE_DIFF_CACHE_ENTRIES),
            generation: 0,
            terminal_clear_requested: false,
            dirty: true,
        }
    }

    pub(crate) fn handle_key(&mut self, key: KeyEvent) -> HzResult<bool> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Ok(true);
        }

        self.mouse_scroll.reset();

        if self.filter_input.is_some() && self.handle_filter_input_key(key) {
            return Ok(false);
        }

        if self.help_menu_open {
            if key.code == KeyCode::Esc || is_plain_char_key(key, '?') {
                self.close_help_menu();
                return Ok(false);
            }
            if key.code == KeyCode::Char('q') {
                return Ok(true);
            }
            return Ok(false);
        }

        if self.branch_menu_open.is_some() {
            match key.code {
                KeyCode::Esc => {
                    self.close_branch_menu();
                    return Ok(false);
                }
                KeyCode::Enter => {
                    self.select_highlighted_branch_match();
                    return Ok(false);
                }
                KeyCode::Tab => {
                    self.cycle_branch_completion(1);
                    return Ok(false);
                }
                KeyCode::BackTab => {
                    self.cycle_branch_completion(-1);
                    return Ok(false);
                }
                KeyCode::Backspace => {
                    self.pop_branch_input();
                    return Ok(false);
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.clear_branch_input();
                    return Ok(false);
                }
                KeyCode::Down => {
                    self.move_branch_selection(1);
                    return Ok(false);
                }
                KeyCode::Up => {
                    self.move_branch_selection(-1);
                    return Ok(false);
                }
                KeyCode::PageDown => {
                    self.move_branch_selection(MAX_BRANCH_MENU_ROWS as isize);
                    return Ok(false);
                }
                KeyCode::PageUp => {
                    self.move_branch_selection(-(MAX_BRANCH_MENU_ROWS as isize));
                    return Ok(false);
                }
                KeyCode::Home => {
                    self.set_branch_selection(0);
                    return Ok(false);
                }
                KeyCode::End => {
                    self.set_branch_selection(usize::MAX);
                    return Ok(false);
                }
                KeyCode::Char(character)
                    if !key.modifiers.contains(KeyModifiers::CONTROL)
                        && !key.modifiers.contains(KeyModifiers::ALT) =>
                {
                    self.push_branch_input(character);
                    return Ok(false);
                }
                _ => {}
            }
        }

        if is_plain_char_key(key, '?') {
            self.toggle_help_menu();
            return Ok(false);
        }

        if self.diff_menu_open {
            if key.code == KeyCode::Esc {
                self.diff_menu_open = false;
                self.dirty = true;
                return Ok(false);
            }
            if key.code == KeyCode::Char('q') {
                return Ok(true);
            }
        }

        match key.code {
            KeyCode::Esc if self.filters_active() => self.clear_all_filters(),
            KeyCode::Esc | KeyCode::Char('q') => return Ok(true),
            KeyCode::Down | KeyCode::Char('j') => self.scroll_or_focus_hunk(1),
            KeyCode::Up | KeyCode::Char('k') => self.scroll_or_focus_hunk(-1),
            KeyCode::Left | KeyCode::Char('h') => {
                self.scroll_horizontally_by(-(HORIZONTAL_SCROLL_STEP as isize));
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.scroll_horizontally_by(HORIZONTAL_SCROLL_STEP as isize);
            }
            KeyCode::PageDown | KeyCode::Char('d') => self.scroll_or_focus_hunk(20),
            KeyCode::PageUp | KeyCode::Char('u') => self.scroll_or_focus_hunk(-20),
            KeyCode::Char('g') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.open_focused_hunk_in_editor();
            }
            KeyCode::Home | KeyCode::Char('g') => self.set_scroll(0),
            KeyCode::End | KeyCode::Char('G') => self.set_scroll(self.max_scroll()),
            KeyCode::Char('f') => self.open_filter_input(DiffFilterKind::File),
            KeyCode::Char('/') => self.open_filter_input(DiffFilterKind::Grep),
            KeyCode::Char('n') if !self.grep_filter.is_empty() => self.move_grep_match(1),
            KeyCode::Char('p') if !self.grep_filter.is_empty() => self.move_grep_match(-1),
            KeyCode::Char('N') if !self.grep_filter.is_empty() => self.move_grep_match(-1),
            KeyCode::Char('n') | KeyCode::Char('J') => self.move_file(1),
            KeyCode::Char('p') | KeyCode::Char('K') => self.move_file(-1),
            KeyCode::Char('b') => self.toggle_file_sidebar(),
            KeyCode::Char(']') => self.next_hunk(),
            KeyCode::Char('[') => self.previous_hunk(),
            KeyCode::Char('s') => self.toggle_layout(),
            KeyCode::Char('r') => self.reload()?,
            _ => {}
        }

        Ok(false)
    }

    pub(crate) fn set_live_updates_enabled(&mut self, enabled: bool) {
        self.live_updates_enabled = enabled;
    }

    pub(crate) fn toggle_help_menu(&mut self) {
        self.help_menu_open = !self.help_menu_open;
        self.dirty = true;
    }

    pub(crate) fn close_help_menu(&mut self) {
        if self.help_menu_open {
            self.help_menu_open = false;
            self.dirty = true;
        }
    }

    pub(crate) fn set_notice(&mut self, text: impl Into<String>) {
        self.notice = Some(Notice {
            text: text.into(),
            expires_at: Instant::now() + NOTICE_TTL,
        });
        self.dirty = true;
    }

    pub(crate) fn expire_notice(&mut self, now: Instant) {
        let expired = self
            .notice
            .as_ref()
            .is_some_and(|notice| now >= notice.expires_at);
        if expired {
            self.notice = None;
            self.dirty = true;
        }
    }

    pub(crate) fn handle_mouse(&mut self, mouse: MouseEvent) -> HzResult<()> {
        if self.help_menu_open {
            if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                self.close_help_menu();
            }
            self.mouse_scroll.reset();
            return Ok(());
        }

        if self.file_sidebar_resizing {
            match mouse.kind {
                MouseEventKind::Drag(MouseButton::Left) | MouseEventKind::Moved => {
                    self.resize_file_sidebar_to_column(mouse.column);
                    return Ok(());
                }
                MouseEventKind::Up(MouseButton::Left) => {
                    self.file_sidebar_resizing = false;
                    self.resize_file_sidebar_to_column(mouse.column);
                    return Ok(());
                }
                _ => {}
            }
        }

        if self.branch_menu_open.is_some() {
            match mouse.kind {
                MouseEventKind::ScrollDown => {
                    self.move_branch_selection(1);
                    return Ok(());
                }
                MouseEventKind::ScrollUp => {
                    self.move_branch_selection(-1);
                    return Ok(());
                }
                _ => {}
            }
        }

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if self.start_file_sidebar_resize(mouse.column, mouse.row) {
                    return Ok(());
                }
                self.handle_click(mouse.column, mouse.row);
            }
            MouseEventKind::ScrollDown => {
                if self.is_file_sidebar_position(mouse.column, mouse.row) {
                    self.mouse_scroll.reset();
                    self.scroll_file_sidebar_by(1);
                    return Ok(());
                }
                self.mouse_scroll_or_focus_hunk(MouseScrollDirection::Down);
            }
            MouseEventKind::ScrollUp => {
                if self.is_file_sidebar_position(mouse.column, mouse.row) {
                    self.mouse_scroll.reset();
                    self.scroll_file_sidebar_by(-1);
                    return Ok(());
                }
                self.mouse_scroll_or_focus_hunk(MouseScrollDirection::Up);
            }
            MouseEventKind::ScrollLeft => {
                if self.is_file_sidebar_position(mouse.column, mouse.row) {
                    self.mouse_scroll.reset();
                    return Ok(());
                }
                self.scroll_horizontally_by(-(HORIZONTAL_SCROLL_STEP as isize));
            }
            MouseEventKind::ScrollRight => {
                if self.is_file_sidebar_position(mouse.column, mouse.row) {
                    self.mouse_scroll.reset();
                    return Ok(());
                }
                self.scroll_horizontally_by(HORIZONTAL_SCROLL_STEP as isize);
            }
            _ => {}
        }
        Ok(())
    }

    pub(crate) fn is_file_sidebar_position(&self, column: u16, row: u16) -> bool {
        self.file_sidebar_open
            && self.file_sidebar_render_width > 0
            && column < self.file_sidebar_render_width
            && row > 0
    }

    pub(crate) fn is_file_sidebar_resize_handle(&self, column: u16, row: u16) -> bool {
        self.is_file_sidebar_position(column, row)
            && column.saturating_add(1) == self.file_sidebar_render_width
    }

    pub(crate) fn start_file_sidebar_resize(&mut self, column: u16, row: u16) -> bool {
        if !self.is_file_sidebar_resize_handle(column, row) {
            return false;
        }

        self.file_sidebar_resizing = true;
        self.resize_file_sidebar_to_column(column);
        true
    }

    pub(crate) fn resize_file_sidebar_to_column(&mut self, column: u16) {
        let width = column.saturating_add(1);
        self.set_file_sidebar_width(width);
    }

    pub(crate) fn handle_click(&mut self, column: u16, row: u16) {
        let clicked_selector = row == 0 && column < diff_selector_width(&self.options);
        let clicked_branch_selector = (row == 0)
            .then(|| self.branch_selector_at(column))
            .flatten();

        if let Some(menu) = self.branch_menu_open {
            if let Some(branch) = self.branch_choice_at(menu, column, row) {
                self.close_branch_menu();
                self.select_branch(menu, branch);
                return;
            }

            if let Some(clicked_menu) = clicked_branch_selector {
                self.toggle_branch_menu(clicked_menu);
                return;
            }

            self.close_branch_menu();
            if clicked_selector {
                self.toggle_diff_menu();
            }
            return;
        }

        if self.diff_menu_open {
            if let Some(choice) = self.diff_choice_at(column, row) {
                self.diff_menu_open = false;
                self.select_diff_choice(choice);
                return;
            }

            if let Some(menu) = clicked_branch_selector {
                self.diff_menu_open = false;
                self.toggle_branch_menu(menu);
                return;
            }

            if clicked_selector {
                self.toggle_diff_menu();
                return;
            }

            self.diff_menu_open = false;
            self.dirty = true;
            return;
        }

        if clicked_selector {
            self.toggle_diff_menu();
        } else if let Some(menu) = clicked_branch_selector {
            self.toggle_branch_menu(menu);
        } else if !self.handle_file_sidebar_click(column, row) {
            self.handle_diff_click(column, row);
        }
    }

    pub(crate) fn handle_file_sidebar_click(&mut self, column: u16, row: u16) -> bool {
        if !self.is_file_sidebar_position(column, row) {
            return false;
        }

        let position = self
            .file_sidebar_scroll
            .saturating_add(usize::from(row - 1));
        let Some(file) = self.model.visible_files().get(position).copied() else {
            return false;
        };

        self.select_file(file);
        true
    }

    pub(crate) fn handle_diff_click(&mut self, column: u16, row: u16) -> bool {
        if row == 0 || self.is_file_sidebar_position(column, row) {
            return false;
        }

        let row_index = self.scroll.saturating_add(usize::from(row - 1));
        self.handle_context_at_row(row_index)
    }

    pub(crate) fn handle_context_at_row(&mut self, row_index: usize) -> bool {
        match self.model.row(row_index) {
            Some(UiRow::Collapsed { .. }) => self.expand_context_at_row(row_index),
            Some(UiRow::ContextHide { file, hunk, .. }) => self.hide_context(file, hunk),
            _ => false,
        }
    }

    pub(crate) fn expand_context_at_row(&mut self, row_index: usize) -> bool {
        let Some(UiRow::Collapsed {
            file,
            hunk,
            old_start,
            new_start,
            lines,
            expanded,
        }) = self.model.row(row_index)
        else {
            return false;
        };

        let Some((side, source_lines)) = self.ensure_context_lines(file) else {
            self.set_notice("context unavailable for this diff");
            return true;
        };

        let total = lines.saturating_add(expanded);
        let source_start = match side {
            DiffSide::Old => old_start,
            DiffSide::New => new_start,
        };
        let available = available_context_lines(source_start, total, source_lines.len());
        let current = expanded.min(available);
        let remaining = available.saturating_sub(current);
        if remaining == 0 {
            self.set_notice("no more context");
            return true;
        }

        let next = current.saturating_add(self.context_expand_count(remaining));
        self.update_max_line_width_for_expanded_context(
            &source_lines,
            source_start,
            total,
            current,
            next,
            context_expansion_direction(hunk),
        );
        self.context_expansions
            .insert(ContextKey { file, hunk }, next);
        let visible_files =
            filtered_file_indices(&self.changeset, &self.file_filter, &self.grep_filter);
        self.replace_model(&visible_files, HunkFocusModelBehavior::PreserveIfValid);
        self.grep_matches = grep_match_rows(&self.changeset, &self.model, &self.grep_filter);
        self.selected_grep_match = None;
        self.set_scroll_with_grep_sync(self.scroll, true, HunkFocusScrollBehavior::Preserve);
        self.sync_grep_match_selection_to_scroll();
        self.set_horizontal_scroll(self.horizontal_scroll);
        self.dirty = true;
        true
    }

    pub(crate) fn hide_context(&mut self, file: usize, hunk: usize) -> bool {
        if self
            .context_expansions
            .remove(&ContextKey { file, hunk })
            .is_none()
        {
            return false;
        }

        let visible_files =
            filtered_file_indices(&self.changeset, &self.file_filter, &self.grep_filter);
        self.replace_model(&visible_files, HunkFocusModelBehavior::PreserveIfValid);
        self.grep_matches = grep_match_rows(&self.changeset, &self.model, &self.grep_filter);
        self.selected_grep_match = None;
        self.set_scroll_with_grep_sync(self.scroll, true, HunkFocusScrollBehavior::Preserve);
        self.sync_grep_match_selection_to_scroll();
        self.set_horizontal_scroll(self.horizontal_scroll);
        self.dirty = true;
        true
    }

    pub(crate) fn context_expand_count(&self, available: usize) -> usize {
        self.theme.diff.context_expansion.expand_count(available)
    }

    pub(crate) fn ensure_context_lines(
        &mut self,
        file: usize,
    ) -> Option<(DiffSide, Arc<Vec<String>>)> {
        for side in [DiffSide::New, DiffSide::Old] {
            if !self.has_context_source(file, side) {
                continue;
            }
            if let Some(lines) = self.context_lines(file, side) {
                return Some((side, lines));
            }
        }
        None
    }

    pub(crate) fn has_context_source(&self, file: usize, side: DiffSide) -> bool {
        self.changeset
            .files
            .get(file)
            .and_then(|file_diff| {
                full_file_source(&self.changeset.repo, &self.options, file_diff, side)
            })
            .is_some()
    }

    pub(crate) fn context_source_side(&self, file: usize) -> Option<DiffSide> {
        for side in [DiffSide::New, DiffSide::Old] {
            match self.context_cache.get(&ContextSourceKey { file, side }) {
                Some(ContextSourceEntry::Lines(_)) => return Some(side),
                Some(ContextSourceEntry::Unavailable) => continue,
                None if self.has_context_source(file, side) => return Some(side),
                None => {}
            }
        }
        None
    }

    pub(crate) fn context_lines(
        &mut self,
        file: usize,
        side: DiffSide,
    ) -> Option<Arc<Vec<String>>> {
        let key = ContextSourceKey { file, side };
        if !self.context_cache.contains_key(&key) {
            let entry = self
                .load_context_lines(file, side)
                .map(ContextSourceEntry::Lines)
                .unwrap_or(ContextSourceEntry::Unavailable);
            self.context_cache.insert(key, entry);
        }

        match self.context_cache.get(&key) {
            Some(ContextSourceEntry::Lines(lines)) => Some(Arc::clone(lines)),
            Some(ContextSourceEntry::Unavailable) | None => None,
        }
    }

    pub(crate) fn load_context_lines(
        &self,
        file: usize,
        side: DiffSide,
    ) -> Option<Arc<Vec<String>>> {
        let file_diff = self.changeset.files.get(file)?;
        let source = full_file_source(&self.changeset.repo, &self.options, file_diff, side)?;
        let text = load_full_file_source(&source).ok()?;
        Some(Arc::new(split_context_source_lines(&text)))
    }

    pub(crate) fn context_line_text(
        &mut self,
        file: usize,
        old_line: usize,
        new_line: usize,
    ) -> String {
        let Some((side, source_lines)) = self.ensure_context_lines(file) else {
            return "context unavailable".to_owned();
        };
        let line_number = match side {
            DiffSide::Old => old_line,
            DiffSide::New => new_line,
        };
        let Some(line_index) = line_number.checked_sub(1) else {
            return String::new();
        };
        source_lines.get(line_index).cloned().unwrap_or_default()
    }

    pub(crate) fn update_max_line_width_for_expanded_context(
        &mut self,
        source_lines: &[String],
        source_start: usize,
        total: usize,
        current: usize,
        next: usize,
        direction: ContextExpansionDirection,
    ) {
        let Some(source_index_start) = source_start.checked_sub(1) else {
            return;
        };
        let (newly_visible_start, newly_visible_end) = match direction {
            ContextExpansionDirection::Up => {
                (total.saturating_sub(next), total.saturating_sub(current))
            }
            ContextExpansionDirection::Down => (current, next),
        };
        for offset in newly_visible_start..newly_visible_end {
            let Some(text) = source_lines.get(source_index_start + offset) else {
                continue;
            };
            self.max_line_width = self.max_line_width.max(text.width());
        }
    }

    pub(crate) fn toggle_diff_menu(&mut self) {
        if self.diff_menu_choices().is_empty() {
            return;
        }
        self.diff_menu_open = !self.diff_menu_open;
        self.branch_menu_open = None;
        self.dirty = true;
    }

    pub(crate) fn close_branch_menu(&mut self) {
        if self.branch_menu_open.is_some()
            || !self.branch_menu_input.is_empty()
            || self.branch_menu_scroll != 0
        {
            self.branch_menu_open = None;
            self.branch_menu_input.clear();
            self.branch_menu_scroll = 0;
            self.branch_menu_selected = 0;
            self.dirty = true;
        }
    }

    pub(crate) fn toggle_branch_menu(&mut self, menu: BranchMenu) {
        if !self.is_branch_diff() || self.comparison_branches.is_empty() {
            return;
        }
        if self.branch_menu_open == Some(menu) {
            self.close_branch_menu();
            return;
        }

        self.branch_menu_open = Some(menu);
        self.diff_menu_open = false;
        self.branch_menu_input.clear();
        self.branch_menu_selected = self
            .branch_ref(menu)
            .and_then(|branch| {
                self.filtered_branches()
                    .iter()
                    .position(|candidate| *candidate == branch)
            })
            .unwrap_or_default()
            .min(self.max_branch_menu_selection());
        self.ensure_branch_selection_visible();
        self.dirty = true;
    }

    pub(crate) fn branch_selector_at(&self, column: u16) -> Option<BranchMenu> {
        [BranchMenu::Head, BranchMenu::Base]
            .into_iter()
            .find(|menu| {
                let Some(start) = self.branch_selector_start(*menu) else {
                    return false;
                };
                let Some(width) = self.branch_selector_width(*menu) else {
                    return false;
                };
                column >= start && column < start.saturating_add(width)
            })
    }

    pub(crate) fn branch_choice_at(
        &self,
        menu: BranchMenu,
        column: u16,
        row: u16,
    ) -> Option<String> {
        let start = self.branch_selector_start(menu)?;
        let width = self.branch_menu_width();
        if column < start || column >= start.saturating_add(width) || row == 0 {
            return None;
        }

        let row_index = usize::from(row - 1);
        if row_index >= self.visible_branch_menu_rows() {
            return None;
        }

        self.filtered_branch(row_index).map(str::to_owned)
    }

    pub(crate) fn filtered_branch(&self, row_index: usize) -> Option<&str> {
        self.filtered_branches()
            .get(self.branch_menu_scroll.saturating_add(row_index))
            .copied()
    }

    pub(crate) fn move_branch_selection(&mut self, delta: isize) {
        let next = if delta < 0 {
            self.branch_menu_selected
                .saturating_sub(delta.unsigned_abs())
        } else {
            self.branch_menu_selected.saturating_add(delta as usize)
        };
        self.set_branch_selection(next);
    }

    pub(crate) fn set_branch_selection(&mut self, selected: usize) {
        let selected = selected.min(self.max_branch_menu_selection());
        if self.branch_menu_selected != selected {
            self.branch_menu_selected = selected;
            self.ensure_branch_selection_visible();
            self.dirty = true;
        }
    }

    pub(crate) fn cycle_branch_completion(&mut self, delta: isize) {
        let len = self.filtered_branches().len();
        if len == 0 {
            return;
        }

        let next = if delta < 0 {
            self.branch_menu_selected
                .checked_sub(1)
                .unwrap_or(len.saturating_sub(1))
        } else {
            (self.branch_menu_selected + 1) % len
        };
        self.set_branch_selection(next);
    }

    pub(crate) fn ensure_branch_selection_visible(&mut self) {
        let max_scroll = self.max_branch_menu_scroll();
        if self.branch_menu_selected < self.branch_menu_scroll {
            self.branch_menu_scroll = self.branch_menu_selected;
        } else if self.branch_menu_selected
            >= self.branch_menu_scroll.saturating_add(MAX_BRANCH_MENU_ROWS)
        {
            self.branch_menu_scroll = self
                .branch_menu_selected
                .saturating_add(1)
                .saturating_sub(MAX_BRANCH_MENU_ROWS);
        }
        self.branch_menu_scroll = self.branch_menu_scroll.min(max_scroll);
    }

    pub(crate) fn max_branch_menu_selection(&self) -> usize {
        self.filtered_branches().len().saturating_sub(1)
    }

    pub(crate) fn max_branch_menu_scroll(&self) -> usize {
        self.filtered_branches()
            .len()
            .saturating_sub(MAX_BRANCH_MENU_ROWS)
    }

    pub(crate) fn visible_branch_menu_rows(&self) -> usize {
        self.filtered_branches().len().min(MAX_BRANCH_MENU_ROWS)
    }

    pub(crate) fn branch_menu_height(&self) -> usize {
        self.visible_branch_menu_rows()
            .max(usize::from(self.filtered_branches().is_empty()))
    }

    pub(crate) fn filtered_branches(&self) -> Vec<&str> {
        let menu = self.branch_menu_open.unwrap_or(BranchMenu::Base);
        let query = self.branch_menu_input.trim().to_ascii_lowercase();
        if query.is_empty() {
            let mut matches: Vec<_> = self
                .comparison_branches
                .iter()
                .enumerate()
                .map(|(index, branch)| (self.branch_pin_rank(menu, branch), index, branch.as_str()))
                .collect();
            matches.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
            return matches.into_iter().map(|(_, _, branch)| branch).collect();
        }

        let mut matches: Vec<_> = self
            .comparison_branches
            .iter()
            .enumerate()
            .filter_map(|(index, branch)| {
                branch_match_score(&query, branch).map(|score| {
                    (
                        self.branch_pin_rank(menu, branch),
                        score,
                        branch.len(),
                        index,
                        branch.as_str(),
                    )
                })
            })
            .collect();
        matches.sort_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| left.1.cmp(&right.1))
                .then_with(|| left.2.cmp(&right.2))
                .then_with(|| left.3.cmp(&right.3))
                .then_with(|| left.4.cmp(right.4))
        });
        matches
            .into_iter()
            .map(|(_, _, _, _, branch)| branch)
            .collect()
    }

    pub(crate) fn branch_pin_rank(&self, menu: BranchMenu, branch: &str) -> usize {
        let current = self.current_head.as_deref();
        let base = self.branch_base.as_deref();
        match menu {
            BranchMenu::Head => {
                if current == Some(branch) {
                    0
                } else if base == Some(branch) {
                    1
                } else {
                    2
                }
            }
            BranchMenu::Base => {
                if base == Some(branch) {
                    0
                } else if current == Some(branch) {
                    1
                } else {
                    2
                }
            }
        }
    }

    pub(crate) fn push_branch_input(&mut self, character: char) {
        self.branch_menu_input.push(character);
        self.branch_menu_scroll = 0;
        self.branch_menu_selected = 0;
        self.dirty = true;
    }

    pub(crate) fn pop_branch_input(&mut self) {
        if self.branch_menu_input.pop().is_some() {
            self.branch_menu_scroll = 0;
            self.branch_menu_selected = 0;
            self.dirty = true;
        }
    }

    pub(crate) fn clear_branch_input(&mut self) {
        if !self.branch_menu_input.is_empty()
            || self.branch_menu_scroll != 0
            || self.branch_menu_selected != 0
        {
            self.branch_menu_input.clear();
            self.branch_menu_scroll = 0;
            self.branch_menu_selected = 0;
            self.dirty = true;
        }
    }

    pub(crate) fn select_highlighted_branch_match(&mut self) {
        let Some(menu) = self.branch_menu_open else {
            return;
        };
        let Some(branch) = self
            .filtered_branches()
            .get(self.branch_menu_selected)
            .map(|branch| (*branch).to_owned())
        else {
            self.set_notice("no matching branch");
            return;
        };
        self.close_branch_menu();
        self.select_branch(menu, branch);
    }

    pub(crate) fn is_branch_diff(&self) -> bool {
        matches!(
            &self.options.source,
            DiffSource::Base(_) | DiffSource::Branch { .. }
        )
    }

    pub(crate) fn branch_ref(&self, menu: BranchMenu) -> Option<&str> {
        match menu {
            BranchMenu::Head => self.branch_head.as_deref(),
            BranchMenu::Base => self.branch_base.as_deref(),
        }
    }

    pub(crate) fn branch_selector_text(&self, menu: BranchMenu) -> Option<String> {
        let branch = self.branch_ref(menu)?;
        let label = self.branch_label(menu, branch);
        if self.branch_menu_open == Some(menu) {
            let width = label.width().max(self.branch_menu_input.width());
            return Some(format!("{} ▾", fit_padded(&self.branch_menu_input, width)));
        }

        Some(format!("{label} ▾"))
    }

    pub(crate) fn branch_label(&self, menu: BranchMenu, branch: &str) -> String {
        match self.branch_marker(menu, branch) {
            Some(marker) => format!("{marker} {branch}"),
            None => branch.to_owned(),
        }
    }

    pub(crate) fn branch_marker(&self, menu: BranchMenu, branch: &str) -> Option<&'static str> {
        let current = self.current_head.as_deref();
        let base = self.branch_base.as_deref();
        match menu {
            BranchMenu::Head => {
                if current == Some(branch) {
                    Some(CURRENT_BRANCH_MARKER)
                } else if base == Some(branch) {
                    Some(BASE_BRANCH_MARKER)
                } else {
                    None
                }
            }
            BranchMenu::Base => {
                if base == Some(branch) {
                    Some(BASE_BRANCH_MARKER)
                } else if current == Some(branch) {
                    Some(CURRENT_BRANCH_MARKER)
                } else {
                    None
                }
            }
        }
    }

    pub(crate) fn branch_selector_width(&self, menu: BranchMenu) -> Option<u16> {
        self.branch_selector_text(menu)
            .map(|text| text.width() as u16)
    }

    pub(crate) fn branch_menu_width(&self) -> u16 {
        let branch_width = branch_menu_width(&self.comparison_branches) as usize;
        let input_width = self.branch_menu_input.width().saturating_add(6).max(20);
        branch_width.max(input_width) as u16
    }

    pub(crate) fn branch_selector_start(&self, menu: BranchMenu) -> Option<u16> {
        if !self.is_branch_diff() {
            return None;
        }

        let head_width = self.branch_selector_width(BranchMenu::Head)?;
        let selector_gap = STATUSLINE_SELECTOR_GAP.width() as u16;
        let head_start = diff_selector_width(&self.options).saturating_add(selector_gap);
        match menu {
            BranchMenu::Head => Some(head_start),
            BranchMenu::Base => Some(
                head_start
                    .saturating_add(head_width)
                    .saturating_add(BRANCH_COMPARISON_SEPARATOR.width() as u16),
            ),
        }
    }

    pub(crate) fn diff_choice_at(&self, column: u16, row: u16) -> Option<DiffChoice> {
        let choices = self.diff_menu_choices();
        let width = diff_menu_width(&choices);
        if column >= width || row == 0 {
            return None;
        }

        choices.get(usize::from(row - 1)).copied()
    }

    pub(crate) fn diff_menu_choices(&self) -> Vec<DiffChoice> {
        if matches!(&self.options.source, DiffSource::Patch(_)) {
            return Vec::new();
        }

        let mut choices = Vec::with_capacity(4);
        choices.extend(WORKTREE_DIFF_CHOICES);
        if self.branch_base.is_some() {
            choices.push(DiffChoice::Branch);
        }
        choices
    }

    pub(crate) fn select_branch(&mut self, menu: BranchMenu, branch: String) {
        let base = match menu {
            BranchMenu::Head => self.branch_base.clone(),
            BranchMenu::Base => Some(branch.clone()),
        };
        let head = match menu {
            BranchMenu::Head => Some(branch.clone()),
            BranchMenu::Base => self.branch_head.clone(),
        };
        let Some((base, head)) = base.zip(head) else {
            self.set_notice("branch diff unavailable");
            return;
        };

        let mut options = self.options.clone();
        options.source = self.branch_source(base, head);
        options.scope = DiffScope::All;

        if options == self.options {
            self.dirty = true;
            return;
        }

        match hz_diff::load_review_ref(&options) {
            Ok(changeset) => {
                let notice = format!("branch {branch}");
                self.replace_loaded_diff(options, changeset, Some(&notice));
            }
            Err(error) => self.set_notice(format!("branch diff unavailable: {error}")),
        }
    }

    pub(crate) fn branch_source(&self, base: String, head: String) -> DiffSource {
        if self.current_head.as_deref() == Some(head.as_str()) {
            DiffSource::Base(base)
        } else {
            DiffSource::Branch { base, head }
        }
    }

    pub(crate) fn select_diff_choice(&mut self, choice: DiffChoice) {
        let Some(options) = self.options_for_choice(choice) else {
            self.set_notice("base branch unavailable");
            return;
        };

        if options == self.options {
            self.dirty = true;
            return;
        }

        match hz_diff::load_review_ref(&options) {
            Ok(changeset) => self.replace_loaded_diff(options, changeset, Some(choice.notice())),
            Err(error) => self.set_notice(format!("diff unavailable: {error}")),
        }
    }

    pub(crate) fn options_for_choice(&self, choice: DiffChoice) -> Option<DiffOptions> {
        let mut options = self.options.clone();
        match choice {
            DiffChoice::Branch => {
                let base = self
                    .branch_base
                    .clone()
                    .or_else(|| default_branch_base(&self.options, &self.changeset.repo))?;
                let head = self
                    .branch_head
                    .clone()
                    .or_else(|| self.current_head.clone())
                    .or_else(|| current_head_label(&self.changeset.repo))?;
                options.source = self.branch_source(base, head);
                options.scope = DiffScope::All;
            }
            DiffChoice::All => {
                options.source = DiffSource::Worktree;
                options.scope = DiffScope::All;
            }
            DiffChoice::Unstaged => {
                options.source = DiffSource::Worktree;
                options.scope = DiffScope::Unstaged;
            }
            DiffChoice::Staged => {
                options.source = DiffSource::Worktree;
                options.scope = DiffScope::Staged;
            }
        }

        Some(options)
    }

    pub(crate) fn scroll_by(&mut self, delta: isize) {
        let next = if delta < 0 {
            self.scroll.saturating_sub(delta.unsigned_abs())
        } else {
            self.scroll.saturating_add(delta as usize)
        };
        self.set_scroll(next);
    }

    pub(crate) fn scroll_or_focus_hunk(&mut self, delta: isize) {
        let previous_scroll = self.scroll;
        self.scroll_by(delta);
        if self.scroll == previous_scroll {
            self.move_focused_hunk(delta);
        }
    }

    pub(crate) fn mouse_scroll_or_focus_hunk(&mut self, direction: MouseScrollDirection) {
        let delta = self.mouse_scroll.scroll_delta(direction, Instant::now());
        let previous_scroll = self.scroll;
        self.scroll_by(delta);
        if self.scroll == previous_scroll {
            let hunk_delta = self.mouse_scroll.hunk_focus_delta(direction);
            if hunk_delta != 0 {
                self.move_focused_hunk(hunk_delta);
            }
        } else {
            self.mouse_scroll.reset_hunk_focus_ticks();
        }
    }

    pub(crate) fn scroll_horizontally_by(&mut self, delta: isize) {
        let next = if delta < 0 {
            self.horizontal_scroll.saturating_sub(delta.unsigned_abs())
        } else {
            self.horizontal_scroll.saturating_add(delta as usize)
        };
        self.set_horizontal_scroll(next);
    }

    pub(crate) fn set_horizontal_scroll(&mut self, scroll: usize) {
        let previous_scroll = self.horizontal_scroll;
        self.horizontal_scroll = scroll.min(self.max_horizontal_scroll());
        if self.horizontal_scroll != previous_scroll {
            self.dirty = true;
        }
    }

    pub(crate) fn set_scroll(&mut self, scroll: usize) {
        self.set_scroll_with_grep_sync(scroll, true, HunkFocusScrollBehavior::ClearOnScroll);
    }

    fn clear_manual_hunk_focus(&mut self) {
        self.manual_hunk_focus = None;
    }

    fn replace_model(
        &mut self,
        visible_files: &[usize],
        hunk_focus_behavior: HunkFocusModelBehavior,
    ) {
        let previous_manual_hunk_focus = self.manual_hunk_focus;
        self.model = UiModel::new_filtered(
            &self.changeset,
            self.layout,
            &self.context_expansions,
            visible_files,
        );
        self.manual_hunk_focus = match hunk_focus_behavior {
            HunkFocusModelBehavior::PreserveIfValid => previous_manual_hunk_focus
                .filter(|(file, hunk)| self.model.hunk_start_row(*file, *hunk).is_some()),
            HunkFocusModelBehavior::Clear => None,
        };
    }

    pub(crate) fn set_scroll_centered_on(&mut self, row: usize) {
        let center_offset = viewport_center_offset(self.viewport_rows);
        self.set_scroll_with_grep_sync(
            row.saturating_sub(center_offset),
            false,
            HunkFocusScrollBehavior::ClearOnScroll,
        );
    }

    pub(crate) fn set_scroll_focused_on_hunk(&mut self, file: usize, hunk: usize) {
        let Some((range, hunk_start)) = hunk_focus_row_range(&self.model, file, hunk) else {
            return;
        };

        let focus_rows = range.end.saturating_sub(range.start).max(1);
        let scroll = if focus_rows > self.viewport_rows {
            // Oversized focus ranges cannot be fully centered. Keep the first
            // useful context row when possible, but never so much context that
            // the hunk header itself falls below the viewport.
            range.start.max(
                hunk_start
                    .saturating_add(1)
                    .saturating_sub(self.viewport_rows),
            )
        } else {
            let focus_center = range.start.saturating_add(focus_rows.saturating_sub(1) / 2);
            focus_center.saturating_sub(viewport_center_offset(self.viewport_rows))
        };
        self.set_scroll_with_grep_sync(scroll, false, HunkFocusScrollBehavior::Preserve);
    }

    fn focused_hunk_in_visible_range(
        &self,
        visible_start: usize,
        visible_end: usize,
        search: HunkFocusSearch,
    ) -> Option<(usize, usize)> {
        match search {
            HunkFocusSearch::FirstVisible => {
                for row_index in visible_start..visible_end {
                    if let Some(hunk_key) = self.model.row(row_index).and_then(|row| row.hunk_key())
                    {
                        return Some(hunk_key);
                    }
                }
                None
            }
            HunkFocusSearch::NearestTo(focus_row) => {
                find_visible_row_outward(visible_start, visible_end, focus_row, |row_index| {
                    self.model.row(row_index).and_then(|row| row.hunk_key())
                })
            }
        }
    }

    fn set_scroll_with_grep_sync(
        &mut self,
        scroll: usize,
        sync_grep: bool,
        hunk_focus_behavior: HunkFocusScrollBehavior,
    ) {
        let previous_scroll = self.scroll;
        let previous_file = self.selected_file;
        self.scroll = scroll.min(self.max_scroll());
        if self.scroll != previous_scroll
            && hunk_focus_behavior == HunkFocusScrollBehavior::ClearOnScroll
        {
            self.clear_manual_hunk_focus();
        }
        if let Some(file) = self.model.file_at_row(self.scroll) {
            self.selected_file = file;
        }
        if sync_grep && self.scroll != previous_scroll {
            self.sync_grep_match_selection_to_scroll();
        }
        if self.scroll != previous_scroll || self.selected_file != previous_file {
            self.dirty = true;
        }
    }

    pub(crate) fn max_scroll(&self) -> usize {
        max_scroll_for_viewport(self.model.len(), self.viewport_rows)
    }

    pub(crate) fn max_horizontal_scroll(&self) -> usize {
        self.max_line_width
            .saturating_sub(diff_content_width(self.layout, self.viewport_width))
    }

    pub(crate) fn focused_hunk_for_viewport(&self, visible_rows: usize) -> Option<(usize, usize)> {
        let visible_start = self.scroll;
        let visible_end = visible_start
            .saturating_add(visible_rows)
            .min(self.model.len());
        if visible_start >= visible_end {
            return None;
        }

        if let Some((file, hunk)) = self.manual_hunk_focus
            && let Some(row) = self.model.hunk_start_row(file, hunk)
            && row >= visible_start
            && row < visible_end
        {
            return Some((file, hunk));
        }

        let search = if max_scroll_for_viewport(self.model.len(), visible_rows) == 0 {
            // When the whole diff fits, start at the first visible hunk; explicit hunk
            // navigation is tracked separately with manual_hunk_focus.
            HunkFocusSearch::FirstVisible
        } else {
            HunkFocusSearch::NearestTo(
                visible_start
                    .saturating_add(viewport_focus_offset(
                        self.scroll,
                        self.model.len(),
                        visible_rows,
                    ))
                    .min(visible_end.saturating_sub(1)),
            )
        };
        self.focused_hunk_in_visible_range(visible_start, visible_end, search)
    }

    pub(crate) fn focused_hunk_editor_target(&self) -> Option<EditorTarget> {
        if matches!(self.options.source, DiffSource::Patch(_)) {
            return None;
        }

        let (file, hunk) = self.focused_hunk_for_viewport(self.viewport_rows)?;
        let file_diff = self.changeset.files.get(file)?;
        let hunk_diff = file_diff.hunks.get(hunk)?;
        let path = file_diff.new_path.as_deref()?;
        let line = self
            .focused_hunk_editor_line(file, hunk)
            .unwrap_or_else(|| hunk_diff.new_start.max(1));

        Some(EditorTarget {
            path: repo_file_path(&self.changeset.repo, path),
            line,
        })
    }

    pub(crate) fn focused_hunk_editor_line(&self, file: usize, hunk: usize) -> Option<usize> {
        let visible_start = self.scroll;
        let visible_end = visible_start
            .saturating_add(self.viewport_rows)
            .min(self.model.len());
        if visible_start >= visible_end {
            return None;
        }

        find_visible_row_outward(
            visible_start,
            visible_end,
            self.viewport_focus_row(),
            |row_index| self.editor_line_at_hunk_row(row_index, file, hunk),
        )
    }

    pub(crate) fn editor_line_at_hunk_row(
        &self,
        row_index: usize,
        file: usize,
        hunk: usize,
    ) -> Option<usize> {
        let hunk_diff = self.changeset.files.get(file)?.hunks.get(hunk)?;
        match self.model.row(row_index)? {
            UiRow::UnifiedLine {
                file: row_file,
                hunk: row_hunk,
                line,
            }
            | UiRow::MetaLine {
                file: row_file,
                hunk: row_hunk,
                line,
            } if row_file == file && row_hunk == hunk => {
                hunk_diff.lines.get(line)?.new_line.map(|line| line.max(1))
            }
            UiRow::SplitLine {
                file: row_file,
                hunk: row_hunk,
                left,
                right,
            } if row_file == file && row_hunk == hunk => right
                .or(left)
                .and_then(|line| hunk_diff.lines.get(line))
                .and_then(|line| line.new_line)
                .map(|line| line.max(1)),
            _ => None,
        }
    }

    pub(crate) fn open_focused_hunk_in_editor(&mut self) {
        let Some(target) = self.focused_hunk_editor_target() else {
            self.set_notice("no editable focused hunk");
            return;
        };
        let Some(editor) = configured_editor() else {
            self.set_notice("set $EDITOR to edit focused hunk");
            return;
        };

        self.diff_menu_open = false;
        self.close_branch_menu();
        self.terminal_clear_requested = true;
        let before = FileFingerprint::read(&target.path);
        match open_editor(&editor, &target) {
            Ok(status) if status.success() => {
                let changed = file_changed_since(&target.path, before);
                match self.editor_reload_behavior(changed) {
                    EditorReloadBehavior::None => self.set_notice("editor closed"),
                    EditorReloadBehavior::Live => {
                        self.set_notice("editor closed; reloading");
                    }
                    EditorReloadBehavior::Sync => match self.reload() {
                        Ok(()) => self.set_notice("editor closed"),
                        Err(error) => {
                            self.set_notice(format!("editor closed; reload failed: {error}"));
                        }
                    },
                }
            }
            Ok(status) => self.set_notice(format!("editor exited with {status}")),
            Err(error) => self.set_notice(format!("editor failed: {error}")),
        }
    }

    pub(crate) fn editor_reload_behavior(&self, target_changed: bool) -> EditorReloadBehavior {
        if !target_changed || !matches!(self.options.source, DiffSource::Worktree) {
            return EditorReloadBehavior::None;
        }

        if self.live_updates_enabled
            && self.live_diff_failed_options.as_ref() != Some(&self.options)
        {
            return EditorReloadBehavior::Live;
        }

        EditorReloadBehavior::Sync
    }

    pub(crate) fn viewport_focus_row(&self) -> usize {
        self.scroll
            .saturating_add(viewport_focus_offset(
                self.scroll,
                self.model.len(),
                self.viewport_rows,
            ))
            .min(self.model.len().saturating_sub(1))
    }

    pub(crate) fn set_viewport_rows(&mut self, rows: usize) {
        let rows = rows.max(1);
        if self.viewport_rows == rows {
            return;
        }

        self.viewport_rows = rows;
        self.set_scroll(self.scroll);
        self.clamp_file_sidebar_scroll(self.visible_file_sidebar_rows());
    }

    pub(crate) fn set_viewport_width(&mut self, width: usize) {
        let width = width.max(1);
        if self.viewport_width == width {
            return;
        }

        self.viewport_width = width;
        self.set_horizontal_scroll(self.horizontal_scroll);
    }

    pub(crate) fn scroll_file_sidebar_by(&mut self, delta: isize) {
        let next = if delta < 0 {
            self.file_sidebar_scroll
                .saturating_sub(delta.unsigned_abs())
        } else {
            self.file_sidebar_scroll.saturating_add(delta as usize)
        };
        self.set_file_sidebar_scroll(next);
    }

    pub(crate) fn set_file_sidebar_scroll(&mut self, scroll: usize) {
        let previous_scroll = self.file_sidebar_scroll;
        self.file_sidebar_scroll =
            scroll.min(self.max_file_sidebar_scroll(self.visible_file_sidebar_rows()));
        if self.file_sidebar_scroll != previous_scroll {
            self.dirty = true;
        }
    }

    pub(crate) fn set_file_sidebar_width(&mut self, width: u16) {
        let total_width = self
            .file_sidebar_render_width
            .saturating_add(self.viewport_width.min(usize::from(u16::MAX)) as u16);
        let max_width = max_file_sidebar_width(total_width);
        if max_width == 0 {
            return;
        }

        let width = width.clamp(FILE_SIDEBAR_MIN_WIDTH, max_width);
        if self.file_sidebar_width != Some(width) {
            self.file_sidebar_width = Some(width);
            self.set_horizontal_scroll(self.horizontal_scroll);
            self.dirty = true;
        }
    }

    pub(crate) fn clamp_file_sidebar_scroll(&mut self, visible_rows: usize) {
        self.file_sidebar_scroll = self
            .file_sidebar_scroll
            .min(self.max_file_sidebar_scroll(visible_rows));
    }

    pub(crate) fn prepare_syntax_for_viewport(&mut self, visible_rows: usize) {
        if visible_rows == 0 || self.syntax.is_none() {
            return;
        }
        let mut requested = HashSet::new();
        let mut requested_files = HashSet::new();

        let visible_start = self.scroll;
        let visible_end = visible_start
            .saturating_add(visible_rows)
            .min(self.model.len());
        self.prepare_syntax_for_range(
            visible_start,
            visible_end,
            SyntaxPriority::Visible,
            &mut requested,
            &mut requested_files,
        );

        let prefetch_rows = visible_rows.saturating_mul(self.syntax_limits.prefetch_viewports);
        let ahead_end = visible_end
            .saturating_add(prefetch_rows)
            .min(self.model.len());
        self.prepare_syntax_for_range(
            visible_end,
            ahead_end,
            SyntaxPriority::Prefetch,
            &mut requested,
            &mut requested_files,
        );

        let behind_start = visible_start.saturating_sub(prefetch_rows);
        self.prepare_syntax_for_range(
            behind_start,
            visible_start,
            SyntaxPriority::Prefetch,
            &mut requested,
            &mut requested_files,
        );
    }

    pub(crate) fn prepare_syntax_for_range(
        &mut self,
        start: usize,
        end: usize,
        priority: SyntaxPriority,
        requested: &mut HashSet<SyntaxPosition>,
        requested_files: &mut HashSet<ContextSourceKey>,
    ) {
        for row_index in start..end {
            let Some(row) = self.model.row(row_index) else {
                continue;
            };
            self.prepare_syntax_for_row(row, priority, requested, requested_files);
        }
    }

    pub(crate) fn prepare_syntax_for_row(
        &mut self,
        row: UiRow,
        priority: SyntaxPriority,
        requested: &mut HashSet<SyntaxPosition>,
        requested_files: &mut HashSet<ContextSourceKey>,
    ) {
        match row {
            UiRow::FileSeparator => {}
            UiRow::UnifiedLine { file, hunk, line } => {
                let Some(diff_line) = self
                    .changeset
                    .files
                    .get(file)
                    .and_then(|file_diff| file_diff.hunks.get(hunk))
                    .and_then(|hunk_diff| hunk_diff.lines.get(line))
                else {
                    return;
                };
                if let Some(side) = unified_syntax_side(diff_line.kind) {
                    self.queue_syntax_hunk(file, hunk, side, priority, requested);
                }
            }
            UiRow::SplitLine {
                file,
                hunk,
                left,
                right,
            } => {
                if left.is_some() {
                    self.queue_syntax_hunk(file, hunk, DiffSide::Old, priority, requested);
                }
                if right.is_some() {
                    self.queue_syntax_hunk(file, hunk, DiffSide::New, priority, requested);
                }
            }
            UiRow::ContextLine { file, .. } => {
                if let Some(side) = self.context_source_side(file) {
                    self.queue_syntax_file(file, side, priority, requested_files);
                }
            }
            UiRow::FileHeader(_)
            | UiRow::BinaryFile(_)
            | UiRow::Collapsed { .. }
            | UiRow::ContextHide { .. }
            | UiRow::HunkHeader { .. }
            | UiRow::MetaLine { .. } => {}
        }
    }

    pub(crate) fn queue_syntax_hunk(
        &mut self,
        file: usize,
        hunk: usize,
        side: DiffSide,
        priority: SyntaxPriority,
        requested: &mut HashSet<SyntaxPosition>,
    ) {
        let position = SyntaxPosition {
            generation: self.generation,
            file,
            hunk,
            side,
        };
        if !requested.insert(position) {
            return;
        }
        if let Some(syntax) = self.syntax.as_mut() {
            syntax.queue_hunk(&self.options, &self.changeset, position, priority);
        }
    }

    pub(crate) fn queue_syntax_file(
        &mut self,
        file: usize,
        side: DiffSide,
        priority: SyntaxPriority,
        requested: &mut HashSet<ContextSourceKey>,
    ) {
        if !requested.insert(ContextSourceKey { file, side }) {
            return;
        }
        if let Some(syntax) = self.syntax.as_mut() {
            syntax.queue_full_file(
                &self.options,
                &self.changeset,
                self.generation,
                file,
                side,
                priority,
            );
        }
    }

    pub(crate) fn drain_syntax(&mut self) {
        if self.syntax_updates_paused() {
            return;
        }

        if let Some(syntax) = self.syntax.as_mut()
            && syntax.drain(self.generation, MAX_SYNTAX_RESULTS_PER_FRAME)
        {
            self.dirty = true;
        }
    }

    pub(crate) fn syntax_stats(&self) -> SyntaxBenchmarkReport {
        self.syntax
            .as_ref()
            .map(SyntaxRuntime::stats)
            .unwrap_or_default()
    }

    pub(crate) fn syntax_updates_paused(&self) -> bool {
        self.filter_input.is_some()
    }

    pub(crate) fn open_filter_input(&mut self, kind: DiffFilterKind) {
        match kind {
            DiffFilterKind::File => self.file_filter_input = self.file_filter.clone(),
            DiffFilterKind::Grep => self.grep_filter_input = self.grep_filter.clone(),
        }
        self.filter_input = Some(kind);
        self.diff_menu_open = false;
        self.close_branch_menu();
        self.dirty = true;
    }

    pub(crate) fn handle_filter_input_key(&mut self, key: KeyEvent) -> bool {
        let Some(kind) = self.filter_input else {
            return false;
        };

        match key.code {
            KeyCode::Esc => {
                self.clear_all_filters();
                self.filter_input = None;
            }
            KeyCode::Enter => {
                self.commit_filter_input(kind);
                self.filter_input = None;
            }
            KeyCode::Backspace if !self.filter_input_query(kind).is_empty() => {
                self.filter_input_query_mut(kind).pop();
                self.sync_filter_input(kind);
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.filter_input_query_mut(kind).clear();
                self.sync_filter_input(kind);
            }
            KeyCode::Char(character)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                self.filter_input_query_mut(kind).push(character);
                self.sync_filter_input(kind);
            }
            _ => {}
        }

        true
    }

    pub(crate) fn filter_query(&self, kind: DiffFilterKind) -> &str {
        match kind {
            DiffFilterKind::File => &self.file_filter,
            DiffFilterKind::Grep => &self.grep_filter,
        }
    }

    pub(crate) fn filter_query_mut(&mut self, kind: DiffFilterKind) -> &mut String {
        match kind {
            DiffFilterKind::File => &mut self.file_filter,
            DiffFilterKind::Grep => &mut self.grep_filter,
        }
    }

    pub(crate) fn filter_input_query(&self, kind: DiffFilterKind) -> &str {
        match kind {
            DiffFilterKind::File => &self.file_filter_input,
            DiffFilterKind::Grep => &self.grep_filter_input,
        }
    }

    pub(crate) fn filter_input_query_mut(&mut self, kind: DiffFilterKind) -> &mut String {
        match kind {
            DiffFilterKind::File => &mut self.file_filter_input,
            DiffFilterKind::Grep => &mut self.grep_filter_input,
        }
    }

    pub(crate) fn commit_filter_input(&mut self, kind: DiffFilterKind) {
        let next = self.filter_input_query(kind).to_owned();
        if self.filter_query(kind) == next {
            self.dirty = true;
            return;
        }

        *self.filter_query_mut(kind) = next;
        self.apply_filter_change(kind);
    }

    pub(crate) fn sync_filter_input(&mut self, kind: DiffFilterKind) {
        let next = self.filter_input_query(kind).to_owned();
        if self.filter_query(kind) == next {
            self.dirty = true;
            return;
        }

        *self.filter_query_mut(kind) = next;
        self.apply_filter_change(kind);
    }

    pub(crate) fn clear_all_filters(&mut self) {
        if self.file_filter.is_empty() && self.grep_filter.is_empty() {
            self.file_filter_input.clear();
            self.grep_filter_input.clear();
            self.dirty = true;
            return;
        }

        self.file_filter.clear();
        self.file_filter_input.clear();
        self.grep_filter.clear();
        self.grep_filter_input.clear();
        self.apply_filters(false);
    }

    pub(crate) fn apply_filter_change(&mut self, kind: DiffFilterKind) {
        let jump_to_grep = kind == DiffFilterKind::Grep && !self.grep_filter.is_empty();
        self.apply_filters(jump_to_grep);
    }

    pub(crate) fn apply_filters(&mut self, jump_to_grep: bool) {
        let selected_path = self
            .changeset
            .files
            .get(self.selected_file)
            .map(|file| file.display_path().to_owned());
        let relative_scroll = self
            .model
            .file_start_row(self.selected_file)
            .map(|start| self.scroll.saturating_sub(start))
            .unwrap_or_default();

        let visible_files =
            filtered_file_indices(&self.changeset, &self.file_filter, &self.grep_filter);
        self.replace_visible_files(
            visible_files,
            selected_path,
            relative_scroll,
            jump_to_grep,
            HunkFocusModelBehavior::PreserveIfValid,
        );
    }

    fn replace_visible_files(
        &mut self,
        visible_files: Vec<usize>,
        selected_path: Option<String>,
        relative_scroll: usize,
        jump_to_grep: bool,
        hunk_focus_behavior: HunkFocusModelBehavior,
    ) {
        let selected_file = selected_path
            .and_then(|path| {
                self.changeset
                    .files
                    .iter()
                    .position(|file| file.display_path() == path)
            })
            .filter(|file| visible_files.contains(file))
            .or_else(|| visible_files.first().copied())
            .unwrap_or(0);

        self.stats = diff_stats_for_files(&self.changeset, &visible_files);
        self.max_line_width = changeset_max_line_width_for_files(&self.changeset, &visible_files);
        self.replace_model(&visible_files, hunk_focus_behavior);
        self.selected_file = selected_file;
        self.grep_matches = grep_match_rows(&self.changeset, &self.model, &self.grep_filter);
        self.selected_grep_match = None;

        let scroll = self
            .model
            .file_start_row(self.selected_file)
            .map(|start| start.saturating_add(relative_scroll))
            .unwrap_or_default();
        let scroll_behavior = match hunk_focus_behavior {
            HunkFocusModelBehavior::PreserveIfValid => HunkFocusScrollBehavior::Preserve,
            HunkFocusModelBehavior::Clear => HunkFocusScrollBehavior::ClearOnScroll,
        };
        self.set_scroll_with_grep_sync(scroll, true, scroll_behavior);
        self.set_horizontal_scroll(self.horizontal_scroll);
        self.ensure_file_sidebar_selection_visible(self.visible_file_sidebar_rows());

        if jump_to_grep && !self.grep_matches.is_empty() {
            self.selected_grep_match = Some(0);
            self.set_scroll_centered_on(self.grep_matches[0]);
        } else {
            self.sync_grep_match_selection_to_scroll();
        }

        self.dirty = true;
    }

    pub(crate) fn filters_active(&self) -> bool {
        !self.file_filter.is_empty() || !self.grep_filter.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn current_grep_match_row(&self) -> Option<usize> {
        self.selected_grep_match
            .and_then(|index| self.grep_matches.get(index).copied())
    }

    pub(crate) fn sync_grep_match_selection_to_scroll(&mut self) {
        if self.grep_filter.is_empty() || self.grep_matches.is_empty() {
            self.selected_grep_match = None;
            return;
        }

        self.selected_grep_match = self
            .grep_matches
            .iter()
            .position(|row| *row >= self.scroll)
            .or_else(|| self.grep_matches.len().checked_sub(1));
    }

    pub(crate) fn move_grep_match(&mut self, delta: isize) {
        if self.grep_matches.is_empty() {
            self.selected_grep_match = None;
            self.set_notice("no grep matches");
            return;
        }

        let len = self.grep_matches.len();
        let current = self.selected_grep_match.unwrap_or_else(|| {
            self.grep_matches
                .iter()
                .position(|row| *row >= self.scroll)
                .unwrap_or(0)
        });
        let next = if delta < 0 {
            current
                .saturating_add(len)
                .saturating_sub(delta.unsigned_abs() % len)
                % len
        } else {
            current.saturating_add(delta as usize) % len
        };

        self.selected_grep_match = Some(next);
        self.set_scroll_centered_on(self.grep_matches[next]);
        self.dirty = true;
    }

    pub(crate) fn syntax_line(
        &mut self,
        file: usize,
        hunk: usize,
        line: usize,
        side: DiffSide,
    ) -> Option<HighlightedLine> {
        self.syntax.as_mut().and_then(|syntax| {
            syntax.line(
                SyntaxPosition {
                    generation: self.generation,
                    file,
                    hunk,
                    side,
                },
                line,
            )
        })
    }

    pub(crate) fn syntax_file_line(
        &mut self,
        file: usize,
        side: DiffSide,
        line_number: usize,
    ) -> Option<HighlightedLine> {
        self.syntax
            .as_mut()
            .and_then(|syntax| syntax.full_file_line(self.generation, file, side, line_number))
    }

    pub(crate) fn inline_ranges(
        &mut self,
        file: usize,
        hunk: usize,
        line: usize,
    ) -> Vec<InlineRange> {
        let key = InlineHunkKey {
            generation: self.generation,
            file,
            hunk,
        };
        if !self.inline_cache.contains_key(&key) {
            let cache = self
                .changeset
                .files
                .get(file)
                .and_then(|file_diff| file_diff.hunks.get(hunk))
                .map(|hunk_diff| InlineHunkEmphasisCache::new(&hunk_diff.lines))
                .unwrap_or_else(|| InlineHunkEmphasisCache::new(&[]));
            self.inline_cache.insert(key, cache);
        }

        let Some(lines) = self
            .changeset
            .files
            .get(file)
            .and_then(|file_diff| file_diff.hunks.get(hunk))
            .map(|hunk_diff| hunk_diff.lines.as_slice())
        else {
            return Vec::new();
        };

        self.inline_cache
            .get_mut(&key)
            .map(|hunk_emphasis| hunk_emphasis.ranges_for_line(lines, line))
            .unwrap_or_default()
    }

    pub(crate) fn move_file(&mut self, delta: isize) {
        let visible_files = self.model.visible_files();
        if visible_files.is_empty() {
            return;
        }

        let current = self
            .model
            .visible_file_position(self.selected_file)
            .unwrap_or_default();
        let next = if delta < 0 {
            current.saturating_sub(delta.unsigned_abs())
        } else {
            current.saturating_add(delta as usize)
        }
        .min(visible_files.len() - 1);

        self.select_file(visible_files[next]);
    }

    pub(crate) fn select_file(&mut self, file: usize) {
        if self.model.visible_files().is_empty() {
            return;
        }

        let next = if self.model.file_start_row(file).is_some() {
            file
        } else {
            self.model
                .visible_files()
                .first()
                .copied()
                .unwrap_or_default()
        };

        if next == self.selected_file {
            self.ensure_file_sidebar_selection_visible(self.visible_file_sidebar_rows());
            self.dirty = true;
            return;
        }

        if let Some(row) = self.model.hunk_start_row(next, 0) {
            self.focus_hunk_row(row);
            return;
        }

        self.selected_file = next;
        if let Some(row) = self.model.file_start_row(next) {
            self.set_scroll(row);
        } else {
            self.dirty = true;
        }
        self.ensure_file_sidebar_selection_visible(self.visible_file_sidebar_rows());
    }

    pub(crate) fn toggle_file_sidebar(&mut self) {
        self.file_sidebar_open = !self.file_sidebar_open;
        self.file_sidebar_resizing = false;
        self.diff_menu_open = false;
        self.close_branch_menu();
        self.ensure_file_sidebar_selection_visible(self.visible_file_sidebar_rows());
        self.set_notice(if self.file_sidebar_open {
            "file sidebar"
        } else {
            "diff only"
        });
    }

    pub(crate) fn visible_file_sidebar_rows(&self) -> usize {
        self.viewport_rows
    }

    pub(crate) fn ensure_file_sidebar_selection_visible(&mut self, visible_rows: usize) {
        let Some(selected_position) = self.model.visible_file_position(self.selected_file) else {
            self.file_sidebar_scroll = 0;
            return;
        };
        if visible_rows == 0 {
            self.file_sidebar_scroll = 0;
            return;
        }

        if selected_position < self.file_sidebar_scroll {
            self.file_sidebar_scroll = selected_position;
        } else if selected_position >= self.file_sidebar_scroll.saturating_add(visible_rows) {
            self.file_sidebar_scroll = self
                .model
                .visible_file_position(self.selected_file)
                .unwrap_or_default()
                .saturating_add(1)
                .saturating_sub(visible_rows);
        }

        self.file_sidebar_scroll = self
            .file_sidebar_scroll
            .min(self.max_file_sidebar_scroll(visible_rows));
    }

    pub(crate) fn max_file_sidebar_scroll(&self, visible_rows: usize) -> usize {
        self.model
            .visible_files()
            .len()
            .saturating_sub(visible_rows.max(1))
    }

    pub(crate) fn next_hunk(&mut self) {
        if let Some(row) = self.model.next_hunk_row(self.hunk_navigation_anchor_row()) {
            self.focus_hunk_row(row);
        }
    }

    pub(crate) fn previous_hunk(&mut self) {
        if let Some(row) = self
            .model
            .previous_hunk_row(self.hunk_navigation_anchor_row())
        {
            self.focus_hunk_row(row);
        }
    }

    pub(crate) fn move_focused_hunk(&mut self, delta: isize) {
        let anchor = self.hunk_navigation_anchor_row();
        let next = if delta < 0 {
            self.model.previous_hunk_row(anchor)
        } else {
            self.model.next_hunk_row(anchor)
        };
        if let Some(row) = next {
            self.focus_hunk_row(row);
        }
    }

    pub(crate) fn hunk_navigation_anchor_row(&self) -> usize {
        if let Some((file, hunk)) = self.focused_hunk_for_viewport(self.viewport_rows)
            && let Some(row) = self.model.hunk_start_row(file, hunk)
        {
            return row;
        }

        self.viewport_focus_row()
    }

    pub(crate) fn focus_hunk_row(&mut self, row: usize) {
        let target_hunk = self.model.row(row).and_then(|row| row.hunk_key());
        let previous_hunk = self.manual_hunk_focus;
        self.clear_manual_hunk_focus();

        let Some((file, hunk)) = target_hunk else {
            self.set_scroll_centered_on(row);
            return;
        };

        self.set_scroll_focused_on_hunk(file, hunk);

        let visible_start = self.scroll;
        let visible_end = visible_start
            .saturating_add(self.viewport_rows)
            .min(self.model.len());
        if let Some(row) = self.model.hunk_start_row(file, hunk)
            && row >= visible_start
            && row < visible_end
        {
            let previous_file = self.selected_file;
            self.manual_hunk_focus = Some((file, hunk));
            self.selected_file = file;
            self.ensure_file_sidebar_selection_visible(self.visible_file_sidebar_rows());
            if self.manual_hunk_focus != previous_hunk || self.selected_file != previous_file {
                self.dirty = true;
            }
        }
    }

    pub(crate) fn toggle_layout(&mut self) {
        self.set_layout(self.layout.toggled(), true);
    }

    pub(crate) fn apply_responsive_layout(&mut self, width: u16) {
        self.viewport_width = (width as usize).max(1);
        self.set_layout(default_layout_for_width(width), true);
        self.set_horizontal_scroll(self.horizontal_scroll);
        self.dirty = true;
    }

    pub(crate) fn set_layout(&mut self, layout: DiffLayoutMode, show_notice: bool) {
        if self.layout == layout {
            return;
        }

        self.layout = layout;
        let visible_files =
            filtered_file_indices(&self.changeset, &self.file_filter, &self.grep_filter);
        self.replace_model(&visible_files, HunkFocusModelBehavior::Clear);
        self.grep_matches = grep_match_rows(&self.changeset, &self.model, &self.grep_filter);
        self.selected_grep_match = None;
        self.set_horizontal_scroll(self.horizontal_scroll);
        let scroll = self
            .model
            .file_start_row(self.selected_file)
            .unwrap_or_default();
        self.set_scroll(scroll);
        self.sync_grep_match_selection_to_scroll();
        self.dirty = true;
        if show_notice {
            self.set_notice(match self.layout {
                DiffLayoutMode::Split => "split view",
                DiffLayoutMode::Unified => "unified view",
            });
        }
    }

    pub(crate) fn reload(&mut self) -> HzResult<()> {
        let changeset = hz_diff::load_review_ref(&self.options)?;
        self.replace_changeset(changeset, Some("reloaded"));
        Ok(())
    }

    pub(crate) fn replace_changeset(&mut self, changeset: Changeset, notice: Option<&str>) {
        self.replace_loaded_diff(self.options.clone(), changeset, notice);
    }

    pub(crate) fn replace_loaded_diff(
        &mut self,
        options: DiffOptions,
        changeset: Changeset,
        notice: Option<&str>,
    ) {
        let options_changed = self.options != options;
        if !options_changed && self.base_changeset == changeset {
            if let Some(notice) = notice {
                self.set_notice(notice);
            }
            return;
        }

        let selected_path = self
            .changeset
            .files
            .get(self.selected_file)
            .map(|file| file.display_path().to_owned());
        let relative_scroll = self
            .model
            .file_start_row(self.selected_file)
            .map(|start| self.scroll.saturating_sub(start))
            .unwrap_or_default();

        let previous_branch_base = self.branch_base.clone();
        let previous_branch_head = self.branch_head.clone();
        self.options = options;
        self.current_head = current_head_label(&changeset.repo);
        self.branch_base = branch_base_from_options(&self.options)
            .or(previous_branch_base)
            .or_else(|| default_branch_base(&self.options, &changeset.repo));
        self.branch_head = branch_head_from_options(&self.options, self.current_head.as_deref())
            .or(previous_branch_head)
            .or_else(|| self.current_head.clone());
        self.comparison_branches = comparison_branches(
            &changeset.repo,
            &[
                self.current_head.as_deref(),
                self.branch_head.as_deref(),
                self.branch_base.as_deref(),
            ],
        );
        self.branch_menu_scroll = self.branch_menu_scroll.min(self.max_branch_menu_scroll());
        self.total_stats = changeset.stats();
        self.base_changeset = changeset.clone();
        self.changeset = changeset;
        self.context_expansions.clear();
        self.context_cache.clear();
        self.generation = self.generation.wrapping_add(1);
        self.inline_cache.clear();
        if let Some(syntax) = self.syntax.as_mut() {
            syntax.clear(self.generation);
        }
        let visible_files =
            filtered_file_indices(&self.changeset, &self.file_filter, &self.grep_filter);
        self.replace_visible_files(
            visible_files,
            selected_path,
            relative_scroll,
            false,
            HunkFocusModelBehavior::Clear,
        );
        if let Some(notice) = notice {
            self.set_notice(notice);
        }
    }
}

pub(crate) fn max_scroll_for_viewport(row_count: usize, viewport_rows: usize) -> usize {
    row_count.saturating_sub(viewport_rows.max(1))
}

pub(crate) fn viewport_center_offset(viewport_rows: usize) -> usize {
    viewport_rows.saturating_sub(1) / 2
}

pub(crate) fn viewport_focus_offset(
    scroll: usize,
    row_count: usize,
    viewport_rows: usize,
) -> usize {
    if row_count == 0 {
        return 0;
    }

    let viewport_rows = viewport_rows.max(1);
    let visible_rows = viewport_rows.min(row_count);
    let center = viewport_center_offset(visible_rows);
    if row_count <= viewport_rows {
        return center;
    }

    let bottom = visible_rows.saturating_sub(1);
    let max_scroll = max_scroll_for_viewport(row_count, viewport_rows);
    let scroll = scroll.min(max_scroll);
    let distance_to_end = max_scroll.saturating_sub(scroll);
    let top_ramp = scroll.min(center);
    let bottom_ramp = bottom.saturating_sub(distance_to_end);

    top_ramp.max(bottom_ramp).min(bottom)
}

fn hunk_focus_row_range(
    model: &UiModel,
    file: usize,
    hunk: usize,
) -> Option<(Range<usize>, usize)> {
    let mut range = model.hunk_row_range(file, hunk)?;
    let hunk_start = range.start;

    while range.start > 0
        && model
            .row(range.start - 1)
            .is_some_and(row_extends_hunk_focus_before)
    {
        range.start -= 1;
    }

    while range.end < model.len()
        && model
            .row(range.end)
            .is_some_and(row_extends_hunk_focus_after)
    {
        range.end += 1;
    }

    Some((range, hunk_start))
}

fn row_extends_hunk_focus_before(row: UiRow) -> bool {
    matches!(
        row,
        UiRow::FileHeader(_)
            | UiRow::Collapsed { .. }
            | UiRow::ContextLine { .. }
            | UiRow::ContextHide { .. }
    )
}

fn row_extends_hunk_focus_after(row: UiRow) -> bool {
    matches!(
        row,
        UiRow::Collapsed { .. } | UiRow::ContextLine { .. } | UiRow::ContextHide { .. }
    )
}

fn find_visible_row_outward<T>(
    visible_start: usize,
    visible_end: usize,
    focus_row: usize,
    mut find: impl FnMut(usize) -> Option<T>,
) -> Option<T> {
    if visible_start >= visible_end {
        return None;
    }

    let focus_row = focus_row.clamp(visible_start, visible_end.saturating_sub(1));
    let max_distance = focus_row
        .saturating_sub(visible_start)
        .max(visible_end.saturating_sub(1).saturating_sub(focus_row));
    for distance in 0..=max_distance {
        if let Some(row_index) = focus_row.checked_add(distance)
            && row_index < visible_end
            && let Some(found) = find(row_index)
        {
            return Some(found);
        }
        if distance > 0
            && let Some(row_index) = focus_row.checked_sub(distance)
            && row_index >= visible_start
            && let Some(found) = find(row_index)
        {
            return Some(found);
        }
    }

    None
}

pub(crate) fn changeset_max_line_width(changeset: &Changeset) -> usize {
    let files: Vec<_> = (0..changeset.files.len()).collect();
    changeset_max_line_width_for_files(changeset, &files)
}

pub(crate) fn changeset_max_line_width_for_files(changeset: &Changeset, files: &[usize]) -> usize {
    files
        .iter()
        .filter_map(|file| changeset.files.get(*file))
        .flat_map(|file| file.hunks.iter())
        .flat_map(|hunk| hunk.lines.iter())
        .map(|line| line.text.width())
        .max()
        .unwrap_or_default()
}

pub(crate) fn diff_content_width(layout: DiffLayoutMode, width: usize) -> usize {
    match layout {
        DiffLayoutMode::Unified => unified_content_width(width),
        DiffLayoutMode::Split => {
            let left_width = width / 2;
            let right_width = width.saturating_sub(left_width);
            split_cell_content_width(left_width).min(split_cell_content_width(right_width))
        }
    }
}

pub(crate) fn unified_content_width(width: usize) -> usize {
    let indicator_width = 1.min(width);
    let gutter_width = UNIFIED_GUTTER_WIDTH.min(width.saturating_sub(indicator_width));
    width.saturating_sub(indicator_width + gutter_width)
}

pub(crate) fn split_cell_content_width(width: usize) -> usize {
    let indicator_width = 1.min(width);
    let gutter_width = GUTTER_WIDTH.min(width.saturating_sub(indicator_width));
    width.saturating_sub(indicator_width + gutter_width)
}
