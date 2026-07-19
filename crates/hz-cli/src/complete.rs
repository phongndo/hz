use std::env;

use crate::{
    CliResult,
    args::{CompleteArgs, CompletionKind},
    write_stdout,
};

pub(crate) fn complete(args: CompleteArgs) -> CliResult<()> {
    let at = args.at.unwrap_or(env::current_dir()?);
    let workspaces = match args.kind {
        CompletionKind::WorkspaceTargets => {
            hz_command::list_workspaces(hz_command::ListWorkspaces {
                of: Some(at),
                scope: hz_command::ListScope::Family,
                pinned: None,
            })
        }
        CompletionKind::TrashTargets => hz_command::trashed_workspaces(at),
    };
    let workspaces = match workspaces {
        Ok(workspaces) => workspaces,
        Err(_) => return Ok(()),
    };
    for workspace in workspaces {
        write_stdout(format_args!("{}\n", workspace.handle))?;
    }
    Ok(())
}
