use clap::Parser;

use crate::args::{Cli, Command, GitCommand, HgCommand};

#[test]
fn workspace_commands_are_top_level() {
    assert!(matches!(
        Cli::try_parse_from(["hz", "new", "child"]).unwrap().command,
        Some(Command::New(_))
    ));
    assert!(matches!(
        Cli::try_parse_from(["hz", "list", "--tree"])
            .unwrap()
            .command,
        Some(Command::List(_))
    ));
    assert!(matches!(
        Cli::try_parse_from(["hz", "rm", "child"]).unwrap().command,
        Some(Command::Remove(_))
    ));
}

#[test]
fn git_operations_remain_namespaced() {
    assert!(matches!(
        Cli::try_parse_from(["hz", "git", "status"])
            .unwrap()
            .command,
        Some(Command::Git {
            command: GitCommand::Status(_)
        })
    ));
    assert!(matches!(
        Cli::try_parse_from(["hz", "git", "handoff", "root"])
            .unwrap()
            .command,
        Some(Command::Git {
            command: GitCommand::Handoff(_)
        })
    ));
    assert!(matches!(
        Cli::try_parse_from(["hz", "hg", "status"]).unwrap().command,
        Some(Command::Hg {
            command: HgCommand::Status(_)
        })
    ));
}

#[test]
fn machine_is_global() {
    assert!(
        Cli::try_parse_from(["hz", "new", "--machine"])
            .unwrap()
            .machine
    );
    assert!(
        Cli::try_parse_from(["hz", "--machine", "new"])
            .unwrap()
            .machine
    );
}
