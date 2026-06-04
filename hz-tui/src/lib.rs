use std::{
    collections::{HashMap, HashSet, VecDeque, hash_map::DefaultHasher},
    env,
    ffi::OsStr,
    fs,
    hash::{Hash, Hasher},
    io,
    panic::{self, AssertUnwindSafe},
    path::{Component, Path, PathBuf},
    process::Command,
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
        MouseButton, MouseEvent, MouseEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use hz_core::{HzError, HzResult};
use hz_diff::{
    Changeset, DiffLine, DiffLineKind, DiffOptions, DiffScope, DiffSource, DiffStats, FileStatus,
};
use hz_syntax::{
    ColorOverrides, DiffBackground, DiffGutterBackground, DiffSettings, DiffSignStyle,
    HighlightedLine, SyntaxClass, SyntaxHighlighter, SyntaxLanguageSet, SyntaxLimits,
    SyntaxSettings, SyntaxThemeConfig, SyntaxThemeSource,
};
use notify::{RecursiveMode, Watcher};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    prelude::{Color, Line, Modifier, Span, Style, Text},
    widgets::{Clear, Paragraph},
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
const HORIZONTAL_SCROLL_STEP: usize = 8;
const MIN_SPLIT_WIDTH: u16 = 120;
const GUTTER_WIDTH: usize = 7;
const UNIFIED_GUTTER_WIDTH: usize = 13;
const DIFF_INDICATOR: &str = "▌";
const EMPTY_DIFF_FILL: char = '╱';
const EMPTY_DIFF_FILL_SPACING: usize = 3;
const NOTICE_TTL: Duration = Duration::from_millis(1_500);
const MAX_SYNTAX_RESULTS_PER_FRAME: usize = 64;
const SYNTAX_THEME_ID: u64 = 0;
const MAX_INLINE_DIFF_LINE_BYTES: usize = 4 * 1024;
const MAX_INLINE_DIFF_TOKENS: usize = 256;
const MAX_INLINE_DIFF_CACHE_ENTRIES: usize = 512;
const MAX_BRANCH_MENU_ROWS: usize = 10;
const BRANCH_COMPARISON_SEPARATOR: &str = " → ";
const CURRENT_BRANCH_MARKER: &str = "●";
const BASE_BRANCH_MARKER: &str = "⌂";

fn line_gutter_fg(kind: DiffLineKind, theme: DiffTheme) -> Color {
    match kind {
        DiffLineKind::Addition => theme.addition_fg,
        DiffLineKind::Deletion => theme.deletion_fg,
        DiffLineKind::Context | DiffLineKind::Meta => theme.foreground,
    }
}

fn line_gutter_bg(kind: DiffLineKind, theme: DiffTheme) -> Color {
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
struct DiffTheme {
    foreground: Color,
    background: Color,
    header: Color,
    file: Color,
    hunk: Color,
    notice: Color,
    muted: Color,
    gutter_bg: Color,
    empty_diff: Color,
    addition_fg: Color,
    addition_gutter_bg: Color,
    addition_bg: Color,
    addition_inline_bg: Color,
    deletion_fg: Color,
    deletion_gutter_bg: Color,
    deletion_bg: Color,
    deletion_inline_bg: Color,
    transparent_background: bool,
    diff: DiffSettings,
    syntax: SyntaxPalette,
}

impl Default for DiffTheme {
    fn default() -> Self {
        Self::system()
    }
}

impl DiffTheme {
    fn system() -> Self {
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
            muted: Color::Indexed(8),
            gutter_bg: Color::Indexed(0),
            empty_diff: Color::Rgb(0x3d, 0x42, 0x49),
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

    fn terminal_dark() -> Self {
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
            muted: Color::Rgb(125, 135, 148),
            gutter_bg: Color::Rgb(12, 16, 20),
            empty_diff: Color::Rgb(38, 45, 54),
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

    fn terminal_light() -> Self {
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
            muted: Color::Rgb(106, 115, 125),
            gutter_bg: Color::Rgb(238, 242, 246),
            empty_diff: Color::Rgb(225, 228, 232),
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

    fn minimal() -> Self {
        Self {
            foreground: Color::Reset,
            background: Color::Reset,
            header: Color::White,
            file: Color::White,
            hunk: Color::Magenta,
            notice: Color::Green,
            muted: Color::DarkGray,
            gutter_bg: Color::Black,
            empty_diff: Color::DarkGray,
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

    fn ansi() -> Self {
        Self {
            foreground: Color::Reset,
            background: Color::Reset,
            header: Color::Indexed(15),
            file: Color::Indexed(15),
            hunk: Color::Indexed(13),
            notice: Color::Indexed(2),
            muted: Color::Indexed(8),
            gutter_bg: Color::Indexed(0),
            empty_diff: Color::Indexed(8),
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

    fn catppuccin_mocha() -> Self {
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
            muted: Color::Rgb(0x6c, 0x70, 0x86),
            gutter_bg: base.blend(RgbColor::new(0, 0, 0), 0.22).color(),
            empty_diff: Color::Rgb(0x31, 0x32, 0x44),
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

    fn gruvbox_dark() -> Self {
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
            muted: Color::Rgb(0x92, 0x83, 0x74),
            gutter_bg: base.blend(RgbColor::new(0, 0, 0), 0.22).color(),
            empty_diff: Color::Rgb(0x3c, 0x38, 0x36),
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

    fn tokyonight() -> Self {
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
            muted: Color::Rgb(0x56, 0x5f, 0x89),
            gutter_bg: base.blend(RgbColor::new(0, 0, 0), 0.22).color(),
            empty_diff: Color::Rgb(0x24, 0x28, 0x3b),
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

    fn dracula() -> Self {
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
            muted: Color::Rgb(0x62, 0x72, 0xa4),
            gutter_bg: base.blend(RgbColor::new(0, 0, 0), 0.22).color(),
            empty_diff: Color::Rgb(0x44, 0x47, 0x5a),
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

    fn base16(scheme: Base16Scheme) -> Self {
        Self {
            foreground: scheme.base05.color(),
            background: scheme.base00.color(),
            header: scheme.base06.color(),
            file: scheme.base05.color(),
            hunk: scheme.base0e.color(),
            notice: scheme.base0b.color(),
            muted: scheme.base03.color(),
            gutter_bg: scheme.base00.blend(RgbColor::new(0, 0, 0), 0.18).color(),
            empty_diff: scheme.base01.color(),
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

    fn with_transparent_background(mut self, transparent: bool) -> Self {
        self.transparent_background = transparent;
        self
    }

    fn with_diff_settings(mut self, diff: DiffSettings) -> Self {
        self.diff = diff;
        self
    }

    fn with_color_overrides(mut self, colors: &ColorOverrides) -> HzResult<Self> {
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
        if let Some(color) = config_color(&colors.muted, "muted")? {
            self.muted = color;
        }
        if let Some(color) = config_color(&colors.gutter_bg, "gutter_bg")? {
            self.gutter_bg = color;
        }
        if let Some(color) = config_color(&colors.empty_diff, "empty_diff")? {
            self.empty_diff = color;
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

fn config_color(value: &Option<String>, name: &str) -> HzResult<Option<Color>> {
    value
        .as_deref()
        .map(|value| parse_config_color(value, name))
        .transpose()
}

fn parse_config_color(value: &str, name: &str) -> HzResult<Color> {
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

fn parse_config_hex_color(value: &str) -> Option<RgbColor> {
    let value = value
        .trim()
        .trim_matches(['\'', '"'])
        .strip_prefix('#')
        .or_else(|| value.trim().strip_prefix("0x"))
        .unwrap_or_else(|| value.trim().trim_matches(['\'', '"']));
    parse_hex_digits(value)
}

fn parse_ansi_index(value: &str) -> Option<u8> {
    let index = value
        .strip_prefix("ansi-")
        .or_else(|| value.strip_prefix("ansi:"))
        .or_else(|| value.strip_prefix("indexed-"))
        .or_else(|| value.strip_prefix("indexed:"))
        .unwrap_or(value);
    index.parse::<u8>().ok()
}

fn parse_named_color(value: &str) -> Option<Color> {
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
struct SyntaxPalette {
    attribute: Option<Color>,
    comment: Option<Color>,
    constant: Option<Color>,
    constructor: Option<Color>,
    function: Option<Color>,
    keyword: Option<Color>,
    label: Option<Color>,
    module: Option<Color>,
    number: Option<Color>,
    operator: Option<Color>,
    property: Option<Color>,
    punctuation: Option<Color>,
    string: Option<Color>,
    tag: Option<Color>,
    r#type: Option<Color>,
    variable: Option<Color>,
}

impl SyntaxPalette {
    fn terminal_dark() -> Self {
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

    fn terminal_light() -> Self {
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

    fn minimal() -> Self {
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

    fn ansi() -> Self {
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

    fn catppuccin_mocha() -> Self {
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

    fn gruvbox_dark() -> Self {
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

    fn tokyonight() -> Self {
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

    fn dracula() -> Self {
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

    fn base16(scheme: Base16Scheme) -> Self {
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

    fn color(self, class: SyntaxClass) -> Option<Color> {
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
struct Base16Scheme {
    base00: RgbColor,
    base01: RgbColor,
    base03: RgbColor,
    base04: RgbColor,
    base05: RgbColor,
    base06: RgbColor,
    base08: RgbColor,
    base09: RgbColor,
    base0a: RgbColor,
    base0b: RgbColor,
    base0c: RgbColor,
    base0d: RgbColor,
    base0e: RgbColor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RgbColor {
    red: u8,
    green: u8,
    blue: u8,
}

impl RgbColor {
    const fn new(red: u8, green: u8, blue: u8) -> Self {
        Self { red, green, blue }
    }

    fn color(self) -> Color {
        Color::Rgb(self.red, self.green, self.blue)
    }

    fn blend(self, other: Self, amount: f32) -> Self {
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

fn diff_theme_from_config(config: &SyntaxThemeConfig) -> HzResult<DiffTheme> {
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

fn load_named_colorscheme(name: &str) -> HzResult<Option<DiffTheme>> {
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

fn colorscheme_paths(dir: &Path, name: &str) -> Vec<PathBuf> {
    let path = dir.join(name);
    if Path::new(name).extension().is_some() {
        return vec![path];
    }

    ["toml", "yaml", "yml"]
        .into_iter()
        .map(|extension| path.with_extension(extension))
        .collect()
}

fn builtin_diff_theme(name: Option<&str>) -> HzResult<DiffTheme> {
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

fn load_base16_scheme(path: &Path) -> HzResult<Base16Scheme> {
    let path = expand_user_path(path);
    let contents = fs::read_to_string(&path)?;
    parse_base16_scheme(&contents).ok_or_else(|| {
        HzError::Usage(format!(
            "failed to parse base16 colorscheme at {}; expected base00 through base0F",
            path.display()
        ))
    })
}

fn expand_user_path(path: &Path) -> PathBuf {
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

fn parse_base16_scheme(contents: &str) -> Option<Base16Scheme> {
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

fn parse_base16_line(line: &str) -> Option<(usize, RgbColor)> {
    let line = line.trim();
    let (key, value) = line.split_once(':').or_else(|| line.split_once('='))?;
    let key = key.trim().trim_matches(['\'', '"']).to_ascii_lowercase();
    let index = base16_index(&key)?;
    let color = parse_hex_color(value)?;
    Some((index, color))
}

fn base16_index(key: &str) -> Option<usize> {
    let suffix = key.strip_prefix("base")?;
    if suffix.len() != 2 || !suffix.starts_with('0') {
        return None;
    }
    usize::from_str_radix(suffix, 16)
        .ok()
        .filter(|index| *index < 16)
}

fn parse_hex_color(value: &str) -> Option<RgbColor> {
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

fn parse_hex_digits(digits: &str) -> Option<RgbColor> {
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
        let _ = render_row(app, app.scroll + offset, row, width);
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
    options: DiffOptions,
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

        let worker = spawn_live_diff_worker(options.clone(), control_rx, reload_tx);

        Ok(Self {
            options,
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
struct SyntaxPosition {
    generation: u64,
    file: usize,
    hunk: usize,
    side: DiffSide,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum SyntaxSourceKind {
    HunkSide { hunk: usize },
    FullFile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct SyntaxSourceId {
    generation: u64,
    file: usize,
    side: DiffSide,
    kind: SyntaxSourceKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct SyntaxKey {
    source: SyntaxSourceId,
    language_hash: u64,
    theme_id: u64,
}

impl SyntaxKey {
    fn generation(self) -> u64 {
        self.source.generation
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct InlineHunkKey {
    generation: u64,
    file: usize,
    hunk: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct InlineRange {
    byte_start: usize,
    byte_end: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct InlineLineEmphasis {
    ranges: Vec<InlineRange>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyntaxSkipReason {
    InvalidPosition,
    NoPath,
    NoLanguage,
    NoSource,
    TooLarge,
    QueueClosed,
    HighlightError,
}

#[derive(Debug, Clone)]
struct HighlightedSide {
    lines: Vec<HighlightedLine>,
}

impl HighlightedSide {
    fn memory_bytes(&self) -> usize {
        self.lines
            .iter()
            .flat_map(|line| line.segments.iter())
            .map(|segment| segment.text.len())
            .sum::<usize>()
            .saturating_add(self.lines.len() * std::mem::size_of::<HighlightedLine>())
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

        if let Some(entry) = self.entries.get_mut(&key) {
            *entry = value;
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
        // O(n) is intentional here: the syntax cache is capped at a small fixed
        // size, so avoiding another dependency/index keeps this simple.
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
        if job.key.generation() != state.generation {
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
        state
            .visible
            .retain(|job| job.key.generation() == generation);
        state
            .prefetch
            .retain(|job| job.key.generation() == generation);
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
                if job.key.generation() == state.generation {
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
    limits: SyntaxLimits,
    result_rx: Receiver<SyntaxResult>,
    queue: SyntaxWorkerQueue,
    cache: LruCache<SyntaxKey, HighlightedSide>,
    pending: HashSet<SyntaxKey>,
    position_keys: HashMap<SyntaxPosition, SyntaxKey>,
    line_maps: HashMap<SyntaxPosition, Vec<Option<usize>>>,
    skipped: HashMap<SyntaxPosition, SyntaxSkipReason>,
    unavailable_full_files: HashSet<SyntaxKey>,
    failed: HashSet<SyntaxKey>,
    stats: SyntaxBenchmarkReport,
    worker: Option<thread::JoinHandle<()>>,
}

impl SyntaxRuntime {
    fn start(settings: &SyntaxSettings) -> HzResult<Option<Self>> {
        let languages = SyntaxLanguageSet::load_with_mode(settings.mode)?;
        Ok(Self::start_with_language_set(languages, settings.limits))
    }

    fn start_with_language_set(languages: SyntaxLanguageSet, limits: SyntaxLimits) -> Option<Self> {
        if languages.is_empty() {
            return None;
        }

        let (result_tx, result_rx) = mpsc::channel();
        let queue = SyntaxWorkerQueue::new(limits.queue_entries, 0);
        let worker_queue = queue.clone();
        let worker = thread::spawn(move || run_syntax_worker(worker_queue, result_tx));

        Some(Self {
            languages,
            limits,
            result_rx,
            queue,
            cache: LruCache::new(limits.cache_entries),
            pending: HashSet::new(),
            position_keys: HashMap::new(),
            line_maps: HashMap::new(),
            skipped: HashMap::new(),
            unavailable_full_files: HashSet::new(),
            failed: HashSet::new(),
            stats: SyntaxBenchmarkReport::default(),
            worker: Some(worker),
        })
    }

    fn start_with_languages(languages: Vec<String>, limits: SyntaxLimits) -> Option<Self> {
        let languages = SyntaxLanguageSet::from_enabled_languages(&languages);
        Self::start_with_language_set(languages, limits)
    }

    fn clear(&mut self, generation: u64) {
        self.cache.clear();
        self.pending.clear();
        self.position_keys.clear();
        self.line_maps.clear();
        self.skipped.clear();
        self.unavailable_full_files.clear();
        self.failed.clear();
        self.queue.set_generation(generation);
    }

    fn queue_hunk(
        &mut self,
        options: &DiffOptions,
        changeset: &Changeset,
        position: SyntaxPosition,
        priority: SyntaxPriority,
    ) {
        let SyntaxPosition {
            generation,
            file,
            hunk,
            side,
        } = position;
        self.stats.queue_requests = self.stats.queue_requests.saturating_add(1);
        if let Some(key) = self.position_keys.get(&position).copied() {
            if self.cache.contains_key(&key) {
                return;
            }
            if self.pending.contains(&key) {
                if priority == SyntaxPriority::Visible {
                    self.queue.promote(key);
                }
                return;
            }
        }
        if self.skipped.contains_key(&position) {
            return;
        }

        let Some(file_diff) = changeset.files.get(file) else {
            self.skip(position, SyntaxSkipReason::InvalidPosition);
            return;
        };
        let Some(path) = syntax_path(file_diff, side) else {
            self.skip(position, SyntaxSkipReason::NoPath);
            return;
        };
        let Some(language) = self.languages.language_for_path(path) else {
            self.skip(position, SyntaxSkipReason::NoLanguage);
            return;
        };
        let Some(hunk_diff) = file_diff.hunks.get(hunk) else {
            self.skip(position, SyntaxSkipReason::InvalidPosition);
            return;
        };

        if let Some(source) = full_file_source(&changeset.repo, options, file_diff, side) {
            let key = SyntaxKey {
                source: SyntaxSourceId {
                    generation,
                    file,
                    side,
                    kind: SyntaxSourceKind::FullFile,
                },
                language_hash: hash_text(&language),
                theme_id: SYNTAX_THEME_ID,
            };

            if !self.unavailable_full_files.contains(&key) {
                if self.failed.contains(&key) {
                    self.skip(position, SyntaxSkipReason::HighlightError);
                    return;
                }

                let line_map = match build_full_file_line_map(&hunk_diff.lines, side) {
                    Ok(line_map) => line_map,
                    Err(reason) => {
                        self.skip(position, reason);
                        return;
                    }
                };

                self.position_keys.insert(position, key);
                self.line_maps.insert(position, line_map);
                if self.queue_job(
                    key,
                    language,
                    SyntaxJobSource::FullFile(source),
                    priority,
                    position,
                ) {
                    return;
                }
                return;
            }
        }

        let source = match build_hunk_source(&hunk_diff.lines, side, self.limits) {
            Ok(source) => source,
            Err(reason) => {
                self.skip(position, reason);
                return;
            }
        };

        let key = SyntaxKey {
            source: SyntaxSourceId {
                generation,
                file,
                side,
                kind: SyntaxSourceKind::HunkSide { hunk },
            },
            language_hash: hash_text(&language),
            theme_id: SYNTAX_THEME_ID,
        };
        self.position_keys.insert(position, key);
        self.line_maps.insert(position, source.line_map.clone());
        if self.failed.contains(&key) {
            self.skip(position, SyntaxSkipReason::HighlightError);
            return;
        }

        self.queue_job(
            key,
            language,
            SyntaxJobSource::Hunk(source),
            priority,
            position,
        );
    }

    fn queue_job(
        &mut self,
        key: SyntaxKey,
        language: String,
        source: SyntaxJobSource,
        priority: SyntaxPriority,
        position: SyntaxPosition,
    ) -> bool {
        if self.cache.contains_key(&key) {
            return true;
        }
        if self.pending.contains(&key) {
            if priority == SyntaxPriority::Visible {
                self.queue.promote(key);
            }
            return true;
        }

        let source_bytes = source.known_bytes();
        let source_lines = source.known_lines();

        let job = SyntaxJob {
            key,
            language,
            source,
            limits: self.limits,
        };

        match self.queue.try_push(job, priority) {
            Ok(push) => {
                if let Some(dropped) = push.dropped {
                    self.pending.remove(&dropped);
                    self.stats.jobs_evicted = self.stats.jobs_evicted.saturating_add(1);
                }
                self.stats.jobs_queued = self.stats.jobs_queued.saturating_add(1);
                self.stats.queue_depth_peak = self.stats.queue_depth_peak.max(push.depth);
                if let Some(source_bytes) = source_bytes {
                    self.stats.source_bytes_queued =
                        self.stats.source_bytes_queued.saturating_add(source_bytes);
                }
                if let Some(source_lines) = source_lines {
                    self.stats.source_lines_queued =
                        self.stats.source_lines_queued.saturating_add(source_lines);
                }
                self.pending.insert(key);
                true
            }
            Err(SyntaxQueueError::Full | SyntaxQueueError::Stale) => {
                self.stats.jobs_rejected = self.stats.jobs_rejected.saturating_add(1);
                false
            }
            Err(SyntaxQueueError::Closed) => {
                self.skip(position, SyntaxSkipReason::QueueClosed);
                false
            }
        }
    }

    fn skip(&mut self, position: SyntaxPosition, reason: SyntaxSkipReason) {
        if self.skipped.insert(position, reason).is_none() {
            self.stats.jobs_skipped = self.stats.jobs_skipped.saturating_add(1);
        }
    }

    fn drain(&mut self, generation: u64, max_results: usize) -> bool {
        let mut changed = false;
        for _ in 0..max_results {
            let Ok(result) = self.result_rx.try_recv() else {
                break;
            };
            self.pending.remove(&result.key);
            if result.key.generation() != generation {
                self.stats.stale_results = self.stats.stale_results.saturating_add(1);
                continue;
            }

            match result.side {
                Ok(success) => {
                    self.cache.insert(result.key, success.side);
                    self.stats.jobs_completed = self.stats.jobs_completed.saturating_add(1);
                    if let Some(source_bytes) = success.source_bytes {
                        self.stats.source_bytes_queued =
                            self.stats.source_bytes_queued.saturating_add(source_bytes);
                    }
                    if let Some(source_lines) = success.source_lines {
                        self.stats.source_lines_queued =
                            self.stats.source_lines_queued.saturating_add(source_lines);
                    }
                    self.stats.cache_entries_peak =
                        self.stats.cache_entries_peak.max(self.cache.len());
                    self.stats.estimated_memory_peak_bytes = self
                        .stats
                        .estimated_memory_peak_bytes
                        .max(self.estimated_memory_bytes() as u64);
                    changed = true;
                }
                Err(SyntaxJobFailure::Unavailable) => {
                    self.handle_unavailable_source(result.key);
                    self.stats.jobs_skipped = self.stats.jobs_skipped.saturating_add(1);
                    changed = true;
                }
                Err(SyntaxJobFailure::HighlightError) => {
                    self.failed.insert(result.key);
                    let positions = self.positions_for_key(result.key);
                    for position in positions {
                        self.skipped
                            .insert(position, SyntaxSkipReason::HighlightError);
                    }
                    self.stats.jobs_failed = self.stats.jobs_failed.saturating_add(1);
                }
            }
        }
        changed
    }

    fn handle_unavailable_source(&mut self, key: SyntaxKey) {
        if matches!(key.source.kind, SyntaxSourceKind::FullFile) {
            self.unavailable_full_files.insert(key);
        } else {
            let positions = self.positions_for_key(key);
            for position in positions {
                self.skipped.insert(position, SyntaxSkipReason::NoSource);
            }
        }

        let positions = self.positions_for_key(key);
        for position in positions {
            self.position_keys.remove(&position);
            self.line_maps.remove(&position);
        }
    }

    fn positions_for_key(&self, key: SyntaxKey) -> Vec<SyntaxPosition> {
        self.position_keys
            .iter()
            .filter_map(|(position, position_key)| (*position_key == key).then_some(*position))
            .collect()
    }

    fn line(&mut self, position: SyntaxPosition, line: usize) -> Option<HighlightedLine> {
        let highlighted = self.position_keys.get(&position).copied().and_then(|key| {
            let source_line = self
                .line_maps
                .get(&position)
                .and_then(|line_map| line_map.get(line))
                .and_then(|source_line| *source_line)?;
            self.cache
                .get(&key)
                .and_then(|side| side.lines.get(source_line))
                .cloned()
        });
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
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[derive(Debug)]
struct SyntaxJob {
    key: SyntaxKey,
    language: String,
    source: SyntaxJobSource,
    limits: SyntaxLimits,
}

#[derive(Debug)]
struct SyntaxResult {
    key: SyntaxKey,
    side: Result<SyntaxSuccess, SyntaxJobFailure>,
}

#[derive(Debug)]
struct SyntaxSuccess {
    side: HighlightedSide,
    source_bytes: Option<u64>,
    source_lines: Option<u64>,
}

#[derive(Debug)]
enum SyntaxJobFailure {
    Unavailable,
    HighlightError,
}

#[derive(Debug)]
struct HunkSource {
    text: String,
    line_map: Vec<Option<usize>>,
    source_lines: usize,
}

#[derive(Debug)]
enum SyntaxJobSource {
    Hunk(HunkSource),
    FullFile(FullFileSource),
}

impl SyntaxJobSource {
    fn known_bytes(&self) -> Option<u64> {
        match self {
            Self::Hunk(source) => Some(source.text.len() as u64),
            Self::FullFile(_) => None,
        }
    }

    fn known_lines(&self) -> Option<u64> {
        match self {
            Self::Hunk(source) => Some(source.source_lines as u64),
            Self::FullFile(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FullFileSource {
    repo: PathBuf,
    kind: FullFileSourceKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum FullFileSourceKind {
    Worktree {
        path: String,
    },
    GitRevision {
        rev: String,
        path: String,
    },
    GitIndex {
        path: String,
    },
    GitMergeBase {
        base: String,
        head: String,
        path: String,
    },
}

fn run_syntax_worker(queue: SyntaxWorkerQueue, result_tx: Sender<SyntaxResult>) {
    let mut highlighter = SyntaxHighlighter::new();
    while let Some(job) = queue.pop() {
        let side = panic::catch_unwind(AssertUnwindSafe(|| {
            let (source, source_bytes, source_lines) = load_job_source(job.source, job.limits)?;
            highlighter
                .highlight(&job.language, &source)
                .map(|highlighted| SyntaxSuccess {
                    side: HighlightedSide {
                        lines: highlighted.lines,
                    },
                    source_bytes,
                    source_lines,
                })
                .map_err(|_| SyntaxJobFailure::HighlightError)
        }))
        .unwrap_or(Err(SyntaxJobFailure::HighlightError));
        if result_tx.send(SyntaxResult { key: job.key, side }).is_err() {
            break;
        }
    }
}

fn load_job_source(
    source: SyntaxJobSource,
    limits: SyntaxLimits,
) -> Result<(String, Option<u64>, Option<u64>), SyntaxJobFailure> {
    match source {
        SyntaxJobSource::Hunk(source) => Ok((source.text, None, None)),
        SyntaxJobSource::FullFile(source) => {
            let text = load_full_file_source(&source).map_err(|_| SyntaxJobFailure::Unavailable)?;
            validate_highlight_source(&text, limits).map_err(|_| SyntaxJobFailure::Unavailable)?;
            let source_bytes = text.len() as u64;
            let source_lines = source_line_count(&text) as u64;
            Ok((text, Some(source_bytes), Some(source_lines)))
        }
    }
}

fn syntax_path(file: &hz_diff::DiffFile, side: DiffSide) -> Option<&str> {
    match side {
        DiffSide::Old => file.old_path.as_deref().or(file.new_path.as_deref()),
        DiffSide::New => file.new_path.as_deref().or(file.old_path.as_deref()),
    }
}

fn build_hunk_source(
    lines: &[DiffLine],
    side: DiffSide,
    limits: SyntaxLimits,
) -> Result<HunkSource, SyntaxSkipReason> {
    let mut text = String::new();
    let mut line_map = vec![None; lines.len()];
    let mut source_lines = 0;

    for (index, line) in lines.iter().enumerate() {
        if !line_belongs_to_side(line.kind, side) {
            continue;
        }
        if line.text.len() > limits.max_line_bytes {
            return Err(SyntaxSkipReason::TooLarge);
        }
        if source_lines > 0 {
            text.push('\n');
        }
        text.push_str(&line.text);
        if text.len() > limits.max_source_bytes {
            return Err(SyntaxSkipReason::TooLarge);
        }
        line_map[index] = Some(source_lines);
        source_lines += 1;
    }

    if source_lines == 0 {
        return Err(SyntaxSkipReason::NoSource);
    }

    Ok(HunkSource {
        text,
        line_map,
        source_lines,
    })
}

fn build_full_file_line_map(
    lines: &[DiffLine],
    side: DiffSide,
) -> Result<Vec<Option<usize>>, SyntaxSkipReason> {
    let mut line_map = vec![None; lines.len()];
    let mut source_lines = 0;

    for (index, line) in lines.iter().enumerate() {
        if !line_belongs_to_side(line.kind, side) {
            continue;
        }

        let Some(line_number) = diff_line_number(line, side) else {
            continue;
        };
        let Some(source_line) = line_number.checked_sub(1) else {
            continue;
        };
        line_map[index] = Some(source_line);
        source_lines += 1;
    }

    if source_lines == 0 {
        return Err(SyntaxSkipReason::NoSource);
    }

    Ok(line_map)
}

fn diff_line_number(line: &DiffLine, side: DiffSide) -> Option<usize> {
    match side {
        DiffSide::Old => line.old_line,
        DiffSide::New => line.new_line,
    }
}

fn full_file_source(
    repo: &Path,
    options: &DiffOptions,
    file: &hz_diff::DiffFile,
    side: DiffSide,
) -> Option<FullFileSource> {
    if matches!(options.source, DiffSource::Patch(_)) {
        return None;
    }
    if !repo.is_dir() {
        return None;
    }

    let path = file_path_for_side(file, side)?.to_owned();
    let kind = match (&options.source, options.scope, side) {
        (DiffSource::Worktree, DiffScope::All, DiffSide::Old) => FullFileSourceKind::GitRevision {
            rev: "HEAD".to_owned(),
            path,
        },
        (DiffSource::Worktree, DiffScope::All, DiffSide::New) => {
            FullFileSourceKind::Worktree { path }
        }
        (DiffSource::Worktree, DiffScope::Staged, DiffSide::Old) => {
            FullFileSourceKind::GitRevision {
                rev: "HEAD".to_owned(),
                path,
            }
        }
        (DiffSource::Worktree, DiffScope::Staged, DiffSide::New) => {
            FullFileSourceKind::GitIndex { path }
        }
        (DiffSource::Worktree, DiffScope::Unstaged, DiffSide::Old) => {
            FullFileSourceKind::GitIndex { path }
        }
        (DiffSource::Worktree, DiffScope::Unstaged, DiffSide::New) => {
            FullFileSourceKind::Worktree { path }
        }
        (DiffSource::Base(base), DiffScope::All, DiffSide::Old) => {
            FullFileSourceKind::GitMergeBase {
                base: base.clone(),
                head: "HEAD".to_owned(),
                path,
            }
        }
        (DiffSource::Base(_), DiffScope::All, DiffSide::New) => FullFileSourceKind::GitRevision {
            rev: "HEAD".to_owned(),
            path,
        },
        (DiffSource::Branch { base, head }, DiffScope::All, DiffSide::Old) => {
            FullFileSourceKind::GitMergeBase {
                base: base.clone(),
                head: head.clone(),
                path,
            }
        }
        (DiffSource::Branch { head, .. }, DiffScope::All, DiffSide::New) => {
            FullFileSourceKind::GitRevision {
                rev: head.clone(),
                path,
            }
        }
        (DiffSource::Range { left, .. }, DiffScope::All, DiffSide::Old) => {
            FullFileSourceKind::GitRevision {
                rev: left.clone(),
                path,
            }
        }
        (DiffSource::Range { right, .. }, DiffScope::All, DiffSide::New) => {
            FullFileSourceKind::GitRevision {
                rev: right.clone(),
                path,
            }
        }
        _ => return None,
    };

    Some(FullFileSource {
        repo: repo.to_owned(),
        kind,
    })
}

fn file_path_for_side(file: &hz_diff::DiffFile, side: DiffSide) -> Option<&str> {
    match side {
        DiffSide::Old => file.old_path.as_deref(),
        DiffSide::New => file.new_path.as_deref(),
    }
}

fn load_full_file_source(source: &FullFileSource) -> Result<String, SyntaxSkipReason> {
    let bytes = match &source.kind {
        FullFileSourceKind::Worktree { path } => read_worktree_file(&source.repo, path)?,
        FullFileSourceKind::GitRevision { rev, path } => {
            git_blob(&source.repo, &format!("{rev}:{path}"))?
        }
        FullFileSourceKind::GitIndex { path } => git_blob(&source.repo, &format!(":{path}"))?,
        FullFileSourceKind::GitMergeBase { base, head, path } => {
            let rev = git_merge_base(&source.repo, base, head)?;
            git_blob(&source.repo, &format!("{rev}:{path}"))?
        }
    };

    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn read_worktree_file(repo: &Path, path: &str) -> Result<Vec<u8>, SyntaxSkipReason> {
    let path = safe_repo_join(repo, path).ok_or(SyntaxSkipReason::NoPath)?;
    let metadata = fs::symlink_metadata(&path).map_err(|_| SyntaxSkipReason::NoSource)?;
    if !metadata.file_type().is_file() {
        return Err(SyntaxSkipReason::NoSource);
    }
    fs::read(path).map_err(|_| SyntaxSkipReason::NoSource)
}

fn safe_repo_join(repo: &Path, path: &str) -> Option<PathBuf> {
    let path = Path::new(path);
    if path.is_absolute() {
        return None;
    }

    let mut joined = repo.to_owned();
    for component in path.components() {
        match component {
            Component::Normal(part) => joined.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(joined)
}

fn git_blob(repo: &Path, object: &str) -> Result<Vec<u8>, SyntaxSkipReason> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["show", "--no-ext-diff", "--no-color", object])
        .output()
        .map_err(|_| SyntaxSkipReason::NoSource)?;
    if !output.status.success() {
        return Err(SyntaxSkipReason::NoSource);
    }
    Ok(output.stdout)
}

fn git_merge_base(repo: &Path, base: &str, head: &str) -> Result<String, SyntaxSkipReason> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["merge-base", base, head])
        .output()
        .map_err(|_| SyntaxSkipReason::NoSource)?;
    if !output.status.success() {
        return Err(SyntaxSkipReason::NoSource);
    }

    let rev = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if rev.is_empty() {
        return Err(SyntaxSkipReason::NoSource);
    }
    Ok(rev)
}

fn validate_highlight_source(source: &str, limits: SyntaxLimits) -> Result<(), SyntaxSkipReason> {
    if source.len() > limits.max_source_bytes {
        return Err(SyntaxSkipReason::TooLarge);
    }
    if source
        .lines()
        .any(|line| line.len() > limits.max_line_bytes)
    {
        return Err(SyntaxSkipReason::TooLarge);
    }
    Ok(())
}

fn source_line_count(source: &str) -> usize {
    source.lines().count().max(1)
}

fn hash_text(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
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

fn compute_hunk_inline_emphasis(lines: &[DiffLine]) -> Vec<InlineLineEmphasis> {
    let mut emphasis = vec![InlineLineEmphasis::default(); lines.len()];
    let mut index = 0usize;

    while index < lines.len() {
        match lines[index].kind {
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
                compute_changed_block_inline_emphasis(lines, &deletions, &additions, &mut emphasis);
            }
            DiffLineKind::Context | DiffLineKind::Meta => index += 1,
        }
    }

    emphasis
}

fn compute_changed_block_inline_emphasis(
    lines: &[DiffLine],
    deletions: &[usize],
    additions: &[usize],
    emphasis: &mut [InlineLineEmphasis],
) {
    let paired_rows = deletions.len().max(additions.len());
    for pair_index in 0..paired_rows {
        match (deletions.get(pair_index), additions.get(pair_index)) {
            (Some(deletion), Some(addition)) => {
                let (old_ranges, new_ranges) =
                    changed_token_ranges(&lines[*deletion].text, &lines[*addition].text);
                emphasis[*deletion].ranges = old_ranges;
                emphasis[*addition].ranges = new_ranges;
            }
            (Some(deletion), None) => {
                emphasis[*deletion].ranges = Vec::new();
            }
            (None, Some(addition)) => {
                emphasis[*addition].ranges = Vec::new();
            }
            (None, None) => {}
        }
    }
}

fn changed_token_ranges(old: &str, new: &str) -> (Vec<InlineRange>, Vec<InlineRange>) {
    if old == new {
        return (Vec::new(), Vec::new());
    }
    if old.len() > MAX_INLINE_DIFF_LINE_BYTES || new.len() > MAX_INLINE_DIFF_LINE_BYTES {
        return (Vec::new(), Vec::new());
    }

    let old_tokens = inline_tokens(old);
    let new_tokens = inline_tokens(new);
    if old_tokens.len() > MAX_INLINE_DIFF_TOKENS || new_tokens.len() > MAX_INLINE_DIFF_TOKENS {
        return (Vec::new(), Vec::new());
    }

    let mut old_changed = vec![true; old_tokens.len()];
    let mut new_changed = vec![true; new_tokens.len()];
    mark_unchanged_lcs_tokens(
        old,
        &old_tokens,
        new,
        &new_tokens,
        &mut old_changed,
        &mut new_changed,
    );

    (
        inline_ranges_from_tokens(&old_tokens, &old_changed),
        inline_ranges_from_tokens(&new_tokens, &new_changed),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct InlineToken {
    byte_start: usize,
    byte_end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InlineCharClass {
    Word,
    Whitespace,
    Other,
}

fn inline_tokens(text: &str) -> Vec<InlineToken> {
    let mut tokens = Vec::new();
    let mut chars = text.char_indices().peekable();

    while let Some((start, ch)) = chars.next() {
        let class = inline_char_class(ch);
        let mut end = start + ch.len_utf8();

        if class != InlineCharClass::Other {
            while let Some((_, next)) = chars.peek().copied() {
                if inline_char_class(next) != class {
                    break;
                }
                let Some((next_start, next)) = chars.next() else {
                    break;
                };
                end = next_start + next.len_utf8();
            }
        }

        tokens.push(InlineToken {
            byte_start: start,
            byte_end: end,
        });
    }

    tokens
}

fn inline_char_class(ch: char) -> InlineCharClass {
    if ch.is_whitespace() {
        InlineCharClass::Whitespace
    } else if ch == '_' || ch.is_alphanumeric() {
        InlineCharClass::Word
    } else {
        InlineCharClass::Other
    }
}

fn mark_unchanged_lcs_tokens(
    old: &str,
    old_tokens: &[InlineToken],
    new: &str,
    new_tokens: &[InlineToken],
    old_changed: &mut [bool],
    new_changed: &mut [bool],
) {
    let cols = new_tokens.len() + 1;
    let mut lengths = vec![0u16; (old_tokens.len() + 1) * cols];

    for old_index in 0..old_tokens.len() {
        for new_index in 0..new_tokens.len() {
            let cell = (old_index + 1) * cols + new_index + 1;
            lengths[cell] = if inline_token_text(old, old_tokens[old_index])
                == inline_token_text(new, new_tokens[new_index])
            {
                lengths[old_index * cols + new_index].saturating_add(1)
            } else {
                lengths[old_index * cols + new_index + 1]
                    .max(lengths[(old_index + 1) * cols + new_index])
            };
        }
    }

    let mut old_index = old_tokens.len();
    let mut new_index = new_tokens.len();
    while old_index > 0 && new_index > 0 {
        if inline_token_text(old, old_tokens[old_index - 1])
            == inline_token_text(new, new_tokens[new_index - 1])
        {
            old_changed[old_index - 1] = false;
            new_changed[new_index - 1] = false;
            old_index -= 1;
            new_index -= 1;
        } else if lengths[(old_index - 1) * cols + new_index]
            >= lengths[old_index * cols + new_index - 1]
        {
            old_index -= 1;
        } else {
            new_index -= 1;
        }
    }
}

fn inline_token_text(text: &str, token: InlineToken) -> &str {
    &text[token.byte_start..token.byte_end]
}

fn inline_ranges_from_tokens(tokens: &[InlineToken], changed: &[bool]) -> Vec<InlineRange> {
    let mut ranges: Vec<InlineRange> = Vec::new();
    for (token, is_changed) in tokens.iter().zip(changed) {
        if !*is_changed {
            continue;
        }
        if let Some(last) = ranges.last_mut()
            && last.byte_end == token.byte_start
        {
            last.byte_end = token.byte_end;
            continue;
        }
        ranges.push(InlineRange {
            byte_start: token.byte_start,
            byte_end: token.byte_end,
        });
    }
    ranges
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
    fn toggled(self) -> Self {
        match self {
            Self::Split => Self::Unified,
            Self::Unified => Self::Split,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiffChoice {
    Branch,
    All,
    Unstaged,
    Staged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BranchMenu {
    Head,
    Base,
}

impl DiffChoice {
    fn label(self) -> &'static str {
        match self {
            Self::Branch => "Branch",
            Self::All => "All changes",
            Self::Unstaged => "Unstaged",
            Self::Staged => "Staged",
        }
    }

    fn notice(self) -> &'static str {
        match self {
            Self::Branch => "branch diff",
            Self::All => "all changes",
            Self::Unstaged => "unstaged changes",
            Self::Staged => "staged changes",
        }
    }
}

const WORKTREE_DIFF_CHOICES: [DiffChoice; 3] =
    [DiffChoice::All, DiffChoice::Unstaged, DiffChoice::Staged];

fn default_branch_base(options: &DiffOptions, repo: &Path) -> Option<String> {
    branch_base_from_options(options)
        .or_else(env_branch_base)
        .or_else(|| git_remote_head_branch(repo))
        .or_else(|| git_local_branch_candidate(repo))
}

fn comparison_branches(repo: &Path, selected_refs: &[Option<&str>]) -> Vec<String> {
    let mut branches = git_branches(repo);
    for selected in selected_refs
        .iter()
        .filter_map(|selected| selected.filter(|reference| !reference.is_empty()))
    {
        if !branches.iter().any(|branch| branch == selected) {
            branches.push(selected.to_owned());
        }
    }
    branches
}

fn branch_match_score(query: &str, branch: &str) -> Option<(usize, usize)> {
    let branch_lower = branch.to_ascii_lowercase();
    if branch_lower == query {
        return Some((0, 0));
    }
    if branch_lower.starts_with(query) {
        return Some((1, branch.len().saturating_sub(query.len())));
    }
    if let Some(index) = branch_lower.find(query) {
        return Some((2, index));
    }
    fuzzy_subsequence_score(query, &branch_lower).map(|score| (3, score))
}

fn fuzzy_subsequence_score(query: &str, branch: &str) -> Option<usize> {
    let mut last_match: Option<usize> = None;
    let mut score = 0usize;
    let mut search_start = 0usize;

    for character in query.chars() {
        let remaining = branch.get(search_start..)?;
        let offset = remaining.find(character)?;
        let index = search_start + offset;
        if let Some(previous) = last_match {
            score = score.saturating_add(index.saturating_sub(previous + 1));
        } else {
            score = score.saturating_add(index);
        }
        last_match = Some(index);
        search_start = index + character.len_utf8();
    }

    Some(score)
}

fn git_branches(repo: &Path) -> Vec<String> {
    if repo.as_os_str().is_empty() || !repo.exists() {
        return Vec::new();
    }

    let output = match Command::new("git")
        .arg("-C")
        .arg(repo)
        .args([
            "for-each-ref",
            "--sort=-committerdate",
            "--format=%(committerdate:unix)%09%(refname:short)",
            "refs/heads",
            "refs/remotes",
        ])
        .output()
    {
        Ok(output) if output.status.success() => output,
        _ => return Vec::new(),
    };

    let mut branches = Vec::new();
    let mut seen = HashSet::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let branch = line
            .split_once('\t')
            .map(|(_, branch)| branch)
            .unwrap_or(line)
            .trim();
        if branch.is_empty() || branch.ends_with("/HEAD") || !seen.insert(branch.to_owned()) {
            continue;
        }
        branches.push(branch.to_owned());
    }
    branches
}

fn branch_base_from_options(options: &DiffOptions) -> Option<String> {
    match &options.source {
        DiffSource::Base(base) if !base.is_empty() => Some(base.clone()),
        DiffSource::Branch { base, .. } if !base.is_empty() => Some(base.clone()),
        _ => None,
    }
}

fn branch_head_from_options(options: &DiffOptions, current_head: Option<&str>) -> Option<String> {
    match &options.source {
        DiffSource::Base(_) => current_head.map(str::to_owned),
        DiffSource::Branch { head, .. } if !head.is_empty() => Some(head.clone()),
        _ => None,
    }
}

fn current_head_label(repo: &Path) -> Option<String> {
    hz_git::current_branch(repo)
        .ok()
        .flatten()
        .or_else(|| git_output(repo, ["rev-parse", "--short", "HEAD"]))
}

fn env_branch_base() -> Option<String> {
    env::var("HZ_BASE_BRANCH")
        .ok()
        .map(|base| base.trim().to_owned())
        .filter(|base| !base.is_empty())
}

fn git_remote_head_branch(repo: &Path) -> Option<String> {
    git_output(
        repo,
        [
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    )
}

fn git_local_branch_candidate(repo: &Path) -> Option<String> {
    if !repo.exists() {
        return None;
    }

    ["main", "master"].into_iter().find_map(|branch| {
        hz_git::branch_exists(repo, branch)
            .ok()
            .filter(|exists| *exists)
            .map(|_| branch.to_owned())
    })
}

fn git_output<const N: usize>(repo: &Path, args: [&str; N]) -> Option<String> {
    if repo.as_os_str().is_empty() || !repo.exists() {
        return None;
    }

    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!value.is_empty()).then_some(value)
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

fn sync_live_diff(live_diff: &mut Option<LiveDiff>, app: &mut DiffApp, live_updates: bool) {
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
    horizontal_scroll: usize,
    viewport_rows: usize,
    viewport_width: usize,
    max_line_width: usize,
    selected_file: usize,
    diff_menu_open: bool,
    branch_menu_open: Option<BranchMenu>,
    branch_menu_input: String,
    branch_menu_scroll: usize,
    branch_menu_selected: usize,
    branch_base: Option<String>,
    branch_head: Option<String>,
    current_head: Option<String>,
    comparison_branches: Vec<String>,
    live_diff_failed_options: Option<DiffOptions>,
    mouse_scroll: MouseScroll,
    notice: Option<Notice>,
    theme: DiffTheme,
    syntax_limits: SyntaxLimits,
    syntax: Option<SyntaxRuntime>,
    inline_cache: LruCache<InlineHunkKey, Vec<InlineLineEmphasis>>,
    generation: u64,
    dirty: bool,
}

fn load_syntax_settings_for_diff(load_user_settings: bool) -> (SyntaxSettings, Option<Notice>) {
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
            changeset,
            stats,
            model,
            layout,
            scroll: 0,
            horizontal_scroll: 0,
            viewport_rows: 1,
            viewport_width: 1,
            max_line_width,
            selected_file: 0,
            diff_menu_open: false,
            branch_menu_open: None,
            branch_menu_input: String::new(),
            branch_menu_scroll: 0,
            branch_menu_selected: 0,
            branch_base,
            branch_head,
            current_head,
            comparison_branches,
            live_diff_failed_options: None,
            mouse_scroll: MouseScroll::default(),
            notice,
            theme,
            syntax_limits,
            syntax,
            inline_cache: LruCache::new(MAX_INLINE_DIFF_CACHE_ENTRIES),
            generation: 0,
            dirty: true,
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> HzResult<bool> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Ok(true);
        }

        self.mouse_scroll.reset();

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
            KeyCode::Esc | KeyCode::Char('q') => return Ok(true),
            KeyCode::Down | KeyCode::Char('j') => self.scroll_by(1),
            KeyCode::Up | KeyCode::Char('k') => self.scroll_by(-1),
            KeyCode::Left | KeyCode::Char('h') => {
                self.scroll_horizontally_by(-(HORIZONTAL_SCROLL_STEP as isize));
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.scroll_horizontally_by(HORIZONTAL_SCROLL_STEP as isize);
            }
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

    fn handle_mouse(&mut self, mouse: MouseEvent) -> HzResult<()> {
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
                self.handle_click(mouse.column, mouse.row);
            }
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
            MouseEventKind::ScrollLeft => {
                self.scroll_horizontally_by(-(HORIZONTAL_SCROLL_STEP as isize));
            }
            MouseEventKind::ScrollRight => {
                self.scroll_horizontally_by(HORIZONTAL_SCROLL_STEP as isize);
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_click(&mut self, column: u16, row: u16) {
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
        }
    }

    fn toggle_diff_menu(&mut self) {
        if self.diff_menu_choices().is_empty() {
            return;
        }
        self.diff_menu_open = !self.diff_menu_open;
        self.branch_menu_open = None;
        self.dirty = true;
    }

    fn close_branch_menu(&mut self) {
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

    fn toggle_branch_menu(&mut self, menu: BranchMenu) {
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

    fn branch_selector_at(&self, column: u16) -> Option<BranchMenu> {
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

    fn branch_choice_at(&self, menu: BranchMenu, column: u16, row: u16) -> Option<String> {
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

    fn filtered_branch(&self, row_index: usize) -> Option<&str> {
        self.filtered_branches()
            .get(self.branch_menu_scroll.saturating_add(row_index))
            .copied()
    }

    fn move_branch_selection(&mut self, delta: isize) {
        let next = if delta < 0 {
            self.branch_menu_selected
                .saturating_sub(delta.unsigned_abs())
        } else {
            self.branch_menu_selected.saturating_add(delta as usize)
        };
        self.set_branch_selection(next);
    }

    fn set_branch_selection(&mut self, selected: usize) {
        let selected = selected.min(self.max_branch_menu_selection());
        if self.branch_menu_selected != selected {
            self.branch_menu_selected = selected;
            self.ensure_branch_selection_visible();
            self.dirty = true;
        }
    }

    fn cycle_branch_completion(&mut self, delta: isize) {
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

    fn ensure_branch_selection_visible(&mut self) {
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

    fn max_branch_menu_selection(&self) -> usize {
        self.filtered_branches().len().saturating_sub(1)
    }

    fn max_branch_menu_scroll(&self) -> usize {
        self.filtered_branches()
            .len()
            .saturating_sub(MAX_BRANCH_MENU_ROWS)
    }

    fn visible_branch_menu_rows(&self) -> usize {
        self.filtered_branches().len().min(MAX_BRANCH_MENU_ROWS)
    }

    fn branch_menu_height(&self) -> usize {
        self.visible_branch_menu_rows()
            .max(usize::from(self.filtered_branches().is_empty()))
    }

    fn filtered_branches(&self) -> Vec<&str> {
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

    fn branch_pin_rank(&self, menu: BranchMenu, branch: &str) -> usize {
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

    fn push_branch_input(&mut self, character: char) {
        self.branch_menu_input.push(character);
        self.branch_menu_scroll = 0;
        self.branch_menu_selected = 0;
        self.dirty = true;
    }

    fn pop_branch_input(&mut self) {
        if self.branch_menu_input.pop().is_some() {
            self.branch_menu_scroll = 0;
            self.branch_menu_selected = 0;
            self.dirty = true;
        }
    }

    fn clear_branch_input(&mut self) {
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

    fn select_highlighted_branch_match(&mut self) {
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

    fn is_branch_diff(&self) -> bool {
        matches!(
            &self.options.source,
            DiffSource::Base(_) | DiffSource::Branch { .. }
        )
    }

    fn branch_ref(&self, menu: BranchMenu) -> Option<&str> {
        match menu {
            BranchMenu::Head => self.branch_head.as_deref(),
            BranchMenu::Base => self.branch_base.as_deref(),
        }
    }

    fn branch_selector_text(&self, menu: BranchMenu) -> Option<String> {
        let branch = self.branch_ref(menu)?;
        let label = self.branch_label(menu, branch);
        if self.branch_menu_open == Some(menu) {
            let width = label.width().max(self.branch_menu_input.width());
            return Some(format!("{} ▾", fit_padded(&self.branch_menu_input, width)));
        }

        Some(format!("{label} ▾"))
    }

    fn branch_label(&self, menu: BranchMenu, branch: &str) -> String {
        match self.branch_marker(menu, branch) {
            Some(marker) => format!("{marker} {branch}"),
            None => branch.to_owned(),
        }
    }

    fn branch_marker(&self, menu: BranchMenu, branch: &str) -> Option<&'static str> {
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

    fn branch_selector_width(&self, menu: BranchMenu) -> Option<u16> {
        self.branch_selector_text(menu)
            .map(|text| text.width() as u16)
    }

    fn branch_menu_width(&self) -> u16 {
        let branch_width = branch_menu_width(&self.comparison_branches) as usize;
        let input_width = self.branch_menu_input.width().saturating_add(6).max(20);
        branch_width.max(input_width) as u16
    }

    fn branch_selector_start(&self, menu: BranchMenu) -> Option<u16> {
        if !self.is_branch_diff() {
            return None;
        }

        let head_width = self.branch_selector_width(BranchMenu::Head)?;
        match menu {
            BranchMenu::Head => Some(diff_selector_width(&self.options)),
            BranchMenu::Base => Some(
                diff_selector_width(&self.options)
                    .saturating_add(head_width)
                    .saturating_add(BRANCH_COMPARISON_SEPARATOR.width() as u16),
            ),
        }
    }

    fn diff_choice_at(&self, column: u16, row: u16) -> Option<DiffChoice> {
        let choices = self.diff_menu_choices();
        let width = diff_menu_width(&choices);
        if column >= width || row == 0 {
            return None;
        }

        choices.get(usize::from(row - 1)).copied()
    }

    fn diff_menu_choices(&self) -> Vec<DiffChoice> {
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

    fn select_branch(&mut self, menu: BranchMenu, branch: String) {
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

    fn branch_source(&self, base: String, head: String) -> DiffSource {
        if self.current_head.as_deref() == Some(head.as_str()) {
            DiffSource::Base(base)
        } else {
            DiffSource::Branch { base, head }
        }
    }

    fn select_diff_choice(&mut self, choice: DiffChoice) {
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

    fn options_for_choice(&self, choice: DiffChoice) -> Option<DiffOptions> {
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

    fn scroll_by(&mut self, delta: isize) {
        let next = if delta < 0 {
            self.scroll.saturating_sub(delta.unsigned_abs())
        } else {
            self.scroll.saturating_add(delta as usize)
        };
        self.set_scroll(next);
    }

    fn scroll_horizontally_by(&mut self, delta: isize) {
        let next = if delta < 0 {
            self.horizontal_scroll.saturating_sub(delta.unsigned_abs())
        } else {
            self.horizontal_scroll.saturating_add(delta as usize)
        };
        self.set_horizontal_scroll(next);
    }

    fn set_horizontal_scroll(&mut self, scroll: usize) {
        let previous_scroll = self.horizontal_scroll;
        self.horizontal_scroll = scroll.min(self.max_horizontal_scroll());
        if self.horizontal_scroll != previous_scroll {
            self.dirty = true;
        }
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

    fn max_horizontal_scroll(&self) -> usize {
        self.max_line_width
            .saturating_sub(diff_content_width(self.layout, self.viewport_width))
    }

    fn set_viewport_rows(&mut self, rows: usize) {
        let rows = rows.max(1);
        if self.viewport_rows == rows {
            return;
        }

        self.viewport_rows = rows;
        self.set_scroll(self.scroll);
    }

    fn set_viewport_width(&mut self, width: usize) {
        let width = width.max(1);
        if self.viewport_width == width {
            return;
        }

        self.viewport_width = width;
        self.set_horizontal_scroll(self.horizontal_scroll);
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

        let prefetch_rows = visible_rows.saturating_mul(self.syntax_limits.prefetch_viewports);
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
        requested: &mut HashSet<SyntaxPosition>,
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
        requested: &mut HashSet<SyntaxPosition>,
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

    fn drain_syntax(&mut self) {
        if let Some(syntax) = self.syntax.as_mut()
            && syntax.drain(self.generation, MAX_SYNTAX_RESULTS_PER_FRAME)
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

    fn inline_ranges(&mut self, file: usize, hunk: usize, line: usize) -> Vec<InlineRange> {
        let key = InlineHunkKey {
            generation: self.generation,
            file,
            hunk,
        };
        if !self.inline_cache.contains_key(&key) {
            let emphasis = self
                .changeset
                .files
                .get(file)
                .and_then(|file_diff| file_diff.hunks.get(hunk))
                .map(|hunk_diff| compute_hunk_inline_emphasis(&hunk_diff.lines))
                .unwrap_or_default();
            self.inline_cache.insert(key, emphasis);
        }

        self.inline_cache
            .get(&key)
            .and_then(|hunk_emphasis| hunk_emphasis.get(line))
            .map(|line_emphasis| line_emphasis.ranges.clone())
            .unwrap_or_default()
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
        self.viewport_width = (width as usize).max(1);
        self.set_layout(default_layout_for_width(width), true);
        self.set_horizontal_scroll(self.horizontal_scroll);
        self.dirty = true;
    }

    fn set_layout(&mut self, layout: DiffLayoutMode, show_notice: bool) {
        if self.layout == layout {
            return;
        }

        self.layout = layout;
        self.model = UiModel::new(&self.changeset, self.layout);
        self.set_horizontal_scroll(self.horizontal_scroll);
        let scroll = self
            .model
            .file_start_row(self.selected_file)
            .unwrap_or_default();
        self.set_scroll(scroll);
        self.dirty = true;
        if show_notice {
            self.set_notice(match self.layout {
                DiffLayoutMode::Split => "split view",
                DiffLayoutMode::Unified => "unified view",
            });
        }
    }

    fn reload(&mut self) -> HzResult<()> {
        let changeset = hz_diff::load_review_ref(&self.options)?;
        self.replace_changeset(changeset, Some("reloaded"));
        Ok(())
    }

    fn replace_changeset(&mut self, changeset: Changeset, notice: Option<&str>) {
        self.replace_loaded_diff(self.options.clone(), changeset, notice);
    }

    fn replace_loaded_diff(
        &mut self,
        options: DiffOptions,
        changeset: Changeset,
        notice: Option<&str>,
    ) {
        let options_changed = self.options != options;
        if !options_changed && self.changeset == changeset {
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
        self.stats = changeset.stats();
        self.max_line_width = changeset_max_line_width(&changeset);
        self.changeset = changeset;
        self.generation = self.generation.wrapping_add(1);
        self.inline_cache.clear();
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
        self.set_horizontal_scroll(self.horizontal_scroll);
        if let Some(notice) = notice {
            self.set_notice(notice);
        }
        self.dirty = true;
    }
}

fn max_scroll_for_viewport(row_count: usize, viewport_rows: usize) -> usize {
    row_count.saturating_sub(viewport_rows.max(1))
}

fn changeset_max_line_width(changeset: &Changeset) -> usize {
    changeset
        .files
        .iter()
        .flat_map(|file| file.hunks.iter())
        .flat_map(|hunk| hunk.lines.iter())
        .map(|line| line.text.width())
        .max()
        .unwrap_or_default()
}

fn diff_content_width(layout: DiffLayoutMode, width: usize) -> usize {
    match layout {
        DiffLayoutMode::Unified => unified_content_width(width),
        DiffLayoutMode::Split => {
            let left_width = width / 2;
            let right_width = width.saturating_sub(left_width);
            split_cell_content_width(left_width).min(split_cell_content_width(right_width))
        }
    }
}

fn unified_content_width(width: usize) -> usize {
    let indicator_width = 1.min(width);
    let gutter_width = UNIFIED_GUTTER_WIDTH.min(width.saturating_sub(indicator_width));
    width.saturating_sub(indicator_width + gutter_width)
}

fn split_cell_content_width(width: usize) -> usize {
    let indicator_width = 1.min(width);
    let gutter_width = GUTTER_WIDTH.min(width.saturating_sub(indicator_width));
    width.saturating_sub(indicator_width + gutter_width)
}

fn draw(frame: &mut Frame<'_>, app: &mut DiffApp) {
    let area = frame.area();
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(area);

    app.set_viewport_rows(vertical[1].height as usize);
    app.set_viewport_width(vertical[1].width as usize);
    draw_header(frame, app, vertical[0]);
    draw_diff(frame, app, vertical[1]);
    draw_diff_menu(frame, app, area);
    draw_branch_menu(frame, app, area);
}

fn draw_header(frame: &mut Frame<'_>, app: &DiffApp, area: Rect) {
    let notice = app
        .notice
        .as_ref()
        .map(|notice| notice.text.as_str())
        .unwrap_or("");
    let mut spans = vec![Span::styled(
        diff_selector_text(&app.options),
        Style::default()
            .fg(app.theme.header)
            .add_modifier(Modifier::BOLD),
    )];
    if app.is_branch_diff()
        && let (Some(head), Some(base)) = (
            app.branch_selector_text(BranchMenu::Head),
            app.branch_selector_text(BranchMenu::Base),
        )
    {
        spans.push(Span::styled(
            head,
            Style::default()
                .fg(app.theme.header)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            BRANCH_COMPARISON_SEPARATOR,
            Style::default().fg(app.theme.muted),
        ));
        spans.push(Span::styled(
            base,
            Style::default()
                .fg(app.theme.header)
                .add_modifier(Modifier::BOLD),
        ));
    } else {
        spans.push(Span::styled(
            diff_comparison_label(&app.options),
            Style::default().fg(app.theme.muted),
        ));
    }
    spans.extend([
        Span::raw("  "),
        Span::styled(
            format!("{} files", format_count(app.stats.files)),
            Style::default().fg(app.theme.foreground),
        ),
        Span::raw("  "),
        Span::styled(
            format!("+{}", format_count(app.stats.additions)),
            Style::default()
                .fg(app.theme.addition_fg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            format!("-{}", format_count(app.stats.deletions)),
            Style::default()
                .fg(app.theme.deletion_fg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            progress_label(app.scroll, app.max_scroll()),
            Style::default().fg(app.theme.header),
        ),
    ]);
    if !notice.is_empty() {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(notice, Style::default().fg(app.theme.notice)));
    }
    let line = Line::from(spans);
    frame.render_widget(
        Paragraph::new(line).style(Style::default().bg(header_bg(app.theme))),
        area,
    );
}

fn draw_diff_menu(frame: &mut Frame<'_>, app: &DiffApp, area: Rect) {
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

fn draw_branch_menu(frame: &mut Frame<'_>, app: &DiffApp, area: Rect) {
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

fn diff_selector_text(options: &DiffOptions) -> String {
    let suffix = if matches!(&options.source, DiffSource::Patch(_)) {
        ""
    } else {
        " ▾"
    };
    format!(" {}{} ", diff_type_label(options), suffix)
}

fn diff_selector_width(options: &DiffOptions) -> u16 {
    diff_selector_text(options).width() as u16
}

fn diff_type_label(options: &DiffOptions) -> &'static str {
    if let Some(choice) = diff_choice_from_options(options) {
        return choice.label();
    }

    match &options.source {
        DiffSource::Range { .. } => "Range",
        DiffSource::Patch(_) => "Patch",
        DiffSource::Worktree | DiffSource::Base(_) | DiffSource::Branch { .. } => "Diff",
    }
}

fn diff_choice_from_options(options: &DiffOptions) -> Option<DiffChoice> {
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

fn diff_comparison_label(options: &DiffOptions) -> String {
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

fn diff_menu_width(choices: &[DiffChoice]) -> u16 {
    choices
        .iter()
        .map(|choice| choice.label().width() + 4)
        .max()
        .unwrap_or_default() as u16
}

fn branch_menu_width(branches: &[String]) -> u16 {
    branches
        .iter()
        .map(|branch| branch.width() + 6)
        .max()
        .unwrap_or_default() as u16
}

fn format_count(count: usize) -> String {
    let digits = count.to_string();
    let mut formatted = String::with_capacity(digits.len() + digits.len() / 3);

    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            formatted.push(',');
        }
        formatted.push(digit);
    }

    formatted
}

fn draw_diff(frame: &mut Frame<'_>, app: &mut DiffApp, area: Rect) {
    if app.model.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "No changes.",
                Style::default().fg(app.theme.muted),
            )))
            .style(Style::default().bg(base_bg(app.theme))),
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
        lines.push(render_row(
            app,
            app.scroll + offset,
            row,
            area.width as usize,
        ));
    }

    frame.render_widget(
        Paragraph::new(Text::from(lines)).style(Style::default().bg(base_bg(app.theme))),
        area,
    );
}

fn render_row(app: &mut DiffApp, row_index: usize, row: UiRow, width: usize) -> Line<'static> {
    let theme = app.theme;
    let horizontal_scroll = app.horizontal_scroll;
    match row {
        UiRow::FileHeader(file_index) => {
            let file = &app.changeset.files[file_index];
            let text = right_aligned(
                &format!("{} {}", status_code(file.status), file.display_path()),
                &format!("+{} -{}", file.additions, file.deletions),
                width,
            );
            Line::from(Span::styled(text, Style::default().fg(theme.file)))
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
                Style::default().fg(theme.muted),
            ))
        }
        UiRow::Collapsed { lines } => {
            let label = format!("⋯ {lines} unchanged");
            Line::from(Span::styled(
                fit_padded(&label, width),
                Style::default().fg(theme.muted),
            ))
        }
        UiRow::HunkHeader { file, hunk } => {
            let hunk = &app.changeset.files[file].hunks[hunk];
            Line::from(Span::styled(
                fit_padded(&hunk.header, width),
                Style::default().fg(theme.hunk),
            ))
        }
        UiRow::UnifiedLine { file, hunk, line } => {
            let diff_line = app.changeset.files[file].hunks[hunk].lines[line].clone();
            let syntax = unified_syntax_side(diff_line.kind)
                .and_then(|side| app.syntax_line(file, hunk, line, side));
            let inline = app.inline_ranges(file, hunk, line);
            render_unified_line_at_scroll(
                &diff_line,
                syntax.as_ref(),
                &inline,
                row_index,
                width,
                theme,
                horizontal_scroll,
            )
        }
        UiRow::MetaLine { file, hunk, line } => {
            let diff_line = app.changeset.files[file].hunks[hunk].lines[line].clone();
            render_unified_line_at_scroll(
                &diff_line,
                None,
                &[],
                row_index,
                width,
                theme,
                horizontal_scroll,
            )
        }
        UiRow::SplitLine {
            file,
            hunk,
            left,
            right,
        } => render_split_line(app, file, hunk, left, right, row_index, width),
    }
}

fn render_unified_line_at_scroll(
    line: &DiffLine,
    syntax: Option<&HighlightedLine>,
    inline: &[InlineRange],
    _row_index: usize,
    width: usize,
    theme: DiffTheme,
    horizontal_scroll: usize,
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
    let indicator_width = 1.min(width);
    let gutter_width = UNIFIED_GUTTER_WIDTH.min(width.saturating_sub(indicator_width));
    let content_width = unified_content_width(width);
    let gutter = format!(
        "{:>5} {:>5} ",
        unified_line_number(line.old_line, line.kind),
        unified_line_number(line.new_line, line.kind)
    );
    let mut spans = Vec::new();
    if indicator_width > 0 {
        spans.push(diff_indicator_span(line.kind, theme));
    }
    if gutter_width > 0 {
        spans.extend(gutter_spans(&gutter, sign, gutter_width, line.kind, theme));
    }
    spans.extend(content_spans_at_scroll(
        &line.text,
        syntax,
        inline,
        line.kind,
        content_width,
        theme,
        horizontal_scroll,
    ));
    Line::from(spans)
}

fn unified_line_number(line: Option<usize>, _kind: DiffLineKind) -> String {
    match line {
        Some(line) => line.to_string(),
        None => String::new(),
    }
}

fn gutter_spans(
    body: &str,
    sign: &str,
    width: usize,
    kind: DiffLineKind,
    theme: DiffTheme,
) -> Vec<Span<'static>> {
    if width == 0 {
        return Vec::new();
    }

    let body_style = Style::default()
        .fg(line_gutter_fg(kind, theme))
        .bg(line_gutter_bg(kind, theme));
    if sign.trim().is_empty() || width == 1 {
        return vec![Span::styled(
            fit_padded(&format!("{body}{sign}"), width),
            body_style,
        )];
    }

    let sign_width = 1;
    let body_width = width.saturating_sub(sign_width);
    vec![
        Span::styled(fit_padded(body, body_width), body_style),
        Span::styled(fit(sign, sign_width), diff_sign_style(kind, theme)),
    ]
}

fn diff_sign_style(kind: DiffLineKind, theme: DiffTheme) -> Style {
    let mut style = Style::default()
        .fg(diff_indicator_fg(kind, theme))
        .bg(line_gutter_bg(kind, theme));
    if theme.diff.sign_style == DiffSignStyle::Bold
        && matches!(kind, DiffLineKind::Addition | DiffLineKind::Deletion)
    {
        style = style.add_modifier(Modifier::BOLD);
    }
    style
}

fn diff_indicator_span(kind: DiffLineKind, theme: DiffTheme) -> Span<'static> {
    Span::styled(DIFF_INDICATOR, diff_indicator_style(kind, theme))
}

fn diff_indicator_style(kind: DiffLineKind, theme: DiffTheme) -> Style {
    Style::default()
        .fg(diff_indicator_fg(kind, theme))
        .bg(line_gutter_bg(kind, theme))
}

fn diff_indicator_fg(kind: DiffLineKind, theme: DiffTheme) -> Color {
    match kind {
        DiffLineKind::Addition => theme.addition_fg,
        DiffLineKind::Deletion => theme.deletion_fg,
        DiffLineKind::Context | DiffLineKind::Meta => theme.muted,
    }
}

fn base_bg(theme: DiffTheme) -> Color {
    if theme.transparent_background {
        Color::Reset
    } else {
        theme.background
    }
}

fn header_bg(theme: DiffTheme) -> Color {
    if theme.transparent_background {
        Color::Reset
    } else {
        theme.gutter_bg
    }
}

fn empty_diff_fill_from(width: usize, row_index: usize, column_offset: usize) -> String {
    (0..width)
        .map(|column| {
            if (column + column_offset + row_index) % EMPTY_DIFF_FILL_SPACING == 0 {
                EMPTY_DIFF_FILL
            } else {
                ' '
            }
        })
        .collect()
}

fn content_spans_at_scroll(
    text: &str,
    syntax: Option<&HighlightedLine>,
    inline: &[InlineRange],
    kind: DiffLineKind,
    width: usize,
    theme: DiffTheme,
    horizontal_scroll: usize,
) -> Vec<Span<'static>> {
    if width == 0 {
        return Vec::new();
    }

    let inline = valid_inline_ranges(text, inline);
    let syntax = syntax.filter(|syntax| syntax_line_matches_text(syntax, text));
    if syntax.is_none() && inline.is_empty() {
        return vec![Span::styled(
            fit_padded_from(text, horizontal_scroll, width),
            line_style(kind, theme),
        )];
    }

    let mut writer = ContentSpanWriter::new(&inline, kind, width, theme, horizontal_scroll);

    if let Some(syntax) = syntax {
        let mut byte_start = 0usize;
        for segment in &syntax.segments {
            if !writer.push_segment(
                &segment.text,
                byte_start,
                syntax_style(segment.class, kind, theme),
            ) {
                break;
            }
            byte_start += segment.text.len();
        }
    } else {
        writer.push_segment(text, 0, line_style(kind, theme));
    }

    writer.finish()
}

fn valid_inline_ranges(text: &str, ranges: &[InlineRange]) -> Vec<InlineRange> {
    let mut valid = ranges
        .iter()
        .filter_map(|range| {
            let byte_start = range.byte_start.min(text.len());
            let byte_end = range.byte_end.min(text.len());
            (byte_start < byte_end
                && text.is_char_boundary(byte_start)
                && text.is_char_boundary(byte_end))
            .then_some(InlineRange {
                byte_start,
                byte_end,
            })
        })
        .collect::<Vec<_>>();
    valid.sort_by_key(|range| (range.byte_start, range.byte_end));
    valid
}

struct ContentSpanWriter<'a> {
    spans: Vec<Span<'static>>,
    inline: &'a [InlineRange],
    kind: DiffLineKind,
    width: usize,
    skip: usize,
    used: usize,
    theme: DiffTheme,
}

impl<'a> ContentSpanWriter<'a> {
    fn new(
        inline: &'a [InlineRange],
        kind: DiffLineKind,
        width: usize,
        theme: DiffTheme,
        horizontal_scroll: usize,
    ) -> Self {
        Self {
            spans: Vec::new(),
            inline,
            kind,
            width,
            skip: horizontal_scroll,
            used: 0,
            theme,
        }
    }

    fn push_segment(
        &mut self,
        segment_text: &str,
        segment_byte_start: usize,
        style: Style,
    ) -> bool {
        let segment_byte_end = segment_byte_start + segment_text.len();
        let mut cursor = segment_byte_start;

        for range in self.inline {
            if self.used >= self.width {
                return false;
            }
            if range.byte_end <= cursor {
                continue;
            }
            if range.byte_start >= segment_byte_end {
                break;
            }

            let normal_end = range.byte_start.min(segment_byte_end);
            if !self.push_piece(segment_text, segment_byte_start, cursor, normal_end, style) {
                return false;
            }

            let inline_start = range.byte_start.max(cursor).min(segment_byte_end);
            let inline_end = range.byte_end.min(segment_byte_end);
            if !self.push_piece(
                segment_text,
                segment_byte_start,
                inline_start,
                inline_end,
                inline_style(style, self.kind, self.theme),
            ) {
                return false;
            }
            cursor = inline_end;
        }

        self.push_piece(
            segment_text,
            segment_byte_start,
            cursor,
            segment_byte_end,
            style,
        )
    }

    fn push_piece(
        &mut self,
        segment_text: &str,
        segment_byte_start: usize,
        byte_start: usize,
        byte_end: usize,
        style: Style,
    ) -> bool {
        if byte_start >= byte_end {
            return true;
        }
        let remaining = self.width.saturating_sub(self.used);
        if remaining == 0 {
            return false;
        }

        let relative_start = byte_start.saturating_sub(segment_byte_start);
        let relative_end = byte_end.saturating_sub(segment_byte_start);
        let mut piece = &segment_text[relative_start..relative_end];
        if self.skip > 0 {
            let (visible, skipped) = skip_display_prefix(piece, self.skip);
            self.skip = self.skip.saturating_sub(skipped);
            piece = visible;
            if piece.is_empty() {
                return true;
            }
        }
        let fitted = fit(piece, remaining);
        if fitted.is_empty() {
            return false;
        }

        let fitted_len = fitted.len();
        self.used += UnicodeWidthStr::width(fitted.as_str());
        self.spans.push(Span::styled(fitted, style));
        fitted_len == piece.len()
    }

    fn finish(mut self) -> Vec<Span<'static>> {
        if self.used < self.width {
            self.spans.push(Span::styled(
                " ".repeat(self.width - self.used),
                line_style(self.kind, self.theme),
            ));
        }
        self.spans
    }
}

fn syntax_line_matches_text(syntax: &HighlightedLine, text: &str) -> bool {
    let mut remaining = text;
    for segment in &syntax.segments {
        if !remaining.starts_with(&segment.text) {
            return false;
        }
        remaining = &remaining[segment.text.len()..];
    }
    remaining.is_empty()
}

fn syntax_style(class: Option<SyntaxClass>, kind: DiffLineKind, theme: DiffTheme) -> Style {
    let mut style = line_style(kind, theme);
    if let Some(color) = class.and_then(|class| syntax_fg(class, theme)) {
        style = style.fg(color);
    }
    style
}

fn inline_style(style: Style, kind: DiffLineKind, theme: DiffTheme) -> Style {
    if theme.transparent_background || theme.diff.inline_background == DiffBackground::None {
        return match kind {
            DiffLineKind::Addition | DiffLineKind::Deletion => style.add_modifier(Modifier::BOLD),
            DiffLineKind::Context | DiffLineKind::Meta => style,
        };
    }

    match kind {
        DiffLineKind::Addition => style
            .bg(inline_bg(kind, theme))
            .add_modifier(Modifier::BOLD),
        DiffLineKind::Deletion => style
            .bg(inline_bg(kind, theme))
            .add_modifier(Modifier::BOLD),
        DiffLineKind::Context | DiffLineKind::Meta => style,
    }
}

fn inline_bg(kind: DiffLineKind, theme: DiffTheme) -> Color {
    match (theme.diff.inline_background, kind) {
        (DiffBackground::Subtle, DiffLineKind::Addition) => theme.addition_bg,
        (DiffBackground::Subtle, DiffLineKind::Deletion) => theme.deletion_bg,
        (_, DiffLineKind::Addition) => theme.addition_inline_bg,
        (_, DiffLineKind::Deletion) => theme.deletion_inline_bg,
        _ => Color::Reset,
    }
}

fn syntax_fg(class: SyntaxClass, theme: DiffTheme) -> Option<Color> {
    theme.syntax.color(class)
}

fn render_split_line(
    app: &mut DiffApp,
    file: usize,
    hunk: usize,
    left: Option<usize>,
    right: Option<usize>,
    row_index: usize,
    width: usize,
) -> Line<'static> {
    if width == 0 {
        return Line::default();
    }
    let theme = app.theme;
    let horizontal_scroll = app.horizontal_scroll;

    let (left_line, right_line) = {
        let lines = &app.changeset.files[file].hunks[hunk].lines;
        (
            left.and_then(|index| lines.get(index)).cloned(),
            right.and_then(|index| lines.get(index)).cloned(),
        )
    };
    let left_syntax = left.and_then(|index| app.syntax_line(file, hunk, index, DiffSide::Old));
    let right_syntax = right.and_then(|index| app.syntax_line(file, hunk, index, DiffSide::New));
    let left_inline = left
        .map(|index| app.inline_ranges(file, hunk, index))
        .unwrap_or_default();
    let right_inline = right
        .map(|index| app.inline_ranges(file, hunk, index))
        .unwrap_or_default();

    let left_width = width / 2;
    let right_width = width.saturating_sub(left_width);
    let mut spans = split_cell_spans_at_scroll(
        left_line.as_ref(),
        left_syntax.as_ref(),
        &left_inline,
        SplitCellRender {
            side: SplitSide::Old,
            row_index,
            width: left_width,
            theme,
        },
        horizontal_scroll,
    );
    spans.extend(split_cell_spans_at_scroll(
        right_line.as_ref(),
        right_syntax.as_ref(),
        &right_inline,
        SplitCellRender {
            side: SplitSide::New,
            row_index,
            width: right_width,
            theme,
        },
        horizontal_scroll,
    ));
    Line::from(spans)
}

#[derive(Debug, Clone, Copy)]
enum SplitSide {
    Old,
    New,
}

#[derive(Debug, Clone, Copy)]
struct SplitCellRender {
    side: SplitSide,
    row_index: usize,
    width: usize,
    theme: DiffTheme,
}

fn split_cell_spans_at_scroll(
    line: Option<&DiffLine>,
    syntax: Option<&HighlightedLine>,
    inline: &[InlineRange],
    render: SplitCellRender,
    horizontal_scroll: usize,
) -> Vec<Span<'static>> {
    let SplitCellRender {
        side,
        row_index,
        width,
        theme,
    } = render;

    if width == 0 {
        return Vec::new();
    }

    let Some(line) = line else {
        let empty_kind = DiffLineKind::Context;
        let indicator_width = 1.min(width);
        let gutter_width = GUTTER_WIDTH.min(width.saturating_sub(indicator_width));
        let content_width = split_cell_content_width(width);
        let mut spans = Vec::new();
        if indicator_width > 0 {
            spans.push(diff_indicator_span(empty_kind, theme));
        }
        if gutter_width > 0 {
            spans.push(Span::styled(
                " ".repeat(gutter_width),
                Style::default().bg(line_gutter_bg(empty_kind, theme)),
            ));
        }
        if content_width > 0 {
            spans.push(Span::styled(
                empty_diff_fill_from(
                    content_width,
                    row_index,
                    indicator_width + gutter_width + horizontal_scroll,
                ),
                Style::default().fg(theme.empty_diff).bg(base_bg(theme)),
            ));
        }
        return spans;
    };

    let indicator_width = 1.min(width);
    let gutter_width = GUTTER_WIDTH.min(width.saturating_sub(indicator_width));
    let content_width = split_cell_content_width(width);
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

    let mut spans = Vec::new();
    if indicator_width > 0 {
        spans.push(diff_indicator_span(line.kind, theme));
    }
    if gutter_width > 0 {
        spans.extend(gutter_spans(
            &format!("{line_number:>5} "),
            sign,
            gutter_width,
            line.kind,
            theme,
        ));
    }
    spans.extend(content_spans_at_scroll(
        &line.text,
        syntax,
        inline,
        line.kind,
        content_width,
        theme,
        horizontal_scroll,
    ));
    spans
}

fn row_bg(kind: DiffLineKind, theme: DiffTheme) -> Color {
    if theme.transparent_background {
        return Color::Reset;
    }

    match (theme.diff.line_background, kind) {
        (DiffBackground::None, _) => theme.background,
        (DiffBackground::Subtle, DiffLineKind::Addition) => theme.addition_bg,
        (DiffBackground::Subtle, DiffLineKind::Deletion) => theme.deletion_bg,
        (DiffBackground::Strong, DiffLineKind::Addition) => theme.addition_inline_bg,
        (DiffBackground::Strong, DiffLineKind::Deletion) => theme.deletion_inline_bg,
        _ => theme.background,
    }
}

fn line_style(kind: DiffLineKind, theme: DiffTheme) -> Style {
    match kind {
        DiffLineKind::Addition => Style::default()
            .fg(theme.foreground)
            .bg(row_bg(kind, theme)),
        DiffLineKind::Deletion => Style::default()
            .fg(theme.foreground)
            .bg(row_bg(kind, theme)),
        DiffLineKind::Meta => Style::default().fg(theme.muted).bg(base_bg(theme)),
        DiffLineKind::Context => Style::default().fg(theme.foreground).bg(base_bg(theme)),
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
    fit_padded_from(text, 0, width)
}

fn fit_padded_from(text: &str, horizontal_scroll: usize, width: usize) -> String {
    let visible = if horizontal_scroll > 0 {
        skip_display_prefix(text, horizontal_scroll).0
    } else {
        text
    };
    let mut out = fit(visible, width);
    let len = UnicodeWidthStr::width(out.as_str());
    if len < width {
        out.push_str(&" ".repeat(width - len));
    }
    out
}

fn skip_display_prefix(text: &str, columns: usize) -> (&str, usize) {
    if columns == 0 {
        return (text, 0);
    }

    let mut skipped = 0usize;
    let mut byte_index = 0usize;
    for (index, ch) in text.char_indices() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if skipped >= columns {
            if ch_width == 0 {
                byte_index = index + ch.len_utf8();
                continue;
            }
            break;
        }

        skipped = skipped.saturating_add(ch_width);
        byte_index = index + ch.len_utf8();
    }

    (&text[byte_index..], skipped)
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
    fn app_clamps_horizontal_scroll_to_diff_content() {
        let changeset = changeset_with_line_text("abcdefghijkl");
        let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);

        app.set_viewport_width(18);
        assert_eq!(diff_content_width(app.layout, app.viewport_width), 4);

        app.scroll_horizontally_by(HORIZONTAL_SCROLL_STEP as isize);
        assert_eq!(app.horizontal_scroll, 8);

        app.scroll_horizontally_by(HORIZONTAL_SCROLL_STEP as isize);
        assert_eq!(app.horizontal_scroll, 8);

        app.scroll_horizontally_by(-(HORIZONTAL_SCROLL_STEP as isize));
        assert_eq!(app.horizontal_scroll, 0);

        app.set_horizontal_scroll(8);
        app.set_viewport_width(80);
        assert_eq!(app.horizontal_scroll, 0);
    }

    #[test]
    fn responsive_layout_preserves_valid_horizontal_scroll() {
        let long_line = "a".repeat(120);
        let changeset = changeset_with_line_text(&long_line);
        let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
        app.set_viewport_width(80);
        app.set_horizontal_scroll(40);

        app.apply_responsive_layout(MIN_SPLIT_WIDTH);

        assert_eq!(app.layout, DiffLayoutMode::Split);
        assert_eq!(app.horizontal_scroll, 40);
    }

    #[test]
    fn responsive_layout_clamps_horizontal_scroll_without_layout_change() {
        let long_line = "a".repeat(100);
        let changeset = changeset_with_line_text(&long_line);
        let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Split);

        app.apply_responsive_layout(MIN_SPLIT_WIDTH);
        assert_eq!(app.layout, DiffLayoutMode::Split);
        app.set_horizontal_scroll(usize::MAX);
        let previous_scroll = app.horizontal_scroll;

        app.apply_responsive_layout(MIN_SPLIT_WIDTH + 40);

        assert_eq!(app.layout, DiffLayoutMode::Split);
        assert!(app.max_horizontal_scroll() < previous_scroll);
        assert_eq!(app.horizontal_scroll, app.max_horizontal_scroll());
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
    fn diff_header_labels_describe_selected_scope() {
        let mut options = DiffOptions::default();

        assert_eq!(diff_selector_text(&options), " All changes ▾ ");
        assert_eq!(diff_comparison_label(&options), "HEAD → working tree");

        options.scope = DiffScope::Unstaged;
        assert_eq!(diff_selector_text(&options), " Unstaged ▾ ");
        assert_eq!(diff_comparison_label(&options), "index → working tree");

        options.scope = DiffScope::Staged;
        assert_eq!(diff_selector_text(&options), " Staged ▾ ");
        assert_eq!(diff_comparison_label(&options), "HEAD → index");

        options.source = DiffSource::Base("origin/main".to_owned());
        options.scope = DiffScope::All;
        assert_eq!(diff_selector_text(&options), " Branch ▾ ");
        assert_eq!(diff_comparison_label(&options), "HEAD → origin/main");

        options.source = DiffSource::Branch {
            base: "origin/main".to_owned(),
            head: "feature/ui".to_owned(),
        };
        assert_eq!(diff_comparison_label(&options), "feature/ui → origin/main");
    }

    #[test]
    fn diff_menu_lists_all_changes_first() {
        let mut app = DiffApp::new(
            DiffOptions::default(),
            changeset_with_context_lines(1),
            DiffLayoutMode::Unified,
        );
        app.branch_base = Some("origin/main".to_owned());

        assert_eq!(
            app.diff_menu_choices(),
            vec![
                DiffChoice::All,
                DiffChoice::Unstaged,
                DiffChoice::Staged,
                DiffChoice::Branch,
            ]
        );
    }

    #[test]
    fn diff_menu_options_preserve_repo_and_untracked_setting() {
        let options = DiffOptions {
            repo: Some(PathBuf::from("/repo")),
            include_untracked: false,
            ..DiffOptions::default()
        };
        let mut app = DiffApp::new(
            options.clone(),
            changeset_with_context_lines(1),
            DiffLayoutMode::Unified,
        );
        app.branch_base = Some("origin/main".to_owned());
        app.branch_head = Some("feature/ui".to_owned());
        app.current_head = Some("feature/ui".to_owned());

        let staged = app.options_for_choice(DiffChoice::Staged).unwrap();
        assert_eq!(staged.repo, options.repo);
        assert!(!staged.include_untracked);
        assert_eq!(staged.source, DiffSource::Worktree);
        assert_eq!(staged.scope, DiffScope::Staged);

        let branch = app.options_for_choice(DiffChoice::Branch).unwrap();
        assert_eq!(branch.source, DiffSource::Base("origin/main".to_owned()));
        assert_eq!(branch.scope, DiffScope::All);
    }

    #[test]
    fn branch_choice_survives_switching_to_worktree_scope() {
        let options = DiffOptions {
            source: DiffSource::Base("origin/main".to_owned()),
            ..DiffOptions::default()
        };
        let mut app = DiffApp::new(
            options,
            changeset_with_context_lines(1),
            DiffLayoutMode::Unified,
        );
        app.branch_base = Some("origin/main".to_owned());
        app.branch_head = Some("feature/header".to_owned());

        app.replace_loaded_diff(
            DiffOptions::default(),
            changeset_with_context_lines(1),
            None,
        );

        assert_eq!(app.branch_base.as_deref(), Some("origin/main"));
        assert_eq!(app.branch_head.as_deref(), Some("feature/header"));
        assert_eq!(
            app.options_for_choice(DiffChoice::Branch)
                .map(|options| options.source),
            Some(DiffSource::Branch {
                base: "origin/main".to_owned(),
                head: "feature/header".to_owned(),
            })
        );
    }

    #[test]
    fn branch_header_exposes_head_and_base_selectors() {
        let options = DiffOptions {
            source: DiffSource::Base("origin/main".to_owned()),
            ..DiffOptions::default()
        };
        let mut app = DiffApp::new(
            options,
            changeset_with_context_lines(1),
            DiffLayoutMode::Unified,
        );
        app.branch_head = Some("feature/ui".to_owned());
        app.branch_base = Some("origin/main".to_owned());
        app.current_head = Some("feature/ui".to_owned());

        assert_eq!(
            app.branch_selector_text(BranchMenu::Head).as_deref(),
            Some("● feature/ui ▾")
        );
        assert_eq!(
            app.branch_selector_text(BranchMenu::Base).as_deref(),
            Some("⌂ origin/main ▾")
        );
        assert_eq!(
            app.branch_selector_at(diff_selector_width(&app.options)),
            Some(BranchMenu::Head)
        );

        app.toggle_branch_menu(BranchMenu::Head);
        let empty_input = app.branch_selector_text(BranchMenu::Head).unwrap();
        assert_eq!(empty_input.width(), "● feature/ui ▾".width());
        assert!(empty_input.trim_start().starts_with('▾'));
        app.push_branch_input('f');
        let typed_input = app.branch_selector_text(BranchMenu::Head).unwrap();
        assert_eq!(typed_input.width(), "● feature/ui ▾".width());
        assert!(typed_input.starts_with('f'));
        app.close_branch_menu();
        assert_eq!(
            app.branch_selector_text(BranchMenu::Head).as_deref(),
            Some("● feature/ui ▾")
        );
    }

    #[test]
    fn branch_menu_scrolls_visible_branch_window() {
        let options = DiffOptions {
            source: DiffSource::Base("branch-00".to_owned()),
            ..DiffOptions::default()
        };
        let mut app = DiffApp::new(
            options,
            changeset_with_context_lines(1),
            DiffLayoutMode::Unified,
        );
        app.comparison_branches = (0..12).map(|index| format!("branch-{index:02}")).collect();

        assert_eq!(app.visible_branch_menu_rows(), MAX_BRANCH_MENU_ROWS);
        assert_eq!(app.max_branch_menu_scroll(), 2);

        app.move_branch_selection(99);
        assert_eq!(app.branch_menu_selected, 11);
        assert_eq!(app.branch_menu_scroll, 2);

        app.move_branch_selection(-1);
        assert_eq!(app.branch_menu_selected, 10);
        assert_eq!(app.branch_menu_scroll, 2);
    }

    #[test]
    fn branch_combo_input_filters_and_completes() {
        let options = DiffOptions {
            source: DiffSource::Base("main".to_owned()),
            ..DiffOptions::default()
        };
        let mut app = DiffApp::new(
            options,
            changeset_with_context_lines(1),
            DiffLayoutMode::Unified,
        );
        app.comparison_branches = vec![
            "main".to_owned(),
            "feature/header".to_owned(),
            "fix/footer".to_owned(),
        ];

        app.push_branch_input('h');
        assert_eq!(app.filtered_branches(), vec!["feature/header"]);

        app.clear_branch_input();
        app.push_branch_input('f');
        app.push_branch_input('h');
        assert_eq!(app.filtered_branches(), vec!["feature/header"]);

        app.branch_menu_open = Some(BranchMenu::Head);
        app.cycle_branch_completion(1);
        assert_eq!(app.branch_menu_selected, 0);
        assert_eq!(app.branch_menu_input, "fh");

        app.clear_branch_input();
        app.push_branch_input('f');
        assert_eq!(
            app.filtered_branches(),
            vec!["fix/footer", "feature/header"]
        );
        app.cycle_branch_completion(1);
        assert_eq!(app.branch_menu_selected, 1);
        app.cycle_branch_completion(-1);
        assert_eq!(app.branch_menu_selected, 0);

        app.clear_branch_input();
        assert!(app.branch_menu_input.is_empty());
    }

    #[test]
    fn branch_combo_pins_current_head_and_base_before_recent_order() {
        let options = DiffOptions {
            source: DiffSource::Base("release".to_owned()),
            ..DiffOptions::default()
        };
        let mut app = DiffApp::new(
            options,
            changeset_with_context_lines(1),
            DiffLayoutMode::Unified,
        );
        app.branch_head = Some("feature/header".to_owned());
        app.current_head = Some("feature/header".to_owned());
        app.branch_base = Some("release".to_owned());
        app.comparison_branches = vec![
            "recent".to_owned(),
            "old".to_owned(),
            "origin/main".to_owned(),
            "release".to_owned(),
            "feature/header".to_owned(),
        ];

        app.branch_menu_open = Some(BranchMenu::Base);
        assert_eq!(
            app.filtered_branches(),
            vec!["release", "feature/header", "recent", "old", "origin/main"]
        );

        app.branch_menu_open = Some(BranchMenu::Head);
        assert_eq!(
            app.filtered_branches(),
            vec!["feature/header", "release", "recent", "old", "origin/main"]
        );
    }

    #[test]
    fn branch_combo_close_clears_input_without_changing_selection() {
        let options = DiffOptions {
            source: DiffSource::Base("main".to_owned()),
            ..DiffOptions::default()
        };
        let mut app = DiffApp::new(
            options,
            changeset_with_context_lines(1),
            DiffLayoutMode::Unified,
        );
        app.branch_base = Some("main".to_owned());
        app.branch_head = Some("feature/header".to_owned());
        app.comparison_branches = vec!["main".to_owned(), "feature/header".to_owned()];

        app.toggle_branch_menu(BranchMenu::Base);
        app.push_branch_input('f');
        app.close_branch_menu();

        assert!(app.branch_menu_open.is_none());
        assert!(app.branch_menu_input.is_empty());
        assert_eq!(app.branch_base.as_deref(), Some("main"));
        assert_eq!(app.branch_head.as_deref(), Some("feature/header"));
        assert_eq!(app.options.source, DiffSource::Base("main".to_owned()));
    }

    #[test]
    fn format_count_groups_thousands() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(42), "42");
        assert_eq!(format_count(999), "999");
        assert_eq!(format_count(1_000), "1,000");
        assert_eq!(format_count(1_009_257), "1,009,257");
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
        assert_eq!(fit_padded_from("abcdef", 2, 3), "cde");
        assert_eq!(skip_display_prefix("e\u{301}f", 1), ("f", 1));
        assert_eq!(right_aligned("界", "x", 5), "界  x");
    }

    #[test]
    fn content_spans_fall_back_when_syntax_text_mismatches_diff_text() {
        let syntax = HighlightedLine {
            segments: vec![hz_syntax::SyntaxSegment {
                byte_start: 0,
                byte_end: 5,
                text: "wrong".to_owned(),
                class: Some(SyntaxClass::Keyword),
            }],
        };

        let spans = content_spans_at_scroll(
            "right",
            Some(&syntax),
            &[],
            DiffLineKind::Addition,
            8,
            DiffTheme::default(),
            0,
        );
        let text = spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();

        assert_eq!(text, "right   ");
        assert_eq!(spans.len(), 1);
    }

    #[test]
    fn empty_diff_fill_draws_shifted_diagonal_pattern() {
        assert_eq!(empty_diff_fill_from(8, 0, 0), "╱  ╱  ╱ ");
        assert_eq!(empty_diff_fill_from(8, 1, 0), "  ╱  ╱  ");
        assert_eq!(empty_diff_fill_from(8, 2, 0), " ╱  ╱  ╱");
    }

    #[test]
    fn split_empty_cells_use_default_gutter_and_hatched_fill() {
        let spans = split_cell_spans_at_scroll(
            None,
            None,
            &[],
            SplitCellRender {
                side: SplitSide::Old,
                row_index: 0,
                width: 12,
                theme: DiffTheme::default(),
            },
            0,
        );

        assert_eq!(span_text(&spans), "▌        ╱  ");
        assert_eq!(spans[0].content.as_ref(), DIFF_INDICATOR);
        assert_eq!(spans[0].style.fg, Some(DiffTheme::default().muted));
        assert_eq!(spans[0].style.bg, Some(DiffTheme::default().gutter_bg));
        assert_eq!(spans[1].content.as_ref(), "       ");
        assert_eq!(spans[1].style.bg, Some(DiffTheme::default().gutter_bg));
        assert_eq!(spans[2].style.fg, Some(DiffTheme::default().empty_diff));
    }

    #[test]
    fn line_gutters_use_theme_background() {
        let theme = DiffTheme::default();
        let line = DiffLine {
            kind: DiffLineKind::Context,
            old_line: Some(7),
            new_line: Some(7),
            text: "same".to_owned(),
        };

        let rendered = render_unified_line_at_scroll(&line, None, &[], 0, 24, theme, 0);

        assert_eq!(rendered.spans[0].style.fg, Some(theme.muted));
        assert_eq!(rendered.spans[0].style.bg, Some(theme.gutter_bg));
        assert_eq!(rendered.spans[1].style.fg, Some(theme.foreground));
        assert_eq!(rendered.spans[1].style.bg, Some(theme.gutter_bg));
    }

    #[test]
    fn changed_line_gutters_use_delta_colors_and_bold_signs() {
        let theme = DiffTheme::default();
        let line = DiffLine {
            kind: DiffLineKind::Addition,
            old_line: None,
            new_line: Some(7),
            text: "added".to_owned(),
        };

        let rendered = render_unified_line_at_scroll(&line, None, &[], 0, 24, theme, 0);

        assert_eq!(rendered.spans[0].style.bg, Some(theme.addition_gutter_bg));
        assert_eq!(rendered.spans[1].style.fg, Some(theme.addition_fg));
        assert_eq!(rendered.spans[1].style.bg, Some(theme.addition_gutter_bg));
        assert_eq!(rendered.spans[2].content.as_ref(), "+");
        assert_eq!(rendered.spans[2].style.fg, Some(theme.addition_fg));
        assert_eq!(rendered.spans[2].style.bg, Some(theme.addition_gutter_bg));
        assert!(
            rendered.spans[2]
                .style
                .add_modifier
                .contains(Modifier::BOLD)
        );
        assert_eq!(rendered.spans[3].style.fg, Some(theme.foreground));
        assert_eq!(rendered.spans[3].style.bg, Some(theme.addition_bg));
    }

    #[test]
    fn split_view_uses_right_indicator_as_separator() {
        let changeset = changeset_with_context_lines(1);
        let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Split);

        let rendered = render_split_line(&mut app, 0, 0, Some(0), Some(0), 0, 24);
        let text = line_text(&rendered);

        assert!(!text.contains('│'));
        assert_eq!(text.chars().nth(12), Some('▌'));
    }

    #[test]
    fn unified_diff_content_scrolls_horizontally() {
        let line = DiffLine {
            kind: DiffLineKind::Context,
            old_line: Some(1),
            new_line: Some(1),
            text: "abcdef".to_owned(),
        };

        let rendered =
            render_unified_line_at_scroll(&line, None, &[], 0, 18, DiffTheme::default(), 2);

        assert!(line_text(&rendered).ends_with("cdef"));
    }

    #[test]
    fn split_diff_content_scrolls_horizontally() {
        let changeset = changeset_with_line_text("abcdef");
        let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Split);
        app.horizontal_scroll = 2;

        let rendered = render_split_line(&mut app, 0, 0, Some(0), Some(0), 0, 24);

        assert_eq!(line_text(&rendered), "▌    1  cdef▌    1  cdef");
    }

    #[test]
    fn diff_lines_start_with_change_indicator() {
        let line = DiffLine {
            kind: DiffLineKind::Addition,
            old_line: None,
            new_line: Some(3),
            text: "new".to_owned(),
        };

        let rendered =
            render_unified_line_at_scroll(&line, None, &[], 0, 24, DiffTheme::default(), 0);

        assert_eq!(rendered.spans[0].content.as_ref(), DIFF_INDICATOR);
        assert_eq!(
            rendered.spans[0].style.fg,
            Some(DiffTheme::default().addition_fg)
        );
        assert!(!line_text(&rendered).contains(EMPTY_DIFF_FILL));
    }

    #[test]
    fn ansi_theme_uses_terminal_palette_indices() {
        let theme = diff_theme_from_config(&SyntaxThemeConfig {
            source: SyntaxThemeSource::Ansi,
            name: None,
            path: None,
        })
        .expect("ansi theme should load");

        assert_eq!(theme.addition_fg, Color::Indexed(2));
        assert_eq!(
            theme.syntax.color(SyntaxClass::Keyword),
            Some(Color::Indexed(13))
        );
    }

    #[test]
    fn system_theme_preserves_terminal_base_and_uses_owned_diff_colors() {
        let theme = builtin_diff_theme(Some("system")).expect("system theme should load");

        assert_eq!(theme.foreground, Color::Reset);
        assert_eq!(theme.background, Color::Reset);
        assert_eq!(theme.file, Color::Reset);
        assert_ne!(theme.addition_fg, Color::Indexed(2));
        assert_ne!(theme.deletion_fg, Color::Indexed(1));
        assert_eq!(row_bg(DiffLineKind::Addition, theme), theme.addition_bg);
        assert_eq!(
            inline_bg(DiffLineKind::Addition, theme),
            theme.addition_inline_bg
        );
        assert_eq!(
            line_gutter_bg(DiffLineKind::Addition, theme),
            theme.addition_gutter_bg
        );
        assert_eq!(
            theme.syntax.color(SyntaxClass::String),
            SyntaxPalette::ansi().color(SyntaxClass::String)
        );
    }

    #[test]
    fn default_theme_alias_uses_system_theme() {
        let theme = builtin_diff_theme(Some("default")).expect("default theme should load");

        assert_eq!(theme, DiffTheme::system());
    }

    #[test]
    fn color_overrides_layer_on_colorscheme() {
        let theme = DiffTheme::system()
            .with_color_overrides(&ColorOverrides {
                bg: Some("#010203".to_owned()),
                addition_bg: Some("#123456".to_owned()),
                deletion_fg: Some("bright-red".to_owned()),
                keyword: Some("ansi-13".to_owned()),
                ..ColorOverrides::default()
            })
            .expect("color overrides should parse");

        assert_eq!(theme.background, Color::Rgb(1, 2, 3));
        assert_eq!(
            row_bg(DiffLineKind::Addition, theme),
            Color::Rgb(0x12, 0x34, 0x56)
        );
        assert_eq!(theme.deletion_fg, Color::LightRed);
        assert_eq!(
            theme.syntax.color(SyntaxClass::Keyword),
            Some(Color::Indexed(13))
        );
    }

    #[test]
    fn packaged_popular_themes_are_available() {
        for name in ["catppuccin-mocha", "gruvbox-dark", "tokyonight", "dracula"] {
            let theme = builtin_diff_theme(Some(name)).expect("built-in theme should load");

            assert_ne!(
                theme.file,
                Color::Reset,
                "{name} should set file foreground"
            );
            assert!(
                theme.syntax.color(SyntaxClass::Keyword).is_some(),
                "{name} should set syntax keyword foreground"
            );
        }
    }

    #[test]
    fn transparent_background_resets_diff_and_inline_backgrounds() {
        let theme = DiffTheme::catppuccin_mocha().with_transparent_background(true);
        let spans = content_spans_at_scroll(
            "changed",
            None,
            &[InlineRange {
                byte_start: 0,
                byte_end: 7,
            }],
            DiffLineKind::Addition,
            8,
            theme,
            0,
        );

        assert_eq!(row_bg(DiffLineKind::Addition, theme), Color::Reset);
        assert_eq!(spans[0].style.bg, Some(Color::Reset));
        assert!(spans[0].style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn base16_theme_parser_accepts_yaml_or_toml_lines() {
        let scheme = parse_base16_scheme(
            r##"
base00: "#000000"
base01: "111111"
base02: "222222"
base03: "333333"
base04 = "444444"
base05 = "555555"
base06 = "666666"
base07 = "777777"
base08 = "888888"
base09 = "999999"
base0A = "aaaaaa"
base0B = "bbbbbb"
base0C = "cccccc"
base0D = "dddddd"
base0E = "eeeeee"
base0F = "ffffff"
"##,
        )
        .expect("base16 scheme should parse");
        let theme = DiffTheme::base16(scheme);

        assert_eq!(theme.muted, Color::Rgb(51, 51, 51));
        assert_eq!(
            theme.syntax.color(SyntaxClass::String),
            Some(Color::Rgb(187, 187, 187))
        );
    }

    #[test]
    fn inline_emphasis_marks_changed_tokens_in_paired_lines() {
        let lines = vec![
            DiffLine {
                kind: DiffLineKind::Deletion,
                old_line: Some(1),
                new_line: None,
                text: "let count = 1;".to_owned(),
            },
            DiffLine {
                kind: DiffLineKind::Addition,
                old_line: None,
                new_line: Some(1),
                text: "let total = 2;".to_owned(),
            },
        ];

        let emphasis = compute_hunk_inline_emphasis(&lines);

        assert_eq!(
            range_texts(&lines[0].text, &emphasis[0].ranges),
            ["count", "1"]
        );
        assert_eq!(
            range_texts(&lines[1].text, &emphasis[1].ranges),
            ["total", "2"]
        );
    }

    #[test]
    fn inline_emphasis_leaves_unpaired_changed_lines_to_line_style() {
        let lines = vec![DiffLine {
            kind: DiffLineKind::Deletion,
            old_line: Some(1),
            new_line: None,
            text: "removed line".to_owned(),
        }];

        let emphasis = compute_hunk_inline_emphasis(&lines);

        assert!(emphasis[0].ranges.is_empty());
    }

    #[test]
    fn inline_diff_skips_expensive_long_line_pairs() {
        let lines = vec![
            DiffLine {
                kind: DiffLineKind::Deletion,
                old_line: Some(1),
                new_line: None,
                text: "a".repeat(MAX_INLINE_DIFF_LINE_BYTES + 1),
            },
            DiffLine {
                kind: DiffLineKind::Addition,
                old_line: None,
                new_line: Some(1),
                text: "b".repeat(MAX_INLINE_DIFF_LINE_BYTES + 1),
            },
        ];

        let emphasis = compute_hunk_inline_emphasis(&lines);

        assert!(emphasis[0].ranges.is_empty());
        assert!(emphasis[1].ranges.is_empty());
    }

    #[test]
    fn content_spans_layers_inline_emphasis_over_syntax() {
        let text = "let value = 2;";
        let number_start = text.find('2').unwrap();
        let syntax = HighlightedLine {
            segments: vec![
                hz_syntax::SyntaxSegment {
                    byte_start: 0,
                    byte_end: 12,
                    text: "let value = ".to_owned(),
                    class: Some(SyntaxClass::Keyword),
                },
                hz_syntax::SyntaxSegment {
                    byte_start: 12,
                    byte_end: 13,
                    text: "2".to_owned(),
                    class: Some(SyntaxClass::Number),
                },
                hz_syntax::SyntaxSegment {
                    byte_start: 13,
                    byte_end: 14,
                    text: ";".to_owned(),
                    class: Some(SyntaxClass::Punctuation),
                },
            ],
        };

        let spans = content_spans_at_scroll(
            text,
            Some(&syntax),
            &[InlineRange {
                byte_start: number_start,
                byte_end: number_start + 1,
            }],
            DiffLineKind::Addition,
            20,
            DiffTheme::default(),
            0,
        );
        let number = spans
            .iter()
            .find(|span| span.content.as_ref() == "2")
            .expect("number span should be split out for inline emphasis");

        assert_eq!(
            number.style.fg,
            syntax_fg(SyntaxClass::Number, DiffTheme::default())
        );
        assert_eq!(
            number.style.bg,
            Some(DiffTheme::default().addition_inline_bg)
        );
        assert!(number.style.add_modifier.contains(Modifier::BOLD));
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

        let fresh = syntax_key_with_generation(1, 0);
        queue
            .try_push(syntax_job(fresh), SyntaxPriority::Visible)
            .unwrap();
        assert_eq!(queue.try_pop().map(|job| job.key), Some(fresh));
    }

    #[test]
    fn oversized_hunks_fall_back_to_plain_diff_text() {
        let limits = SyntaxLimits::default();
        let text = "x".repeat(limits.max_line_bytes);
        let line_count = (limits.max_source_bytes / limits.max_line_bytes) + 2;
        let lines = (0..line_count)
            .map(|index| DiffLine {
                kind: DiffLineKind::Context,
                old_line: Some(index + 1),
                new_line: Some(index + 1),
                text: text.clone(),
            })
            .collect::<Vec<_>>();

        assert_eq!(
            build_hunk_source(&lines, DiffSide::New, limits).unwrap_err(),
            SyntaxSkipReason::TooLarge
        );
    }

    #[test]
    fn oversized_lines_disable_hunk_highlighting() {
        let limits = SyntaxLimits::default();
        let lines = vec![
            DiffLine {
                kind: DiffLineKind::Context,
                old_line: Some(1),
                new_line: Some(1),
                text: "x".repeat(limits.max_line_bytes + 1),
            },
            DiffLine {
                kind: DiffLineKind::Context,
                old_line: Some(2),
                new_line: Some(2),
                text: "let value = 1;".to_owned(),
            },
        ];

        assert_eq!(
            build_hunk_source(&lines, DiffSide::New, limits).unwrap_err(),
            SyntaxSkipReason::TooLarge
        );
    }

    #[test]
    fn hunk_source_excludes_diff_meta_lines_and_preserves_empty_lines() {
        let lines = vec![
            DiffLine {
                kind: DiffLineKind::Context,
                old_line: Some(1),
                new_line: Some(1),
                text: "let a = 1;".to_owned(),
            },
            DiffLine {
                kind: DiffLineKind::Meta,
                old_line: None,
                new_line: None,
                text: "\\ No newline at end of file".to_owned(),
            },
            DiffLine {
                kind: DiffLineKind::Addition,
                old_line: None,
                new_line: Some(2),
                text: String::new(),
            },
        ];

        let source = build_hunk_source(&lines, DiffSide::New, SyntaxLimits::default()).unwrap();

        assert_eq!(source.text, "let a = 1;\n");
        assert_eq!(source.line_map, vec![Some(0), None, Some(1)]);
        assert_eq!(source.source_lines, 2);
    }

    #[test]
    fn hunk_source_keeps_single_line_without_trailing_newline_marker() {
        let lines = vec![DiffLine {
            kind: DiffLineKind::Addition,
            old_line: None,
            new_line: Some(1),
            text: "let value = 1;".to_owned(),
        }];

        let source = build_hunk_source(&lines, DiffSide::New, SyntaxLimits::default()).unwrap();

        assert_eq!(source.text, "let value = 1;");
        assert_eq!(source.line_map, vec![Some(0)]);
        assert_eq!(source.source_lines, 1);
    }

    #[test]
    fn hunk_source_preserves_leading_empty_lines() {
        let lines = vec![
            DiffLine {
                kind: DiffLineKind::Addition,
                old_line: None,
                new_line: Some(1),
                text: String::new(),
            },
            DiffLine {
                kind: DiffLineKind::Addition,
                old_line: None,
                new_line: Some(2),
                text: "let value = 1;".to_owned(),
            },
        ];

        let source = build_hunk_source(&lines, DiffSide::New, SyntaxLimits::default()).unwrap();

        assert_eq!(source.text, "\nlet value = 1;");
        assert_eq!(source.line_map, vec![Some(0), Some(1)]);
        assert_eq!(source.source_lines, 2);
    }

    #[test]
    fn full_file_line_map_uses_absolute_line_numbers() {
        let lines = vec![
            DiffLine {
                kind: DiffLineKind::Deletion,
                old_line: Some(10),
                new_line: None,
                text: "old".to_owned(),
            },
            DiffLine {
                kind: DiffLineKind::Addition,
                old_line: None,
                new_line: Some(11),
                text: "new".to_owned(),
            },
            DiffLine {
                kind: DiffLineKind::Context,
                old_line: Some(12),
                new_line: Some(12),
                text: "same".to_owned(),
            },
        ];

        assert_eq!(
            build_full_file_line_map(&lines, DiffSide::Old).unwrap(),
            vec![Some(9), None, Some(11)]
        );
        assert_eq!(
            build_full_file_line_map(&lines, DiffSide::New).unwrap(),
            vec![None, Some(10), Some(11)]
        );
    }

    #[test]
    fn full_file_sources_cover_diff_modes_and_statuses() {
        let repo = std::env::temp_dir();
        let file = hz_diff::DiffFile {
            old_path: Some("old.rs".to_owned()),
            new_path: Some("new.rs".to_owned()),
            status: hz_diff::FileStatus::Renamed,
            hunks: Vec::new(),
            additions: 0,
            deletions: 0,
            is_binary: false,
        };

        assert_eq!(
            full_file_source(&repo, &DiffOptions::default(), &file, DiffSide::Old)
                .unwrap()
                .kind,
            FullFileSourceKind::GitRevision {
                rev: "HEAD".to_owned(),
                path: "old.rs".to_owned(),
            }
        );
        assert_eq!(
            full_file_source(&repo, &DiffOptions::default(), &file, DiffSide::New)
                .unwrap()
                .kind,
            FullFileSourceKind::Worktree {
                path: "new.rs".to_owned(),
            }
        );

        let staged = DiffOptions {
            scope: DiffScope::Staged,
            ..DiffOptions::default()
        };
        assert_eq!(
            full_file_source(&repo, &staged, &file, DiffSide::New)
                .unwrap()
                .kind,
            FullFileSourceKind::GitIndex {
                path: "new.rs".to_owned(),
            }
        );

        let unstaged = DiffOptions {
            scope: DiffScope::Unstaged,
            ..DiffOptions::default()
        };
        assert_eq!(
            full_file_source(&repo, &unstaged, &file, DiffSide::Old)
                .unwrap()
                .kind,
            FullFileSourceKind::GitIndex {
                path: "old.rs".to_owned(),
            }
        );

        let base = DiffOptions {
            source: DiffSource::Base("main".to_owned()),
            ..DiffOptions::default()
        };
        assert_eq!(
            full_file_source(&repo, &base, &file, DiffSide::Old)
                .unwrap()
                .kind,
            FullFileSourceKind::GitMergeBase {
                base: "main".to_owned(),
                head: "HEAD".to_owned(),
                path: "old.rs".to_owned(),
            }
        );

        let range = DiffOptions {
            source: DiffSource::Range {
                left: "left".to_owned(),
                right: "right".to_owned(),
            },
            ..DiffOptions::default()
        };
        assert_eq!(
            full_file_source(&repo, &range, &file, DiffSide::New)
                .unwrap()
                .kind,
            FullFileSourceKind::GitRevision {
                rev: "right".to_owned(),
                path: "new.rs".to_owned(),
            }
        );

        let patch = DiffOptions {
            source: DiffSource::Patch(hz_diff::PatchSource::Stdin(Arc::from(""))),
            ..DiffOptions::default()
        };
        assert!(full_file_source(&repo, &patch, &file, DiffSide::New).is_none());

        let deleted = hz_diff::DiffFile {
            new_path: None,
            status: hz_diff::FileStatus::Deleted,
            ..file.clone()
        };
        assert!(
            full_file_source(&repo, &DiffOptions::default(), &deleted, DiffSide::New).is_none()
        );
    }

    #[test]
    fn branch_full_file_source_uses_merge_base_and_head_revision() {
        let repo = std::env::temp_dir();
        let file = hz_diff::DiffFile {
            old_path: Some("old.rs".to_owned()),
            new_path: Some("new.rs".to_owned()),
            status: hz_diff::FileStatus::Modified,
            hunks: Vec::new(),
            additions: 0,
            deletions: 0,
            is_binary: false,
        };
        let base = "origin/main".to_owned();
        let head = "feature/full-file".to_owned();
        let branch = DiffOptions {
            source: DiffSource::Branch {
                base: base.clone(),
                head: head.clone(),
            },
            scope: DiffScope::All,
            ..DiffOptions::default()
        };

        assert_eq!(
            full_file_source(&repo, &branch, &file, DiffSide::Old)
                .unwrap()
                .kind,
            FullFileSourceKind::GitMergeBase {
                base,
                head: head.clone(),
                path: "old.rs".to_owned(),
            }
        );
        assert_eq!(
            full_file_source(&repo, &branch, &file, DiffSide::New)
                .unwrap()
                .kind,
            FullFileSourceKind::GitRevision {
                rev: head,
                path: "new.rs".to_owned(),
            }
        );
    }

    #[test]
    fn full_file_source_loads_worktree_index_and_revision_contents() {
        let repo = temp_test_dir("full-file-source");
        fs::create_dir_all(&repo).expect("repo directory should be created");
        git(&repo, &["init", "-q"]);
        git(&repo, &["config", "user.email", "test@example.com"]);
        git(&repo, &["config", "user.name", "Test"]);

        fs::write(repo.join("file.rs"), "fn old() {}\n").expect("old file should be written");
        git(&repo, &["add", "file.rs"]);
        git(&repo, &["commit", "-q", "-m", "init"]);

        fs::write(repo.join("file.rs"), "fn new() {}\n").expect("new file should be written");
        assert_eq!(
            load_full_file_source(&FullFileSource {
                repo: repo.clone(),
                kind: FullFileSourceKind::GitRevision {
                    rev: "HEAD".to_owned(),
                    path: "file.rs".to_owned(),
                },
            })
            .unwrap(),
            "fn old() {}\n"
        );
        assert_eq!(
            load_full_file_source(&FullFileSource {
                repo: repo.clone(),
                kind: FullFileSourceKind::Worktree {
                    path: "file.rs".to_owned(),
                },
            })
            .unwrap(),
            "fn new() {}\n"
        );

        git(&repo, &["add", "file.rs"]);
        assert_eq!(
            load_full_file_source(&FullFileSource {
                repo: repo.clone(),
                kind: FullFileSourceKind::GitIndex {
                    path: "file.rs".to_owned(),
                },
            })
            .unwrap(),
            "fn new() {}\n"
        );

        fs::remove_dir_all(repo).expect("repo directory should be removed");
    }

    #[test]
    fn queue_close_wakes_blocked_pop() {
        let queue = SyntaxWorkerQueue::new(8, 0);
        let worker_queue = queue.clone();
        let worker = thread::spawn(move || worker_queue.pop());

        queue.close();

        assert!(worker.join().unwrap().is_none());
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

    fn changeset_with_line_text(text: &str) -> Changeset {
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
                    old_count: 1,
                    new_start: 1,
                    new_count: 1,
                    lines: vec![DiffLine {
                        kind: DiffLineKind::Context,
                        old_line: Some(1),
                        new_line: Some(1),
                        text: text.to_owned(),
                    }],
                }],
                additions: 0,
                deletions: 0,
                is_binary: false,
            }],
            raw_patch: String::new(),
        }
    }

    fn syntax_key(file: usize) -> SyntaxKey {
        syntax_key_with_generation(0, file)
    }

    fn syntax_key_with_generation(generation: u64, file: usize) -> SyntaxKey {
        SyntaxKey {
            source: SyntaxSourceId {
                generation,
                file,
                side: DiffSide::New,
                kind: SyntaxSourceKind::HunkSide { hunk: 0 },
            },
            language_hash: 1,
            theme_id: SYNTAX_THEME_ID,
        }
    }

    fn syntax_job(key: SyntaxKey) -> SyntaxJob {
        SyntaxJob {
            key,
            language: "rust".to_owned(),
            source: SyntaxJobSource::Hunk(HunkSource {
                text: "fn main() {}".to_owned(),
                line_map: vec![Some(0)],
                source_lines: 1,
            }),
            limits: SyntaxLimits::default(),
        }
    }

    fn range_texts(text: &str, ranges: &[InlineRange]) -> Vec<String> {
        ranges
            .iter()
            .map(|range| text[range.byte_start..range.byte_end].to_owned())
            .collect()
    }

    fn line_text(line: &Line<'_>) -> String {
        span_text(&line.spans)
    }

    fn span_text(spans: &[Span<'_>]) -> String {
        spans.iter().map(|span| span.content.as_ref()).collect()
    }

    fn temp_test_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "hz-tui-{name}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time should be after unix epoch")
                .as_nanos()
        ))
    }

    fn git(repo: &Path, args: &[&str]) {
        let output = Command::new("git")
            .current_dir(repo)
            .args(args)
            .output()
            .expect("git should run");
        assert!(
            output.status.success(),
            "git failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
