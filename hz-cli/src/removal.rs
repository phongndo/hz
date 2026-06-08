use std::{
    collections::HashSet,
    io::{self, IsTerminal, Write},
};

use hz_core::HzResult;

use crate::{
    CliResult,
    args::{HandoffWorktreeArgs, RemoveWorktreeArgs},
    worktree_output::{
        StyleColor, print_warnings, render_handoff, render_removed_worktree, styled,
    },
    write_stderr, write_stdout,
};

pub(crate) fn remove_worktree(args: RemoveWorktreeArgs) -> CliResult<()> {
    let debug = args.debug;
    let force = args.force;
    let requested_target_count = args.targets.len();
    let candidates = find_removal_candidates(&args)?;
    let mut removable = Vec::new();
    let mut removed = Vec::new();

    for candidate in candidates {
        if candidate.confirm_unmanaged && !confirm_unmanaged_removal(&candidate.worktree)? {
            write_stderr(format_args!("not removed\n"))?;
            continue;
        }

        removable.push(candidate.worktree);
    }

    if !args.no_cleanup {
        for candidate in &removable {
            if should_run_cleanup_for_removal(candidate) {
                hz_command::run_lifecycle_for_entry(candidate, hz_command::LifecycleKind::Cleanup)?;
            }
        }
    }

    let mut removal_errors = Vec::new();
    for candidate in removable {
        let target = worktree_branch_or_handle(&candidate).to_owned();
        match hz_command::remove_found_worktree_with_force(candidate, force) {
            Ok(entry) => removed.push(entry),
            Err(error) => removal_errors.push(format!("{target}: {error}")),
        }
    }

    if args.json {
        write_stdout(format_args!(
            "{}\n",
            removed_worktrees_json(requested_target_count, &removed)?
        ))?;
    } else if debug {
        for entry in &removed {
            write_stdout(format_args!(
                "{}",
                render_removed_worktree(entry, io::stdout().is_terminal())
            ))?;
        }
    }

    if !removal_errors.is_empty() {
        return Err(hz_core::HzError::Usage(format!(
            "failed to remove one or more worktrees: {}",
            removal_errors.join("; ")
        ))
        .into());
    }

    Ok(())
}

#[derive(Debug)]
pub(crate) struct RemovalCandidate {
    pub(crate) worktree: hz_command::WorktreeEntry,
    pub(crate) confirm_unmanaged: bool,
}

pub(crate) fn find_removal_candidates(
    args: &RemoveWorktreeArgs,
) -> HzResult<Vec<RemovalCandidate>> {
    let mut candidates = Vec::with_capacity(args.targets.len());
    let mut seen = HashSet::new();

    for target in &args.targets {
        let candidate = hz_command::find_worktree(hz_command::FindWorktree {
            target: target.clone(),
            repo: args.repo.clone(),
        })?;

        if !seen.insert((candidate.repo.clone(), candidate.path.clone())) {
            return Err(hz_core::HzError::Usage(format!(
                "duplicate worktree target: {target}"
            )));
        }

        let confirm_unmanaged = should_confirm_unmanaged_removal(args, &candidate)?;
        candidates.push(RemovalCandidate {
            worktree: candidate,
            confirm_unmanaged,
        });
    }

    Ok(candidates)
}

pub(crate) fn removed_worktrees_json(
    requested_target_count: usize,
    removed: &[hz_command::WorktreeEntry],
) -> HzResult<String> {
    if requested_target_count == 1
        && let [entry] = removed
    {
        return Ok(serde_json::to_string_pretty(entry)?);
    }

    Ok(serde_json::to_string_pretty(removed)?)
}

pub(crate) fn should_confirm_unmanaged_removal(
    args: &RemoveWorktreeArgs,
    worktree: &hz_command::WorktreeEntry,
) -> HzResult<bool> {
    should_confirm_unmanaged_removal_with_stdin(args, worktree, io::stdin().is_terminal())
}

pub(crate) fn should_confirm_unmanaged_removal_with_stdin(
    args: &RemoveWorktreeArgs,
    worktree: &hz_command::WorktreeEntry,
    stdin_is_terminal: bool,
) -> HzResult<bool> {
    if worktree.source == hz_command::WorktreeSource::Managed
        || args.force
        || hz_command::is_user_managed_worktree_path(worktree)?
    {
        return Ok(false);
    }

    if args.json {
        return Err(hz_core::HzError::Usage(
            "refusing to remove unmanaged worktree in --json mode without --force".to_owned(),
        ));
    }

    if !stdin_is_terminal {
        return Err(hz_core::HzError::Usage(
            "refusing to prompt for unmanaged worktree removal without a terminal; use --force"
                .to_owned(),
        ));
    }

    Ok(true)
}

pub(crate) fn confirm_unmanaged_removal(worktree: &hz_command::WorktreeEntry) -> CliResult<bool> {
    let color = io::stderr().is_terminal();
    write_stderr(format_args!(
        "{} {} at {} is not managed by hz. Delete it with git worktree remove? [y/N] ",
        styled("!", StyleColor::Yellow, color),
        styled(
            worktree_branch_or_handle(worktree),
            StyleColor::White,
            color
        ),
        styled(
            &worktree.path.display().to_string(),
            StyleColor::White,
            color
        )
    ))?;
    io::stderr().flush()?;

    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes" | "YES" | "Yes"))
}

pub(crate) fn should_run_cleanup_for_removal(worktree: &hz_command::WorktreeEntry) -> bool {
    worktree.source == hz_command::WorktreeSource::Managed
        || hz_command::is_user_managed_worktree_path(worktree).unwrap_or(false)
}

pub(crate) fn worktree_branch_or_handle(worktree: &hz_command::WorktreeEntry) -> &str {
    worktree.branch.as_deref().unwrap_or(&worktree.handle)
}

pub(crate) fn handoff_worktree(args: HandoffWorktreeArgs) -> CliResult<()> {
    let handoff = hz_command::handoff_worktree(hz_command::HandoffWorktree {
        target: args.target,
        mode: if args.branch {
            hz_command::HandoffMode::Branch
        } else {
            hz_command::HandoffMode::Patch
        },
        repo: args.repo,
        create: args.create,
        max_detached_worktrees: args.max_detached,
        max_branch_worktrees: args.max_branch_worktrees,
    })?;

    if args.json {
        write_stdout(format_args!(
            "{}\n",
            serde_json::to_string_pretty(&handoff)?
        ))?;
    } else if args.path_only {
        write_stdout(format_args!("{}\n", handoff.to.path.display()))?;
        print_warnings(&handoff.warnings, io::stderr().is_terminal())?;
    } else {
        write_stdout(format_args!(
            "{}",
            render_handoff(&handoff, io::stdout().is_terminal())
        ))?;
    }

    Ok(())
}
