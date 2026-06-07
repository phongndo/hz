use std::{
    io, thread,
    time::{Duration, Instant},
};

use crate::{
    DiffApp, DiffBenchmarkOptions, DiffBenchmarkReport, DiffLayoutMode, SyntaxRuntime,
    SyntaxStartupMode, default_layout_for_width, max_scroll_for_viewport, render_row, run_loop,
    sync_live_diff,
};
use crossterm::{
    cursor::Show,
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use hz_core::HzResult;
use hz_diff::{Changeset, DiffOptions};
use ratatui::{Terminal, backend::CrosstermBackend};

pub fn run() -> HzResult<()> {
    run_diff(DiffOptions::default())
}

pub fn run_diff(options: DiffOptions) -> HzResult<()> {
    run_diff_with_live_updates(options, true)
}

pub fn run_diff_with_live_updates(options: DiffOptions, live_updates: bool) -> HzResult<()> {
    run_diff_with_live_updates_and_syntax(options, live_updates, true)
}

pub fn run_diff_with_live_updates_and_syntax(
    options: DiffOptions,
    live_updates: bool,
    syntax_enabled: bool,
) -> HzResult<()> {
    let changeset = hz_diff::load_review_ref(&options)?;

    let mut cleanup = TerminalCleanup::install()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    let layout = default_layout_for_width(terminal.size()?.width);
    let syntax_mode = if syntax_enabled {
        SyntaxStartupMode::Config
    } else {
        SyntaxStartupMode::Disabled
    };
    let mut app = DiffApp::new_with_syntax(options, changeset, layout, syntax_mode);
    let mut live_diff = None;
    sync_live_diff(&mut live_diff, &mut app, live_updates);

    let result = run_loop(&mut terminal, &mut app, live_updates, &mut live_diff);
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

pub(crate) fn sanitize_benchmark_options(
    mut options: DiffBenchmarkOptions,
) -> DiffBenchmarkOptions {
    options.width = options.width.max(1);
    options.viewport_rows = options.viewport_rows.max(1);
    options.scroll_step = options.scroll_step.max(1);
    options.max_scroll_steps = options.max_scroll_steps.max(1);
    options
}

pub(crate) fn render_viewport_for_benchmark(app: &mut DiffApp, width: usize) {
    app.prepare_syntax_for_viewport(app.viewport_rows);
    for offset in 0..app.viewport_rows {
        let Some(row) = app.model.row(app.scroll + offset) else {
            continue;
        };
        let _ = render_row(app, app.scroll + offset, row, width);
    }
}

pub(crate) fn benchmark_scroll_pass(
    app: &mut DiffApp,
    positions: &[usize],
    width: usize,
) -> (u128, u128) {
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

pub(crate) fn benchmark_scroll_positions(
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

pub(crate) fn settle_syntax_for_benchmark(app: &mut DiffApp) -> Option<Duration> {
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

pub(crate) struct TerminalCleanup {
    pub(crate) active: bool,
}

impl TerminalCleanup {
    pub(crate) fn install() -> HzResult<Self> {
        enable_raw_mode()?;
        let mut cleanup = Self { active: true };
        let mut stdout = io::stdout();
        if let Err(error) = execute!(stdout, EnterAlternateScreen, EnableMouseCapture) {
            let _ = cleanup.cleanup();
            return Err(error.into());
        }

        Ok(cleanup)
    }

    pub(crate) fn cleanup(&mut self) -> HzResult<()> {
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
