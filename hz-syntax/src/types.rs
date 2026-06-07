use std::{
    collections::{BTreeSet, HashMap},
    path::PathBuf,
};

use crate::{
    cache_dir, detect_language_name, enabled_language_set_for_mode, has_highlights,
    highlighted_text_from_events, highlights_query, installed_language_set, is_language_trusted,
    language_vec_to_set, load_config, load_settings, normalize_language_name, trusted_language_set,
};
use hz_core::{HzError, HzResult};
use serde::{Deserialize, Serialize};
use tree_sitter_highlight::{HighlightConfiguration, Highlighter};
use tree_sitter_language_pack::LanguageRegistry;

pub(crate) const CONFIG_DIR: &str = "hz";
pub(crate) const CONFIG_FILE: &str = "tree-sitter.json";
pub(crate) const SETTINGS_FILE: &str = "config.toml";
pub(crate) const LEGACY_SETTINGS_FILE: &str = "syntax.toml";
pub(crate) const COLORSCHEME_DIR: &str = "colorscheme";
pub(crate) const LANGUAGE_PACK_VERSION: &str = "1.9.0-rc.18";
// Lockfile copied from the matching tree-sitter-language-pack GitHub release.
// Non-bundled parser installs seed this manifest before any download so the
// release bundle hash is pinned by hz instead of trusted on first use.
pub(crate) const TRUSTED_PARSER_MANIFEST: &str = include_str!("tree_sitter_parsers_lock.json");
pub(crate) const TRUSTED_PARSER_MANIFEST_SHA256: &str =
    "be3db342638e23ceac0844de831ce86baa2d7dacf2666fd42f42619d831da115";
pub(crate) const ARTIFACT_SOURCE: &str = "github:kreuzberg-dev/tree-sitter-language-pack@parsers.json-sha256:be3db342638e23ceac0844de831ce86baa2d7dacf2666fd42f42619d831da115";

pub const DEFAULT_MAX_HIGHLIGHT_SOURCE_BYTES: usize = 1024 * 1024;
pub const DEFAULT_MAX_HIGHLIGHT_LINE_BYTES: usize = 8 * 1024;
pub const DEFAULT_HIGHLIGHT_CACHE_ENTRIES: usize = 512;
pub const DEFAULT_HIGHLIGHT_QUEUE_ENTRIES: usize = 512;
pub const DEFAULT_HIGHLIGHT_PREFETCH_VIEWPORTS: usize = 1;

pub(crate) const CORE_LANGUAGES: &[&str] = &[
    "rust",
    "c",
    "cpp",
    "python",
    "typescript",
    "javascript",
    "tsx",
    "zig",
    "cmake",
    "bash",
    "toml",
    "json",
    "yaml",
    "markdown",
];

pub(crate) const LANGUAGE_ALIASES: &[(&str, &str)] = &[
    ("bazel", "starlark"),
    ("c++", "cpp"),
    ("cc", "cpp"),
    ("c#", "csharp"),
    ("cxx", "cpp"),
    ("gradle", "groovy"),
    ("ignorefile", "gitignore"),
    ("js", "javascript"),
    ("lisp", "commonlisp"),
    ("node", "javascript"),
    ("python3", "python"),
    ("makefile", "make"),
    ("shell", "bash"),
    ("sh", "bash"),
    ("ts", "typescript"),
];

pub(crate) const BASENAME_LANGUAGES: &[(&str, &str)] = &[
    ("build", "starlark"),
    ("build.bazel", "starlark"),
    ("workspace", "starlark"),
    ("workspace.bazel", "starlark"),
    ("module.bazel", "starlark"),
    ("dockerfile", "dockerfile"),
    ("makefile", "make"),
    ("gnumakefile", "make"),
    ("bsdmakefile", "make"),
    ("cmakelists.txt", "cmake"),
    (".bazelrc", "starlark"),
    (".clang-format", "yaml"),
    (".clang-tidy", "yaml"),
    (".dockerignore", "gitignore"),
    (".gitignore", "gitignore"),
];

