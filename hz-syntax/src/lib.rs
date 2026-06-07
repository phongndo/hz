mod highlight;
mod language;
mod paths;
mod storage;
#[cfg(test)]
mod tests;
mod types;

pub use highlight::detect_language_from_path;
pub use language::{
    add_languages, available_languages, clean_cache, doctor, enabled_languages,
    installed_languages, language_statuses, remove_languages, update_languages,
};
pub use paths::{cache_dir, colorscheme_dir, config_path, load_settings, settings_path};
pub use types::*;

pub(crate) use highlight::*;
#[cfg(test)]
pub(crate) use language::*;
pub(crate) use storage::*;
