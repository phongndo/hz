use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use crate::{CONFIG_FILE, CreateWorktree, HZ_DIR, HandoffMode, HandoffWorktree, LifecycleKind};
use hz_core::{HzError, HzResult};
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadRepoConfig {
    pub repo: Option<PathBuf>,
}

pub fn load_repo_config(input: LoadRepoConfig) -> HzResult<HzConfig> {
    let repo = config_repo(input.repo.as_deref())?;
    HzConfig::load(&repo)
}

pub(crate) fn create_worktree_with_config_defaults(
    mut input: CreateWorktree,
) -> HzResult<CreateWorktree> {
    let creates_branch = creates_branch_worktree(&input);
    let needs_detached_limit =
        input.max_detached_worktrees.is_none() && creates_detached_worktree(&input);
    let needs_branch_limit = input.max_branch_worktrees.is_none() && creates_branch;
    if input.base.is_none() || needs_detached_limit || needs_branch_limit {
        let repo = config_repo(input.repo.as_deref())?;
        let config = HzConfig::load(&repo)?;
        if input.base.is_none()
            && let Some(base) = config.default_base()
        {
            input.base = Some(base.to_owned());
        }
        if needs_detached_limit {
            input.max_detached_worktrees = Some(config.max_detached_worktrees());
        }
        if needs_branch_limit {
            input.max_branch_worktrees = Some(config.max_branch_worktrees());
        }
    }

    Ok(input)
}

pub(crate) fn config_repo(repo: Option<&Path>) -> HzResult<PathBuf> {
    let current = hz_git::repository_root(repo)?;
    hz_git::main_worktree(&current)
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct HzConfig {
    pub lifecycle: Option<LifecycleConfig>,
    pub worktree: Option<WorktreeConfig>,
    pub list: Option<ListConfig>,
    pub color: Option<ColorConfig>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct LifecycleConfig {
    pub setup: Option<Vec<String>>,
    pub cleanup: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct WorktreeConfig {
    pub default_base: Option<String>,
    pub max_detached: Option<usize>,
    pub max_branch_worktrees: Option<usize>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ListConfig {
    pub headers: Option<ListHeaders>,
    pub columns: Option<Vec<ListColumn>>,
    pub compact_columns: Option<Vec<ListColumn>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ListHeaders {
    Auto,
    Always,
    Never,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ListColumn {
    Marker,
    Target,
    Branch,
    Handle,
    Status,
    Base,
    Modified,
    Path,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ColorConfig {
    pub mode: Option<ColorMode>,
    pub scheme: Option<String>,
    #[serde(default)]
    pub schemes: HashMap<String, ColorSchemeConfig>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorMode {
    Auto,
    Always,
    Never,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ColorSchemeConfig {
    pub header: Option<String>,
    pub target: Option<String>,
    pub branch: Option<String>,
    pub handle: Option<String>,
    pub base: Option<String>,
    pub modified: Option<String>,
    pub path: Option<String>,
    pub clean: Option<String>,
    pub dirty: Option<String>,
    pub unknown: Option<String>,
    pub current: Option<String>,
    pub local: Option<String>,
}

impl HzConfig {
    pub fn default_base(&self) -> Option<&str> {
        self.worktree
            .as_ref()
            .and_then(|worktree| worktree.default_base.as_deref())
            .filter(|base| !base.is_empty())
    }

    pub(crate) fn load(worktree: &Path) -> HzResult<Self> {
        let path = config_path(worktree);
        if !path.exists() {
            return Ok(Self::default());
        }

        let contents = fs::read_to_string(&path)?;
        toml::from_str(&contents)
            .map_err(|error| HzError::Usage(format!("failed to parse {}: {error}", path.display())))
    }

    pub(crate) fn lifecycle_command(&self, kind: LifecycleKind) -> Option<&[String]> {
        let lifecycle = self.lifecycle.as_ref()?;
        match kind {
            LifecycleKind::Setup => lifecycle.setup.as_deref(),
            LifecycleKind::Cleanup => lifecycle.cleanup.as_deref(),
        }
        .filter(|command| !command.is_empty())
    }

    pub(crate) fn max_detached_worktrees(&self) -> usize {
        self.worktree
            .as_ref()
            .and_then(|worktree| worktree.max_detached)
            .unwrap_or(hz_worktree::DEFAULT_MAX_DETACHED_WORKTREES)
    }

    pub(crate) fn max_branch_worktrees(&self) -> usize {
        self.worktree
            .as_ref()
            .and_then(|worktree| worktree.max_branch_worktrees)
            .unwrap_or(hz_worktree::DEFAULT_MAX_BRANCH_WORKTREES)
    }
}

pub(crate) fn with_configured_handoff_limits(
    mut input: HandoffWorktree,
) -> HzResult<HandoffWorktree> {
    if input.mode == HandoffMode::Patch && input.create {
        if input.target.is_none() && input.max_detached_worktrees.is_none() {
            input.max_detached_worktrees = Some(configured_detached_limit(input.repo.as_deref())?);
        }
        if input.target.is_some() && input.max_branch_worktrees.is_none() {
            input.max_branch_worktrees =
                Some(configured_branch_worktree_limit(input.repo.as_deref())?);
        }
    }
    Ok(input)
}

pub(crate) fn creates_detached_worktree(input: &CreateWorktree) -> bool {
    input.name.is_none() && input.branch.is_none()
}

pub(crate) fn creates_branch_worktree(input: &CreateWorktree) -> bool {
    input.name.is_some() || input.branch.is_some()
}

pub(crate) fn configured_detached_limit(repo: Option<&Path>) -> HzResult<usize> {
    let repo = config_repo(repo)?;
    Ok(HzConfig::load(&repo)?.max_detached_worktrees())
}

pub(crate) fn configured_branch_worktree_limit(repo: Option<&Path>) -> HzResult<usize> {
    let repo = config_repo(repo)?;
    Ok(HzConfig::load(&repo)?.max_branch_worktrees())
}

pub(crate) fn config_path(repo: &Path) -> PathBuf {
    repo.join(HZ_DIR).join(CONFIG_FILE)
}
