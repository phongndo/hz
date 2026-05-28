use hz_core::HzResult;

pub use hz_diff::DiffOptions;
pub use hz_worktree::{
    CreateWorktree, CreatedWorktree, HandoffWorktree, SwitchWorktree, WorktreeHandoff,
};

pub fn create_worktree(input: CreateWorktree) -> HzResult<CreatedWorktree> {
    hz_worktree::create(input)
}

pub fn switch_worktree(input: SwitchWorktree) -> HzResult<hz_core::paths::WorktreeTarget> {
    hz_worktree::switch(input)
}

pub fn handoff_worktree(input: HandoffWorktree) -> HzResult<WorktreeHandoff> {
    hz_worktree::handoff(input)
}

pub fn diff(input: DiffOptions) -> HzResult<String> {
    hz_diff::render(input)
}
