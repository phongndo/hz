#![allow(unused_imports)]

use crate::*;
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

pub fn config_path() -> HzResult<PathBuf> {
    config_home().map(|path| path.join(CONFIG_DIR).join(CONFIG_FILE))
}

pub fn settings_path() -> HzResult<PathBuf> {
    config_home().map(|path| path.join(CONFIG_DIR).join(SETTINGS_FILE))
}

pub(crate) fn legacy_settings_path() -> HzResult<PathBuf> {
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
