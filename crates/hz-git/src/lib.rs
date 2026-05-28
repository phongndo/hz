use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitRepository {
    pub root: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitWorktreeSpec {
    pub repo: Option<PathBuf>,
    pub path: Option<PathBuf>,
    pub base: Option<String>,
    pub branch: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitDiffSpec {
    pub repo: Option<PathBuf>,
    pub base: Option<String>,
    pub stat: bool,
}