pub(crate) const HIGHLIGHT_NAMES: &[&str] = &[
    "attribute",
    "boolean",
    "character",
    "comment",
    "constant",
    "constant.builtin",
    "constructor",
    "embedded",
    "function",
    "function.builtin",
    "function.method",
    "keyword",
    "label",
    "module",
    "namespace",
    "number",
    "operator",
    "property",
    "property.builtin",
    "punctuation",
    "punctuation.bracket",
    "punctuation.delimiter",
    "punctuation.special",
    "string",
    "string.escape",
    "string.special",
    "tag",
    "type",
    "type.builtin",
    "variable",
    "variable.builtin",
    "variable.parameter",
];

pub(crate) const ASM_HIGHLIGHTS_QUERY: &str = r#"
(line_comment) @comment
(meta kind: (meta_ident) @keyword)
(label (ident) @label)
(instruction kind: (word) @function)
(reg (word) @variable.builtin)
(int) @number
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SyntaxClass {
    Attribute,
    Comment,
    Constant,
    Constructor,
    Function,
    Keyword,
    Label,
    Module,
    Number,
    Operator,
    Property,
    Punctuation,
    String,
    Tag,
    Type,
    Variable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxSegment {
    pub byte_start: usize,
    pub byte_end: usize,
    pub text: String,
    pub class: Option<SyntaxClass>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HighlightedLine {
    pub segments: Vec<SyntaxSegment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HighlightedText {
    pub lines: Vec<HighlightedLine>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StoredSyntaxConfig {
    #[serde(default)]
    pub(crate) languages: Vec<String>,
    #[serde(default)]
    pub(crate) parsers: Vec<StoredParserArtifact>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StoredParserArtifact {
    pub(crate) language: String,
    pub(crate) version: String,
    pub(crate) path: PathBuf,
    pub(crate) sha256: String,
    pub(crate) installed_at_unix: u64,
    pub(crate) source: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub(crate) struct StoredSyntaxSettings {
    pub(crate) mode: Option<SyntaxMode>,
    pub(crate) colorscheme: Option<StoredSyntaxThemeConfig>,
    pub(crate) theme: Option<StoredSyntaxThemeConfig>,
    #[serde(default)]
    pub(crate) colors: ColorOverrides,
    #[serde(default, flatten)]
    pub(crate) color_overrides: ColorOverrides,
    #[serde(default, alias = "background_transparent", alias = "transparent_bg")]
    pub(crate) transparent_background: bool,
    #[serde(default)]
    pub(crate) diff: StoredDiffSettings,
    #[serde(default)]
    pub(crate) limits: StoredSyntaxLimits,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub(crate) struct StoredDiffSettings {
    pub(crate) line_background: Option<DiffBackground>,
    pub(crate) gutter_background: Option<DiffGutterBackground>,
    pub(crate) inline_background: Option<DiffBackground>,
    #[serde(alias = "word_background", alias = "word_diff_background")]
    pub(crate) word_background: Option<DiffBackground>,
    pub(crate) sign_style: Option<DiffSignStyle>,
    #[serde(
        alias = "context_lines",
        alias = "context_expand",
        alias = "expand_context"
    )]
    pub(crate) context_expansion: Option<StoredDiffContextExpansion>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub(crate) enum StoredDiffContextExpansion {
    Lines(usize),
    Mode(StoredDiffContextExpansionMode),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum StoredDiffContextExpansionMode {
    Full,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub(crate) enum StoredSyntaxThemeConfig {
    Name(String),
    Table(StoredSyntaxThemeTable),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub(crate) struct StoredSyntaxThemeTable {
    pub(crate) source: Option<SyntaxThemeSource>,
    pub(crate) name: Option<String>,
    pub(crate) path: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub(crate) struct StoredSyntaxLimits {
    pub(crate) max_source_kib: Option<usize>,
    pub(crate) max_line_kib: Option<usize>,
    pub(crate) cache_entries: Option<usize>,
    pub(crate) queue_entries: Option<usize>,
    pub(crate) prefetch_viewports: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxSettings {
    pub mode: SyntaxMode,
    pub theme: SyntaxThemeConfig,
    pub colors: ColorOverrides,
    pub transparent_background: bool,
    pub diff: DiffSettings,
    pub limits: SyntaxLimits,
}

impl Default for SyntaxSettings {
    fn default() -> Self {
        Self {
            mode: SyntaxMode::Enabled,
            theme: SyntaxThemeConfig::default(),
            colors: ColorOverrides::default(),
            transparent_background: false,
            diff: DiffSettings::default(),
            limits: SyntaxLimits::default(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct ColorOverrides {
    #[serde(alias = "background")]
    pub bg: Option<String>,
    #[serde(alias = "foreground")]
    pub fg: Option<String>,
    pub header: Option<String>,
    pub file: Option<String>,
    pub hunk: Option<String>,
    pub notice: Option<String>,
    pub muted: Option<String>,
    pub gutter_bg: Option<String>,
    pub empty_diff: Option<String>,
    pub search_match_fg: Option<String>,
    pub search_match_bg: Option<String>,
    pub addition_fg: Option<String>,
    pub addition_gutter_bg: Option<String>,
    pub addition_bg: Option<String>,
    pub addition_inline_bg: Option<String>,
    pub deletion_fg: Option<String>,
    pub deletion_gutter_bg: Option<String>,
    pub deletion_bg: Option<String>,
    pub deletion_inline_bg: Option<String>,
    pub attribute: Option<String>,
    pub comment: Option<String>,
    pub constant: Option<String>,
    pub constructor: Option<String>,
    pub function: Option<String>,
    pub keyword: Option<String>,
    pub label: Option<String>,
    pub module: Option<String>,
    pub number: Option<String>,
    pub operator: Option<String>,
    pub property: Option<String>,
    pub punctuation: Option<String>,
    pub string: Option<String>,
    pub tag: Option<String>,
    pub r#type: Option<String>,
    pub variable: Option<String>,
}

impl ColorOverrides {
    pub(crate) fn overlay(self, overrides: Self) -> Self {
        Self {
            bg: overrides.bg.or(self.bg),
            fg: overrides.fg.or(self.fg),
            header: overrides.header.or(self.header),
            file: overrides.file.or(self.file),
            hunk: overrides.hunk.or(self.hunk),
            notice: overrides.notice.or(self.notice),
            muted: overrides.muted.or(self.muted),
            gutter_bg: overrides.gutter_bg.or(self.gutter_bg),
            empty_diff: overrides.empty_diff.or(self.empty_diff),
            search_match_fg: overrides.search_match_fg.or(self.search_match_fg),
            search_match_bg: overrides.search_match_bg.or(self.search_match_bg),
            addition_fg: overrides.addition_fg.or(self.addition_fg),
            addition_gutter_bg: overrides.addition_gutter_bg.or(self.addition_gutter_bg),
            addition_bg: overrides.addition_bg.or(self.addition_bg),
            addition_inline_bg: overrides.addition_inline_bg.or(self.addition_inline_bg),
            deletion_fg: overrides.deletion_fg.or(self.deletion_fg),
            deletion_gutter_bg: overrides.deletion_gutter_bg.or(self.deletion_gutter_bg),
            deletion_bg: overrides.deletion_bg.or(self.deletion_bg),
            deletion_inline_bg: overrides.deletion_inline_bg.or(self.deletion_inline_bg),
            attribute: overrides.attribute.or(self.attribute),
            comment: overrides.comment.or(self.comment),
            constant: overrides.constant.or(self.constant),
            constructor: overrides.constructor.or(self.constructor),
            function: overrides.function.or(self.function),
            keyword: overrides.keyword.or(self.keyword),
            label: overrides.label.or(self.label),
            module: overrides.module.or(self.module),
            number: overrides.number.or(self.number),
            operator: overrides.operator.or(self.operator),
            property: overrides.property.or(self.property),
            punctuation: overrides.punctuation.or(self.punctuation),
            string: overrides.string.or(self.string),
            tag: overrides.tag.or(self.tag),
            r#type: overrides.r#type.or(self.r#type),
            variable: overrides.variable.or(self.variable),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffSettings {
    pub line_background: DiffBackground,
    pub gutter_background: DiffGutterBackground,
    pub inline_background: DiffBackground,
    pub sign_style: DiffSignStyle,
    pub context_expansion: DiffContextExpansion,
}

impl Default for DiffSettings {
    fn default() -> Self {
        Self {
            line_background: DiffBackground::Subtle,
            gutter_background: DiffGutterBackground::Delta,
            inline_background: DiffBackground::Strong,
            sign_style: DiffSignStyle::Bold,
            context_expansion: DiffContextExpansion::Lines(20),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffContextExpansion {
    Lines(usize),
    Full,
}

impl DiffContextExpansion {
    pub fn expand_count(self, available: usize) -> usize {
        match self {
            Self::Lines(lines) => available.min(lines.max(1)),
            Self::Full => available,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiffBackground {
    None,
    #[default]
    Subtle,
    Strong,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiffGutterBackground {
    Base,
    #[default]
    Delta,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiffSignStyle {
    Normal,
    #[default]
    Bold,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SyntaxMode {
    #[default]
    Enabled,
    Builtin,
    All,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxThemeConfig {
    pub source: SyntaxThemeSource,
    pub name: Option<String>,
    pub path: Option<PathBuf>,
}

impl Default for SyntaxThemeConfig {
    fn default() -> Self {
        Self {
            source: SyntaxThemeSource::Builtin,
            name: Some("system".to_owned()),
            path: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SyntaxThemeSource {
    #[default]
    Builtin,
    Ansi,
    Base16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyntaxLimits {
    pub max_source_bytes: usize,
    pub max_line_bytes: usize,
    pub cache_entries: usize,
    pub queue_entries: usize,
    pub prefetch_viewports: usize,
}

impl Default for SyntaxLimits {
    fn default() -> Self {
        Self {
            max_source_bytes: DEFAULT_MAX_HIGHLIGHT_SOURCE_BYTES,
            max_line_bytes: DEFAULT_MAX_HIGHLIGHT_LINE_BYTES,
            cache_entries: DEFAULT_HIGHLIGHT_CACHE_ENTRIES,
            queue_entries: DEFAULT_HIGHLIGHT_QUEUE_ENTRIES,
            prefetch_viewports: DEFAULT_HIGHLIGHT_PREFETCH_VIEWPORTS,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxParserArtifact {
    pub language: String,
    pub version: String,
    pub path: PathBuf,
    pub sha256: String,
    pub installed_at_unix: u64,
    pub source: String,
}

impl From<&StoredParserArtifact> for SyntaxParserArtifact {
    fn from(artifact: &StoredParserArtifact) -> Self {
        Self {
            language: artifact.language.clone(),
            version: artifact.version.clone(),
            path: artifact.path.clone(),
            sha256: artifact.sha256.clone(),
            installed_at_unix: artifact.installed_at_unix,
            source: artifact.source.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxLanguageStatus {
    pub language: String,
    pub enabled: bool,
    pub installed: bool,
    pub trusted: bool,
    pub has_highlights: bool,
    pub version: Option<String>,
    pub artifact: Option<SyntaxParserArtifact>,
    pub source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxAddResult {
    pub added: Vec<String>,
    pub already_enabled: Vec<String>,
    pub without_highlights: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SyntaxAvailableFilter {
    #[default]
    All,
    Installed,
    Enabled,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyntaxUpdateResult {
    pub updated: Vec<String>,
    pub bundled: Vec<String>,
    pub not_installed: Vec<String>,
    pub unavailable: Vec<String>,
    pub without_highlights: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxRemoveResult {
    pub removed: Vec<String>,
    pub missing: Vec<String>,
    pub cache_deleted: Vec<String>,
    pub cache_missing: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxCleanResult {
    pub parser_artifacts_removed: usize,
    pub artifact_records_removed: usize,
    pub enabled_languages_kept: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxDoctorIssue {
    pub language: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxDoctorReport {
    pub statuses: Vec<SyntaxLanguageStatus>,
    pub issues: Vec<SyntaxDoctorIssue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxLanguageSet {
    pub(crate) enabled: BTreeSet<String>,
    pub(crate) installed: BTreeSet<String>,
    pub(crate) trusted: BTreeSet<String>,
}

impl SyntaxLanguageSet {
    pub fn load() -> HzResult<Self> {
        let settings = load_settings()?;
        Self::load_with_mode(settings.mode)
    }

    pub fn load_with_mode(mode: SyntaxMode) -> HzResult<Self> {
        let config = load_config()?;
        let installed = installed_language_set();
        let trusted = trusted_language_set(&installed, &config);
        Ok(Self {
            enabled: enabled_language_set_for_mode(mode, &config, &trusted),
            trusted,
            installed,
        })
    }

    pub fn from_enabled_languages(languages: &[String]) -> Self {
        let installed = installed_language_set();
        let config = load_config().unwrap_or_default();
        Self {
            enabled: language_vec_to_set(languages),
            trusted: trusted_language_set(&installed, &config),
            installed,
        }
    }

    pub fn is_empty(&self) -> bool {
        !self
            .enabled
            .iter()
            .any(|language| self.is_highlight_ready(language))
    }

    pub fn language_for_path(&self, path: &str) -> Option<String> {
        let language = normalize_language_name(detect_language_name(path)?.to_owned());
        self.is_highlight_ready(&language).then_some(language)
    }

    pub fn is_highlight_ready(&self, language: &str) -> bool {
        self.enabled.contains(language)
            && (self.trusted.contains(language) || tree_sitter_language_pack::has_parser(language))
            && has_highlights(language)
    }
}

pub struct SyntaxHighlighter {
    pub(crate) registry: LanguageRegistry,
    pub(crate) highlighter: Highlighter,
    pub(crate) configs: HashMap<String, HighlightConfiguration>,
    pub(crate) trusted_languages: BTreeSet<String>,
}

impl Default for SyntaxHighlighter {
    fn default() -> Self {
        let registry = LanguageRegistry::new();
        if let Ok(cache) = cache_dir() {
            registry.add_extra_libs_dir(PathBuf::from(cache));
        }

        Self {
            registry,
            highlighter: Highlighter::new(),
            configs: HashMap::new(),
            trusted_languages: BTreeSet::new(),
        }
    }
}

impl SyntaxHighlighter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn highlight(&mut self, language: &str, source: &str) -> HzResult<HighlightedText> {
        let language = normalize_language_name(language.to_owned());
        if !self.ensure_language_trusted(&language) {
            return Err(HzError::Usage(format!(
                "tree-sitter language '{language}' is not trusted; run `hz ts add {language}`"
            )));
        }

        self.ensure_config(&language)?;
        let config = self
            .configs
            .get(&language)
            .ok_or_else(|| HzError::Usage(format!("failed to cache {language} highlights")))?;
        let highlights = self
            .highlighter
            .highlight(config, source.as_bytes(), None, |_| None)
            .map_err(|error| HzError::Usage(format!("failed to highlight {language}: {error}")))?;
        highlighted_text_from_events(source, highlights)
    }

    pub(crate) fn ensure_language_trusted(&mut self, language: &str) -> bool {
        if self.trusted_languages.contains(language) {
            return true;
        }
        if !is_language_trusted(language) {
            return false;
        }
        self.trusted_languages.insert(language.to_owned());
        true
    }

    pub(crate) fn ensure_config(&mut self, language: &str) -> HzResult<()> {
        if !self.configs.contains_key(language) {
            let language_fn = self
                .registry
                .get_language(language)
                .map_err(|error| HzError::Usage(format!("failed to load {language}: {error}")))?;
            let highlights_query = highlights_query(language)
                .ok_or_else(|| HzError::Usage(format!("{language} has no highlights query")))?;
            let mut config =
                HighlightConfiguration::new(language_fn, language, highlights_query, "", "")
                    .map_err(|error| {
                        HzError::Usage(format!(
                            "failed to configure {language} highlights: {error}"
                        ))
                    })?;
            config.configure(HIGHLIGHT_NAMES);
            self.configs.insert(language.to_owned(), config);
        }
        Ok(())
    }
}
