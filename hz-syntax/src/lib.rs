use std::{
    collections::{BTreeSet, HashMap},
    env, fs,
    path::{Path, PathBuf},
};

use hz_core::{HzError, HzResult};
use serde::{Deserialize, Serialize};
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};
use tree_sitter_language_pack::LanguageRegistry;

const CONFIG_DIR: &str = "hz";
const CONFIG_FILE: &str = "tree-sitter.json";

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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxLanguageStatus {
    pub language: String,
    pub enabled: bool,
    pub installed: bool,
    pub has_highlights: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxAddResult {
    pub added: Vec<String>,
    pub already_enabled: Vec<String>,
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
}

impl SyntaxLanguageSet {
    pub fn load() -> HzResult<Self> {
        Ok(Self {
            enabled: enabled_language_set()?,
            installed: installed_language_set(),
        })
    }

    pub fn from_enabled_languages(languages: &[String]) -> Self {
        Self {
            enabled: language_vec_to_set(languages),
            installed: installed_language_set(),
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
            && (self.installed.contains(language)
                || tree_sitter_language_pack::has_parser(language))
            && has_highlights(language)
    }
}

pub struct SyntaxHighlighter {
    registry: LanguageRegistry,
    highlighter: Highlighter,
    configs: HashMap<String, HighlightConfiguration>,
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
        }
    }
}

