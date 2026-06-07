use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    ARTIFACT_SOURCE, ASM_HIGHLIGHTS_QUERY, BASENAME_LANGUAGES, CORE_LANGUAGES,
    DiffContextExpansion, DiffSettings, LANGUAGE_ALIASES, LANGUAGE_PACK_VERSION,
    StoredDiffContextExpansion, StoredDiffContextExpansionMode, StoredDiffSettings,
    StoredParserArtifact, StoredSyntaxConfig, StoredSyntaxLimits, StoredSyntaxSettings,
    StoredSyntaxThemeConfig, StoredSyntaxThemeTable, SyntaxLimits, SyntaxMode, SyntaxSettings,
    SyntaxThemeConfig, SyntaxThemeSource, TRUSTED_PARSER_MANIFEST, TRUSTED_PARSER_MANIFEST_SHA256,
    cache_dir, config_path, load_settings,
};
use hz_core::{HzError, HzResult};
use sha2::{Digest, Sha256};
use tree_sitter_language_pack::LanguageRegistry;

pub(crate) fn config_home() -> HzResult<PathBuf> {
    if let Some(path) = env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }

    env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .map(|home| home.join(".config"))
        .ok_or_else(|| HzError::Usage("could not determine config directory".to_owned()))
}

pub(crate) fn load_config() -> HzResult<StoredSyntaxConfig> {
    let path = config_path()?;
    if !path.exists() {
        return Ok(StoredSyntaxConfig::default());
    }

    let contents = fs::read_to_string(&path)?;
    serde_json::from_str(&contents).map_err(Into::into)
}

pub(crate) fn save_config(config: &StoredSyntaxConfig) -> HzResult<()> {
    let path = config_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let contents = serde_json::to_vec_pretty(config)?;
    fs::write(path, contents)?;
    Ok(())
}

pub(crate) fn parse_settings(contents: &str) -> Result<SyntaxSettings, toml::de::Error> {
    let stored: StoredSyntaxSettings = toml::from_str(contents)?;
    Ok(settings_from_stored(stored))
}

pub(crate) fn settings_from_stored(stored: StoredSyntaxSettings) -> SyntaxSettings {
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

pub(crate) fn diff_from_stored(stored: StoredDiffSettings) -> DiffSettings {
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
        context_expansion: stored
            .context_expansion
            .map(diff_context_expansion_from_stored)
            .unwrap_or(defaults.context_expansion),
    }
}

pub(crate) fn diff_context_expansion_from_stored(
    stored: StoredDiffContextExpansion,
) -> DiffContextExpansion {
    match stored {
        StoredDiffContextExpansion::Lines(lines) => DiffContextExpansion::Lines(lines.max(1)),
        StoredDiffContextExpansion::Mode(StoredDiffContextExpansionMode::Full) => {
            DiffContextExpansion::Full
        }
    }
}

pub(crate) fn theme_config_from_stored(stored: StoredSyntaxThemeConfig) -> SyntaxThemeConfig {
    match stored {
        StoredSyntaxThemeConfig::Name(name) => theme_config_from_name(name),
        StoredSyntaxThemeConfig::Table(table) => theme_config_from_table(table),
    }
}

