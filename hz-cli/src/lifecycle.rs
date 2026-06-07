use std::io::{self, IsTerminal};

use hz_core::HzResult;

use crate::{args::LifecycleArgs, worktree_output::render_lifecycle_run};

pub(crate) fn run_lifecycle(args: LifecycleArgs, kind: hz_command::LifecycleKind) -> HzResult<()> {
    let run = hz_command::run_lifecycle(hz_command::RunLifecycle {
        target: args.target,
        repo: args.repo,
        kind,
    })?;
    print!("{}", render_lifecycle_run(&run, io::stdout().is_terminal()));
    Ok(())
}
