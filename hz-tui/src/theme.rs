use std::{
    env,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use hz_core::{HzError, HzResult};
use hz_diff::DiffLineKind;
use hz_syntax::{
    ColorOverrides, DiffGutterBackground, DiffSettings, SyntaxClass, SyntaxThemeConfig,
    SyntaxThemeSource,
};
use ratatui::prelude::Color;

pub(crate) const EVENT_POLL: Duration = Duration::from_millis(120);
pub(crate) const LIVE_RELOAD_DEBOUNCE: Duration = Duration::from_millis(200);
pub(crate) const MAX_READY_EVENTS_PER_FRAME: usize = 64;
pub(crate) const MOUSE_SCROLL_HISTORY_SIZE: usize = 3;
pub(crate) const MOUSE_SCROLL_STREAK_TIMEOUT: Duration = Duration::from_millis(150);
pub(crate) const MOUSE_SCROLL_MIN_TICK_INTERVAL: Duration = Duration::from_millis(6);
pub(crate) const MOUSE_SCROLL_ACCEL_A: f64 = 0.4;
pub(crate) const MOUSE_SCROLL_ACCEL_TAU: f64 = 4.0;
pub(crate) const MOUSE_SCROLL_MAX_MULTIPLIER: f64 = 3.0;
pub(crate) const MOUSE_SCROLL_REFERENCE_INTERVAL_MS: f64 = 100.0;
pub(crate) const HORIZONTAL_SCROLL_STEP: usize = 8;
pub(crate) const MIN_SPLIT_WIDTH: u16 = 120;
pub(crate) const GUTTER_WIDTH: usize = 7;
pub(crate) const UNIFIED_GUTTER_WIDTH: usize = 13;
pub(crate) const DIFF_INDICATOR: &str = "▌";
pub(crate) const EMPTY_DIFF_FILL: char = '╱';
pub(crate) const EMPTY_DIFF_FILL_SPACING: usize = 3;
pub(crate) const NOTICE_TTL: Duration = Duration::from_millis(1_500);
pub(crate) const MAX_SYNTAX_RESULTS_PER_FRAME: usize = 64;
pub(crate) const SYNTAX_THEME_ID: u64 = 0;
pub(crate) const MAX_INLINE_DIFF_LINE_BYTES: usize = 4 * 1024;
pub(crate) const MAX_INLINE_DIFF_TOKENS: usize = 256;
pub(crate) const MAX_INLINE_DIFF_CACHE_ENTRIES: usize = 512;
pub(crate) const MAX_BRANCH_MENU_ROWS: usize = 10;
pub(crate) const FILE_SIDEBAR_MIN_WIDTH: u16 = 20;
pub(crate) const FILE_SIDEBAR_MAX_WIDTH: u16 = 40;
pub(crate) const FILE_SIDEBAR_MIN_DIFF_WIDTH: u16 = 30;
pub(crate) const BRANCH_COMPARISON_SEPARATOR: &str = " → ";
pub(crate) const CURRENT_BRANCH_MARKER: &str = "●";
pub(crate) const BASE_BRANCH_MARKER: &str = "⌂";
pub(crate) const STATUSLINE_BG: Color = Color::Rgb(0x24, 0x25, 0x2b);
pub(crate) const STATUSLINE_ACCENT_BG: Color = Color::Rgb(0xe5, 0x9a, 0xca);
pub(crate) const STATUSLINE_ACCENT_FG: Color = Color::Rgb(0x24, 0x24, 0x2b);
pub(crate) const STATUSLINE_INFO_BG: Color = Color::Rgb(0x48, 0x49, 0x52);
pub(crate) const STATUSLINE_INFO_FG: Color = Color::Rgb(0xd7, 0xd6, 0xe8);
pub(crate) const STATUSLINE_SELECTOR_GAP: &str = " ";
pub(crate) const HELP_MENU_WIDTH: u16 = 90;
pub(crate) const HELP_MENU_HORIZONTAL_PADDING: u16 = 2;
pub(crate) const HELP_MENU_VERTICAL_PADDING: u16 = 1;
pub(crate) const HELP_MENU_TWO_COLUMN_MIN_WIDTH: usize = 64;
pub(crate) const HELP_MENU_COLUMN_GAP: usize = 4;
pub(crate) const HELP_KEY_COLUMN_WIDTH: usize = 17;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HelpMenuRow {
    Section(&'static str),
    Binding(&'static str, &'static str),
}

pub(crate) const HELP_MENU_LEFT_ROWS: &[HelpMenuRow] = &[
    HelpMenuRow::Section("Global"),
    HelpMenuRow::Binding("?", "toggle this help"),
    HelpMenuRow::Binding("q", "quit"),
    HelpMenuRow::Binding("Ctrl-C", "force quit"),
    HelpMenuRow::Binding("Esc", "close"),
    HelpMenuRow::Section("Navigate"),
    HelpMenuRow::Binding("j/k, ↑/↓", "scroll"),
    HelpMenuRow::Binding("d/u, PgDn/PgUp", "page"),
    HelpMenuRow::Binding("g/G, Home/End", "top / bottom"),
    HelpMenuRow::Binding("h/l, ←/→", "horizontal"),
    HelpMenuRow::Binding("J/K", "file"),
    HelpMenuRow::Binding("]/[", "hunk"),
    HelpMenuRow::Binding("Ctrl-G", "edit focused hunk"),
];

pub(crate) const HELP_MENU_RIGHT_ROWS: &[HelpMenuRow] = &[
    HelpMenuRow::Section("Actions"),
    HelpMenuRow::Binding("f", "filter files"),
    HelpMenuRow::Binding("/", "grep diff"),
    HelpMenuRow::Binding("n/p", "next / previous grep match"),
    HelpMenuRow::Binding("b", "toggle file sidebar"),
    HelpMenuRow::Binding("s", "split / unified"),
    HelpMenuRow::Binding("r", "reload diff"),
    HelpMenuRow::Binding("1/2/3/4", "all / branch / unstaged / staged"),
    HelpMenuRow::Section("Filter input"),
    HelpMenuRow::Binding("Enter", "keep filter"),
    HelpMenuRow::Binding("Esc", "clear active filters"),
    HelpMenuRow::Binding("Backspace", "delete char"),
    HelpMenuRow::Binding("Ctrl-U", "clear input"),
    HelpMenuRow::Section("Branch filter"),
    HelpMenuRow::Binding("type", "filter branches"),
    HelpMenuRow::Binding("Enter", "select branch"),
    HelpMenuRow::Binding("Tab/Shift-Tab", "cycle matches"),
    HelpMenuRow::Binding("Backspace", "delete char"),
    HelpMenuRow::Binding("Ctrl-U", "clear filter"),
    HelpMenuRow::Binding("↑/↓, PgUp/PgDn", "move"),
    HelpMenuRow::Binding("Home/End", "first / last match"),
];

pub(crate) fn line_gutter_fg(kind: DiffLineKind, theme: DiffTheme) -> Color {
    match kind {
        DiffLineKind::Addition => theme.addition_fg,
        DiffLineKind::Deletion => theme.deletion_fg,
        DiffLineKind::Context | DiffLineKind::Meta => theme.foreground,
    }
}

pub(crate) fn line_gutter_bg(kind: DiffLineKind, theme: DiffTheme) -> Color {
    if theme.transparent_background {
        return Color::Reset;
    }

    match (theme.diff.gutter_background, kind) {
        (DiffGutterBackground::Delta, DiffLineKind::Addition) => theme.addition_gutter_bg,
        (DiffGutterBackground::Delta, DiffLineKind::Deletion) => theme.deletion_gutter_bg,
        _ => theme.gutter_bg,
    }
}

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
    pub file_filter_micros: u128,
    pub legacy_file_filter_micros: u128,
    pub grep_filter_micros: u128,
    pub legacy_grep_filter_micros: u128,
    pub file_filter_apply_micros: u128,
    pub grep_filter_apply_micros: u128,
    pub hunk_navigation_steps: usize,
    pub hunk_navigation_total_micros: u128,
    pub hunk_navigation_max_micros: u128,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DiffTheme {
    pub(crate) foreground: Color,
    pub(crate) background: Color,
    pub(crate) header: Color,
    pub(crate) file: Color,
    pub(crate) hunk: Color,
    pub(crate) notice: Color,
    pub(crate) cursor: Color,
    pub(crate) muted: Color,
    pub(crate) gutter_bg: Color,
    pub(crate) empty_diff: Color,
    pub(crate) search_match_fg: Color,
    pub(crate) search_match_bg: Color,
    pub(crate) addition_fg: Color,
    pub(crate) addition_gutter_bg: Color,
    pub(crate) addition_bg: Color,
    pub(crate) addition_inline_bg: Color,
    pub(crate) deletion_fg: Color,
    pub(crate) deletion_gutter_bg: Color,
    pub(crate) deletion_bg: Color,
    pub(crate) deletion_inline_bg: Color,
    pub(crate) transparent_background: bool,
    pub(crate) diff: DiffSettings,
    pub(crate) syntax: SyntaxPalette,
}

impl Default for DiffTheme {
    fn default() -> Self {
        Self::system()
    }
}

impl DiffTheme {
    pub(crate) fn system() -> Self {
        let base = RgbColor::new(0x11, 0x13, 0x15);
        let green = RgbColor::new(0x88, 0xd3, 0x9b);
        let red = RgbColor::new(0xf0, 0xa0, 0xa0);
        Self {
            foreground: Color::Reset,
            background: Color::Reset,
            header: Color::Reset,
            file: Color::Reset,
            hunk: Color::Indexed(13),
            notice: green.color(),
            cursor: Color::White,
            muted: Color::Rgb(0x7d, 0x87, 0x94),
            gutter_bg: Color::Indexed(0),
            empty_diff: Color::Rgb(0x3d, 0x42, 0x49),
            search_match_fg: Color::Indexed(0),
            search_match_bg: Color::Indexed(3),
            addition_fg: green.color(),
            addition_gutter_bg: base.blend(green, 0.12).color(),
            addition_bg: Color::Rgb(0x1f, 0x30, 0x25),
            addition_inline_bg: base.blend(green, 0.28).color(),
            deletion_fg: red.color(),
            deletion_gutter_bg: base.blend(red, 0.12).color(),
            deletion_bg: Color::Rgb(0x37, 0x25, 0x26),
            deletion_inline_bg: base.blend(red, 0.28).color(),
            transparent_background: false,
            diff: DiffSettings::default(),
            syntax: SyntaxPalette::ansi(),
        }
    }

    pub(crate) fn terminal_dark() -> Self {
        let base = RgbColor::new(0x12, 0x12, 0x12);
        let green = RgbColor::new(0x9b, 0xd6, 0xa6);
        let red = RgbColor::new(0xe8, 0x8d, 0x8d);
        Self {
            foreground: Color::Reset,
            background: base.color(),
            header: Color::Rgb(220, 225, 232),
            file: Color::Rgb(215, 218, 224),
            hunk: Color::Rgb(205, 130, 170),
            notice: Color::Green,
            cursor: Color::White,
            muted: Color::Rgb(125, 135, 148),
            gutter_bg: Color::Rgb(12, 16, 20),
            empty_diff: Color::Rgb(38, 45, 54),
            search_match_fg: Color::Indexed(0),
            search_match_bg: Color::Indexed(3),
            addition_fg: Color::Indexed(2),
            addition_gutter_bg: base.blend(green, 0.035).color(),
            addition_bg: base.blend(green, 0.045).color(),
            addition_inline_bg: base.blend(green, 0.14).color(),
            deletion_fg: Color::Indexed(1),
            deletion_gutter_bg: base.blend(red, 0.035).color(),
            deletion_bg: base.blend(red, 0.045).color(),
            deletion_inline_bg: base.blend(red, 0.14).color(),
            transparent_background: false,
            diff: DiffSettings::default(),
            syntax: SyntaxPalette::terminal_dark(),
        }
    }

    pub(crate) fn terminal_light() -> Self {
        let base = RgbColor::new(0xff, 0xff, 0xff);
        let green = RgbColor::new(0x22, 0x5f, 0x2d);
        let red = RgbColor::new(0xb0, 0x38, 0x37);
        Self {
            foreground: Color::Reset,
            background: base.color(),
            header: Color::Rgb(36, 41, 47),
            file: Color::Rgb(45, 51, 59),
            hunk: Color::Rgb(138, 43, 92),
            notice: Color::Green,
            cursor: Color::Black,
            muted: Color::Rgb(106, 115, 125),
            gutter_bg: Color::Rgb(238, 242, 246),
            empty_diff: Color::Rgb(225, 228, 232),
            search_match_fg: Color::Rgb(36, 41, 47),
            search_match_bg: Color::Rgb(0xff, 0xec, 0x99),
            addition_fg: Color::Indexed(2),
            addition_gutter_bg: base.blend(green, 0.035).color(),
            addition_bg: base.blend(green, 0.045).color(),
            addition_inline_bg: base.blend(green, 0.14).color(),
            deletion_fg: Color::Indexed(1),
            deletion_gutter_bg: base.blend(red, 0.035).color(),
            deletion_bg: base.blend(red, 0.045).color(),
            deletion_inline_bg: base.blend(red, 0.14).color(),
            transparent_background: false,
            diff: DiffSettings::default(),
            syntax: SyntaxPalette::terminal_light(),
        }
    }

    pub(crate) fn minimal() -> Self {
        Self {
            foreground: Color::Reset,
            background: Color::Reset,
            header: Color::White,
            file: Color::White,
            hunk: Color::Magenta,
            notice: Color::Green,
            cursor: Color::White,
            muted: Color::DarkGray,
            gutter_bg: Color::Black,
            empty_diff: Color::DarkGray,
            search_match_fg: Color::Black,
            search_match_bg: Color::Yellow,
            addition_fg: Color::Green,
            addition_gutter_bg: Color::Black,
            addition_bg: Color::Reset,
            addition_inline_bg: Color::Green,
            deletion_fg: Color::Red,
            deletion_gutter_bg: Color::Black,
            deletion_bg: Color::Reset,
            deletion_inline_bg: Color::Red,
            transparent_background: false,
            diff: DiffSettings::default(),
            syntax: SyntaxPalette::minimal(),
        }
    }

    pub(crate) fn ansi() -> Self {
        Self {
            foreground: Color::Reset,
            background: Color::Reset,
            header: Color::Indexed(15),
            file: Color::Indexed(15),
            hunk: Color::Indexed(13),
            notice: Color::Indexed(2),
            cursor: Color::Indexed(15),
            muted: Color::Indexed(8),
            gutter_bg: Color::Indexed(0),
            empty_diff: Color::Indexed(8),
            search_match_fg: Color::Indexed(0),
            search_match_bg: Color::Indexed(3),
            addition_fg: Color::Indexed(2),
            addition_gutter_bg: Color::Indexed(0),
            addition_bg: Color::Reset,
            addition_inline_bg: Color::Indexed(22),
            deletion_fg: Color::Indexed(1),
            deletion_gutter_bg: Color::Indexed(0),
            deletion_bg: Color::Reset,
            deletion_inline_bg: Color::Indexed(52),
            transparent_background: false,
            diff: DiffSettings::default(),
            syntax: SyntaxPalette::ansi(),
        }
    }

    pub(crate) fn catppuccin_mocha() -> Self {
        let base = RgbColor::new(0x1e, 0x1e, 0x2e);
        let green = RgbColor::new(0xa6, 0xe3, 0xa1);
        let red = RgbColor::new(0xf3, 0x8b, 0xa8);
        Self {
            foreground: Color::Rgb(0xcd, 0xd6, 0xf4),
            background: base.color(),
            header: Color::Rgb(0xb4, 0xbe, 0xfe),
            file: Color::Rgb(0xcd, 0xd6, 0xf4),
            hunk: Color::Rgb(0xcb, 0xa6, 0xf7),
            notice: green.color(),
            cursor: Color::Rgb(0xf5, 0xe0, 0xdc),
            muted: Color::Rgb(0x6c, 0x70, 0x86),
            gutter_bg: base.blend(RgbColor::new(0, 0, 0), 0.22).color(),
            empty_diff: Color::Rgb(0x31, 0x32, 0x44),
            search_match_fg: base.color(),
            search_match_bg: Color::Rgb(0xf9, 0xe2, 0xaf),
            addition_fg: green.color(),
            addition_gutter_bg: base.blend(green, 0.035).color(),
            addition_bg: base.blend(green, 0.045).color(),
            addition_inline_bg: base.blend(green, 0.14).color(),
            deletion_fg: red.color(),
            deletion_gutter_bg: base.blend(red, 0.035).color(),
            deletion_bg: base.blend(red, 0.045).color(),
            deletion_inline_bg: base.blend(red, 0.14).color(),
            transparent_background: false,
            diff: DiffSettings::default(),
            syntax: SyntaxPalette::catppuccin_mocha(),
        }
    }

    pub(crate) fn gruvbox_dark() -> Self {
        let base = RgbColor::new(0x28, 0x28, 0x28);
        let green = RgbColor::new(0xb8, 0xbb, 0x26);
        let red = RgbColor::new(0xfb, 0x49, 0x34);
        Self {
            foreground: Color::Rgb(0xeb, 0xdb, 0xb2),
            background: base.color(),
            header: Color::Rgb(0xfb, 0xf1, 0xc7),
            file: Color::Rgb(0xeb, 0xdb, 0xb2),
            hunk: Color::Rgb(0xd3, 0x86, 0x9b),
            notice: green.color(),
            cursor: Color::Rgb(0xfb, 0xf1, 0xc7),
            muted: Color::Rgb(0x92, 0x83, 0x74),
            gutter_bg: base.blend(RgbColor::new(0, 0, 0), 0.22).color(),
            empty_diff: Color::Rgb(0x3c, 0x38, 0x36),
            search_match_fg: base.color(),
            search_match_bg: Color::Rgb(0xfa, 0xbd, 0x2f),
            addition_fg: green.color(),
            addition_gutter_bg: base.blend(green, 0.035).color(),
            addition_bg: base.blend(green, 0.045).color(),
            addition_inline_bg: base.blend(green, 0.14).color(),
            deletion_fg: red.color(),
            deletion_gutter_bg: base.blend(red, 0.035).color(),
            deletion_bg: base.blend(red, 0.045).color(),
            deletion_inline_bg: base.blend(red, 0.14).color(),
            transparent_background: false,
            diff: DiffSettings::default(),
            syntax: SyntaxPalette::gruvbox_dark(),
        }
    }

    pub(crate) fn tokyonight() -> Self {
        let base = RgbColor::new(0x1a, 0x1b, 0x26);
        let green = RgbColor::new(0x9e, 0xce, 0x6a);
        let red = RgbColor::new(0xf7, 0x76, 0x8e);
        Self {
            foreground: Color::Rgb(0xc0, 0xca, 0xf5),
            background: base.color(),
            header: Color::Rgb(0xc0, 0xca, 0xf5),
            file: Color::Rgb(0xc0, 0xca, 0xf5),
            hunk: Color::Rgb(0xbb, 0x9a, 0xf7),
            notice: green.color(),
            cursor: Color::Rgb(0xc0, 0xca, 0xf5),
            muted: Color::Rgb(0x56, 0x5f, 0x89),
            gutter_bg: base.blend(RgbColor::new(0, 0, 0), 0.22).color(),
            empty_diff: Color::Rgb(0x24, 0x28, 0x3b),
            search_match_fg: base.color(),
            search_match_bg: Color::Rgb(0xe0, 0xaf, 0x68),
            addition_fg: green.color(),
            addition_gutter_bg: base.blend(green, 0.035).color(),
            addition_bg: base.blend(green, 0.045).color(),
            addition_inline_bg: base.blend(green, 0.14).color(),
            deletion_fg: red.color(),
            deletion_gutter_bg: base.blend(red, 0.035).color(),
            deletion_bg: base.blend(red, 0.045).color(),
            deletion_inline_bg: base.blend(red, 0.14).color(),
            transparent_background: false,
            diff: DiffSettings::default(),
            syntax: SyntaxPalette::tokyonight(),
        }
    }

    pub(crate) fn dracula() -> Self {
        let base = RgbColor::new(0x28, 0x2a, 0x36);
        let green = RgbColor::new(0x50, 0xfa, 0x7b);
        let red = RgbColor::new(0xff, 0x55, 0x55);
        Self {
            foreground: Color::Rgb(0xf8, 0xf8, 0xf2),
            background: base.color(),
            header: Color::Rgb(0xf8, 0xf8, 0xf2),
            file: Color::Rgb(0xf8, 0xf8, 0xf2),
            hunk: Color::Rgb(0xff, 0x79, 0xc6),
            notice: green.color(),
            cursor: Color::Rgb(0xf8, 0xf8, 0xf2),
            muted: Color::Rgb(0x62, 0x72, 0xa4),
            gutter_bg: base.blend(RgbColor::new(0, 0, 0), 0.22).color(),
            empty_diff: Color::Rgb(0x44, 0x47, 0x5a),
            search_match_fg: base.color(),
            search_match_bg: Color::Rgb(0xf1, 0xfa, 0x8c),
            addition_fg: green.color(),
            addition_gutter_bg: base.blend(green, 0.035).color(),
            addition_bg: base.blend(green, 0.045).color(),
            addition_inline_bg: base.blend(green, 0.14).color(),
            deletion_fg: red.color(),
            deletion_gutter_bg: base.blend(red, 0.035).color(),
            deletion_bg: base.blend(red, 0.045).color(),
            deletion_inline_bg: base.blend(red, 0.14).color(),
            transparent_background: false,
            diff: DiffSettings::default(),
            syntax: SyntaxPalette::dracula(),
        }
    }

    pub(crate) fn base16(scheme: Base16Scheme) -> Self {
        Self {
            foreground: scheme.base05.color(),
            background: scheme.base00.color(),
            header: scheme.base06.color(),
            file: scheme.base05.color(),
            hunk: scheme.base0e.color(),
            notice: scheme.base0b.color(),
            cursor: scheme.base05.color(),
            muted: scheme.base03.color(),
            gutter_bg: scheme.base00.blend(RgbColor::new(0, 0, 0), 0.18).color(),
            empty_diff: scheme.base01.color(),
            search_match_fg: scheme.base00.color(),
            search_match_bg: scheme.base0a.color(),
            addition_fg: scheme.base0b.color(),
            addition_gutter_bg: scheme.base00.blend(scheme.base0b, 0.035).color(),
            addition_bg: scheme.base00.blend(scheme.base0b, 0.045).color(),
            addition_inline_bg: scheme.base00.blend(scheme.base0b, 0.14).color(),
            deletion_fg: scheme.base08.color(),
            deletion_gutter_bg: scheme.base00.blend(scheme.base08, 0.035).color(),
            deletion_bg: scheme.base00.blend(scheme.base08, 0.045).color(),
            deletion_inline_bg: scheme.base00.blend(scheme.base08, 0.14).color(),
            transparent_background: false,
            diff: DiffSettings::default(),
            syntax: SyntaxPalette::base16(scheme),
        }
    }

    pub(crate) fn with_transparent_background(mut self, transparent: bool) -> Self {
        self.transparent_background = transparent;
        self
    }

    pub(crate) fn with_diff_settings(mut self, diff: DiffSettings) -> Self {
        self.diff = diff;
        self
    }

    pub(crate) fn with_color_overrides(mut self, colors: &ColorOverrides) -> HzResult<Self> {
        if let Some(color) = config_color(&colors.bg, "bg")? {
            self.background = color;
        }
        if let Some(color) = config_color(&colors.fg, "fg")? {
            self.foreground = color;
        }
        if let Some(color) = config_color(&colors.header, "header")? {
            self.header = color;
        }
        if let Some(color) = config_color(&colors.file, "file")? {
            self.file = color;
        }
        if let Some(color) = config_color(&colors.hunk, "hunk")? {
            self.hunk = color;
        }
        if let Some(color) = config_color(&colors.notice, "notice")? {
            self.notice = color;
        }
        if let Some(color) = config_color(&colors.cursor, "cursor")? {
            self.cursor = color;
        }
        if let Some(color) = config_color(&colors.muted, "muted")? {
            self.muted = color;
        }
        if let Some(color) = config_color(&colors.gutter_bg, "gutter_bg")? {
            self.gutter_bg = color;
        }
        if let Some(color) = config_color(&colors.empty_diff, "empty_diff")? {
            self.empty_diff = color;
        }
        if let Some(color) = config_color(&colors.search_match_fg, "search_match_fg")? {
            self.search_match_fg = color;
        }
        if let Some(color) = config_color(&colors.search_match_bg, "search_match_bg")? {
            self.search_match_bg = color;
        }
        if let Some(color) = config_color(&colors.addition_fg, "addition_fg")? {
            self.addition_fg = color;
        }
        if let Some(color) = config_color(&colors.addition_gutter_bg, "addition_gutter_bg")? {
            self.addition_gutter_bg = color;
        }
        if let Some(color) = config_color(&colors.addition_bg, "addition_bg")? {
            self.addition_bg = color;
        }
        if let Some(color) = config_color(&colors.addition_inline_bg, "addition_inline_bg")? {
            self.addition_inline_bg = color;
        }
        if let Some(color) = config_color(&colors.deletion_fg, "deletion_fg")? {
            self.deletion_fg = color;
        }
        if let Some(color) = config_color(&colors.deletion_gutter_bg, "deletion_gutter_bg")? {
            self.deletion_gutter_bg = color;
        }
        if let Some(color) = config_color(&colors.deletion_bg, "deletion_bg")? {
            self.deletion_bg = color;
        }
        if let Some(color) = config_color(&colors.deletion_inline_bg, "deletion_inline_bg")? {
            self.deletion_inline_bg = color;
        }
        if let Some(color) = config_color(&colors.attribute, "attribute")? {
            self.syntax.attribute = Some(color);
        }
        if let Some(color) = config_color(&colors.comment, "comment")? {
            self.syntax.comment = Some(color);
        }
        if let Some(color) = config_color(&colors.constant, "constant")? {
            self.syntax.constant = Some(color);
        }
        if let Some(color) = config_color(&colors.constructor, "constructor")? {
            self.syntax.constructor = Some(color);
        }
        if let Some(color) = config_color(&colors.function, "function")? {
            self.syntax.function = Some(color);
        }
        if let Some(color) = config_color(&colors.keyword, "keyword")? {
            self.syntax.keyword = Some(color);
        }
        if let Some(color) = config_color(&colors.label, "label")? {
            self.syntax.label = Some(color);
        }
        if let Some(color) = config_color(&colors.module, "module")? {
            self.syntax.module = Some(color);
        }
        if let Some(color) = config_color(&colors.number, "number")? {
            self.syntax.number = Some(color);
        }
        if let Some(color) = config_color(&colors.operator, "operator")? {
            self.syntax.operator = Some(color);
        }
        if let Some(color) = config_color(&colors.property, "property")? {
            self.syntax.property = Some(color);
        }
        if let Some(color) = config_color(&colors.punctuation, "punctuation")? {
            self.syntax.punctuation = Some(color);
        }
        if let Some(color) = config_color(&colors.string, "string")? {
            self.syntax.string = Some(color);
        }
        if let Some(color) = config_color(&colors.tag, "tag")? {
            self.syntax.tag = Some(color);
        }
        if let Some(color) = config_color(&colors.r#type, "type")? {
            self.syntax.r#type = Some(color);
        }
        if let Some(color) = config_color(&colors.variable, "variable")? {
            self.syntax.variable = Some(color);
        }
        Ok(self)
    }
}

pub(crate) fn config_color(value: &Option<String>, name: &str) -> HzResult<Option<Color>> {
    value
        .as_deref()
        .map(|value| parse_config_color(value, name))
        .transpose()
}

pub(crate) fn parse_config_color(value: &str, name: &str) -> HzResult<Color> {
    let trimmed = value.trim();
    let lower = trimmed.to_ascii_lowercase();

    if matches!(lower.as_str(), "default" | "reset" | "none") {
        return Ok(Color::Reset);
    }

    if let Some(color) = parse_config_hex_color(trimmed) {
        return Ok(color.color());
    }

    if let Some(index) = parse_ansi_index(&lower) {
        return Ok(Color::Indexed(index));
    }

    if let Some(color) = parse_named_color(&lower) {
        return Ok(color);
    }

    Err(HzError::Usage(format!(
        "invalid color for {name}: {value}; expected #rrggbb, ansi-N, or a named color"
    )))
}

pub(crate) fn parse_config_hex_color(value: &str) -> Option<RgbColor> {
    let value = value
        .trim()
        .trim_matches(['\'', '"'])
        .strip_prefix('#')
        .or_else(|| value.trim().strip_prefix("0x"))
        .unwrap_or_else(|| value.trim().trim_matches(['\'', '"']));
    parse_hex_digits(value)
}

pub(crate) fn parse_ansi_index(value: &str) -> Option<u8> {
    let index = value
        .strip_prefix("ansi-")
        .or_else(|| value.strip_prefix("ansi:"))
        .or_else(|| value.strip_prefix("indexed-"))
        .or_else(|| value.strip_prefix("indexed:"))
        .unwrap_or(value);
    index.parse::<u8>().ok()
}

pub(crate) fn parse_named_color(value: &str) -> Option<Color> {
    match value.replace('_', "-").as_str() {
        "black" => Some(Color::Black),
        "red" => Some(Color::Red),
        "green" => Some(Color::Green),
        "yellow" => Some(Color::Yellow),
        "blue" => Some(Color::Blue),
        "magenta" | "purple" => Some(Color::Magenta),
        "cyan" => Some(Color::Cyan),
        "gray" | "grey" => Some(Color::Gray),
        "dark-gray" | "dark-grey" | "bright-black" => Some(Color::DarkGray),
        "white" | "bright-white" => Some(Color::White),
        "bright-red" | "light-red" => Some(Color::LightRed),
        "bright-green" | "light-green" => Some(Color::LightGreen),
        "bright-yellow" | "light-yellow" => Some(Color::LightYellow),
        "bright-blue" | "light-blue" => Some(Color::LightBlue),
        "bright-magenta" | "light-magenta" | "bright-purple" | "light-purple" => {
            Some(Color::LightMagenta)
        }
        "bright-cyan" | "light-cyan" => Some(Color::LightCyan),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SyntaxPalette {
    pub(crate) attribute: Option<Color>,
    pub(crate) comment: Option<Color>,
    pub(crate) constant: Option<Color>,
    pub(crate) constructor: Option<Color>,
    pub(crate) function: Option<Color>,
    pub(crate) keyword: Option<Color>,
    pub(crate) label: Option<Color>,
    pub(crate) module: Option<Color>,
    pub(crate) number: Option<Color>,
    pub(crate) operator: Option<Color>,
    pub(crate) property: Option<Color>,
    pub(crate) punctuation: Option<Color>,
    pub(crate) string: Option<Color>,
    pub(crate) tag: Option<Color>,
    pub(crate) r#type: Option<Color>,
    pub(crate) variable: Option<Color>,
}

impl SyntaxPalette {
    pub(crate) fn terminal_dark() -> Self {
        Self {
            attribute: Some(Color::Rgb(150, 200, 240)),
            comment: Some(Color::Rgb(125, 135, 148)),
            constant: Some(Color::Rgb(229, 192, 123)),
            constructor: Some(Color::Rgb(102, 217, 239)),
            function: Some(Color::Rgb(130, 190, 255)),
            keyword: Some(Color::Rgb(198, 153, 230)),
            label: Some(Color::Rgb(150, 180, 255)),
            module: Some(Color::Rgb(150, 180, 255)),
            number: Some(Color::Rgb(229, 192, 123)),
            operator: Some(Color::Rgb(220, 170, 255)),
            property: Some(Color::Rgb(150, 200, 240)),
            punctuation: Some(Color::Rgb(125, 135, 148)),
            string: Some(Color::Rgb(173, 219, 177)),
            tag: Some(Color::Rgb(240, 150, 150)),
            r#type: Some(Color::Rgb(102, 217, 239)),
            variable: None,
        }
    }

    pub(crate) fn terminal_light() -> Self {
        Self {
            attribute: Some(Color::Rgb(0, 92, 197)),
            comment: Some(Color::Rgb(106, 115, 125)),
            constant: Some(Color::Rgb(177, 82, 0)),
            constructor: Some(Color::Rgb(0, 95, 115)),
            function: Some(Color::Rgb(0, 92, 197)),
            keyword: Some(Color::Rgb(111, 66, 193)),
            label: Some(Color::Rgb(0, 92, 197)),
            module: Some(Color::Rgb(0, 92, 197)),
            number: Some(Color::Rgb(177, 82, 0)),
            operator: Some(Color::Rgb(111, 66, 193)),
            property: Some(Color::Rgb(0, 92, 197)),
            punctuation: Some(Color::Rgb(106, 115, 125)),
            string: Some(Color::Rgb(34, 134, 58)),
            tag: Some(Color::Rgb(176, 56, 55)),
            r#type: Some(Color::Rgb(0, 95, 115)),
            variable: None,
        }
    }

    pub(crate) fn minimal() -> Self {
        Self {
            attribute: None,
            comment: Some(Color::DarkGray),
            constant: Some(Color::Yellow),
            constructor: Some(Color::Cyan),
            function: Some(Color::Blue),
            keyword: Some(Color::Magenta),
            label: Some(Color::Blue),
            module: Some(Color::Blue),
            number: Some(Color::Yellow),
            operator: Some(Color::Magenta),
            property: None,
            punctuation: Some(Color::DarkGray),
            string: Some(Color::Green),
            tag: Some(Color::Red),
            r#type: Some(Color::Cyan),
            variable: None,
        }
    }

    pub(crate) fn ansi() -> Self {
        Self {
            attribute: Some(Color::Indexed(12)),
            comment: Some(Color::Indexed(8)),
            constant: Some(Color::Indexed(11)),
            constructor: Some(Color::Indexed(14)),
            function: Some(Color::Indexed(12)),
            keyword: Some(Color::Indexed(13)),
            label: Some(Color::Indexed(12)),
            module: Some(Color::Indexed(12)),
            number: Some(Color::Indexed(11)),
            operator: Some(Color::Indexed(13)),
            property: Some(Color::Indexed(12)),
            punctuation: Some(Color::Indexed(8)),
            string: Some(Color::Indexed(10)),
            tag: Some(Color::Indexed(9)),
            r#type: Some(Color::Indexed(14)),
            variable: None,
        }
    }

    pub(crate) fn catppuccin_mocha() -> Self {
        Self {
            attribute: Some(Color::Rgb(0x94, 0xe2, 0xd5)),
            comment: Some(Color::Rgb(0x6c, 0x70, 0x86)),
            constant: Some(Color::Rgb(0xfa, 0xb3, 0x87)),
            constructor: Some(Color::Rgb(0xf9, 0xe2, 0xaf)),
            function: Some(Color::Rgb(0x89, 0xb4, 0xfa)),
            keyword: Some(Color::Rgb(0xcb, 0xa6, 0xf7)),
            label: Some(Color::Rgb(0xb4, 0xbe, 0xfe)),
            module: Some(Color::Rgb(0xb4, 0xbe, 0xfe)),
            number: Some(Color::Rgb(0xfa, 0xb3, 0x87)),
            operator: Some(Color::Rgb(0xcb, 0xa6, 0xf7)),
            property: Some(Color::Rgb(0x89, 0xdc, 0xeb)),
            punctuation: Some(Color::Rgb(0x6c, 0x70, 0x86)),
            string: Some(Color::Rgb(0xa6, 0xe3, 0xa1)),
            tag: Some(Color::Rgb(0xf3, 0x8b, 0xa8)),
            r#type: Some(Color::Rgb(0xf9, 0xe2, 0xaf)),
            variable: None,
        }
    }

    pub(crate) fn gruvbox_dark() -> Self {
        Self {
            attribute: Some(Color::Rgb(0x8e, 0xc0, 0x7c)),
            comment: Some(Color::Rgb(0x92, 0x83, 0x74)),
            constant: Some(Color::Rgb(0xfe, 0x80, 0x19)),
            constructor: Some(Color::Rgb(0xfa, 0xbd, 0x2f)),
            function: Some(Color::Rgb(0x83, 0xa5, 0x98)),
            keyword: Some(Color::Rgb(0xfb, 0x49, 0x34)),
            label: Some(Color::Rgb(0xd3, 0x86, 0x9b)),
            module: Some(Color::Rgb(0x83, 0xa5, 0x98)),
            number: Some(Color::Rgb(0xd3, 0x86, 0x9b)),
            operator: Some(Color::Rgb(0xfe, 0x80, 0x19)),
            property: Some(Color::Rgb(0x8e, 0xc0, 0x7c)),
            punctuation: Some(Color::Rgb(0x92, 0x83, 0x74)),
            string: Some(Color::Rgb(0xb8, 0xbb, 0x26)),
            tag: Some(Color::Rgb(0xfb, 0x49, 0x34)),
            r#type: Some(Color::Rgb(0xfa, 0xbd, 0x2f)),
            variable: None,
        }
    }

    pub(crate) fn tokyonight() -> Self {
        Self {
            attribute: Some(Color::Rgb(0x73, 0xda, 0xca)),
            comment: Some(Color::Rgb(0x56, 0x5f, 0x89)),
            constant: Some(Color::Rgb(0xff, 0x9e, 0x64)),
            constructor: Some(Color::Rgb(0xe0, 0xaf, 0x68)),
            function: Some(Color::Rgb(0x7a, 0xa2, 0xf7)),
            keyword: Some(Color::Rgb(0xbb, 0x9a, 0xf7)),
            label: Some(Color::Rgb(0x7a, 0xa2, 0xf7)),
            module: Some(Color::Rgb(0x7a, 0xa2, 0xf7)),
            number: Some(Color::Rgb(0xff, 0x9e, 0x64)),
            operator: Some(Color::Rgb(0xbb, 0x9a, 0xf7)),
            property: Some(Color::Rgb(0x73, 0xda, 0xca)),
            punctuation: Some(Color::Rgb(0x56, 0x5f, 0x89)),
            string: Some(Color::Rgb(0x9e, 0xce, 0x6a)),
            tag: Some(Color::Rgb(0xf7, 0x76, 0x8e)),
            r#type: Some(Color::Rgb(0x2a, 0xc3, 0xde)),
            variable: None,
        }
    }

    pub(crate) fn dracula() -> Self {
        Self {
            attribute: Some(Color::Rgb(0x8b, 0xe9, 0xfd)),
            comment: Some(Color::Rgb(0x62, 0x72, 0xa4)),
            constant: Some(Color::Rgb(0xbd, 0x93, 0xf9)),
            constructor: Some(Color::Rgb(0x8b, 0xe9, 0xfd)),
            function: Some(Color::Rgb(0x50, 0xfa, 0x7b)),
            keyword: Some(Color::Rgb(0xff, 0x79, 0xc6)),
            label: Some(Color::Rgb(0xbd, 0x93, 0xf9)),
            module: Some(Color::Rgb(0xbd, 0x93, 0xf9)),
            number: Some(Color::Rgb(0xbd, 0x93, 0xf9)),
            operator: Some(Color::Rgb(0xff, 0x79, 0xc6)),
            property: Some(Color::Rgb(0x8b, 0xe9, 0xfd)),
            punctuation: Some(Color::Rgb(0x62, 0x72, 0xa4)),
            string: Some(Color::Rgb(0xf1, 0xfa, 0x8c)),
            tag: Some(Color::Rgb(0xff, 0x55, 0x55)),
            r#type: Some(Color::Rgb(0x8b, 0xe9, 0xfd)),
            variable: None,
        }
    }

    pub(crate) fn base16(scheme: Base16Scheme) -> Self {
        Self {
            attribute: Some(scheme.base0c.color()),
            comment: Some(scheme.base03.color()),
            constant: Some(scheme.base09.color()),
            constructor: Some(scheme.base0a.color()),
            function: Some(scheme.base0d.color()),
            keyword: Some(scheme.base0e.color()),
            label: Some(scheme.base0d.color()),
            module: Some(scheme.base0d.color()),
            number: Some(scheme.base09.color()),
            operator: Some(scheme.base0e.color()),
            property: Some(scheme.base0c.color()),
            punctuation: Some(scheme.base04.color()),
            string: Some(scheme.base0b.color()),
            tag: Some(scheme.base08.color()),
            r#type: Some(scheme.base0a.color()),
            variable: None,
        }
    }

    pub(crate) fn color(self, class: SyntaxClass) -> Option<Color> {
        match class {
            SyntaxClass::Attribute => self.attribute,
            SyntaxClass::Comment => self.comment,
            SyntaxClass::Constant => self.constant,
            SyntaxClass::Constructor => self.constructor,
            SyntaxClass::Function => self.function,
            SyntaxClass::Keyword => self.keyword,
            SyntaxClass::Label => self.label,
            SyntaxClass::Module => self.module,
            SyntaxClass::Number => self.number,
            SyntaxClass::Operator => self.operator,
            SyntaxClass::Property => self.property,
            SyntaxClass::Punctuation => self.punctuation,
            SyntaxClass::String => self.string,
            SyntaxClass::Tag => self.tag,
            SyntaxClass::Type => self.r#type,
            SyntaxClass::Variable => self.variable,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Base16Scheme {
    pub(crate) base00: RgbColor,
    pub(crate) base01: RgbColor,
    pub(crate) base03: RgbColor,
    pub(crate) base04: RgbColor,
    pub(crate) base05: RgbColor,
    pub(crate) base06: RgbColor,
    pub(crate) base08: RgbColor,
    pub(crate) base09: RgbColor,
    pub(crate) base0a: RgbColor,
    pub(crate) base0b: RgbColor,
    pub(crate) base0c: RgbColor,
    pub(crate) base0d: RgbColor,
    pub(crate) base0e: RgbColor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RgbColor {
    pub(crate) red: u8,
    pub(crate) green: u8,
    pub(crate) blue: u8,
}

impl RgbColor {
    const fn new(red: u8, green: u8, blue: u8) -> Self {
        Self { red, green, blue }
    }

    pub(crate) fn color(self) -> Color {
        Color::Rgb(self.red, self.green, self.blue)
    }

    pub(crate) fn blend(self, other: Self, amount: f32) -> Self {
        let amount = amount.clamp(0.0, 1.0);
        let mix = |a: u8, b: u8| -> u8 {
            ((f32::from(a) * (1.0 - amount)) + (f32::from(b) * amount)).round() as u8
        };
        Self {
            red: mix(self.red, other.red),
            green: mix(self.green, other.green),
            blue: mix(self.blue, other.blue),
        }
    }
}

pub(crate) fn diff_theme_from_config(config: &SyntaxThemeConfig) -> HzResult<DiffTheme> {
    match config.source {
        SyntaxThemeSource::Builtin => {
            let name = config.name.as_deref();
            match builtin_diff_theme(name) {
                Ok(theme) => Ok(theme),
                Err(error) => {
                    if let Some(name) = name {
                        if let Some(theme) = load_named_colorscheme(name)? {
                            return Ok(theme);
                        }
                    }
                    Err(error)
                }
            }
        }
        SyntaxThemeSource::Ansi => Ok(DiffTheme::ansi()),
        SyntaxThemeSource::Base16 => {
            let path = config.path.as_ref().ok_or_else(|| {
                HzError::Usage("base16 colorscheme requires colorscheme.path".to_owned())
            })?;
            Ok(DiffTheme::base16(load_base16_scheme(path)?))
        }
    }
}

pub(crate) fn load_named_colorscheme(name: &str) -> HzResult<Option<DiffTheme>> {
    let name = name.trim();
    if name.is_empty() || Path::new(name).file_name().and_then(OsStr::to_str) != Some(name) {
        return Ok(None);
    }

    let colorscheme_dir = hz_syntax::colorscheme_dir()?;
    for path in colorscheme_paths(&colorscheme_dir, name) {
        if path.exists() {
            return Ok(Some(DiffTheme::base16(load_base16_scheme(&path)?)));
        }
    }
    Ok(None)
}

pub(crate) fn colorscheme_paths(dir: &Path, name: &str) -> Vec<PathBuf> {
    let path = dir.join(name);
    if Path::new(name).extension().is_some() {
        return vec![path];
    }

    ["toml", "yaml", "yml"]
        .into_iter()
        .map(|extension| path.with_extension(extension))
        .collect()
}

pub(crate) fn builtin_diff_theme(name: Option<&str>) -> HzResult<DiffTheme> {
    let name = name.unwrap_or("system").trim().to_ascii_lowercase();
    match name.as_str() {
        "system" | "default" | "" => Ok(DiffTheme::system()),
        "terminal-dark" | "hz-dark" | "dark" => Ok(DiffTheme::terminal_dark()),
        "terminal-light" | "hz-light" | "light" => Ok(DiffTheme::terminal_light()),
        "minimal" => Ok(DiffTheme::minimal()),
        "catppuccin" | "catppuccin-mocha" | "mocha" => Ok(DiffTheme::catppuccin_mocha()),
        "gruvbox" | "gruvbox-dark" => Ok(DiffTheme::gruvbox_dark()),
        "tokyonight" | "tokyo-night" | "tokyonight-night" => Ok(DiffTheme::tokyonight()),
        "dracula" => Ok(DiffTheme::dracula()),
        name => Err(HzError::Usage(format!("unknown colorscheme '{name}'"))),
    }
}

pub(crate) fn load_base16_scheme(path: &Path) -> HzResult<Base16Scheme> {
    let path = expand_user_path(path);
    let contents = fs::read_to_string(&path)?;
    parse_base16_scheme(&contents).ok_or_else(|| {
        HzError::Usage(format!(
            "failed to parse base16 colorscheme at {}; expected base00 through base0F",
            path.display()
        ))
    })
}

pub(crate) fn expand_user_path(path: &Path) -> PathBuf {
    let path_text = path.to_string_lossy();
    if path_text == "~" {
        return env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| path.to_path_buf());
    }
    if let Some(rest) = path_text.strip_prefix("~/") {
        if let Some(home) = env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    path.to_path_buf()
}

pub(crate) fn parse_base16_scheme(contents: &str) -> Option<Base16Scheme> {
    let mut colors: [Option<RgbColor>; 16] = [None; 16];
    for line in contents.lines() {
        let Some((index, color)) = parse_base16_line(line) else {
            continue;
        };
        colors[index] = Some(color);
    }

    if colors.iter().any(Option::is_none) {
        return None;
    }

    Some(Base16Scheme {
        base00: colors[0]?,
        base01: colors[1]?,
        base03: colors[3]?,
        base04: colors[4]?,
        base05: colors[5]?,
        base06: colors[6]?,
        base08: colors[8]?,
        base09: colors[9]?,
        base0a: colors[10]?,
        base0b: colors[11]?,
        base0c: colors[12]?,
        base0d: colors[13]?,
        base0e: colors[14]?,
    })
}

pub(crate) fn parse_base16_line(line: &str) -> Option<(usize, RgbColor)> {
    let line = line.trim();
    let (key, value) = line.split_once(':').or_else(|| line.split_once('='))?;
    let key = key.trim().trim_matches(['\'', '"']).to_ascii_lowercase();
    let index = base16_index(&key)?;
    let color = parse_hex_color(value)?;
    Some((index, color))
}

pub(crate) fn base16_index(key: &str) -> Option<usize> {
    let suffix = key.strip_prefix("base")?;
    if suffix.len() != 2 || !suffix.starts_with('0') {
        return None;
    }
    usize::from_str_radix(suffix, 16)
        .ok()
        .filter(|index| *index < 16)
}

pub(crate) fn parse_hex_color(value: &str) -> Option<RgbColor> {
    let value = value.trim();
    if let Some(hash) = value.find('#') {
        return parse_hex_digits(value.get(hash + 1..hash + 7)?);
    }

    let token = value
        .trim_matches(['\'', '"', ',', ' '])
        .split_whitespace()
        .next()?;
    parse_hex_digits(token.trim_matches(['\'', '"', ',']))
}

pub(crate) fn parse_hex_digits(digits: &str) -> Option<RgbColor> {
    if digits.len() < 6
        || !digits.as_bytes()[..6]
            .iter()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return None;
    }
    Some(RgbColor {
        red: u8::from_str_radix(&digits[0..2], 16).ok()?,
        green: u8::from_str_radix(&digits[2..4], 16).ok()?,
        blue: u8::from_str_radix(&digits[4..6], 16).ok()?,
    })
}
