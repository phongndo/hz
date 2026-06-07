#![allow(unused_imports)]

use crate::*;
use std::{
    collections::HashSet,
    env,
    ffi::{OsStr, OsString},
    fs,
    io::{self, IsTerminal, Read, Write},
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, ExitCode, Stdio},
    sync::Arc,
};

use clap::{
    Args, Parser, Subcommand, ValueEnum,
    builder::styling::{AnsiColor, Styles},
};
use crossterm::terminal as crossterm_terminal;
use hz_core::HzResult;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub(crate) fn complete(args: CompleteArgs) -> HzResult<()> {
    let include_local = args.kind == CompletionKind::WorktreeTargets;
    let Ok(candidates) = worktree_completion_candidates(args.repo, include_local) else {
        return Ok(());
    };

    for candidate in candidates {
        println!("{candidate}");
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
