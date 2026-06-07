use super::*;
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

#[test]
fn language_pack_version_matches_workspace_dependency() {
    let workspace_manifest = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("hz-syntax manifest should be in a workspace member")
        .join("Cargo.toml");
    let manifest = fs::read_to_string(workspace_manifest).unwrap();
    let manifest: toml::Value = toml::from_str(&manifest).unwrap();
    let dependency = &manifest["workspace"]["dependencies"]["tree-sitter-language-pack"];
    let version = dependency
        .as_str()
        .or_else(|| dependency.get("version").and_then(toml::Value::as_str))
        .expect("workspace tree-sitter-language-pack version should be declared");

    assert_eq!(LANGUAGE_PACK_VERSION, version);
}

#[test]
fn trusted_parser_manifest_matches_pinned_language_pack_version() {
    let manifest: serde_json::Value = serde_json::from_str(TRUSTED_PARSER_MANIFEST).unwrap();

    assert_eq!(manifest["version"], LANGUAGE_PACK_VERSION);
    assert_eq!(
        sha256_file(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("src/tree_sitter_parsers_lock.json")
        )
        .unwrap(),
        TRUSTED_PARSER_MANIFEST_SHA256
    );
    assert!(ARTIFACT_SOURCE.contains(TRUSTED_PARSER_MANIFEST_SHA256));
}

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
context_expand = 42
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
    assert_eq!(
        settings.diff.context_expansion,
        DiffContextExpansion::Lines(42)
    );
}

#[test]
fn syntax_settings_supports_full_context_expansion() {
    let settings = parse_settings(
        r#"
[diff]
context_expand = "full"
"#,
    )
    .expect("settings should parse");

    assert_eq!(settings.diff.context_expansion, DiffContextExpansion::Full);
    assert_eq!(settings.diff.context_expansion.expand_count(123), 123);
}

#[test]
fn syntax_settings_clamps_zero_context_expansion_to_one() {
    let settings = parse_settings(
        r#"
[diff]
context_expand = 0
"#,
    )
    .expect("settings should parse");

    assert_eq!(
        settings.diff.context_expansion,
        DiffContextExpansion::Lines(1)
    );
    assert_eq!(settings.diff.context_expansion.expand_count(10), 1);
}

#[test]
fn context_expansion_count_clamps_direct_zero_lines_to_one() {
    assert_eq!(DiffContextExpansion::Lines(0).expand_count(10), 1);
    assert_eq!(DiffContextExpansion::Lines(0).expand_count(0), 0);
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
