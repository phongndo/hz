use std::collections::BTreeSet;

use crate::{
    SyntaxAddResult, SyntaxAvailableFilter, SyntaxCleanResult, SyntaxDoctorIssue,
    SyntaxDoctorReport, SyntaxLanguageStatus, SyntaxParserArtifact, SyntaxRemoveResult,
    SyntaxUpdateResult, core_enabled_language_set, enabled_language_set,
    enabled_language_set_for_mode, has_highlights, install_language, installed_language_set,
    language_pack_version, language_vec_to_set, load_config, load_language_without_download,
    load_settings, local_parser_language_set, normalize_language_names, parser_artifact_map,
    reject_core_language_removal, remove_cached_language, save_config, trusted_language_set,
    update_all_language_set, upsert_parser_artifact,
};
use hz_core::{HzError, HzResult};

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

pub(crate) fn doctor_issues(statuses: &[SyntaxLanguageStatus]) -> Vec<SyntaxDoctorIssue> {
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
