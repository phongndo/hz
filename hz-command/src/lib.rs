mod config;
mod init;
mod lifecycle;
mod shell;
#[cfg(test)]
mod tests;
mod worktree;

pub use hz_worktree::{
    CreateWorktree, CreatedWorktree, FindWorktree, FindWorktrees, ForkWorktree, ForkedWorktree,
    HandoffMode, HandoffWorktree, ListWorktrees, LocalWorktree, LocalWorktreeInfo, PathWorktree,
    RemoveWorktree, WorktreeEntry, WorktreeHandoff, WorktreeSource, WorktreeStatus,
};

pub use config::{
    ColorConfig, ColorMode, ColorSchemeConfig, HzConfig, LifecycleConfig, ListColumn, ListConfig,
    ListHeaders, LoadRepoConfig, WorktreeConfig, load_repo_config, load_repo_config_at,
};
pub use init::{InitRepo, RepoInit, init_repo};
pub use lifecycle::{
    LifecycleKind, LifecycleRun, RunLifecycle, run_lifecycle, run_lifecycle_for_entry,
};
pub use shell::{
    Shell, ShellInit, install_shell_integration, shell_init_comment, shell_init_line,
    shell_integration,
};
pub use worktree::{
    create_worktree, create_worktree_with_lifecycle, current_worktree_path, find_worktree,
    find_worktrees, fork_worktree, handoff_worktree, is_user_managed_worktree_path,
    list_worktree_targets, list_worktree_targets_with_repo, list_worktrees,
    list_worktrees_with_local, local_worktree, path_worktree, remove_found_worktree,
    remove_found_worktree_with_force, remove_worktree,
};

const HZ_DIR: &str = ".hz";
const CONFIG_FILE: &str = "hz.toml";
const ENVIRONMENT_DIR: &str = "environment";
const SETUP_SCRIPT: &str = "setup";
const CLEANUP_SCRIPT: &str = "cleanup";

pub(crate) use config::*;
#[cfg(test)]
pub(crate) use init::*;
pub(crate) use lifecycle::*;
#[cfg(test)]
pub(crate) use shell::*;
