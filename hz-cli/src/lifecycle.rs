use std::io::{self, IsTerminal};

use crate::{CliResult, args::LifecycleArgs, worktree_output::render_lifecycle_run, write_stdout};

pub(crate) fn run_lifecycle(args: LifecycleArgs, kind: hz_command::LifecycleKind) -> CliResult<()> {
    let run = run_lifecycle_command(args, kind)?;
    write_stdout(format_args!(
        "{}",
        render_lifecycle_run(&run, io::stdout().is_terminal())
    ))?;
    Ok(())
}

pub(crate) fn run_lifecycle_json(
    args: LifecycleArgs,
    kind: hz_command::LifecycleKind,
) -> CliResult<()> {
    let run = run_lifecycle_command(args, kind)?;
    write_stdout(format_args!("{}\n", serde_json::to_string_pretty(&run)?))?;
    Ok(())
}

fn run_lifecycle_command(
    args: LifecycleArgs,
    kind: hz_command::LifecycleKind,
) -> CliResult<hz_command::LifecycleRun> {
    Ok(hz_command::run_lifecycle(hz_command::RunLifecycle {
        target: args.target,
        repo: args.repo,
        kind,
    })?)
}
