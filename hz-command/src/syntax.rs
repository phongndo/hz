#![allow(unused_imports)]

use crate::*;
use std::{
    collections::HashMap,
    env, fs,
    io::{self, ErrorKind, Read, Write},
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, Stdio},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use hz_core::{HzError, HzResult};
use serde::{Deserialize, Serialize};

pub fn syntax_add(languages: &[String]) -> HzResult<SyntaxAddResult> {
    hz_syntax::add_languages(languages)
}

pub fn syntax_update(languages: &[String], all: bool) -> HzResult<SyntaxUpdateResult> {
    hz_syntax::update_languages(languages, all)
}

pub fn syntax_remove(languages: &[String]) -> HzResult<SyntaxRemoveResult> {
    hz_syntax::remove_languages(languages)
}

pub fn syntax_statuses() -> HzResult<Vec<SyntaxLanguageStatus>> {
    hz_syntax::language_statuses()
}

pub fn syntax_available_languages(filter: SyntaxAvailableFilter) -> HzResult<Vec<String>> {
    hz_syntax::available_languages(filter)
}

pub fn syntax_clean_cache() -> HzResult<SyntaxCleanResult> {
    hz_syntax::clean_cache()
}

pub fn syntax_cache_dir() -> HzResult<String> {
    hz_syntax::cache_dir()
}

pub fn syntax_config_path() -> HzResult<PathBuf> {
    hz_syntax::config_path()
}

pub fn syntax_settings_path() -> HzResult<PathBuf> {
    hz_syntax::settings_path()
}

pub fn syntax_colorscheme_dir() -> HzResult<PathBuf> {
    hz_syntax::colorscheme_dir()
}

pub fn syntax_doctor() -> HzResult<SyntaxDoctorReport> {
    hz_syntax::doctor()
}
