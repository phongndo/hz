use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    env, fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use hz_core::{HzError, HzResult};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};
use tree_sitter_language_pack::LanguageRegistry;

const CONFIG_DIR: &str = "hz";
const CONFIG_FILE: &str = "tree-sitter.json";
const SETTINGS_FILE: &str = "config.toml";
const LEGACY_SETTINGS_FILE: &str = "syntax.toml";
const COLORSCHEME_DIR: &str = "colorscheme";
const LANGUAGE_PACK_VERSION: &str = "1.9.0-rc.17";
const ARTIFACT_SOURCE: &str = "github:kreuzberg-dev/tree-sitter-language-pack";

pub const DEFAULT_MAX_HIGHLIGHT_SOURCE_BYTES: usize = 128 * 1024;
pub const DEFAULT_MAX_HIGHLIGHT_LINE_BYTES: usize = 8 * 1024;
pub const DEFAULT_HIGHLIGHT_CACHE_ENTRIES: usize = 512;
pub const DEFAULT_HIGHLIGHT_QUEUE_ENTRIES: usize = 512;
pub const DEFAULT_HIGHLIGHT_PREFETCH_VIEWPORTS: usize = 1;

const CORE_LANGUAGES: &[&str] = &[
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

const LANGUAGE_ALIASES: &[(&str, &str)] = &[
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

const BASENAME_LANGUAGES: &[(&str, &str)] = &[
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

const HIGHLIGHT_NAMES: &[&str] = &[
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

const ASM_HIGHLIGHTS_QUERY: &str = r#"
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
struct StoredSyntaxConfig {
    #[serde(default)]
    languages: Vec<String>,
    #[serde(default)]
    parsers: Vec<StoredParserArtifact>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct StoredParserArtifact {
    language: String,
    version: String,
    path: PathBuf,
    sha256: String,
    installed_at_unix: u64,
    source: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
struct StoredSyntaxSettings {
    mode: Option<SyntaxMode>,
    colorscheme: Option<StoredSyntaxThemeConfig>,
    theme: Option<StoredSyntaxThemeConfig>,
    #[serde(default)]
    colors: ColorOverrides,
    #[serde(default, flatten)]
    color_overrides: ColorOverrides,
    #[serde(default, alias = "background_transparent", alias = "transparent_bg")]
    transparent_background: bool,
    #[serde(default)]
    diff: StoredDiffSettings,
    #[serde(default)]
    limits: StoredSyntaxLimits,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
struct StoredDiffSettings {
    line_background: Option<DiffBackground>,
    gutter_background: Option<DiffGutterBackground>,
    inline_background: Option<DiffBackground>,
    #[serde(alias = "word_background", alias = "word_diff_background")]
    word_background: Option<DiffBackground>,
    sign_style: Option<DiffSignStyle>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
enum StoredSyntaxThemeConfig {
    Name(String),
    Table(StoredSyntaxThemeTable),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
struct StoredSyntaxThemeTable {
    source: Option<SyntaxThemeSource>,
    name: Option<String>,
    path: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
struct StoredSyntaxLimits {
    max_source_kib: Option<usize>,
    max_line_kib: Option<usize>,
    cache_entries: Option<usize>,
    queue_entries: Option<usize>,
    prefetch_viewports: Option<usize>,
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
    fn overlay(self, overrides: Self) -> Self {
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
}

impl Default for DiffSettings {
    fn default() -> Self {
        Self {
            line_background: DiffBackground::Subtle,
            gutter_background: DiffGutterBackground::Delta,
            inline_background: DiffBackground::Strong,
            sign_style: DiffSignStyle::Bold,
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
    enabled: BTreeSet<String>,
    installed: BTreeSet<String>,
    trusted: BTreeSet<String>,
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
    registry: LanguageRegistry,
    highlighter: Highlighter,
    configs: HashMap<String, HighlightConfiguration>,
    trusted_languages: BTreeSet<String>,
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

    fn ensure_language_trusted(&mut self, language: &str) -> bool {
        if self.trusted_languages.contains(language) {
            return true;
        }
        if !is_language_trusted(language) {
            return false;
        }
        self.trusted_languages.insert(language.to_owned());
        true
    }

    fn ensure_config(&mut self, language: &str) -> HzResult<()> {
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

pub fn config_path() -> HzResult<PathBuf> {
    config_home().map(|path| path.join(CONFIG_DIR).join(CONFIG_FILE))
}

pub fn settings_path() -> HzResult<PathBuf> {
    config_home().map(|path| path.join(CONFIG_DIR).join(SETTINGS_FILE))
}

fn legacy_settings_path() -> HzResult<PathBuf> {
    config_home().map(|path| path.join(CONFIG_DIR).join(LEGACY_SETTINGS_FILE))
}

pub fn colorscheme_dir() -> HzResult<PathBuf> {
    config_home().map(|path| path.join(CONFIG_DIR).join(COLORSCHEME_DIR))
}

pub fn load_settings() -> HzResult<SyntaxSettings> {
    let mut path = settings_path()?;
    if !path.exists() {
        let legacy_path = legacy_settings_path()?;
        if legacy_path.exists() {
            path = legacy_path;
        }
    }
    if !path.exists() {
        return Ok(SyntaxSettings::default());
    }

    let contents = fs::read_to_string(&path)?;
    parse_settings(&contents)
        .map_err(|error| HzError::Usage(format!("failed to parse {}: {error}", path.display())))
}

pub fn cache_dir() -> HzResult<String> {
    tree_sitter_language_pack::cache_dir()
        .map_err(|error| HzError::Usage(format!("failed to resolve tree-sitter cache: {error}")))
}

pub fn available_languages(filter: SyntaxAvailableFilter) -> HzResult<Vec<String>> {
    match filter {
        SyntaxAvailableFilter::All => {
            tree_sitter_language_pack::manifest_languages().map_err(|error| {
                HzError::Usage(format!("failed to list tree-sitter languages: {error}"))
            })
        }
        SyntaxAvailableFilter::Installed => Ok(local_parser_language_set().into_iter().collect()),
        SyntaxAvailableFilter::Enabled => enabled_languages(),
    }
}

pub fn enabled_languages() -> HzResult<Vec<String>> {
    Ok(enabled_language_set()?.into_iter().collect())
}

pub fn installed_languages() -> Vec<String> {
    installed_language_set().into_iter().collect()
}

pub fn language_statuses() -> HzResult<Vec<SyntaxLanguageStatus>> {
    let settings = load_settings()?;
    let config = load_config()?;
    let installed = installed_language_set();
    let trusted = trusted_language_set(&installed, &config);
    let enabled = enabled_language_set_for_mode(settings.mode, &config, &trusted);
    let artifacts = parser_artifact_map(&config);
    let pack_version = language_pack_version();
    let mut languages = enabled
        .union(&installed)
        .cloned()
        .collect::<BTreeSet<String>>();
    languages.extend(core_enabled_language_set());

    if languages.is_empty() {
        languages.extend(installed.iter().cloned());
    }

    Ok(languages
        .into_iter()
        .map(|language| {
            let built_in = tree_sitter_language_pack::has_parser(&language);
            let artifact = (!built_in)
                .then(|| artifacts.get(&language).map(SyntaxParserArtifact::from))
                .flatten();
            let artifact_source = artifact.as_ref().map(|artifact| artifact.source.clone());
            let artifact_version = artifact.as_ref().map(|artifact| artifact.version.clone());
            SyntaxLanguageStatus {
                enabled: enabled.contains(&language),
                installed: built_in || installed.contains(&language),
                trusted: built_in || trusted.contains(&language),
                has_highlights: has_highlights(&language),
                version: if built_in {
                    Some(pack_version.clone())
                } else {
                    artifact_version
                },
                source: if built_in {
                    Some("bundled".to_owned())
                } else {
                    artifact_source
                },
                artifact,
                language,
            }
        })
        .collect())
}

pub fn add_languages(languages: &[String]) -> HzResult<SyntaxAddResult> {
    if languages.is_empty() {
        return Err(HzError::Usage("provide at least one language".to_owned()));
    }

    let requested = normalize_language_names(languages);
    let mut config = load_config()?;
    let mut enabled = language_vec_to_set(&config.languages);
    let mut added = Vec::new();
    let mut already_enabled = Vec::new();
    let mut without_highlights = Vec::new();

    for language in requested {
        let artifact = install_language(&language)?;
        upsert_parser_artifact(&mut config, &language, artifact);

        if !has_highlights(&language) {
            without_highlights.push(language.clone());
        }

        if enabled.insert(language.clone()) {
            added.push(language);
        } else {
            already_enabled.push(language);
        }
    }

    config.languages = enabled.into_iter().collect();
    save_config(&config)?;

    Ok(SyntaxAddResult {
        added,
        already_enabled,
        without_highlights,
    })
}

pub fn update_languages(languages: &[String], all: bool) -> HzResult<SyntaxUpdateResult> {
    if all && !languages.is_empty() {
        return Err(HzError::Usage(
            "use `hz ts update --all` without language names".to_owned(),
        ));
    }
    if !all && languages.is_empty() {
        return Err(HzError::Usage(
            "provide at least one language or use --all".to_owned(),
        ));
    }

    let mut config = load_config()?;
    let configured = language_vec_to_set(&config.languages);
    let installed = installed_language_set();
    let requested = if all {
        update_all_language_set(&config, &installed)
    } else {
        normalize_language_names(languages)
    };
    let mut result = SyntaxUpdateResult::default();

    for language in requested {
        if !tree_sitter_language_pack::has_language(&language) {
            if all {
                result.unavailable.push(language);
                continue;
            }
            return Err(HzError::Usage(format!(
                "tree-sitter language '{language}' is not known"
            )));
        }

        if !has_highlights(&language) {
            result.without_highlights.push(language.clone());
        }

        if tree_sitter_language_pack::has_parser(&language) {
            result.bundled.push(language);
            continue;
        }

        if !installed.contains(&language) && !configured.contains(&language) {
            result.not_installed.push(language);
            continue;
        }

        remove_cached_language(&language)?;
        let artifact = install_language(&language)?;
        upsert_parser_artifact(&mut config, &language, artifact);
        result.updated.push(language);
    }

    if !result.updated.is_empty() {
        save_config(&config)?;
    }

    Ok(result)
}

pub fn remove_languages(languages: &[String]) -> HzResult<SyntaxRemoveResult> {
    if languages.is_empty() {
        return Err(HzError::Usage("provide at least one language".to_owned()));
    }

    let requested = normalize_language_names(languages);
    reject_core_language_removal(&requested)?;
    let mut config = load_config()?;
    let mut enabled = language_vec_to_set(&config.languages);
    let mut removed = Vec::new();
    let mut missing = Vec::new();
    let mut cache_deleted = Vec::new();
    let mut cache_missing = Vec::new();

    for language in &requested {
        if enabled.remove(language.as_str()) {
            removed.push(language.clone());
        } else {
            missing.push(language.clone());
        }
    }
    config
        .parsers
        .retain(|artifact| !requested.contains(&artifact.language));

    config.languages = enabled.into_iter().collect();
    save_config(&config)?;

    for language in requested {
        if remove_cached_language(&language)? {
            cache_deleted.push(language);
        } else {
            cache_missing.push(language);
        }
    }

    Ok(SyntaxRemoveResult {
        removed,
        missing,
        cache_deleted,
        cache_missing,
    })
}

pub fn clean_cache() -> HzResult<SyntaxCleanResult> {
    let parser_artifacts_removed = installed_language_set().len();
    let mut config = load_config()?;
    let artifact_records_removed = config.parsers.len();
    let enabled_languages_kept = language_vec_to_set(&config.languages).len();

    tree_sitter_language_pack::clean_cache()
        .map_err(|error| HzError::Usage(format!("failed to clean tree-sitter cache: {error}")))?;
    config.parsers.clear();
    save_config(&config)?;

    Ok(SyntaxCleanResult {
        parser_artifacts_removed,
        artifact_records_removed,
        enabled_languages_kept,
    })
}

pub fn doctor() -> HzResult<SyntaxDoctorReport> {
    let statuses = language_statuses()?;
    let issues = doctor_issues(&statuses);

    Ok(SyntaxDoctorReport { statuses, issues })
}

fn doctor_issues(statuses: &[SyntaxLanguageStatus]) -> Vec<SyntaxDoctorIssue> {
    let mut issues = Vec::new();

    for status in statuses {
        if !status.enabled {
            continue;
        }
        if !tree_sitter_language_pack::has_language(&status.language) {
            issues.push(SyntaxDoctorIssue {
                language: status.language.clone(),
                message: "enabled in config, but language is not known; run `hz ts rm`".to_owned(),
            });
            continue;
        }
        if !status.installed {
            issues.push(SyntaxDoctorIssue {
                language: status.language.clone(),
                message: "enabled in config, but parser cache file is missing; run `hz ts add`"
                    .to_owned(),
            });
            continue;
        }
        if !status.trusted {
            issues.push(SyntaxDoctorIssue {
                language: status.language.clone(),
                message:
                    "parser exists, but no matching trusted checksum is recorded; run `hz ts add`"
                        .to_owned(),
            });
            continue;
        }
        if !status.has_highlights {
            issues.push(SyntaxDoctorIssue {
                language: status.language.clone(),
                message: "parser is available, but no bundled highlights query exists; diff will render plain text"
                    .to_owned(),
            });
        }
        if let Err(error) = load_language_without_download(&status.language) {
            issues.push(SyntaxDoctorIssue {
                language: status.language.clone(),
                message: format!(
                    "parser cache exists, but failed to load without downloading: {error}"
                ),
            });
        }
    }

    issues
}

pub fn detect_language_from_path(path: &str) -> Option<String> {
    detect_language_name(path).map(|language| language.to_owned())
}

fn highlighted_text_from_events<'a>(
    source: &str,
    highlights: impl Iterator<Item = Result<HighlightEvent, tree_sitter_highlight::Error>> + 'a,
) -> HzResult<HighlightedText> {
    let line_count = source.split('\n').count();
    let mut lines = vec![HighlightedLine::default(); line_count];
    let mut line_index = 0;
    let mut stack = Vec::new();

    for event in highlights {
        match event
            .map_err(|error| HzError::Usage(format!("failed to highlight source: {error}")))?
        {
            HighlightEvent::HighlightStart(highlight) => stack.push(highlight.0),
            HighlightEvent::HighlightEnd => {
                stack.pop();
            }
            HighlightEvent::Source { start, end } => {
                let class = stack
                    .last()
                    .and_then(|index| HIGHLIGHT_NAMES.get(*index))
                    .and_then(|name| syntax_class(name));
                push_source_segment(
                    &mut lines,
                    &mut line_index,
                    start,
                    &source.as_bytes()[start..end],
                    class,
                );
            }
        }
    }

    Ok(HighlightedText { lines })
}

fn push_source_segment(
    lines: &mut [HighlightedLine],
    line_index: &mut usize,
    byte_start: usize,
    mut bytes: &[u8],
    class: Option<SyntaxClass>,
) {
    let mut offset = 0usize;
    while let Some(newline) = bytes.iter().position(|byte| *byte == b'\n') {
        push_line_segment(
            lines,
            *line_index,
            byte_start.saturating_add(offset),
            byte_start.saturating_add(offset).saturating_add(newline),
            &bytes[..newline],
            class,
        );
        *line_index = line_index
            .saturating_add(1)
            .min(lines.len().saturating_sub(1));
        offset = offset.saturating_add(newline + 1);
        bytes = &bytes[newline + 1..];
    }
    push_line_segment(
        lines,
        *line_index,
        byte_start.saturating_add(offset),
        byte_start
            .saturating_add(offset)
            .saturating_add(bytes.len()),
        bytes,
        class,
    );
}

fn push_line_segment(
    lines: &mut [HighlightedLine],
    line_index: usize,
    byte_start: usize,
    byte_end: usize,
    bytes: &[u8],
    class: Option<SyntaxClass>,
) {
    if bytes.is_empty() || line_index >= lines.len() {
        return;
    }

    let text = String::from_utf8_lossy(bytes).into_owned();
    let Some(last) = lines[line_index].segments.last_mut() else {
        lines[line_index].segments.push(SyntaxSegment {
            byte_start,
            byte_end,
            text,
            class,
        });
        return;
    };

    if last.class == class && last.byte_end == byte_start {
        last.text.push_str(&text);
        last.byte_end = byte_end;
    } else {
        lines[line_index].segments.push(SyntaxSegment {
            byte_start,
            byte_end,
            text,
            class,
        });
    }
}

fn syntax_class(name: &str) -> Option<SyntaxClass> {
    let namespace = name.split('.').next().unwrap_or(name);
    let class = if namespace == "comment" {
        SyntaxClass::Comment
    } else if namespace == "keyword" || name == "boolean" {
        SyntaxClass::Keyword
    } else if namespace == "string" || name == "character" {
        SyntaxClass::String
    } else if namespace == "number" {
        SyntaxClass::Number
    } else if namespace == "type" {
        SyntaxClass::Type
    } else if namespace == "function" {
        SyntaxClass::Function
    } else if namespace == "constructor" {
        SyntaxClass::Constructor
    } else if namespace == "constant" {
        SyntaxClass::Constant
    } else if namespace == "property" {
        SyntaxClass::Property
    } else if namespace == "punctuation" {
        SyntaxClass::Punctuation
    } else if namespace == "operator" {
        SyntaxClass::Operator
    } else if namespace == "tag" {
        SyntaxClass::Tag
    } else if namespace == "attribute" {
        SyntaxClass::Attribute
    } else if namespace == "module" || namespace == "namespace" {
        SyntaxClass::Module
    } else if namespace == "label" {
        SyntaxClass::Label
    } else if namespace == "variable" {
        SyntaxClass::Variable
    } else {
        return None;
    };
    Some(class)
}

fn config_home() -> HzResult<PathBuf> {
    if let Some(path) = env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }

    env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .map(|home| home.join(".config"))
        .ok_or_else(|| HzError::Usage("could not determine config directory".to_owned()))
}

fn load_config() -> HzResult<StoredSyntaxConfig> {
    let path = config_path()?;
    if !path.exists() {
        return Ok(StoredSyntaxConfig::default());
    }

    let contents = fs::read_to_string(&path)?;
    serde_json::from_str(&contents).map_err(Into::into)
}

fn save_config(config: &StoredSyntaxConfig) -> HzResult<()> {
    let path = config_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let contents = serde_json::to_vec_pretty(config)?;
    fs::write(path, contents)?;
    Ok(())
}

fn parse_settings(contents: &str) -> Result<SyntaxSettings, toml::de::Error> {
    let stored: StoredSyntaxSettings = toml::from_str(contents)?;
    Ok(settings_from_stored(stored))
}

fn settings_from_stored(stored: StoredSyntaxSettings) -> SyntaxSettings {
    let colorscheme = stored.colorscheme.or(stored.theme);

    SyntaxSettings {
        mode: stored.mode.unwrap_or_default(),
        theme: colorscheme
            .map(theme_config_from_stored)
            .unwrap_or_default(),
        colors: stored.colors.overlay(stored.color_overrides),
        transparent_background: stored.transparent_background,
        diff: diff_from_stored(stored.diff),
        limits: limits_from_stored(stored.limits),
    }
}

fn diff_from_stored(stored: StoredDiffSettings) -> DiffSettings {
    let defaults = DiffSettings::default();
    DiffSettings {
        line_background: stored.line_background.unwrap_or(defaults.line_background),
        gutter_background: stored
            .gutter_background
            .unwrap_or(defaults.gutter_background),
        inline_background: stored
            .inline_background
            .or(stored.word_background)
            .unwrap_or(defaults.inline_background),
        sign_style: stored.sign_style.unwrap_or(defaults.sign_style),
    }
}

fn theme_config_from_stored(stored: StoredSyntaxThemeConfig) -> SyntaxThemeConfig {
    match stored {
        StoredSyntaxThemeConfig::Name(name) => theme_config_from_name(name),
        StoredSyntaxThemeConfig::Table(table) => theme_config_from_table(table),
    }
}

fn theme_config_from_name(name: String) -> SyntaxThemeConfig {
    let name = name.trim().to_owned();
    if let Some(source) = theme_source_from_name(&name) {
        return SyntaxThemeConfig {
            source,
            name: None,
            path: None,
        };
    }

    SyntaxThemeConfig {
        source: SyntaxThemeSource::Builtin,
        name: (!name.is_empty()).then_some(name),
        path: None,
    }
}

fn theme_config_from_table(table: StoredSyntaxThemeTable) -> SyntaxThemeConfig {
    let name = table
        .name
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty());
    let source = table
        .source
        .or_else(|| name.as_deref().and_then(theme_source_from_name))
        .or_else(|| table.path.as_ref().map(|_| SyntaxThemeSource::Base16))
        .unwrap_or_default();
    let name = if theme_source_from_name(name.as_deref().unwrap_or_default()).is_some() {
        None
    } else {
        name
    };

    SyntaxThemeConfig {
        source,
        name,
        path: table.path,
    }
}

fn theme_source_from_name(name: &str) -> Option<SyntaxThemeSource> {
    match name.trim().to_ascii_lowercase().as_str() {
        "ansi" | "terminal" => Some(SyntaxThemeSource::Ansi),
        "base16" => Some(SyntaxThemeSource::Base16),
        _ => None,
    }
}

fn limits_from_stored(stored: StoredSyntaxLimits) -> SyntaxLimits {
    let defaults = SyntaxLimits::default();
    SyntaxLimits {
        max_source_bytes: kib_or_default(stored.max_source_kib, defaults.max_source_bytes),
        max_line_bytes: kib_or_default(stored.max_line_kib, defaults.max_line_bytes),
        cache_entries: non_zero_or_default(stored.cache_entries, defaults.cache_entries),
        queue_entries: non_zero_or_default(stored.queue_entries, defaults.queue_entries),
        prefetch_viewports: stored
            .prefetch_viewports
            .unwrap_or(defaults.prefetch_viewports),
    }
}

fn kib_or_default(kib: Option<usize>, default: usize) -> usize {
    kib.and_then(|kib| kib.checked_mul(1024))
        .filter(|bytes| *bytes > 0)
        .unwrap_or(default)
}

fn non_zero_or_default(value: Option<usize>, default: usize) -> usize {
    value.filter(|value| *value > 0).unwrap_or(default)
}

fn enabled_language_set() -> HzResult<BTreeSet<String>> {
    let settings = load_settings()?;
    let config = load_config()?;
    let installed = installed_language_set();
    let trusted = trusted_language_set(&installed, &config);
    Ok(enabled_language_set_for_mode(
        settings.mode,
        &config,
        &trusted,
    ))
}

fn enabled_language_set_for_mode(
    mode: SyntaxMode,
    config: &StoredSyntaxConfig,
    trusted: &BTreeSet<String>,
) -> BTreeSet<String> {
    match mode {
        SyntaxMode::Enabled => enabled_language_set_from_config(config),
        SyntaxMode::Builtin => bundled_highlight_language_set(),
        SyntaxMode::All => {
            let mut enabled = bundled_highlight_language_set();
            enabled.extend(trusted.iter().cloned());
            enabled
        }
    }
}

fn enabled_language_set_from_config(config: &StoredSyntaxConfig) -> BTreeSet<String> {
    let mut enabled = language_vec_to_set(&config.languages);
    enabled.extend(core_enabled_language_set());
    enabled
}

fn bundled_highlight_language_set() -> BTreeSet<String> {
    tree_sitter_language_pack::available_languages()
        .into_iter()
        .map(normalize_language_name)
        .filter(|language| {
            tree_sitter_language_pack::has_parser(language) && has_highlights(language)
        })
        .collect()
}

fn core_enabled_language_set() -> BTreeSet<String> {
    CORE_LANGUAGES
        .iter()
        .map(|language| normalize_language_name((*language).to_owned()))
        .filter(|language| tree_sitter_language_pack::has_parser(language))
        .collect()
}

fn reject_core_language_removal(requested: &BTreeSet<String>) -> HzResult<()> {
    let core = core_enabled_language_set();
    let blocked = requested
        .intersection(&core)
        .cloned()
        .collect::<Vec<String>>();
    if blocked.is_empty() {
        return Ok(());
    }

    Err(HzError::Usage(format!(
        "cannot remove core syntax languages: {}; use `hz diff --no-syntax` to disable syntax for a run",
        blocked.join(", ")
    )))
}

fn local_parser_language_set() -> BTreeSet<String> {
    let installed = installed_language_set();
    let mut languages = installed.clone();
    languages.extend(
        tree_sitter_language_pack::available_languages()
            .into_iter()
            .map(normalize_language_name)
            .filter(|language| {
                tree_sitter_language_pack::has_parser(language) || installed.contains(language)
            }),
    );
    languages
}

fn update_all_language_set(
    config: &StoredSyntaxConfig,
    installed: &BTreeSet<String>,
) -> BTreeSet<String> {
    let mut languages = language_vec_to_set(&config.languages);
    languages.extend(installed.iter().cloned());
    languages
}

fn installed_language_set() -> BTreeSet<String> {
    tree_sitter_language_pack::downloaded_languages()
        .into_iter()
        .map(normalize_language_name)
        .collect()
}

fn trusted_language_set(
    installed: &BTreeSet<String>,
    config: &StoredSyntaxConfig,
) -> BTreeSet<String> {
    let artifacts = parser_artifact_map(config);
    installed
        .iter()
        .filter(|language| parser_artifact_is_trusted(language, &artifacts))
        .cloned()
        .collect()
}

fn parser_artifact_map(config: &StoredSyntaxConfig) -> BTreeMap<String, StoredParserArtifact> {
    config
        .parsers
        .iter()
        .cloned()
        .map(|mut artifact| {
            artifact.language = normalize_language_name(artifact.language);
            (artifact.language.clone(), artifact)
        })
        .collect()
}

fn language_vec_to_set(languages: &[String]) -> BTreeSet<String> {
    languages
        .iter()
        .cloned()
        .map(normalize_language_name)
        .filter(|language| !language.is_empty())
        .collect()
}

fn normalize_language_names(languages: &[String]) -> BTreeSet<String> {
    languages
        .iter()
        .cloned()
        .map(normalize_language_name)
        .filter(|language| !language.is_empty())
        .collect()
}

fn normalize_language_name(language: String) -> String {
    let language = language.trim().to_ascii_lowercase();
    if language.is_empty() {
        return String::new();
    }
    if let Some(language) = detect_language_from_basename(&language) {
        return language.to_owned();
    }
    if let Some(language) = tree_sitter_language_pack::detect_language_from_path(&language) {
        return language.to_owned();
    }
    let language = language.trim_start_matches('.');
    let language = language_alias(language).unwrap_or(language);
    tree_sitter_language_pack::detect_language_from_extension(language)
        .unwrap_or(language)
        .to_owned()
}

fn detect_language_name(path: &str) -> Option<&'static str> {
    detect_language_from_basename(path)
        .or_else(|| tree_sitter_language_pack::detect_language_from_path(path))
        .or_else(|| tree_sitter_language_pack::detect_language(path))
}

fn language_alias(language: &str) -> Option<&'static str> {
    LANGUAGE_ALIASES
        .iter()
        .find_map(|(alias, target)| (*alias == language).then_some(*target))
}

fn detect_language_from_basename(path: &str) -> Option<&'static str> {
    let name = Path::new(path).file_name()?.to_str()?;
    BASENAME_LANGUAGES
        .iter()
        .find_map(|(basename, language)| name.eq_ignore_ascii_case(basename).then_some(*language))
}

fn is_language_trusted(language: &str) -> bool {
    if tree_sitter_language_pack::has_parser(language) {
        return true;
    }

    let Ok(config) = load_config() else {
        return false;
    };
    let installed = installed_language_set();
    installed.contains(language)
        && parser_artifact_is_trusted(language, &parser_artifact_map(&config))
}

fn load_language_without_download(language: &str) -> Result<(), String> {
    let registry = LanguageRegistry::new();
    if let Ok(cache) = cache_dir() {
        registry.add_extra_libs_dir(PathBuf::from(cache));
    }
    registry
        .get_language(language)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn has_highlights(language: &str) -> bool {
    highlights_query(language).is_some()
}

fn highlights_query(language: &str) -> Option<&'static str> {
    match language {
        "asm" => Some(ASM_HIGHLIGHTS_QUERY),
        "typescript" | "tsx" => tree_sitter_language_pack::get_highlights_query("javascript"),
        _ => tree_sitter_language_pack::get_highlights_query(language),
    }
}

fn install_language(language: &str) -> HzResult<Option<StoredParserArtifact>> {
    if !tree_sitter_language_pack::has_parser(language)
        && !is_language_trusted(language)
        && let Some(path) = expected_cached_language_path(language)?
    {
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }

    tree_sitter_language_pack::get_language(language).map_err(|error| {
        HzError::Usage(format!(
            "failed to install tree-sitter language '{language}': {error}"
        ))
    })?;

    if tree_sitter_language_pack::has_parser(language) {
        return Ok(None);
    }

    let path = expected_cached_language_path(language)?.ok_or_else(|| {
        HzError::Usage(format!(
            "failed to resolve parser artifact path for tree-sitter language '{language}'"
        ))
    })?;
    if !path.exists() {
        return Err(HzError::Usage(format!(
            "tree-sitter language '{language}' loaded, but parser artifact is missing at {}",
            path.display()
        )));
    }

    Ok(Some(StoredParserArtifact {
        language: language.to_owned(),
        version: language_pack_version(),
        sha256: sha256_file(&path)?,
        installed_at_unix: unix_time_now(),
        source: ARTIFACT_SOURCE.to_owned(),
        path,
    }))
}

fn upsert_parser_artifact(
    config: &mut StoredSyntaxConfig,
    language: &str,
    artifact: Option<StoredParserArtifact>,
) {
    config
        .parsers
        .retain(|existing| existing.language != language);
    if let Some(artifact) = artifact {
        config.parsers.push(artifact);
    }
}

fn parser_artifact_is_trusted(
    language: &str,
    artifacts: &BTreeMap<String, StoredParserArtifact>,
) -> bool {
    let Some(artifact) = artifacts.get(language) else {
        return false;
    };
    if artifact.version != language_pack_version() || artifact.source != ARTIFACT_SOURCE {
        return false;
    }
    let Ok(Some(expected_path)) = expected_cached_language_path(language) else {
        return false;
    };
    if artifact.path != expected_path || !artifact.path.exists() {
        return false;
    }
    sha256_file(&artifact.path).is_ok_and(|sha256| sha256 == artifact.sha256)
}

fn expected_cached_language_path(language: &str) -> HzResult<Option<PathBuf>> {
    let cache = PathBuf::from(cache_dir()?);
    Ok(Some(
        tree_sitter_language_pack::DownloadManager::with_cache_dir(&language_pack_version(), cache)
            .lib_path(language),
    ))
}

fn sha256_file(path: &Path) -> HzResult<String> {
    let bytes = fs::read(path)?;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Ok(hex_encode(&hasher.finalize()))
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn unix_time_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn language_pack_version() -> String {
    cache_dir()
        .ok()
        .and_then(|cache| {
            Path::new(&cache)
                .parent()
                .and_then(|parent| parent.file_name())
                .and_then(|version| version.to_str())
                .and_then(|version| version.strip_prefix('v'))
                .map(str::to_owned)
        })
        .unwrap_or_else(|| LANGUAGE_PACK_VERSION.to_owned())
}

fn remove_cached_language(language: &str) -> HzResult<bool> {
    let cache = PathBuf::from(cache_dir()?);
    let mut candidates = BTreeSet::new();
    if let Some(path) = cached_language_path(&cache, language) {
        candidates.insert(path);
    }
    candidates.extend(scan_cached_language_paths(&cache, language));

    let mut removed = false;
    for path in candidates {
        match fs::remove_file(&path) {
            Ok(()) => removed = true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(removed)
}

fn cached_language_path(cache: &Path, language: &str) -> Option<PathBuf> {
    let version = cache
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|version| version.to_str())
        .and_then(|version| version.strip_prefix('v'))?;
    Some(
        tree_sitter_language_pack::DownloadManager::with_cache_dir(version, cache.to_path_buf())
            .lib_path(language),
    )
}

fn scan_cached_language_paths(cache: &Path, language: &str) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(cache) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| cached_filename_matches_language(name, language))
        })
        .collect()
}

fn cached_filename_matches_language(name: &str, language: &str) -> bool {
    let name = name.strip_prefix("lib").unwrap_or(name);
    let Some(name) = name
        .strip_prefix("tree_sitter_")
        .or_else(|| name.strip_prefix("tree-sitter-"))
    else {
        return false;
    };
    let Some(name) = name
        .strip_suffix(".so")
        .or_else(|| name.strip_suffix(".dylib"))
        .or_else(|| name.strip_suffix(".dll"))
    else {
        return false;
    };

    name == language || name.replace('_', "") == language.replace('_', "")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_extensions_to_language_names() {
        assert_eq!(normalize_language_name("rs".to_owned()), "rust");
        assert_eq!(normalize_language_name(".mlir".to_owned()), "mlir");
        assert_eq!(normalize_language_name("rust".to_owned()), "rust");
        assert_eq!(normalize_language_name("shell".to_owned()), "bash");
        assert_eq!(normalize_language_name("c++".to_owned()), "cpp");
        assert_eq!(normalize_language_name("cc".to_owned()), "cpp");
        assert_eq!(normalize_language_name("cxx".to_owned()), "cpp");
        assert_eq!(normalize_language_name("js".to_owned()), "javascript");
        assert_eq!(normalize_language_name("ts".to_owned()), "typescript");
        assert_eq!(normalize_language_name("src/lib.rs".to_owned()), "rust");
    }

    #[test]
    fn maps_common_basenames_to_language_names() {
        assert_eq!(normalize_language_name("Makefile".to_owned()), "make");
        assert_eq!(
            normalize_language_name("CMakeLists.txt".to_owned()),
            "cmake"
        );
        assert_eq!(
            normalize_language_name("BUILD.bazel".to_owned()),
            "starlark"
        );
        assert_eq!(normalize_language_name(".clang-format".to_owned()), "yaml");
    }

    #[test]
    fn splits_highlighted_segments_by_line() {
        let mut lines = vec![HighlightedLine::default(), HighlightedLine::default()];
        let mut line = 0;
        push_source_segment(
            &mut lines,
            &mut line,
            10,
            b"hello\nworld",
            Some(SyntaxClass::String),
        );

        assert_eq!(line, 1);
        assert_eq!(lines[0].segments[0].text, "hello");
        assert_eq!(lines[0].segments[0].byte_start, 10);
        assert_eq!(lines[0].segments[0].byte_end, 15);
        assert_eq!(lines[1].segments[0].text, "world");
        assert_eq!(lines[1].segments[0].byte_start, 16);
        assert_eq!(lines[1].segments[0].byte_end, 21);
        assert_eq!(lines[1].segments[0].class, Some(SyntaxClass::String));
    }

    #[test]
    fn maps_highlight_names_to_coarse_classes() {
        assert_eq!(syntax_class("keyword.function"), Some(SyntaxClass::Keyword));
        assert_eq!(syntax_class("function.method"), Some(SyntaxClass::Function));
        assert_eq!(syntax_class("typewriter"), None);
        assert_eq!(syntax_class("unknown"), None);
    }

    #[test]
    fn detects_compiler_languages_by_path() {
        assert_eq!(detect_language_from_path("foo.ll").as_deref(), Some("llvm"));
        assert_eq!(
            detect_language_from_path("foo.mlir").as_deref(),
            Some("mlir")
        );
        assert_eq!(
            detect_language_from_path("foo.nasm").as_deref(),
            Some("nasm")
        );
        assert_eq!(
            detect_language_from_path("Makefile").as_deref(),
            Some("make")
        );
        assert_eq!(
            detect_language_from_path("CMakeLists.txt").as_deref(),
            Some("cmake")
        );
        assert_eq!(
            detect_language_from_path(".clang-format").as_deref(),
            Some("yaml")
        );
        assert_eq!(
            detect_language_from_path("WORKSPACE").as_deref(),
            Some("starlark")
        );
    }

    #[test]
    fn compiler_languages_have_queries_where_expected() {
        assert!(has_highlights("llvm"));
        assert!(has_highlights("mlir"));
        assert!(has_highlights("asm"));
        assert!(has_highlights("nasm"));
        assert!(has_highlights("typescript"));
        assert!(has_highlights("tsx"));
        assert!(has_highlights("tablegen"));
    }

    #[test]
    fn typescript_query_fallback_highlights() {
        let mut highlighter = SyntaxHighlighter::new();

        let highlighted = highlighter
            .highlight("typescript", "const value: number = 1;")
            .expect("typescript should use javascript query fallback");

        assert!(!highlighted.lines[0].segments.is_empty());
        assert!(highlighter.trusted_languages.contains("typescript"));
    }

    #[test]
    fn core_languages_are_bundled() {
        for language in CORE_LANGUAGES {
            assert!(
                tree_sitter_language_pack::has_parser(language),
                "core language should be statically bundled: {language}"
            );
        }
    }

    #[test]
    fn niche_languages_are_not_core_enabled() {
        let core = core_enabled_language_set();

        assert!(!core.contains("llvm"));
        assert!(!core.contains("mlir"));
        assert!(!core.contains("asm"));
        assert!(!core.contains("tablegen"));
        assert!(!core.contains("nix"));
    }

    #[test]
    fn removing_core_languages_is_rejected() {
        let requested = BTreeSet::from(["rust".to_owned(), "ruby".to_owned()]);

        let error = reject_core_language_removal(&requested)
            .unwrap_err()
            .to_string();

        assert!(error.contains("cannot remove core syntax languages: rust"));
        assert!(!error.contains("ruby"));
    }

    #[test]
    fn update_all_targets_configured_and_cached_languages() {
        let config = StoredSyntaxConfig {
            languages: vec!["ruby".to_owned(), "shell".to_owned()],
            parsers: Vec::new(),
        };
        let installed = BTreeSet::from(["elixir".to_owned()]);

        let languages = update_all_language_set(&config, &installed);

        assert_eq!(
            languages,
            BTreeSet::from(["bash".to_owned(), "elixir".to_owned(), "ruby".to_owned()])
        );
    }

    #[test]
    fn syntax_settings_default_to_enabled_system_colorscheme() {
        let settings = parse_settings("").expect("empty settings should parse");

        assert_eq!(settings.mode, SyntaxMode::Enabled);
        assert_eq!(settings.theme.source, SyntaxThemeSource::Builtin);
        assert_eq!(settings.theme.name.as_deref(), Some("system"));
        assert!(!settings.transparent_background);
        assert_eq!(settings.diff, DiffSettings::default());
        assert_eq!(settings.limits, SyntaxLimits::default());
    }

    #[test]
    fn syntax_settings_supports_ansi_colorscheme_and_limits() {
        let settings = parse_settings(
            r#"
mode = "builtin"
colorscheme = "ansi"
transparent_background = true

[limits]
max_source_kib = 64
max_line_kib = 4
cache_entries = 128
queue_entries = 256
prefetch_viewports = 2

[diff]
line_background = "subtle"
gutter_background = "delta"
inline_background = "strong"
sign_style = "bold"
"#,
        )
        .expect("settings should parse");

        assert_eq!(settings.mode, SyntaxMode::Builtin);
        assert_eq!(settings.theme.source, SyntaxThemeSource::Ansi);
        assert_eq!(settings.theme.name, None);
        assert!(settings.transparent_background);
        assert_eq!(settings.limits.max_source_bytes, 64 * 1024);
        assert_eq!(settings.limits.max_line_bytes, 4 * 1024);
        assert_eq!(settings.limits.cache_entries, 128);
        assert_eq!(settings.limits.queue_entries, 256);
        assert_eq!(settings.limits.prefetch_viewports, 2);
        assert_eq!(settings.diff.line_background, DiffBackground::Subtle);
        assert_eq!(settings.diff.gutter_background, DiffGutterBackground::Delta);
        assert_eq!(settings.diff.inline_background, DiffBackground::Strong);
        assert_eq!(settings.diff.sign_style, DiffSignStyle::Bold);
    }

    #[test]
    fn syntax_settings_supports_legacy_theme_key() {
        let settings = parse_settings(
            r#"
theme = "ansi"
"#,
        )
        .expect("legacy theme key should parse");

        assert_eq!(settings.theme.source, SyntaxThemeSource::Ansi);
        assert_eq!(settings.theme.name, None);
    }

    #[test]
    fn syntax_settings_prefers_colorscheme_over_legacy_theme() {
        let settings = parse_settings(
            r#"
colorscheme = "system"
theme = "ansi"
"#,
        )
        .expect("settings should parse");

        assert_eq!(settings.theme.source, SyntaxThemeSource::Builtin);
        assert_eq!(settings.theme.name.as_deref(), Some("system"));
    }

    #[test]
    fn syntax_settings_supports_color_overrides() {
        let settings = parse_settings(
            r##"
colorscheme = "system"
bg = "#111315"
addition_bg = "#1f3025"

[colors]
addition_bg = "#222222"
deletion_bg = "#372526"
"##,
        )
        .expect("settings should parse");

        assert_eq!(settings.colors.bg.as_deref(), Some("#111315"));
        assert_eq!(settings.colors.addition_bg.as_deref(), Some("#1f3025"));
        assert_eq!(settings.colors.deletion_bg.as_deref(), Some("#372526"));
    }

    #[test]
    fn syntax_settings_supports_word_background_alias() {
        let settings = parse_settings(
            r#"
[diff]
line_background = "none"
word_background = "subtle"
sign_style = "normal"
"#,
        )
        .expect("settings should parse");

        assert_eq!(settings.diff.line_background, DiffBackground::None);
        assert_eq!(settings.diff.inline_background, DiffBackground::Subtle);
        assert_eq!(settings.diff.sign_style, DiffSignStyle::Normal);
    }

    #[test]
    fn syntax_settings_accept_background_transparent_alias() {
        let settings =
            parse_settings("background_transparent = true").expect("settings should parse alias");

        assert!(settings.transparent_background);
    }

    #[test]
    fn syntax_settings_supports_base16_colorscheme_table() {
        let settings = parse_settings(
            r#"
mode = "all"

[colorscheme]
source = "base16"
path = "~/themes/example.yaml"
"#,
        )
        .expect("settings should parse");

        assert_eq!(settings.mode, SyntaxMode::All);
        assert_eq!(settings.theme.source, SyntaxThemeSource::Base16);
        assert_eq!(
            settings.theme.path,
            Some(PathBuf::from("~/themes/example.yaml"))
        );
    }

    #[test]
    fn syntax_modes_choose_enabled_languages_without_downloads() {
        let config = StoredSyntaxConfig {
            languages: vec!["definitely_custom_language".to_owned()],
            parsers: Vec::new(),
        };
        let trusted = BTreeSet::from(["elixir".to_owned()]);

        let enabled = enabled_language_set_for_mode(SyntaxMode::Enabled, &config, &trusted);
        let builtin = enabled_language_set_for_mode(SyntaxMode::Builtin, &config, &trusted);
        let all = enabled_language_set_for_mode(SyntaxMode::All, &config, &trusted);

        assert!(enabled.contains("rust"));
        assert!(enabled.contains("definitely_custom_language"));
        assert!(!builtin.contains("definitely_custom_language"));
        assert!(builtin.contains("rust"));
        assert!(all.contains("rust"));
        assert!(all.contains("elixir"));
        assert!(!all.contains("definitely_custom_language"));
    }

    #[test]
    fn language_set_falls_back_when_parser_is_missing() {
        let language = ["abl", "agda", "cobol", "desktop", "devicetree"]
            .into_iter()
            .find(|language| {
                tree_sitter_language_pack::has_language(language)
                    && !tree_sitter_language_pack::has_parser(language)
            })
            .unwrap_or("definitely_not_bundled");
        let languages = SyntaxLanguageSet {
            enabled: BTreeSet::from([language.to_owned()]),
            installed: BTreeSet::new(),
            trusted: BTreeSet::new(),
        };

        assert!(!languages.is_highlight_ready(language));
        assert!(languages.is_empty());
    }

    #[test]
    fn language_set_falls_back_when_highlight_query_is_missing() {
        let languages = SyntaxLanguageSet {
            enabled: BTreeSet::from(["desktop".to_owned()]),
            installed: BTreeSet::from(["desktop".to_owned()]),
            trusted: BTreeSet::from(["desktop".to_owned()]),
        };

        assert!(tree_sitter_language_pack::has_language("desktop"));
        assert!(!has_highlights("desktop"));
        assert!(!languages.is_highlight_ready("desktop"));
        assert!(languages.is_empty());
    }

    #[test]
    fn diff_highlighter_does_not_download_missing_parser() {
        let before = installed_language_set();
        let Some(language) = ["abl", "agda", "cobol", "desktop", "devicetree"]
            .into_iter()
            .find(|language| {
                tree_sitter_language_pack::has_language(language)
                    && !tree_sitter_language_pack::has_parser(language)
                    && !before.contains(*language)
            })
        else {
            return;
        };
        let mut highlighter = SyntaxHighlighter::new();

        let error = highlighter
            .highlight(language, "x")
            .unwrap_err()
            .to_string();

        assert!(error.contains("not trusted"));
        assert_eq!(installed_language_set(), before);
    }

    #[test]
    fn doctor_reports_stale_enabled_config() {
        let issues = doctor_issues(&[SyntaxLanguageStatus {
            language: "definitely_not_a_tree_sitter_language".to_owned(),
            enabled: true,
            installed: false,
            trusted: false,
            has_highlights: false,
            version: None,
            artifact: None,
            source: None,
        }]);

        assert_eq!(issues.len(), 1);
        assert!(issues[0].message.contains("not known"));
    }

    #[test]
    fn doctor_reports_missing_parser_cache_file() {
        let issues = doctor_issues(&[SyntaxLanguageStatus {
            language: "rust".to_owned(),
            enabled: true,
            installed: false,
            trusted: false,
            has_highlights: true,
            version: None,
            artifact: None,
            source: None,
        }]);

        assert_eq!(issues.len(), 1);
        assert!(issues[0].message.contains("parser cache file is missing"));
    }

    #[test]
    fn doctor_reports_untrusted_parser_cache_file() {
        let issues = doctor_issues(&[SyntaxLanguageStatus {
            language: "rust".to_owned(),
            enabled: true,
            installed: true,
            trusted: false,
            has_highlights: true,
            version: None,
            artifact: None,
            source: None,
        }]);

        assert_eq!(issues.len(), 1);
        assert!(issues[0].message.contains("trusted checksum"));
    }

    #[test]
    fn cached_language_filename_matching_handles_platform_names() {
        assert!(cached_filename_matches_language(
            "libtree_sitter_rust.dylib",
            "rust"
        ));
        assert!(cached_filename_matches_language(
            "tree_sitter_c_sharp.dll",
            "csharp"
        ));
        assert!(!cached_filename_matches_language(
            "libtree_sitter_rust.dylib",
            "python"
        ));
    }
}
