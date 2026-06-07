use crate::{
    CliResult,
    args::{CompleteArgs, CompletionKind},
    removal::worktree_branch_or_handle,
    write_stdout,
};
use std::path::PathBuf;

use hz_core::HzResult;

pub(crate) fn complete(args: CompleteArgs) -> CliResult<()> {
    let include_local = args.kind == CompletionKind::WorktreeTargets;
    let Ok(candidates) = worktree_completion_candidates(args.repo, include_local) else {
        return Ok(());
    };

    for candidate in candidates {
        write_stdout(format_args!("{candidate}\n"))?;
    }

    Ok(())
}

pub(crate) fn worktree_completion_candidates(
    repo: Option<PathBuf>,
    include_local: bool,
) -> HzResult<Vec<String>> {
    let worktrees = hz_command::list_worktree_targets(hz_command::ListWorktrees { repo })?;
    let mut candidates = Vec::new();

    if include_local {
        candidates.push("local".to_owned());
    }

    for worktree in worktrees {
        push_worktree_completion_candidate(&mut candidates, &worktree);
    }

    Ok(candidates)
}

pub(crate) fn push_worktree_completion_candidate(
    candidates: &mut Vec<String>,
    worktree: &hz_command::WorktreeEntry,
) {
    push_completion_candidate(
        candidates,
        Some(worktree_branch_or_handle(worktree).to_owned()),
    );
}

pub(crate) fn push_completion_candidate(candidates: &mut Vec<String>, candidate: Option<String>) {
    let Some(candidate) = candidate else {
        return;
    };

    if !candidate.is_empty() && !candidates.iter().any(|existing| existing == &candidate) {
        candidates.push(candidate);
    }
}
