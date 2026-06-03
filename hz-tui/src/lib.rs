use std::{
    collections::{HashMap, HashSet, VecDeque},
    ffi::OsStr,
    hash::Hash,
    io,
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
    },
    thread,
    time::{Duration, Instant},
};

use crossterm::{
    cursor::Show,
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers,
        MouseEvent, MouseEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use hz_core::{HzError, HzResult};
use hz_diff::{Changeset, DiffLine, DiffLineKind, DiffOptions, DiffSource, DiffStats, FileStatus};
use hz_syntax::{HighlightedLine, SyntaxClass, SyntaxHighlighter, SyntaxLanguageSet};
use notify::{RecursiveMode, Watcher};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    prelude::{Color, Line, Modifier, Span, Style, Text},
    widgets::Paragraph,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const EVENT_POLL: Duration = Duration::from_millis(120);
const LIVE_RELOAD_DEBOUNCE: Duration = Duration::from_millis(200);
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
const NOTICE_TTL: Duration = Duration::from_millis(1_500);
const MAX_HIGHLIGHT_HUNK_BYTES: usize = 128 * 1024;
const MAX_HIGHLIGHT_LINE_BYTES: usize = 8 * 1024;
const MAX_HIGHLIGHT_CACHE_ENTRIES: usize = 512;
const MAX_HIGHLIGHT_QUEUE_ENTRIES: usize = 512;
const HIGHLIGHT_PREFETCH_VIEWPORTS: usize = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffBenchmarkOptions {
    pub width: usize,
    pub viewport_rows: usize,
    pub scroll_step: usize,
    pub max_scroll_steps: usize,
}

impl Default for DiffBenchmarkOptions {
    fn default() -> Self {
        Self {
            width: 160,
            viewport_rows: 40,
            scroll_step: 20,
            max_scroll_steps: 200,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffBenchmarkReport {
    pub syntax_enabled: bool,
    pub row_count: usize,
    pub file_count: usize,
    pub hunk_count: usize,
    pub open_micros: u128,
    pub initial_render_micros: u128,
    pub cold_scroll_steps: usize,
    pub cold_scroll_total_micros: u128,
    pub cold_scroll_max_micros: u128,
    pub syntax_settle_micros: Option<u128>,
    pub warm_scroll_steps: usize,
    pub warm_scroll_total_micros: u128,
    pub warm_scroll_max_micros: u128,
    pub warm_cache_hits: u64,
    pub warm_cache_misses: u64,
    pub syntax: SyntaxBenchmarkReport,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyntaxBenchmarkReport {
    pub queue_requests: u64,
    pub jobs_queued: u64,
    pub jobs_completed: u64,
    pub jobs_failed: u64,
    pub jobs_skipped: u64,
    pub jobs_rejected: u64,
    pub jobs_evicted: u64,
    pub stale_results: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub cache_entries_peak: usize,
    pub queue_depth_peak: usize,
    pub source_bytes_queued: u64,
    pub source_lines_queued: u64,
    pub estimated_memory_peak_bytes: u64,
}

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
    run_diff_with_live_updates(options, true)
}

pub fn run_diff_with_live_updates(options: DiffOptions, live_updates: bool) -> HzResult<()> {
    let changeset = hz_diff::load_review_ref(&options)?;

    let mut cleanup = TerminalCleanup::install()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    let layout = default_layout_for_width(terminal.size()?.width);
    let mut app = DiffApp::new(options, changeset, layout);
    let live_diff = if live_updates && live_diff_supported(&app.options) {
        match LiveDiff::start(app.options.clone(), &app.changeset.repo) {
            Ok(live_diff) => Some(live_diff),
            Err(error) => {
                app.set_notice(format!("live reload unavailable: {error}"));
                None
            }
        }
    } else {
        None
    };

    let result = run_loop(
        &mut terminal,
        &mut app,
        live_diff.as_ref().map(|live_diff| &live_diff.reload_rx),
    );
    let cleanup_result = cleanup.cleanup();

    result?;
    cleanup_result
}

pub fn benchmark_diff_view(
    changeset: Changeset,
    syntax_languages: Option<Vec<String>>,
    options: DiffBenchmarkOptions,
) -> DiffBenchmarkReport {
    let options = sanitize_benchmark_options(options);
    let file_count = changeset.files.len();
    let hunk_count = changeset.files.iter().map(|file| file.hunks.len()).sum();
    let syntax_mode = syntax_languages
        .map(SyntaxStartupMode::Languages)
        .unwrap_or(SyntaxStartupMode::Disabled);

    let open_start = Instant::now();
    let mut app = DiffApp::new_with_syntax(
        DiffOptions::default(),
        changeset,
        DiffLayoutMode::Split,
        syntax_mode,
    );
    let open_micros = open_start.elapsed().as_micros();
    let row_count = app.model.len();
    let syntax_enabled = app.syntax.is_some();

    app.set_viewport_rows(options.viewport_rows);

    let initial_render_start = Instant::now();
    render_viewport_for_benchmark(&mut app, options.width);
    let initial_render_micros = initial_render_start.elapsed().as_micros();

    let positions = benchmark_scroll_positions(
        app.model.len(),
        options.viewport_rows,
        options.scroll_step,
        options.max_scroll_steps,
    );
    let (cold_scroll_total_micros, cold_scroll_max_micros) =
        benchmark_scroll_pass(&mut app, &positions, options.width);

    let syntax_settle_micros =
        settle_syntax_for_benchmark(&mut app).map(|duration| duration.as_micros());

    let before_warm_stats = app.syntax_stats();
    let (warm_scroll_total_micros, warm_scroll_max_micros) =
        benchmark_scroll_pass(&mut app, &positions, options.width);
    let after_warm_stats = app.syntax_stats();

    DiffBenchmarkReport {
        syntax_enabled,
        row_count,
        file_count,
        hunk_count,
        open_micros,
        initial_render_micros,
        cold_scroll_steps: positions.len(),
        cold_scroll_total_micros,
        cold_scroll_max_micros,
        syntax_settle_micros,
        warm_scroll_steps: positions.len(),
        warm_scroll_total_micros,
        warm_scroll_max_micros,
        warm_cache_hits: after_warm_stats
            .cache_hits
            .saturating_sub(before_warm_stats.cache_hits),
        warm_cache_misses: after_warm_stats
            .cache_misses
            .saturating_sub(before_warm_stats.cache_misses),
        syntax: after_warm_stats,
    }
}

fn sanitize_benchmark_options(mut options: DiffBenchmarkOptions) -> DiffBenchmarkOptions {
    options.width = options.width.max(1);
    options.viewport_rows = options.viewport_rows.max(1);
    options.scroll_step = options.scroll_step.max(1);
    options.max_scroll_steps = options.max_scroll_steps.max(1);
    options
}

fn render_viewport_for_benchmark(app: &mut DiffApp, width: usize) {
    app.prepare_syntax_for_viewport(app.viewport_rows);
    for offset in 0..app.viewport_rows {
        let Some(row) = app.model.row(app.scroll + offset) else {
            continue;
        };
        let _ = render_row(app, row, width);
    }
}

fn benchmark_scroll_pass(app: &mut DiffApp, positions: &[usize], width: usize) -> (u128, u128) {
    let mut total = 0u128;
    let mut max = 0u128;
    for position in positions {
        let start = Instant::now();
        app.drain_syntax();
        app.set_scroll(*position);
        render_viewport_for_benchmark(app, width);
        let elapsed = start.elapsed().as_micros();
        total = total.saturating_add(elapsed);
        max = max.max(elapsed);
    }
    (total, max)
}

fn benchmark_scroll_positions(
    row_count: usize,
    viewport_rows: usize,
    scroll_step: usize,
    max_steps: usize,
) -> Vec<usize> {
    let max_scroll = max_scroll_for_viewport(row_count, viewport_rows);
    let mut positions = Vec::new();
    let mut position = 0usize;

    while positions.len() < max_steps {
        positions.push(position);
        if position >= max_scroll {
            break;
        }
        position = position.saturating_add(scroll_step).min(max_scroll);
    }

    positions
}

fn settle_syntax_for_benchmark(app: &mut DiffApp) -> Option<Duration> {
    app.syntax.as_ref()?;

    let start = Instant::now();
    let timeout = Duration::from_secs(30);
    loop {
        app.drain_syntax();
        let idle = app.syntax.as_ref().is_none_or(SyntaxRuntime::is_idle);
        if idle || start.elapsed() >= timeout {
            return Some(start.elapsed());
        }
        thread::sleep(Duration::from_millis(1));
    }
}

struct TerminalCleanup {
    active: bool,
}

impl TerminalCleanup {
    fn install() -> HzResult<Self> {
        enable_raw_mode()?;
        let mut cleanup = Self { active: true };
        let mut stdout = io::stdout();
        if let Err(error) = execute!(stdout, EnterAlternateScreen, EnableMouseCapture) {
            let _ = cleanup.cleanup();
            return Err(error.into());
        }

        Ok(cleanup)
    }

    fn cleanup(&mut self) -> HzResult<()> {
        if !self.active {
            return Ok(());
        }
        self.active = false;

        let raw_mode_result = disable_raw_mode();
        let mut stdout = io::stdout();
        let screen_result = execute!(stdout, DisableMouseCapture, LeaveAlternateScreen, Show);

        raw_mode_result?;
        screen_result?;
        Ok(())
    }
}

impl Drop for TerminalCleanup {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

#[derive(Debug)]
struct LiveDiff {
    _watcher: notify::RecommendedWatcher,
    _worker: thread::JoinHandle<()>,
    control_tx: Sender<LiveDiffCommand>,
    reload_rx: Receiver<LiveDiffReload>,
}

impl LiveDiff {
    fn start(options: DiffOptions, repo: &Path) -> HzResult<Self> {
        let watch_spec = live_diff_watch_spec(repo)?;
        let filter = watch_spec.filter.clone();
        let (control_tx, control_rx) = mpsc::channel();
        let (reload_tx, reload_rx) = mpsc::channel();
        let watcher_tx = control_tx.clone();

        let mut watcher =
            notify::recommended_watcher(move |result: Result<notify::Event, notify::Error>| {
                match result {
                    Ok(event) if filter.is_relevant_event(&event) => {
                        let _ = watcher_tx.send(LiveDiffCommand::Changed);
                    }
                    Ok(_) | Err(_) => {}
                }
            })
            .map_err(|error| watcher_error("failed to start live diff watcher", error))?;

        for watch_path in &watch_spec.watch_paths {
            if !watch_path.path.exists() {
                continue;
            }
            watcher
                .watch(&watch_path.path, watch_path.recursive_mode())
                .map_err(|error| {
                    watcher_error(
                        &format!("failed to watch {}", watch_path.path.display()),
                        error,
                    )
                })?;
        }

        let worker = spawn_live_diff_worker(options, control_rx, reload_tx);

        Ok(Self {
            _watcher: watcher,
            _worker: worker,
            control_tx,
            reload_rx,
        })
    }
}

impl Drop for LiveDiff {
    fn drop(&mut self) {
        let _ = self.control_tx.send(LiveDiffCommand::Stop);
    }
}

#[derive(Debug)]
enum LiveDiffCommand {
    Changed,
    Stop,
}

#[derive(Debug)]
enum LiveDiffReload {
    Loaded(HzResult<Changeset>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum DiffSide {
    Old,
    New,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct SyntaxKey {
    generation: u64,
    file: usize,
    hunk: usize,
    side: DiffSide,
}

#[derive(Debug, Clone)]
struct HighlightedSide {
    lines: Vec<Option<HighlightedLine>>,
}

impl HighlightedSide {
    fn memory_bytes(&self) -> usize {
        self.lines
            .iter()
            .filter_map(Option::as_ref)
            .flat_map(|line| line.segments.iter())
            .map(|segment| segment.text.len())
            .sum::<usize>()
            .saturating_add(self.lines.len() * std::mem::size_of::<Option<HighlightedLine>>())
    }
}

#[derive(Debug)]
struct LruCache<K, V> {
    entries: HashMap<K, V>,
    order: VecDeque<K>,
    capacity: usize,
}

impl<K, V> LruCache<K, V>
where
    K: Copy + Eq + Hash,
{
    fn new(capacity: usize) -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
            capacity,
        }
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
    }

    fn len(&self) -> usize {
        self.entries.len()
    }

    fn values(&self) -> impl Iterator<Item = &V> {
        self.entries.values()
    }

    fn contains_key(&self, key: &K) -> bool {
        self.entries.contains_key(key)
    }

    fn insert(&mut self, key: K, value: V) {
        if self.capacity == 0 {
            return;
        }

        if self.entries.contains_key(&key) {
            self.entries.insert(key, value);
            self.touch(&key);
            return;
        }

        while self.entries.len() >= self.capacity {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            self.entries.remove(&oldest);
        }

        self.entries.insert(key, value);
        self.order.push_back(key);
    }

    fn get(&mut self, key: &K) -> Option<&V> {
        if !self.entries.contains_key(key) {
            return None;
        }

        self.touch(key);
        self.entries.get(key)
    }

    fn touch(&mut self, key: &K) {
        if let Some(index) = self.order.iter().position(|candidate| candidate == key) {
            self.order.remove(index);
        }
        self.order.push_back(*key);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyntaxPriority {
    Visible,
    Prefetch,
}

#[derive(Debug, Clone)]
struct SyntaxWorkerQueue {
    inner: Arc<SyntaxWorkerQueueInner>,
}

#[derive(Debug)]
struct SyntaxWorkerQueueInner {
    state: Mutex<SyntaxWorkerQueueState>,
    ready: Condvar,
    capacity: usize,
}

#[derive(Debug)]
struct SyntaxWorkerQueueState {
    generation: u64,
    visible: VecDeque<SyntaxJob>,
    prefetch: VecDeque<SyntaxJob>,
    closed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SyntaxQueuePush {
    dropped: Option<SyntaxKey>,
    depth: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyntaxQueueError {
    Full,
    Closed,
    Stale,
}

impl SyntaxWorkerQueue {
    fn new(capacity: usize, generation: u64) -> Self {
        Self {
            inner: Arc::new(SyntaxWorkerQueueInner {
                state: Mutex::new(SyntaxWorkerQueueState {
                    generation,
                    visible: VecDeque::new(),
                    prefetch: VecDeque::new(),
                    closed: false,
                }),
                ready: Condvar::new(),
                capacity,
            }),
        }
    }

    fn try_push(
        &self,
        job: SyntaxJob,
        priority: SyntaxPriority,
    ) -> Result<SyntaxQueuePush, SyntaxQueueError> {
        let mut state = self
            .inner
            .state
            .lock()
            .map_err(|_| SyntaxQueueError::Closed)?;
        if state.closed {
            return Err(SyntaxQueueError::Closed);
        }
        if job.key.generation != state.generation {
            return Err(SyntaxQueueError::Stale);
        }
        if self.inner.capacity == 0 {
            return Err(SyntaxQueueError::Full);
        }

        let mut dropped = None;
        if state.len() >= self.inner.capacity {
            match priority {
                SyntaxPriority::Visible => {
                    let Some(evicted) = state.prefetch.pop_back() else {
                        return Err(SyntaxQueueError::Full);
                    };
                    dropped = Some(evicted.key);
                }
                SyntaxPriority::Prefetch => return Err(SyntaxQueueError::Full),
            }
        }

        match priority {
            SyntaxPriority::Visible => state.visible.push_back(job),
            SyntaxPriority::Prefetch => state.prefetch.push_back(job),
        }
        let depth = state.len();
        self.inner.ready.notify_one();
        Ok(SyntaxQueuePush { dropped, depth })
    }

    fn promote(&self, key: SyntaxKey) -> bool {
        let Ok(mut state) = self.inner.state.lock() else {
            return false;
        };
        if state.closed {
            return false;
        }

        let Some(index) = state.prefetch.iter().position(|job| job.key == key) else {
            return false;
        };
        let Some(job) = state.prefetch.remove(index) else {
            return false;
        };
        state.visible.push_back(job);
        self.inner.ready.notify_one();
        true
    }

    fn set_generation(&self, generation: u64) {
        let Ok(mut state) = self.inner.state.lock() else {
            return;
        };
        state.generation = generation;
        state.visible.retain(|job| job.key.generation == generation);
        state
            .prefetch
            .retain(|job| job.key.generation == generation);
        self.inner.ready.notify_all();
    }

    fn pop(&self) -> Option<SyntaxJob> {
        let mut state = self.inner.state.lock().ok()?;
        loop {
            if state.closed {
                return None;
            }

            let job = state
                .visible
                .pop_front()
                .or_else(|| state.prefetch.pop_front());
            if let Some(job) = job {
                if job.key.generation == state.generation {
                    return Some(job);
                }
                continue;
            }

            state = self.inner.ready.wait(state).ok()?;
        }
    }

    fn close(&self) {
        let Ok(mut state) = self.inner.state.lock() else {
            return;
        };
        state.closed = true;
        state.visible.clear();
        state.prefetch.clear();
        self.inner.ready.notify_all();
    }

    fn len(&self) -> usize {
        let Ok(state) = self.inner.state.lock() else {
            return 0;
        };
        state.len()
    }

    #[cfg(test)]
    fn try_pop(&self) -> Option<SyntaxJob> {
        let mut state = self.inner.state.lock().ok()?;
        state
            .visible
            .pop_front()
            .or_else(|| state.prefetch.pop_front())
    }
}

impl SyntaxWorkerQueueState {
    fn len(&self) -> usize {
        self.visible.len() + self.prefetch.len()
    }
}

#[derive(Debug)]
struct SyntaxRuntime {
    languages: SyntaxLanguageSet,
    result_rx: Receiver<SyntaxResult>,
    _worker: thread::JoinHandle<()>,
    queue: SyntaxWorkerQueue,
    cache: LruCache<SyntaxKey, HighlightedSide>,
    pending: HashSet<SyntaxKey>,
    skipped: HashSet<SyntaxKey>,
    stats: SyntaxBenchmarkReport,
}

impl SyntaxRuntime {
    fn start() -> HzResult<Option<Self>> {
        let languages = SyntaxLanguageSet::load()?;
        if languages.is_empty() {
            return Ok(None);
        }

        let (result_tx, result_rx) = mpsc::channel();
        let queue = SyntaxWorkerQueue::new(MAX_HIGHLIGHT_QUEUE_ENTRIES, 0);
        let worker_queue = queue.clone();
        let worker = thread::spawn(move || run_syntax_worker(worker_queue, result_tx));

        Ok(Some(Self {
            languages,
            result_rx,
            _worker: worker,
            queue,
            cache: LruCache::new(MAX_HIGHLIGHT_CACHE_ENTRIES),
            pending: HashSet::new(),
            skipped: HashSet::new(),
            stats: SyntaxBenchmarkReport::default(),
        }))
    }

    fn start_with_languages(languages: Vec<String>) -> Option<Self> {
        let languages = SyntaxLanguageSet::from_enabled_languages(&languages);
        if languages.is_empty() {
            return None;
        }

        let (result_tx, result_rx) = mpsc::channel();
        let queue = SyntaxWorkerQueue::new(MAX_HIGHLIGHT_QUEUE_ENTRIES, 0);
        let worker_queue = queue.clone();
        let worker = thread::spawn(move || run_syntax_worker(worker_queue, result_tx));

        Some(Self {
            languages,
            result_rx,
            _worker: worker,
            queue,
            cache: LruCache::new(MAX_HIGHLIGHT_CACHE_ENTRIES),
            pending: HashSet::new(),
            skipped: HashSet::new(),
            stats: SyntaxBenchmarkReport::default(),
        })
    }

    fn clear(&mut self, generation: u64) {
        self.cache.clear();
        self.pending.clear();
        self.skipped.clear();
        self.queue.set_generation(generation);
    }

    fn queue_hunk(
        &mut self,
        changeset: &Changeset,
        generation: u64,
        file: usize,
        hunk: usize,
        side: DiffSide,
        priority: SyntaxPriority,
    ) {
        let key = SyntaxKey {
            generation,
            file,
            hunk,
            side,
        };
        self.stats.queue_requests = self.stats.queue_requests.saturating_add(1);
        if self.cache.contains_key(&key) || self.skipped.contains(&key) {
            return;
        }
        if self.pending.contains(&key) {
            if priority == SyntaxPriority::Visible {
                self.queue.promote(key);
            }
            return;
        }

        let Some(file_diff) = changeset.files.get(file) else {
            self.skipped.insert(key);
            self.stats.jobs_skipped = self.stats.jobs_skipped.saturating_add(1);
            return;
        };
        let Some(path) = syntax_path(file_diff, side) else {
            self.skipped.insert(key);
            self.stats.jobs_skipped = self.stats.jobs_skipped.saturating_add(1);
            return;
        };
        let Some(language) = self.languages.language_for_path(path) else {
            self.skipped.insert(key);
            self.stats.jobs_skipped = self.stats.jobs_skipped.saturating_add(1);
            return;
        };
        let Some(hunk_diff) = file_diff.hunks.get(hunk) else {
            self.skipped.insert(key);
            self.stats.jobs_skipped = self.stats.jobs_skipped.saturating_add(1);
            return;
        };
        let Some(source) = build_hunk_source(&hunk_diff.lines, side) else {
            self.skipped.insert(key);
            self.stats.jobs_skipped = self.stats.jobs_skipped.saturating_add(1);
            return;
        };

        let source_bytes = source.text.len() as u64;
        let source_lines = source.line_map.len() as u64;

        let job = SyntaxJob {
            key,
            language,
            source,
        };

        match self.queue.try_push(job, priority) {
            Ok(push) => {
                if let Some(dropped) = push.dropped {
                    self.pending.remove(&dropped);
                    self.stats.jobs_evicted = self.stats.jobs_evicted.saturating_add(1);
                }
                self.stats.jobs_queued = self.stats.jobs_queued.saturating_add(1);
                self.stats.queue_depth_peak = self.stats.queue_depth_peak.max(push.depth);
                self.stats.source_bytes_queued =
                    self.stats.source_bytes_queued.saturating_add(source_bytes);
                self.stats.source_lines_queued =
                    self.stats.source_lines_queued.saturating_add(source_lines);
                self.pending.insert(key);
            }
            Err(SyntaxQueueError::Full | SyntaxQueueError::Stale) => {
                self.stats.jobs_rejected = self.stats.jobs_rejected.saturating_add(1);
            }
            Err(SyntaxQueueError::Closed) => {
                self.skipped.insert(key);
                self.stats.jobs_skipped = self.stats.jobs_skipped.saturating_add(1);
            }
        }
    }

    fn drain(&mut self, generation: u64) -> bool {
        let mut changed = false;
        while let Ok(result) = self.result_rx.try_recv() {
            self.pending.remove(&result.key);
            if result.key.generation != generation {
                self.stats.stale_results = self.stats.stale_results.saturating_add(1);
                continue;
            }

            match result.side {
                Ok(side) => {
                    self.cache.insert(result.key, side);
                    self.stats.jobs_completed = self.stats.jobs_completed.saturating_add(1);
                    self.stats.cache_entries_peak =
                        self.stats.cache_entries_peak.max(self.cache.len());
                    self.stats.estimated_memory_peak_bytes = self
                        .stats
                        .estimated_memory_peak_bytes
                        .max(self.estimated_memory_bytes() as u64);
                    changed = true;
                }
                Err(()) => {
                    self.skipped.insert(result.key);
                    self.stats.jobs_failed = self.stats.jobs_failed.saturating_add(1);
                }
            }
        }
        changed
    }

    fn line(&mut self, key: SyntaxKey, line: usize) -> Option<HighlightedLine> {
        let highlighted = self
            .cache
            .get(&key)
            .and_then(|side| side.lines.get(line))
            .and_then(Option::as_ref)
            .cloned();
        if highlighted.is_some() {
            self.stats.cache_hits = self.stats.cache_hits.saturating_add(1);
        } else {
            self.stats.cache_misses = self.stats.cache_misses.saturating_add(1);
        }
        highlighted
    }

    fn is_idle(&self) -> bool {
        self.pending.is_empty() && self.queue.len() == 0
    }

    fn stats(&self) -> SyntaxBenchmarkReport {
        self.stats.clone()
    }

    fn estimated_memory_bytes(&self) -> usize {
        self.cache.values().map(HighlightedSide::memory_bytes).sum()
    }
}

impl Drop for SyntaxRuntime {
    fn drop(&mut self) {
        self.queue.close();
    }
}

#[derive(Debug)]
struct SyntaxJob {
    key: SyntaxKey,
    language: String,
    source: HunkSource,
}

#[derive(Debug)]
struct SyntaxResult {
    key: SyntaxKey,
    side: Result<HighlightedSide, ()>,
}

#[derive(Debug)]
struct HunkSource {
    text: String,
    line_map: Vec<usize>,
    line_count: usize,
}

fn run_syntax_worker(queue: SyntaxWorkerQueue, result_tx: Sender<SyntaxResult>) {
    let mut highlighter = SyntaxHighlighter::new();
    while let Some(job) = queue.pop() {
        let side = highlighter
            .highlight(&job.language, &job.source.text)
            .map(|highlighted| highlighted_side(job.source, highlighted.lines))
            .map_err(|_| ());
        if result_tx.send(SyntaxResult { key: job.key, side }).is_err() {
            break;
        }
    }
}

fn highlighted_side(source: HunkSource, lines: Vec<HighlightedLine>) -> HighlightedSide {
    let mut mapped = vec![None; source.line_count];
    for (source_line, diff_line) in lines.into_iter().zip(source.line_map) {
        if let Some(slot) = mapped.get_mut(diff_line) {
            *slot = Some(source_line);
        }
    }
    HighlightedSide { lines: mapped }
}

fn syntax_path(file: &hz_diff::DiffFile, side: DiffSide) -> Option<&str> {
    match side {
        DiffSide::Old => file.old_path.as_deref().or(file.new_path.as_deref()),
        DiffSide::New => file.new_path.as_deref().or(file.old_path.as_deref()),
    }
}

fn build_hunk_source(lines: &[DiffLine], side: DiffSide) -> Option<HunkSource> {
    let mut text = String::new();
    let mut line_map = Vec::new();

    for (index, line) in lines.iter().enumerate() {
        if !line_belongs_to_side(line.kind, side) || line.text.len() > MAX_HIGHLIGHT_LINE_BYTES {
            continue;
        }
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&line.text);
        if text.len() > MAX_HIGHLIGHT_HUNK_BYTES {
            return None;
        }
        line_map.push(index);
    }

    (!line_map.is_empty()).then_some(HunkSource {
        text,
        line_map,
        line_count: lines.len(),
    })
}

fn line_belongs_to_side(kind: DiffLineKind, side: DiffSide) -> bool {
    matches!(
        (side, kind),
        (
            DiffSide::Old,
            DiffLineKind::Context | DiffLineKind::Deletion
        ) | (
            DiffSide::New,
            DiffLineKind::Context | DiffLineKind::Addition
        )
    )
}

fn unified_syntax_side(kind: DiffLineKind) -> Option<DiffSide> {
    match kind {
        DiffLineKind::Deletion => Some(DiffSide::Old),
        DiffLineKind::Addition | DiffLineKind::Context => Some(DiffSide::New),
        DiffLineKind::Meta => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LiveDiffWatchPath {
    path: PathBuf,
    recursive: bool,
}

impl LiveDiffWatchPath {
    fn recursive_mode(&self) -> RecursiveMode {
        if self.recursive {
            RecursiveMode::Recursive
        } else {
            RecursiveMode::NonRecursive
        }
    }
}

#[derive(Debug, Clone)]
struct LiveDiffWatchSpec {
    watch_paths: Vec<LiveDiffWatchPath>,
    filter: LiveDiffFilter,
}

impl LiveDiffWatchSpec {
    fn new(repo: &Path) -> Self {
        let mut spec = Self {
            watch_paths: Vec::new(),
            filter: LiveDiffFilter {
                repo: repo.to_path_buf(),
                git_state_paths: Vec::new(),
            },
        };
        spec.add_watch_path(repo.to_path_buf(), true);
        spec
    }

    fn add_git_state_path(&mut self, path: PathBuf) {
        if !self
            .filter
            .git_state_paths
            .iter()
            .any(|known| known == &path)
        {
            self.filter.git_state_paths.push(path);
        }
    }

    fn add_watch_path(&mut self, path: PathBuf, recursive: bool) {
        if let Some(existing) = self
            .watch_paths
            .iter_mut()
            .find(|watch_path| watch_path.path == path)
        {
            existing.recursive |= recursive;
            return;
        }

        self.watch_paths.push(LiveDiffWatchPath { path, recursive });
    }

    fn add_git_state(&mut self, path: PathBuf) {
        self.add_git_state_path(path.clone());
        if path.is_dir() {
            self.add_watch_path(path, true);
        } else if let Some(parent) = path.parent() {
            self.add_watch_path(parent.to_path_buf(), false);
        }
    }
}

#[derive(Debug, Clone)]
struct LiveDiffFilter {
    repo: PathBuf,
    git_state_paths: Vec<PathBuf>,
}

impl LiveDiffFilter {
    fn is_relevant_event(&self, event: &notify::Event) -> bool {
        if matches!(event.kind, notify::EventKind::Access(_)) {
            return false;
        }

        if event.paths.is_empty() {
            return true;
        }

        event.paths.iter().any(|path| self.is_relevant_path(path))
    }

    fn is_relevant_path(&self, path: &Path) -> bool {
        let joined;
        let path = if path.is_absolute() || path.starts_with(&self.repo) {
            path
        } else {
            joined = self.repo.join(path);
            &joined
        };

        if self.is_git_state_path(path) {
            return true;
        }

        if self.is_inside_repo_dot_git(path) {
            return false;
        }

        path.starts_with(&self.repo)
    }

    fn is_git_state_path(&self, path: &Path) -> bool {
        self.git_state_paths.iter().any(|state_path| {
            path == state_path
                || path.starts_with(state_path)
                || state_path.parent().is_some_and(|parent| path == parent)
        })
    }

    fn is_inside_repo_dot_git(&self, path: &Path) -> bool {
        let Ok(relative) = path.strip_prefix(&self.repo) else {
            return false;
        };

        relative
            .components()
            .next()
            .is_some_and(|component| component.as_os_str() == OsStr::new(".git"))
    }
}

fn live_diff_supported(options: &DiffOptions) -> bool {
    matches!(options.source, DiffSource::Worktree)
}

fn live_diff_watch_spec(repo: &Path) -> HzResult<LiveDiffWatchSpec> {
    let mut spec = LiveDiffWatchSpec::new(repo);
    for git_path in [
        "index",
        "index.lock",
        "HEAD",
        "HEAD.lock",
        "refs",
        "packed-refs",
        "packed-refs.lock",
        "info/exclude",
        "config",
    ] {
        spec.add_git_state(hz_git::git_path(repo, git_path)?);
    }
    Ok(spec)
}

fn spawn_live_diff_worker(
    options: DiffOptions,
    control_rx: Receiver<LiveDiffCommand>,
    reload_tx: Sender<LiveDiffReload>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        while let Ok(LiveDiffCommand::Changed) = control_rx.recv() {
            if !wait_for_live_diff_quiet_period(&control_rx) {
                break;
            }

            let changeset = hz_diff::load_review_ref(&options);
            if reload_tx.send(LiveDiffReload::Loaded(changeset)).is_err() {
                break;
            }
        }
    })
}

fn wait_for_live_diff_quiet_period(control_rx: &Receiver<LiveDiffCommand>) -> bool {
    loop {
        match control_rx.recv_timeout(LIVE_RELOAD_DEBOUNCE) {
            Ok(LiveDiffCommand::Changed) => continue,
            Ok(LiveDiffCommand::Stop) | Err(RecvTimeoutError::Disconnected) => return false,
            Err(RecvTimeoutError::Timeout) => return true,
        }
    }
}

fn watcher_error(context: &str, error: notify::Error) -> HzError {
    HzError::Usage(format!("{context}: {error}"))
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

fn run_loop(
    terminal: &mut CrosstermTerminal,
    app: &mut DiffApp,
    live_reload_rx: Option<&Receiver<LiveDiffReload>>,
) -> HzResult<()> {
    loop {
        app.expire_notice(Instant::now());
        drain_live_reloads(app, live_reload_rx);
        app.drain_syntax();
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

fn drain_live_reloads(app: &mut DiffApp, live_reload_rx: Option<&Receiver<LiveDiffReload>>) {
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
struct Notice {
    text: String,
    expires_at: Instant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SyntaxStartupMode {
    Config,
    Disabled,
    Languages(Vec<String>),
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
    notice: Option<Notice>,
    syntax: Option<SyntaxRuntime>,
    generation: u64,
    dirty: bool,
}

impl DiffApp {
    fn new(options: DiffOptions, changeset: Changeset, layout: DiffLayoutMode) -> Self {
        Self::new_with_syntax(options, changeset, layout, SyntaxStartupMode::Config)
    }

    fn new_with_syntax(
        options: DiffOptions,
        changeset: Changeset,
        layout: DiffLayoutMode,
        syntax_mode: SyntaxStartupMode,
    ) -> Self {
        let model = UiModel::new(&changeset, layout);
        let stats = changeset.stats();
        let (syntax, notice) = match syntax_mode {
            SyntaxStartupMode::Config => match SyntaxRuntime::start() {
                Ok(syntax) => (syntax, None),
                Err(error) => (
                    None,
                    Some(Notice {
                        text: format!("syntax disabled: {error}"),
                        expires_at: Instant::now() + NOTICE_TTL,
                    }),
                ),
            },
            SyntaxStartupMode::Disabled => (None, None),
            SyntaxStartupMode::Languages(languages) => {
                (SyntaxRuntime::start_with_languages(languages), None)
            }
        };
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
            notice,
            syntax,
            generation: 0,
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

    fn set_notice(&mut self, text: impl Into<String>) {
        self.notice = Some(Notice {
            text: text.into(),
            expires_at: Instant::now() + NOTICE_TTL,
        });
        self.dirty = true;
    }

    fn expire_notice(&mut self, now: Instant) {
        let expired = self
            .notice
            .as_ref()
            .is_some_and(|notice| now >= notice.expires_at);
        if expired {
            self.notice = None;
            self.dirty = true;
        }
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

    fn prepare_syntax_for_viewport(&mut self, visible_rows: usize) {
        if visible_rows == 0 {
            return;
        }
        let mut requested = HashSet::new();

        let visible_start = self.scroll;
        let visible_end = visible_start
            .saturating_add(visible_rows)
            .min(self.model.len());
        self.prepare_syntax_for_range(
            visible_start,
            visible_end,
            SyntaxPriority::Visible,
            &mut requested,
        );

        let prefetch_rows = visible_rows.saturating_mul(HIGHLIGHT_PREFETCH_VIEWPORTS);
        let ahead_end = visible_end
            .saturating_add(prefetch_rows)
            .min(self.model.len());
        self.prepare_syntax_for_range(
            visible_end,
            ahead_end,
            SyntaxPriority::Prefetch,
            &mut requested,
        );

        let behind_start = visible_start.saturating_sub(prefetch_rows);
        self.prepare_syntax_for_range(
            behind_start,
            visible_start,
            SyntaxPriority::Prefetch,
            &mut requested,
        );
    }

    fn prepare_syntax_for_range(
        &mut self,
        start: usize,
        end: usize,
        priority: SyntaxPriority,
        requested: &mut HashSet<SyntaxKey>,
    ) {
        for row_index in start..end {
            let Some(row) = self.model.row(row_index) else {
                continue;
            };
            self.prepare_syntax_for_row(row, priority, requested);
        }
    }

    fn prepare_syntax_for_row(
        &mut self,
        row: UiRow,
        priority: SyntaxPriority,
        requested: &mut HashSet<SyntaxKey>,
    ) {
        match row {
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
            UiRow::FileHeader(_)
            | UiRow::BinaryFile(_)
            | UiRow::Collapsed { .. }
            | UiRow::HunkHeader { .. }
            | UiRow::MetaLine { .. } => {}
        }
    }

    fn queue_syntax_hunk(
        &mut self,
        file: usize,
        hunk: usize,
        side: DiffSide,
        priority: SyntaxPriority,
        requested: &mut HashSet<SyntaxKey>,
    ) {
        let key = SyntaxKey {
            generation: self.generation,
            file,
            hunk,
            side,
        };
        if !requested.insert(key) {
            return;
        }
        if let Some(syntax) = self.syntax.as_mut() {
            syntax.queue_hunk(&self.changeset, self.generation, file, hunk, side, priority);
        }
    }

    fn drain_syntax(&mut self) {
        if let Some(syntax) = self.syntax.as_mut()
            && syntax.drain(self.generation)
        {
            self.dirty = true;
        }
    }

    fn syntax_stats(&self) -> SyntaxBenchmarkReport {
        self.syntax
            .as_ref()
            .map(SyntaxRuntime::stats)
            .unwrap_or_default()
    }

    fn syntax_line(
        &mut self,
        file: usize,
        hunk: usize,
        line: usize,
        side: DiffSide,
    ) -> Option<HighlightedLine> {
        self.syntax.as_mut().and_then(|syntax| {
            syntax.line(
                SyntaxKey {
                    generation: self.generation,
                    file,
                    hunk,
                    side,
                },
                line,
            )
        })
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
            self.set_notice(self.layout.label());
        }
    }

    fn reload(&mut self) -> HzResult<()> {
        let changeset = hz_diff::load_review_ref(&self.options)?;
        self.replace_changeset(changeset, Some("reloaded"));
        Ok(())
    }

    fn replace_changeset(&mut self, changeset: Changeset, notice: Option<&str>) {
        if self.changeset == changeset {
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
        self.generation = self.generation.wrapping_add(1);
        if let Some(syntax) = self.syntax.as_mut() {
            syntax.clear(self.generation);
        }
        self.model = UiModel::new(&self.changeset, self.layout);
        self.selected_file = selected_file.min(self.changeset.files.len().saturating_sub(1));
        let scroll = self
            .model
            .file_start_row(self.selected_file)
            .map(|start| start.saturating_add(relative_scroll))
            .unwrap_or_default();
        self.set_scroll(scroll);
        if let Some(notice) = notice {
            self.set_notice(notice);
        }
        self.dirty = true;
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
    let notice = app
        .notice
        .as_ref()
        .map(|notice| notice.text.as_str())
        .unwrap_or("");
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

fn draw_diff(frame: &mut Frame<'_>, app: &mut DiffApp, area: Rect) {
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
    app.prepare_syntax_for_viewport(visible_rows);

    let mut lines = Vec::with_capacity(visible_rows);
    for offset in 0..visible_rows {
        let Some(row) = app.model.row(app.scroll + offset) else {
            continue;
        };
        lines.push(render_row(app, row, area.width as usize));
    }

    frame.render_widget(Paragraph::new(Text::from(lines)), area);
}

fn render_row(app: &mut DiffApp, row: UiRow, width: usize) -> Line<'static> {
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
        UiRow::UnifiedLine { file, hunk, line } => {
            let diff_line = app.changeset.files[file].hunks[hunk].lines[line].clone();
            let syntax = unified_syntax_side(diff_line.kind)
                .and_then(|side| app.syntax_line(file, hunk, line, side));
            render_unified_line(&diff_line, syntax.as_ref(), width)
        }
        UiRow::MetaLine { file, hunk, line } => {
            let diff_line = app.changeset.files[file].hunks[hunk].lines[line].clone();
            render_unified_line(&diff_line, None, width)
        }
        UiRow::SplitLine {
            file,
            hunk,
            left,
            right,
        } => render_split_line(app, file, hunk, left, right, width),
    }
}

fn render_unified_line(
    line: &DiffLine,
    syntax: Option<&HighlightedLine>,
    width: usize,
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
    let mut spans = vec![Span::styled(
        fit_padded(&gutter, gutter_width),
        Style::default().fg(muted()).bg(row_bg(line.kind)),
    )];
    spans.extend(content_spans(&line.text, syntax, line.kind, content_width));
    Line::from(spans)
}

fn content_spans(
    text: &str,
    syntax: Option<&HighlightedLine>,
    kind: DiffLineKind,
    width: usize,
) -> Vec<Span<'static>> {
    if width == 0 {
        return Vec::new();
    }

    let Some(syntax) = syntax else {
        return vec![Span::styled(fit_padded(text, width), line_style(kind))];
    };

    let mut spans = Vec::new();
    let mut used = 0;
    for segment in &syntax.segments {
        if used >= width {
            break;
        }
        let fitted = fit(&segment.text, width - used);
        if fitted.is_empty() {
            break;
        }
        used += UnicodeWidthStr::width(fitted.as_str());
        spans.push(Span::styled(fitted, syntax_style(segment.class, kind)));
    }

    if used < width {
        spans.push(Span::styled(" ".repeat(width - used), line_style(kind)));
    }

    spans
}

fn syntax_style(class: Option<SyntaxClass>, kind: DiffLineKind) -> Style {
    let mut style = line_style(kind);
    if let Some(color) = class.and_then(syntax_fg) {
        style = style.fg(color);
    }
    style
}

fn syntax_fg(class: SyntaxClass) -> Option<Color> {
    let color = match class {
        SyntaxClass::Comment => muted(),
        SyntaxClass::Keyword => Color::Rgb(198, 153, 230),
        SyntaxClass::String => Color::Rgb(173, 219, 177),
        SyntaxClass::Number | SyntaxClass::Constant => Color::Rgb(229, 192, 123),
        SyntaxClass::Type | SyntaxClass::Constructor => Color::Rgb(102, 217, 239),
        SyntaxClass::Function => Color::Rgb(130, 190, 255),
        SyntaxClass::Property | SyntaxClass::Attribute => Color::Rgb(150, 200, 240),
        SyntaxClass::Punctuation => muted(),
        SyntaxClass::Operator => Color::Rgb(220, 170, 255),
        SyntaxClass::Tag => Color::Rgb(240, 150, 150),
        SyntaxClass::Module | SyntaxClass::Label => Color::Rgb(150, 180, 255),
        SyntaxClass::Variable => return None,
    };
    Some(color)
}

fn render_split_line(
    app: &mut DiffApp,
    file: usize,
    hunk: usize,
    left: Option<usize>,
    right: Option<usize>,
    width: usize,
) -> Line<'static> {
    if width == 0 {
        return Line::default();
    }

    let (left_line, right_line) = {
        let lines = &app.changeset.files[file].hunks[hunk].lines;
        (
            left.and_then(|index| lines.get(index)).cloned(),
            right.and_then(|index| lines.get(index)).cloned(),
        )
    };
    let left_syntax = left.and_then(|index| app.syntax_line(file, hunk, index, DiffSide::Old));
    let right_syntax = right.and_then(|index| app.syntax_line(file, hunk, index, DiffSide::New));

    let separator_width = usize::from(width > 1);
    let left_width = width.saturating_sub(separator_width) / 2;
    let right_width = width.saturating_sub(separator_width + left_width);
    let mut spans = split_cell_spans(
        left_line.as_ref(),
        left_syntax.as_ref(),
        SplitSide::Old,
        left_width,
    );
    if separator_width > 0 {
        spans.push(Span::styled("│", Style::default().fg(muted())));
    }
    spans.extend(split_cell_spans(
        right_line.as_ref(),
        right_syntax.as_ref(),
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

fn split_cell_spans(
    line: Option<&DiffLine>,
    syntax: Option<&HighlightedLine>,
    side: SplitSide,
    width: usize,
) -> Vec<Span<'static>> {
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

    let mut spans = vec![Span::styled(
        fit_padded(&format!("{line_number:>5} {sign}"), gutter_width),
        Style::default().fg(muted()).bg(row_bg(line.kind)),
    )];
    spans.extend(content_spans(&line.text, syntax, line.kind, content_width));
    spans
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
    let len = UnicodeWidthStr::width(out.as_str());
    if len < width {
        out.push_str(&" ".repeat(width - len));
    }
    out
}

fn right_aligned(left: &str, right: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }

    let left_len = UnicodeWidthStr::width(left);
    let right_len = UnicodeWidthStr::width(right);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

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
        assert_eq!(right_aligned("界", "x", 5), "界  x");
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

        let fresh = SyntaxKey {
            generation: 1,
            file: 0,
            hunk: 0,
            side: DiffSide::New,
        };
        queue
            .try_push(syntax_job(fresh), SyntaxPriority::Visible)
            .unwrap();
        assert_eq!(queue.try_pop().map(|job| job.key), Some(fresh));
    }

    #[test]
    fn oversized_hunks_fall_back_to_plain_diff_text() {
        let text = "x".repeat(MAX_HIGHLIGHT_LINE_BYTES);
        let line_count = (MAX_HIGHLIGHT_HUNK_BYTES / MAX_HIGHLIGHT_LINE_BYTES) + 2;
        let lines = (0..line_count)
            .map(|index| DiffLine {
                kind: DiffLineKind::Context,
                old_line: Some(index + 1),
                new_line: Some(index + 1),
                text: text.clone(),
            })
            .collect::<Vec<_>>();

        assert!(build_hunk_source(&lines, DiffSide::New).is_none());
    }

    #[test]
    fn oversized_lines_fall_back_without_disabling_the_whole_hunk() {
        let lines = vec![
            DiffLine {
                kind: DiffLineKind::Context,
                old_line: Some(1),
                new_line: Some(1),
                text: "x".repeat(MAX_HIGHLIGHT_LINE_BYTES + 1),
            },
            DiffLine {
                kind: DiffLineKind::Context,
                old_line: Some(2),
                new_line: Some(2),
                text: "let value = 1;".to_owned(),
            },
        ];

        let source = build_hunk_source(&lines, DiffSide::New).unwrap();

        assert_eq!(source.text, "let value = 1;");
        assert_eq!(source.line_map, vec![1]);
        assert_eq!(source.line_count, 2);
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

    fn syntax_key(file: usize) -> SyntaxKey {
        SyntaxKey {
            generation: 0,
            file,
            hunk: 0,
            side: DiffSide::New,
        }
    }

    fn syntax_job(key: SyntaxKey) -> SyntaxJob {
        SyntaxJob {
            key,
            language: "rust".to_owned(),
            source: HunkSource {
                text: "fn main() {}".to_owned(),
                line_map: vec![0],
                line_count: 1,
            },
        }
    }
}
