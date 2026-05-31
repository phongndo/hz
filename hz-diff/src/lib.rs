use std::path::PathBuf;

use hz_core::{HzError, HzResult};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffOptions {
    pub repo: Option<PathBuf>,
    pub base: Option<String>,
    pub stat: bool,
}

pub fn render(options: DiffOptions) -> HzResult<String> {
    let _ = options;
    Err(HzError::NotImplemented("diff render"))
}
