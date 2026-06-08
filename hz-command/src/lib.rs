mod config;
mod diff;
mod init;
mod lifecycle;
mod shell;
mod syntax;
#[cfg(test)]
mod tests;
mod worktree;

pub use hz_diff::{DiffOptions, DiffScope, DiffSource, PatchSource};
pub use hz_syntax::{
    SyntaxAddResult, SyntaxAvailableFilter, SyntaxCleanResult, SyntaxDoctorReport,
    SyntaxLanguageStatus, SyntaxLimits, SyntaxMode, SyntaxRemoveResult, SyntaxSettings,
    SyntaxThemeConfig, SyntaxThemeSource, SyntaxUpdateResult,
};
pub use hz_worktree::{
    CreateWorktree, CreatedWorktree, FindWorktree, HandoffMode, HandoffWorktree, ListWorktrees,
    LocalWorktree, LocalWorktreeInfo, PathWorktree, RemoveWorktree, WorktreeEntry, WorktreeHandoff,
    WorktreeSource, WorktreeStatus,
};

pub use config::{
    ColorConfig, ColorMode, ColorSchemeConfig, HzConfig, LifecycleConfig, ListColumn, ListConfig,
    ListHeaders, LoadRepoConfig, WorktreeConfig, load_repo_config,
};
pub use diff::{diff, diff_bytes, github_pr_diff_options};
pub use init::{InitRepo, RepoInit, init_repo};
pub use lifecycle::{
    LifecycleKind, LifecycleRun, RunLifecycle, run_lifecycle, run_lifecycle_for_entry,
};
pub use shell::{
    Shell, ShellInit, install_shell_integration, shell_init_comment, shell_init_line,
    shell_integration,
};
pub use syntax::{
    syntax_add, syntax_available_languages, syntax_cache_dir, syntax_clean_cache,
    syntax_colorscheme_dir, syntax_config_path, syntax_doctor, syntax_remove, syntax_settings_path,
    syntax_statuses, syntax_update,
};
pub use worktree::{
    create_worktree, create_worktree_with_lifecycle, current_worktree_path, find_worktree,
    handoff_worktree, is_user_managed_worktree_path, list_worktree_targets, list_worktrees,
    local_worktree, path_worktree, remove_found_worktree, remove_found_worktree_with_force,
    remove_worktree,
};

const HZ_DIR: &str = ".hz";
const CONFIG_FILE: &str = "hz.toml";
const ENVIRONMENT_DIR: &str = "environment";
const SETUP_SCRIPT: &str = "setup";
const CLEANUP_SCRIPT: &str = "cleanup";

pub(crate) use config::*;
#[cfg(test)]
pub(crate) use diff::*;
#[cfg(test)]
pub(crate) use init::*;
pub(crate) use lifecycle::*;
#[cfg(test)]
pub(crate) use shell::*;
