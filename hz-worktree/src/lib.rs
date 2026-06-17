mod create;
mod fork;
mod handles;
mod handoff;
mod list;
mod prune;
mod registry;
mod remove;
mod target;
#[cfg(test)]
mod tests;
mod types;
mod worktree_include;

pub use create::{create, path};
pub use fork::fork;
pub use handoff::handoff;
pub use list::{current_path, find, find_many, list, list_targets, local};
pub use registry::is_hz_worktree_path;
pub use remove::{remove, remove_found, remove_found_with_force};
pub use types::{
    CreateWorktree, CreatedWorktree, DEFAULT_MAX_BRANCH_WORKTREES, DEFAULT_MAX_DETACHED_WORKTREES,
    FindWorktree, FindWorktrees, ForkWorktree, ForkedWorktree, HandoffMode, HandoffWorktree,
    ListWorktrees, LocalWorktree, LocalWorktreeInfo, PathWorktree, RemoveWorktree, WorktreeEntry,
    WorktreeHandoff, WorktreeSource, WorktreeStatus,
};

pub(crate) use create::*;
#[cfg(test)]
pub(crate) use fork::*;
pub(crate) use handles::*;
pub(crate) use handoff::*;
pub(crate) use list::*;
pub(crate) use prune::*;
pub(crate) use registry::*;
pub(crate) use remove::*;
pub(crate) use target::*;
pub(crate) use types::*;
pub(crate) use worktree_include::*;