pub(crate) fn theme_config_from_name(name: String) -> SyntaxThemeConfig {
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

pub(crate) fn theme_config_from_table(table: StoredSyntaxThemeTable) -> SyntaxThemeConfig {
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

pub(crate) fn theme_source_from_name(name: &str) -> Option<SyntaxThemeSource> {
    match name.trim().to_ascii_lowercase().as_str() {
        "ansi" | "terminal" => Some(SyntaxThemeSource::Ansi),
        "base16" => Some(SyntaxThemeSource::Base16),
        _ => None,
    }
}

pub(crate) fn limits_from_stored(stored: StoredSyntaxLimits) -> SyntaxLimits {
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

pub(crate) fn kib_or_default(kib: Option<usize>, default: usize) -> usize {
    kib.and_then(|kib| kib.checked_mul(1024))
        .filter(|bytes| *bytes > 0)
        .unwrap_or(default)
}

pub(crate) fn non_zero_or_default(value: Option<usize>, default: usize) -> usize {
    value.filter(|value| *value > 0).unwrap_or(default)
}

pub(crate) fn enabled_language_set() -> HzResult<BTreeSet<String>> {
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

pub(crate) fn enabled_language_set_for_mode(
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

pub(crate) fn enabled_language_set_from_config(config: &StoredSyntaxConfig) -> BTreeSet<String> {
    let mut enabled = language_vec_to_set(&config.languages);
    enabled.extend(core_enabled_language_set());
    enabled
}

pub(crate) fn bundled_highlight_language_set() -> BTreeSet<String> {
    tree_sitter_language_pack::available_languages()
        .into_iter()
        .map(normalize_language_name)
        .filter(|language| {
            tree_sitter_language_pack::has_parser(language) && has_highlights(language)
        })
        .collect()
}

pub(crate) fn core_enabled_language_set() -> BTreeSet<String> {
    CORE_LANGUAGES
        .iter()
        .map(|language| normalize_language_name((*language).to_owned()))
        .filter(|language| tree_sitter_language_pack::has_parser(language))
        .collect()
}

pub(crate) fn reject_core_language_removal(requested: &BTreeSet<String>) -> HzResult<()> {
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

pub(crate) fn local_parser_language_set() -> BTreeSet<String> {
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

pub(crate) fn update_all_language_set(
    config: &StoredSyntaxConfig,
    installed: &BTreeSet<String>,
) -> BTreeSet<String> {
    let mut languages = language_vec_to_set(&config.languages);
    languages.extend(installed.iter().cloned());
    languages
}

pub(crate) fn installed_language_set() -> BTreeSet<String> {
    tree_sitter_language_pack::downloaded_languages()
        .into_iter()
        .map(normalize_language_name)
        .collect()
}

pub(crate) fn trusted_language_set(
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

pub(crate) fn parser_artifact_map(
    config: &StoredSyntaxConfig,
) -> BTreeMap<String, StoredParserArtifact> {
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

pub(crate) fn language_vec_to_set(languages: &[String]) -> BTreeSet<String> {
    languages
        .iter()
        .cloned()
        .map(normalize_language_name)
        .filter(|language| !language.is_empty())
        .collect()
}

pub(crate) fn normalize_language_names(languages: &[String]) -> BTreeSet<String> {
    languages
        .iter()
        .cloned()
        .map(normalize_language_name)
        .filter(|language| !language.is_empty())
        .collect()
}

pub(crate) fn normalize_language_name(language: String) -> String {
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

pub(crate) fn detect_language_name(path: &str) -> Option<&'static str> {
    detect_language_from_basename(path)
        .or_else(|| tree_sitter_language_pack::detect_language_from_path(path))
        .or_else(|| tree_sitter_language_pack::detect_language(path))
}

pub(crate) fn language_alias(language: &str) -> Option<&'static str> {
    LANGUAGE_ALIASES
        .iter()
        .find_map(|(alias, target)| (*alias == language).then_some(*target))
}

pub(crate) fn detect_language_from_basename(path: &str) -> Option<&'static str> {
    let name = Path::new(path).file_name()?.to_str()?;
    BASENAME_LANGUAGES
        .iter()
        .find_map(|(basename, language)| name.eq_ignore_ascii_case(basename).then_some(*language))
}

pub(crate) fn is_language_trusted(language: &str) -> bool {
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

pub(crate) fn load_language_without_download(language: &str) -> Result<(), String> {
    let registry = LanguageRegistry::new();
    if let Ok(cache) = cache_dir() {
        registry.add_extra_libs_dir(PathBuf::from(cache));
    }
    registry
        .get_language(language)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

pub(crate) fn has_highlights(language: &str) -> bool {
    highlights_query(language).is_some()
}

pub(crate) fn highlights_query(language: &str) -> Option<&'static str> {
    match language {
        "asm" => Some(ASM_HIGHLIGHTS_QUERY),
        "typescript" | "tsx" => tree_sitter_language_pack::get_highlights_query("javascript"),
        _ => tree_sitter_language_pack::get_highlights_query(language),
    }
}

pub(crate) fn install_language(language: &str) -> HzResult<Option<StoredParserArtifact>> {
    if tree_sitter_language_pack::has_parser(language) {
        tree_sitter_language_pack::get_language(language).map_err(|error| {
            HzError::Usage(format!(
                "failed to load bundled tree-sitter language '{language}': {error}"
            ))
        })?;
        return Ok(None);
    }

    if !is_language_trusted(language)
        && let Some(path) = expected_cached_language_path(language)?
    {
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }

    // DownloadManager downloads and extracts without loading the native library.
    // Keep it that way until hz has seeded and re-verified its pinned manifest.
    write_trusted_parser_manifest()?;
    let cache = PathBuf::from(cache_dir()?);
    tree_sitter_language_pack::DownloadManager::with_cache_dir(&language_pack_version(), cache)
        .ensure_languages(&[language])
        .map_err(|error| {
            HzError::Usage(format!(
                "failed to install tree-sitter language '{language}' from trusted parser lock: {error}"
            ))
        })?;
    verify_trusted_parser_manifest()?;

    let artifact = stored_parser_artifact(language)?;
    load_language_without_download(language).map_err(|error| {
        HzError::Usage(format!(
            "failed to load tree-sitter language '{language}' from verified parser cache: {error}"
        ))
    })?;

    Ok(Some(artifact))
}

pub(crate) fn stored_parser_artifact(language: &str) -> HzResult<StoredParserArtifact> {
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

    Ok(StoredParserArtifact {
        language: language.to_owned(),
        version: language_pack_version(),
        sha256: sha256_file(&path)?,
        installed_at_unix: unix_time_now(),
        source: ARTIFACT_SOURCE.to_owned(),
        path,
    })
}

pub(crate) fn upsert_parser_artifact(
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

pub(crate) fn parser_artifact_is_trusted(
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

pub(crate) fn expected_cached_language_path(language: &str) -> HzResult<Option<PathBuf>> {
    let cache = PathBuf::from(cache_dir()?);
    Ok(Some(
        tree_sitter_language_pack::DownloadManager::with_cache_dir(&language_pack_version(), cache)
            .lib_path(language),
    ))
}

pub(crate) fn write_trusted_parser_manifest() -> HzResult<()> {
    let path = trusted_parser_manifest_path()?;
    if path.exists()
        && sha256_file(&path).is_ok_and(|sha256| sha256 == TRUSTED_PARSER_MANIFEST_SHA256)
    {
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, TRUSTED_PARSER_MANIFEST.as_bytes())?;
    Ok(())
}

pub(crate) fn verify_trusted_parser_manifest() -> HzResult<()> {
    let path = trusted_parser_manifest_path()?;
    let sha256 = sha256_file(&path)?;
    if sha256 == TRUSTED_PARSER_MANIFEST_SHA256 {
        return Ok(());
    }

    Err(HzError::Usage(format!(
        "tree-sitter parser manifest at {} did not match shipped parser lock (expected {}, got {})",
        path.display(),
        TRUSTED_PARSER_MANIFEST_SHA256,
        sha256
    )))
}

pub(crate) fn trusted_parser_manifest_path() -> HzResult<PathBuf> {
    let cache = PathBuf::from(cache_dir()?);
    cache
        .parent()
        .map(|path| path.join("manifest.json"))
        .ok_or_else(|| HzError::Usage("tree-sitter cache directory has no parent".to_owned()))
}

pub(crate) fn sha256_file(path: &Path) -> HzResult<String> {
    let bytes = fs::read(path)?;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Ok(hex_encode(&hasher.finalize()))
}

pub(crate) fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

pub(crate) fn unix_time_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

pub(crate) fn language_pack_version() -> String {
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

pub(crate) fn remove_cached_language(language: &str) -> HzResult<bool> {
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

pub(crate) fn cached_language_path(cache: &Path, language: &str) -> Option<PathBuf> {
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

pub(crate) fn scan_cached_language_paths(cache: &Path, language: &str) -> Vec<PathBuf> {
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

pub(crate) fn cached_filename_matches_language(name: &str, language: &str) -> bool {
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