impl SyntaxHighlighter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn highlight(&mut self, language: &str, source: &str) -> HzResult<HighlightedText> {
        let language = normalize_language_name(language.to_owned());
        if !is_language_installed(&language) {
            return Err(HzError::Usage(format!(
                "tree-sitter language '{language}' is not installed; run `hz ts add {language}`"
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

pub fn cache_dir() -> HzResult<String> {
    tree_sitter_language_pack::cache_dir()
        .map_err(|error| HzError::Usage(format!("failed to resolve tree-sitter cache: {error}")))
}

pub fn available_languages() -> HzResult<Vec<String>> {
    tree_sitter_language_pack::manifest_languages()
        .map_err(|error| HzError::Usage(format!("failed to list tree-sitter languages: {error}")))
}

pub fn enabled_languages() -> HzResult<Vec<String>> {
    Ok(enabled_language_set()?.into_iter().collect())
}

pub fn installed_languages() -> Vec<String> {
    installed_language_set().into_iter().collect()
}

pub fn language_statuses() -> HzResult<Vec<SyntaxLanguageStatus>> {
    let enabled = enabled_language_set()?;
    let installed = installed_language_set();
    let mut languages = enabled
        .union(&installed)
        .cloned()
        .collect::<BTreeSet<String>>();

    if languages.is_empty() {
        languages.extend(installed.iter().cloned());
    }

    Ok(languages
        .into_iter()
        .map(|language| SyntaxLanguageStatus {
            enabled: enabled.contains(&language),
            installed: is_language_installed_with_set(&language, &installed),
            has_highlights: has_highlights(&language),
            language,
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
        tree_sitter_language_pack::get_language(&language).map_err(|error| {
            HzError::Usage(format!(
                "failed to install tree-sitter language '{language}': {error}"
            ))
        })?;

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

pub fn remove_languages(languages: &[String]) -> HzResult<SyntaxRemoveResult> {
    if languages.is_empty() {
        return Err(HzError::Usage("provide at least one language".to_owned()));
    }

    let requested = normalize_language_names(languages);
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

pub fn clean_cache() -> HzResult<()> {
    tree_sitter_language_pack::clean_cache()
        .map_err(|error| HzError::Usage(format!("failed to clean tree-sitter cache: {error}")))
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
    mut bytes: &[u8],
    class: Option<SyntaxClass>,
) {
    while let Some(newline) = bytes.iter().position(|byte| *byte == b'\n') {
        push_line_segment(lines, *line_index, &bytes[..newline], class);
        *line_index = line_index
            .saturating_add(1)
            .min(lines.len().saturating_sub(1));
        bytes = &bytes[newline + 1..];
    }
    push_line_segment(lines, *line_index, bytes, class);
}

fn push_line_segment(
    lines: &mut [HighlightedLine],
    line_index: usize,
    bytes: &[u8],
    class: Option<SyntaxClass>,
) {
    if bytes.is_empty() || line_index >= lines.len() {
        return;
    }

    let text = String::from_utf8_lossy(bytes).into_owned();
    let Some(last) = lines[line_index].segments.last_mut() else {
        lines[line_index]
            .segments
            .push(SyntaxSegment { text, class });
        return;
    };

    if last.class == class {
        last.text.push_str(&text);
    } else {
        lines[line_index]
            .segments
            .push(SyntaxSegment { text, class });
    }
}

fn syntax_class(name: &str) -> Option<SyntaxClass> {
    let class = if name.starts_with("comment") {
        SyntaxClass::Comment
    } else if name.starts_with("keyword") || name == "boolean" {
        SyntaxClass::Keyword
    } else if name.starts_with("string") || name == "character" {
        SyntaxClass::String
    } else if name.starts_with("number") {
        SyntaxClass::Number
    } else if name.starts_with("type") {
        SyntaxClass::Type
    } else if name.starts_with("function") {
        SyntaxClass::Function
    } else if name.starts_with("constructor") {
        SyntaxClass::Constructor
    } else if name.starts_with("constant") {
        SyntaxClass::Constant
    } else if name.starts_with("property") {
        SyntaxClass::Property
    } else if name.starts_with("punctuation") {
        SyntaxClass::Punctuation
    } else if name.starts_with("operator") {
        SyntaxClass::Operator
    } else if name.starts_with("tag") {
        SyntaxClass::Tag
    } else if name.starts_with("attribute") {
        SyntaxClass::Attribute
    } else if name.starts_with("module") || name.starts_with("namespace") {
        SyntaxClass::Module
    } else if name.starts_with("label") {
        SyntaxClass::Label
    } else if name.starts_with("variable") {
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

fn enabled_language_set() -> HzResult<BTreeSet<String>> {
    Ok(language_vec_to_set(&load_config()?.languages))
}

fn installed_language_set() -> BTreeSet<String> {
    tree_sitter_language_pack::downloaded_languages()
        .into_iter()
        .map(normalize_language_name)
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
    let language = language.trim().trim_start_matches('.').to_ascii_lowercase();
    let language = match language.as_str() {
        "bazel" => "starlark",
        "c++" => "cpp",
        "c#" => "csharp",
        "gradle" => "groovy",
        "ignorefile" => "gitignore",
        "lisp" => "commonlisp",
        "makefile" => "make",
        "shell" | "sh" => "bash",
        _ => language.as_str(),
    };
    tree_sitter_language_pack::detect_language_from_extension(&language)
        .unwrap_or(language)
        .to_owned()
}

fn detect_language_name(path: &str) -> Option<&'static str> {
    tree_sitter_language_pack::detect_language_from_path(path)
        .or_else(|| tree_sitter_language_pack::detect_language(path))
}

fn is_language_installed(language: &str) -> bool {
    is_language_installed_with_set(language, &installed_language_set())
}

fn is_language_installed_with_set(language: &str, installed: &BTreeSet<String>) -> bool {
    installed.contains(language) || tree_sitter_language_pack::has_parser(language)
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
        _ => tree_sitter_language_pack::get_highlights_query(language),
    }
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
    }

    #[test]
    fn splits_highlighted_segments_by_line() {
        let mut lines = vec![HighlightedLine::default(), HighlightedLine::default()];
        let mut line = 0;
        push_source_segment(
            &mut lines,
            &mut line,
            b"hello\nworld",
            Some(SyntaxClass::String),
        );

        assert_eq!(line, 1);
        assert_eq!(lines[0].segments[0].text, "hello");
        assert_eq!(lines[1].segments[0].text, "world");
        assert_eq!(lines[1].segments[0].class, Some(SyntaxClass::String));
    }

    #[test]
    fn maps_highlight_names_to_coarse_classes() {
        assert_eq!(syntax_class("keyword.function"), Some(SyntaxClass::Keyword));
        assert_eq!(syntax_class("function.method"), Some(SyntaxClass::Function));
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
    }

    #[test]
    fn compiler_languages_have_queries_where_expected() {
        assert!(has_highlights("llvm"));
        assert!(has_highlights("mlir"));
        assert!(has_highlights("asm"));
        assert!(has_highlights("nasm"));
    }

    #[test]
    fn language_set_falls_back_when_parser_is_missing() {
        let languages = SyntaxLanguageSet {
            enabled: BTreeSet::from(["rust".to_owned()]),
            installed: BTreeSet::new(),
        };

        assert!(!languages.is_highlight_ready("rust"));
        assert_eq!(languages.language_for_path("src/main.rs"), None);
        assert!(languages.is_empty());
    }

    #[test]
    fn language_set_falls_back_when_highlight_query_is_missing() {
        let languages = SyntaxLanguageSet {
            enabled: BTreeSet::from(["desktop".to_owned()]),
            installed: BTreeSet::from(["desktop".to_owned()]),
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

        assert!(error.contains("not installed"));
        assert_eq!(installed_language_set(), before);
    }

    #[test]
    fn doctor_reports_stale_enabled_config() {
        let issues = doctor_issues(&[SyntaxLanguageStatus {
            language: "definitely_not_a_tree_sitter_language".to_owned(),
            enabled: true,
            installed: false,
            has_highlights: false,
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
            has_highlights: true,
        }]);

        assert_eq!(issues.len(), 1);
        assert!(issues[0].message.contains("parser cache file is missing"));
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
