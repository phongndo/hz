use crate::{
    CliResult,
    args::AgentCommand,
    lifecycle::run_lifecycle_json,
    removal::{handoff_worktree, remove_worktree_json_array},
    worktree_output::{
        create_worktree, fork_worktree, list_worktrees, path_worktree, pin_worktree, pwd_worktree,
        unpin_worktree,
    },
};

pub(crate) fn run_agent_command(command: AgentCommand) -> CliResult<()> {
    match command {
        AgentCommand::New(mut args) => {
            args.json = true;
            args.debug = false;
            args.path_only = false;
            create_worktree(args)
        }
        AgentCommand::Fork(mut args) => {
            args.json = true;
            args.path_only = false;
            fork_worktree(args)
        }
        AgentCommand::Path(mut args) => {
            args.json = true;
            args.path_only = false;
            path_worktree(args)
        }
        AgentCommand::List(mut args) => {
            args.json = true;
            list_worktrees(args)
        }
        AgentCommand::Pwd(mut args) => {
            args.json = true;
            pwd_worktree(args)
        }
        AgentCommand::Remove(mut args) => {
            args.json = true;
            args.debug = false;
            remove_worktree_json_array(args)
        }
        AgentCommand::Pin(mut args) => {
            args.json = true;
            pin_worktree(args)
        }
        AgentCommand::Unpin(mut args) => {
            args.json = true;
            unpin_worktree(args)
        }
        AgentCommand::Handoff(mut args) => {
            args.json = true;
            args.path_only = false;
            handoff_worktree(args)
        }
        AgentCommand::Setup(args) => run_lifecycle_json(args, hz_command::LifecycleKind::Setup),
        AgentCommand::Cleanup(args) => run_lifecycle_json(args, hz_command::LifecycleKind::Cleanup),
    }
}
