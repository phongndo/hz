use std::{
    collections::{BTreeSet, HashMap},
    env,
    ffi::{OsStr, OsString},
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
    time::{SystemTime, UNIX_EPOCH},
};

use super::*;
use crate::{args::*, complete::*, removal::*, update::*, worktree_output::*};

struct FailingWriter(io::ErrorKind);

impl Write for FailingWriter {
    fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
        Err(io::Error::from(self.0))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct FlushFailingWriter {
    bytes: Vec<u8>,
}

impl Write for FlushFailingWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.bytes.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Err(io::Error::from(io::ErrorKind::BrokenPipe))
    }
}

#[test]
fn stdout_write_ignores_broken_pipe() {
    assert!(
        write_all_ignore_broken_pipe(FailingWriter(io::ErrorKind::BrokenPipe), b"diff").is_ok()
    );
}

#[test]
fn stdout_flush_ignores_broken_pipe_after_successful_write() {
    let mut writer = FlushFailingWriter { bytes: Vec::new() };

    assert!(write_all_ignore_broken_pipe(&mut writer, b"diff").is_ok());
    assert_eq!(writer.bytes, b"diff");
}

#[test]
fn stdout_write_returns_other_errors() {
    let error =
        write_all_ignore_broken_pipe(FailingWriter(io::ErrorKind::PermissionDenied), b"diff")
            .unwrap_err();

    assert!(matches!(
        error,
        CliError::Hz(hz_core::HzError::Io(error))
            if error.kind() == io::ErrorKind::PermissionDenied
    ));
}

#[test]
fn stdout_broken_pipe_errors_exit_cleanly() {
    let error = stdout_write_error(io::Error::from(io::ErrorKind::BrokenPipe));

    assert!(is_clean_exit_error(&error));
}

#[test]
fn non_stdout_or_non_broken_pipe_errors_do_not_exit_cleanly() {
    let stderr_broken_pipe = CliError::from(hz_core::HzError::Io(io::Error::from(
        io::ErrorKind::BrokenPipe,
    )));
    let stdout_permission_denied =
        stdout_write_error(io::Error::from(io::ErrorKind::PermissionDenied));
    let json_error = CliError::from(serde_json::from_str::<serde_json::Value>("{").unwrap_err());
    let usage_error = CliError::from(hz_core::HzError::Usage("usage".to_owned()));

    assert!(!is_clean_exit_error(&stderr_broken_pipe));
    assert!(!is_clean_exit_error(&stdout_permission_denied));
    assert!(!is_clean_exit_error(&json_error));
    assert!(!is_clean_exit_error(&usage_error));
}

#[test]
fn default_help_renders_usage() {
    let mut output = Vec::new();

    write_default_help(&mut output).unwrap();

    let help = String::from_utf8(output).unwrap();
    assert!(help.contains("usage:"));
    assert!(help.contains("commands:"));
    assert!(!help.contains("Hello word"));
}

#[test]
fn agent_commands_parse_under_machine_namespace() {
    let cli = Cli::try_parse_from(["hz", "agent", "current"]).unwrap();
    assert!(matches!(
        cli.command,
        Some(Command::Agent {
            command: AgentCommand::Pwd(_)
        })
    ));

    let cli = Cli::try_parse_from(["hz", "agent", "rm", "feature/ui"]).unwrap();
    let Some(Command::Agent {
        command: AgentCommand::Remove(args),
    }) = cli.command
    else {
        panic!("agent rm should parse as agent remove");
    };
    assert_eq!(args.targets, vec!["feature/ui".to_owned()]);
}

#[test]
fn list_output_uses_branch_as_display_identifier() {
    let output = render_worktree_list(&[hz_command::WorktreeEntry {
        id: "entry-id".to_owned(),
        handle: "generated-handle".to_owned(),
        repo: PathBuf::from("/repo"),
        path: PathBuf::from("/worktrees/entry-id"),
        branch: Some("feature/ui".to_owned()),
        base: None,
        source: hz_command::WorktreeSource::Managed,
        created_at_unix: 0,
        modified_at_unix: 0,
        status: hz_command::WorktreeStatus::Unknown,
    }]);

    assert!(output.contains("target"));
    assert!(output.contains("feature/ui"));
    assert!(!output.contains("generated-handle"));
}

#[test]
fn list_output_is_empty_when_there_are_no_worktrees() {
    assert_eq!(render_worktree_list(&[]), "");
}

#[test]
fn list_output_uses_handle_when_branch_is_missing() {
    let output = render_worktree_list(&[hz_command::WorktreeEntry {
        id: "entry-id".to_owned(),
        handle: "generated-handle".to_owned(),
        repo: PathBuf::from("/repo"),
        path: PathBuf::from("/worktrees/entry"),
        branch: None,
        base: None,
        source: hz_command::WorktreeSource::Git,
        created_at_unix: 0,
        modified_at_unix: 0,
        status: hz_command::WorktreeStatus::Unknown,
    }]);
    let row = output
        .lines()
        .nth(1)
        .expect("worktree row should be rendered");
    let columns: Vec<_> = row.split_whitespace().collect();

    assert_eq!(
        columns,
        vec!["generated-handle", "?", "-", "/worktrees/entry"]
    );
}

#[test]
fn home_relative_paths_use_tilde_only_for_home_children() {
    let home = PathBuf::from("/Users/dev");

    assert_eq!(
        home_relative_path(&PathBuf::from("/Users/dev/.hz/worktrees/hz"), &home).as_deref(),
        Some("~/.hz/worktrees/hz")
    );
    assert_eq!(
        home_relative_path(&PathBuf::from("/Users/dev"), &home).as_deref(),
        Some("~")
    );
    assert_eq!(
        home_relative_path(&PathBuf::from("/Users/dev-other/project"), &home),
        None
    );
}

#[test]
fn list_output_widths_count_terminal_columns() {
    let output = render_worktree_list(&[hz_command::WorktreeEntry {
        id: "entry-id".to_owned(),
        handle: "generated-handle".to_owned(),
        repo: PathBuf::from("/repo"),
        path: PathBuf::from("/worktrees/entry"),
        branch: Some("ééééé".to_owned()),
        base: None,
        source: hz_command::WorktreeSource::Managed,
        created_at_unix: 0,
        modified_at_unix: 0,
        status: hz_command::WorktreeStatus::Unknown,
    }]);

    assert!(output.starts_with("  target status modified path"));
}

#[test]
fn list_output_omits_source_column() {
    let output = render_worktree_list(&[
        hz_command::WorktreeEntry {
            id: "managed-id".to_owned(),
            handle: "alpha".to_owned(),
            repo: PathBuf::from("/repo"),
            path: PathBuf::from("/worktrees/alpha"),
            branch: Some("alpha".to_owned()),
            base: None,
            source: hz_command::WorktreeSource::Managed,
            created_at_unix: 0,
            modified_at_unix: 0,
            status: hz_command::WorktreeStatus::Unknown,
        },
        hz_command::WorktreeEntry {
            id: "git-id".to_owned(),
            handle: "beta".to_owned(),
            repo: PathBuf::from("/repo"),
            path: PathBuf::from("/worktrees/beta"),
            branch: None,
            base: None,
            source: hz_command::WorktreeSource::Git,
            created_at_unix: 0,
            modified_at_unix: 0,
            status: hz_command::WorktreeStatus::Unknown,
        },
    ]);

    assert!(!output.contains("source"));
    assert!(!output.lines().any(|line| line.ends_with(" managed")));
    assert!(!output.lines().any(|line| line.ends_with(" git")));
}

#[test]
fn list_output_renders_status_and_modified_columns() {
    let dirty_at = unix_now();
    let created_at = dirty_at.saturating_sub(60 * 60);
    let output = render_worktree_list(&[
        hz_command::WorktreeEntry {
            id: "dirty-id".to_owned(),
            handle: "dirty-worktree".to_owned(),
            repo: PathBuf::from("/repo"),
            path: PathBuf::from("/worktrees/dirty"),
            branch: Some("dirty-worktree".to_owned()),
            base: None,
            source: hz_command::WorktreeSource::Managed,
            created_at_unix: 0,
            modified_at_unix: dirty_at,
            status: hz_command::WorktreeStatus::Dirty,
        },
        hz_command::WorktreeEntry {
            id: "clean-id".to_owned(),
            handle: "clean-worktree".to_owned(),
            repo: PathBuf::from("/repo"),
            path: PathBuf::from("/worktrees/clean"),
            branch: Some("clean-worktree".to_owned()),
            base: None,
            source: hz_command::WorktreeSource::Managed,
            created_at_unix: created_at,
            modified_at_unix: 0,
            status: hz_command::WorktreeStatus::Clean,
        },
    ]);

    let headers = output
        .lines()
        .next()
        .expect("header should render")
        .split_whitespace()
        .collect::<Vec<_>>();
    assert_eq!(headers[0], "target");
    assert_eq!(headers[1], "status");
    assert!(output.contains("modified"));
    assert!(output.contains("!"));
    assert!(output.contains("ok"));
    assert!(output.contains(&format_modified_at(dirty_at)));
    assert!(output.contains(&format_modified_at(created_at)));
}

#[test]
fn modified_datetime_uses_date_like_shape() {
    let datetime = time::OffsetDateTime::from_unix_timestamp(1_704_067_200).unwrap();

    assert_eq!(format_modified_datetime(datetime), "Jan  1 00:00");
}

#[test]
fn unix_timestamp_uses_date_like_shape() {
    let formatted = format_unix_timestamp(1_704_067_200).expect("timestamp should format");

    let month = formatted
        .get(0..3)
        .expect("formatted timestamp should start with an ASCII month");
    assert!(matches!(
        month,
        "Jan"
            | "Feb"
            | "Mar"
            | "Apr"
            | "May"
            | "Jun"
            | "Jul"
            | "Aug"
            | "Sep"
            | "Oct"
            | "Nov"
            | "Dec"
    ));
    assert_eq!(formatted.len(), "Jan  1 00:00".len());
}

#[test]
fn list_output_centers_unicode_status() {
    let output = render_worktree_rows_with_options(
        &[WorktreeListRow {
            target: "feature/ui".to_owned(),
            branch: Some("feature/ui".to_owned()),
            handle: Some("f7a7".to_owned()),
            base: None,
            status: hz_command::WorktreeStatus::Clean,
            modified_at_unix: 0,
            path: PathBuf::from("/worktrees/entry"),
            local: false,
            current: false,
        }],
        false,
        list_glyphs(true),
        None,
        WorktreeListOptions {
            headers: hz_command::ListHeaders::Always,
            columns: vec![hz_command::ListColumn::Status],
            ..WorktreeListOptions::default()
        },
    );

    let status = output.lines().nth(1).expect("status row should render");

    assert_eq!(status, "  ✓   ");
}

#[test]
fn list_output_uses_configured_headers_and_columns() {
    let output = render_worktree_rows_with_options(
        &[WorktreeListRow {
            target: "feature/ui".to_owned(),
            branch: Some("feature/ui".to_owned()),
            handle: Some("f7a7".to_owned()),
            base: Some("dev".to_owned()),
            status: hz_command::WorktreeStatus::Clean,
            modified_at_unix: 0,
            path: PathBuf::from("/worktrees/entry"),
            local: false,
            current: false,
        }],
        false,
        list_glyphs(false),
        None,
        WorktreeListOptions {
            headers: hz_command::ListHeaders::Always,
            columns: vec![
                hz_command::ListColumn::Branch,
                hz_command::ListColumn::Base,
                hz_command::ListColumn::Status,
            ],
            ..WorktreeListOptions::default()
        },
    );

    assert!(output.starts_with("branch"));
    assert!(output.contains("base"));
    assert!(output.contains("feature/ui"));
    assert!(output.contains("dev"));
    assert!(!output.contains("path"));
}

#[test]
fn list_output_can_render_terminal_color() {
    let output = render_worktree_list_with_style(
        &[hz_command::WorktreeEntry {
            id: "entry-id".to_owned(),
            handle: "generated-handle".to_owned(),
            repo: PathBuf::from("/repo"),
            path: PathBuf::from("/worktrees/entry"),
            branch: Some("feature/ui".to_owned()),
            base: None,
            source: hz_command::WorktreeSource::Managed,
            created_at_unix: 0,
            modified_at_unix: 0,
            status: hz_command::WorktreeStatus::Unknown,
        }],
        true,
    );

    assert!(output.contains("\x1b["));
    assert!(output.contains("\x1b[35mfeature/ui"));
    assert!(output.contains("\x1b[37m/worktrees/entry"));
    assert!(!output.contains("\x1b[34m"));
}

#[test]
fn list_output_can_render_custom_color_scheme() {
    let mut schemes = HashMap::new();
    schemes.insert(
        "blueprint".to_owned(),
        hz_command::ColorSchemeConfig {
            target: Some("blue".to_owned()),
            clean: Some("cyan".to_owned()),
            ..hz_command::ColorSchemeConfig::default()
        },
    );
    let options = WorktreeListOptions {
        colors: list_colors(Some(&hz_command::ColorConfig {
            mode: None,
            scheme: Some("blueprint".to_owned()),
            schemes,
        })),
        ..WorktreeListOptions::default()
    };
    let output = render_worktree_rows_with_options(
        &[WorktreeListRow {
            target: "feature/ui".to_owned(),
            branch: Some("feature/ui".to_owned()),
            handle: Some("f7a7".to_owned()),
            base: None,
            status: hz_command::WorktreeStatus::Clean,
            modified_at_unix: 0,
            path: PathBuf::from("/worktrees/entry"),
            local: false,
            current: false,
        }],
        true,
        list_glyphs(false),
        None,
        options,
    );

    assert!(output.contains("\x1b[34mfeature/ui"));
    assert!(output.contains("\x1b[36m  ok"));
}

#[test]
fn list_output_marks_current_worktree() {
    let local = hz_command::LocalWorktreeInfo {
        repo: PathBuf::from("/repo"),
        path: PathBuf::from("/repo"),
        branch: Some("main".to_owned()),
        status: hz_command::WorktreeStatus::Clean,
        modified_at_unix: 0,
        handoff_from: None,
    };
    let output = render_worktree_list_with_context(
        &local,
        &[hz_command::WorktreeEntry {
            id: "entry-id".to_owned(),
            handle: "generated-handle".to_owned(),
            repo: PathBuf::from("/repo"),
            path: PathBuf::from("/worktrees/entry"),
            branch: Some("feature/ui".to_owned()),
            base: None,
            source: hz_command::WorktreeSource::Managed,
            created_at_unix: 0,
            modified_at_unix: 0,
            status: hz_command::WorktreeStatus::Unknown,
        }],
        Some(&PathBuf::from("/worktrees/entry")),
        false,
        list_glyphs(false),
        None,
    );

    let current_row = output
        .lines()
        .find(|line| line.contains("feature/ui"))
        .expect("current worktree row should be rendered");

    assert!(current_row.starts_with("@ feature/ui"));
    assert!(output.contains("~ local"));
    assert!(!output.contains("note"));
}

#[test]
fn list_output_does_not_mark_local_without_current_worktree() {
    let local = hz_command::LocalWorktreeInfo {
        repo: PathBuf::from("/repo"),
        path: PathBuf::from("/repo"),
        branch: Some("main".to_owned()),
        status: hz_command::WorktreeStatus::Clean,
        modified_at_unix: 0,
        handoff_from: None,
    };
    let output =
        render_worktree_list_with_context(&local, &[], None, false, list_glyphs(false), None);

    let local_row = output
        .lines()
        .find(|line| line.contains("local"))
        .expect("local worktree row should be rendered");

    assert!(local_row.starts_with("~ local"));
    assert!(!local_row.starts_with("@ local"));
}

#[test]
fn local_list_row_omits_note_column() {
    let local = hz_command::LocalWorktreeInfo {
        repo: PathBuf::from("/repo"),
        path: PathBuf::from("/repo"),
        branch: Some("feature/ui".to_owned()),
        status: hz_command::WorktreeStatus::Dirty,
        modified_at_unix: 0,
        handoff_from: Some("f7a7".to_owned()),
    };
    let output = render_worktree_list_with_context(
        &local,
        &[],
        Some(&PathBuf::from("/repo")),
        false,
        list_glyphs(false),
        None,
    );

    assert!(output.contains("@ local"));
    assert!(!output.contains("note"));
    assert!(!output.contains("branch feature/ui"));
    assert!(!output.contains("<- f7a7"));
}

#[test]
fn list_output_can_render_unicode_glyphs() {
    let local = hz_command::LocalWorktreeInfo {
        repo: PathBuf::from("/repo"),
        path: PathBuf::from("/repo"),
        branch: Some("feature/ui".to_owned()),
        status: hz_command::WorktreeStatus::Clean,
        modified_at_unix: 0,
        handoff_from: Some("f7a7".to_owned()),
    };
    let output = render_worktree_list_with_context(
        &local,
        &[],
        Some(&PathBuf::from("/repo")),
        true,
        list_glyphs(true),
        None,
    );

    assert!(output.contains("●"));
    assert!(output.contains("✓"));
    assert!(!output.contains("note"));
    assert!(!output.contains("branch feature/ui"));
    assert!(!output.contains("← f7a7"));
}

#[test]
fn list_output_truncates_to_terminal_width() {
    let local = hz_command::LocalWorktreeInfo {
        repo: PathBuf::from("/repo"),
        path: PathBuf::from("/repo"),
        branch: Some("main".to_owned()),
        status: hz_command::WorktreeStatus::Clean,
        modified_at_unix: 0,
        handoff_from: None,
    };
    let output = render_worktree_list_with_context(
        &local,
        &[hz_command::WorktreeEntry {
            id: "entry-id".to_owned(),
            handle: "generated-handle".to_owned(),
            repo: PathBuf::from("/repo"),
            path: PathBuf::from(
                "/very/long/worktrees/path/that/would/otherwise/wrap/in/a/small/terminal",
            ),
            branch: Some(
                "feat(worktree)/very-long-branch-name-that-would-push-the-table".to_owned(),
            ),
            base: None,
            source: hz_command::WorktreeSource::Managed,
            created_at_unix: 0,
            modified_at_unix: 0,
            status: hz_command::WorktreeStatus::Clean,
        }],
        Some(&PathBuf::from("/worktrees/entry")),
        false,
        list_glyphs(true),
        Some(72),
    );

    assert!(output.contains("…"));
    assert!(output.lines().all(|line| display_width(line) <= 72));
}

#[test]
fn list_output_uses_compact_rows_for_tiny_terminals() {
    let output = render_worktree_rows(
        &[WorktreeListRow {
            target: "feat(worktree)/very-long-branch-name".to_owned(),
            branch: Some("feat(worktree)/very-long-branch-name".to_owned()),
            handle: Some("handle".to_owned()),
            base: None,
            status: hz_command::WorktreeStatus::Dirty,
            modified_at_unix: 0,
            path: PathBuf::from("/very/long/worktree/path"),
            local: false,
            current: false,
        }],
        false,
        list_glyphs(true),
        Some(32),
    );

    assert!(!output.contains("target"));
    assert!(output.lines().all(|line| display_width(line) <= 32));
}

#[test]
fn compact_rows_truncate_configured_columns_to_terminal_width() {
    let output = render_worktree_rows_with_options(
        &[WorktreeListRow {
            target: "feat(worktree)/very-long-branch-name".to_owned(),
            branch: Some("feat(worktree)/very-long-branch-name".to_owned()),
            handle: Some("handle".to_owned()),
            base: None,
            status: hz_command::WorktreeStatus::Dirty,
            modified_at_unix: 0,
            path: PathBuf::from("/very/long/worktree/path"),
            local: false,
            current: false,
        }],
        false,
        list_glyphs(true),
        Some(36),
        WorktreeListOptions {
            compact_columns: vec![
                hz_command::ListColumn::Marker,
                hz_command::ListColumn::Target,
                hz_command::ListColumn::Path,
            ],
            ..WorktreeListOptions::default()
        },
    );

    assert!(output.contains("…"));
    assert!(output.lines().all(|line| display_width(line) <= 36));
}

#[test]
fn display_width_uses_terminal_columns() {
    assert_eq!(display_width("測試"), 4);

    let truncated = truncate_middle("feature/測試/worktree", 12, list_glyphs(true));

    assert!(truncated.contains("…"));
    assert!(display_width(&truncated) <= 12);
}

#[test]
fn created_output_renders_human_summary() {
    let output = render_created_worktree(
        &hz_command::CreatedWorktree {
            id: "entry-id".to_owned(),
            name: "generated-handle".to_owned(),
            handle: "generated-handle".to_owned(),
            repo: PathBuf::from("/repo"),
            path: PathBuf::from("/worktrees/entry"),
            branch: Some("feature/ui".to_owned()),
            base: Some("main".to_owned()),
            source: hz_command::WorktreeSource::Managed,
            warnings: Vec::new(),
        },
        false,
    );

    assert!(output.starts_with("+ created  feature/ui"));
    assert!(output.contains("handle  generated-handle"));
    assert!(output.contains("path    /worktrees/entry"));
    assert!(output.contains("base    main"));
}

#[test]
fn created_output_renders_detached_worktree_summary() {
    let output = render_created_worktree(
        &hz_command::CreatedWorktree {
            id: "entry-id".to_owned(),
            name: "generated-handle".to_owned(),
            handle: "generated-handle".to_owned(),
            repo: PathBuf::from("/repo"),
            path: PathBuf::from("/worktrees/entry"),
            branch: None,
            base: None,
            source: hz_command::WorktreeSource::Managed,
            warnings: Vec::new(),
        },
        false,
    );

    assert!(output.starts_with("+ created  generated-handle"));
    assert!(output.contains("branch  detached"));
    assert!(output.contains("path    /worktrees/entry"));
}

#[test]
fn created_output_renders_prune_warnings() {
    let output = render_created_worktree(
        &hz_command::CreatedWorktree {
            id: "entry-id".to_owned(),
            name: "generated-handle".to_owned(),
            handle: "generated-handle".to_owned(),
            repo: PathBuf::from("/repo"),
            path: PathBuf::from("/worktrees/entry"),
            branch: None,
            base: None,
            source: hz_command::WorktreeSource::Managed,
            warnings: vec![
                "created worktree, but failed to prune detached worktrees: permission denied"
                    .to_owned(),
            ],
        },
        false,
    );

    assert!(output.contains(
        "warning  created worktree, but failed to prune detached worktrees: permission denied"
    ));
}

#[test]
fn removed_output_renders_human_summary() {
    let output = render_removed_worktree(
        &hz_command::WorktreeEntry {
            id: "entry-id".to_owned(),
            handle: "generated-handle".to_owned(),
            repo: PathBuf::from("/repo"),
            path: PathBuf::from("/worktrees/entry"),
            branch: Some("feature/ui".to_owned()),
            base: None,
            source: hz_command::WorktreeSource::Managed,
            created_at_unix: 0,
            modified_at_unix: 0,
            status: hz_command::WorktreeStatus::Unknown,
        },
        false,
    );

    assert!(output.starts_with("- removed  feature/ui"));
    assert!(output.contains("path    /worktrees/entry"));
}

#[test]
fn handoff_output_renders_human_summary() {
    let output = render_handoff(
        &hz_command::WorktreeHandoff {
            repo: PathBuf::from("/repo"),
            mode: hz_command::HandoffMode::Patch,
            branch: Some("feature/ui".to_owned()),
            from: hz_core::paths::WorktreeTarget {
                name: "local".to_owned(),
                path: PathBuf::from("/repo"),
            },
            to: hz_core::paths::WorktreeTarget {
                name: "feature/ui".to_owned(),
                path: PathBuf::from("/worktrees/entry"),
            },
            changed: true,
            warnings: Vec::new(),
        },
        false,
    );

    assert!(output.contains("repo    /repo"));
    assert!(output.contains("mode    patch"));
    assert!(output.contains("branch  feature/ui"));
    assert!(output.contains("< from  local"));
    assert!(output.contains("> to    feature/ui"));
}

#[test]
fn handoff_output_renders_prune_warnings() {
    let output = render_handoff(
        &hz_command::WorktreeHandoff {
            repo: PathBuf::from("/repo"),
            mode: hz_command::HandoffMode::Patch,
            branch: Some("feature/ui".to_owned()),
            from: hz_core::paths::WorktreeTarget {
                name: "local".to_owned(),
                path: PathBuf::from("/repo"),
            },
            to: hz_core::paths::WorktreeTarget {
                name: "generated-handle".to_owned(),
                path: PathBuf::from("/worktrees/entry"),
            },
            changed: true,
            warnings: vec![
                "created worktree, but failed to prune detached worktrees: permission denied"
                    .to_owned(),
            ],
        },
        false,
    );

    assert!(output.contains(
        "warning  created worktree, but failed to prune detached worktrees: permission denied"
    ));
}

#[test]
fn shell_init_output_renders_status() {
    let output = render_shell_init(
        "zsh",
        &hz_command::ShellInit {
            path: PathBuf::from("/home/me/.zshrc"),
            line: "eval \"$(hz shell zsh)\"",
            changed: true,
        },
        false,
    );

    assert!(output.starts_with("+ installed  zsh"));
    assert!(output.contains("path    /home/me/.zshrc"));
}

#[test]
fn remove_accepts_short_force_flag() {
    let cli = Cli::try_parse_from([
        "hz",
        "rm",
        "-r",
        "/repo",
        "-j",
        "-d",
        "-f",
        "--no-cleanup",
        "target",
    ])
    .unwrap();

    match cli.command {
        Some(Command::Remove(args)) => {
            assert_eq!(args.targets, vec!["target".to_owned()]);
            assert_eq!(args.repo, Some(PathBuf::from("/repo")));
            assert!(args.json);
            assert!(args.debug);
            assert!(args.force);
            assert!(args.no_cleanup);
        }
        command => panic!("expected remove command, got {command:?}"),
    }
}

#[test]
fn remove_accepts_multiple_targets() {
    let cli = Cli::try_parse_from(["hz", "rm", "cartesian-alpha", "archimedean-beta"]).unwrap();

    match cli.command {
        Some(Command::Remove(args)) => {
            assert_eq!(
                args.targets,
                vec!["cartesian-alpha".to_owned(), "archimedean-beta".to_owned()]
            );
        }
        command => panic!("expected remove command, got {command:?}"),
    }
}

#[test]
fn handoff_accepts_optional_branch_and_path_only() {
    let cli = Cli::try_parse_from(["hz", "handoff", "-r", "/repo", "-j", "feature/ui"]).unwrap();

    match cli.command {
        Some(Command::Handoff(args)) => {
            assert_eq!(args.target.as_deref(), Some("feature/ui"));
            assert_eq!(args.repo, Some(PathBuf::from("/repo")));
            assert!(args.json);
            assert!(!args.branch);
            assert!(!args.create);
        }
        command => panic!("expected handoff command, got {command:?}"),
    }

    let cli = Cli::try_parse_from(["hz", "handoff", "708e", "-b", "--path-only"]).unwrap();
    match cli.command {
        Some(Command::Handoff(args)) => {
            assert_eq!(args.target.as_deref(), Some("708e"));
            assert!(args.branch);
            assert!(args.path_only);
        }
        command => panic!("expected handoff command, got {command:?}"),
    }

    let cli = Cli::try_parse_from([
        "hz",
        "handoff",
        "--new",
        "--max-detached",
        "3",
        "--max-branch-worktrees",
        "4",
        "feature/ui",
    ])
    .unwrap();
    match cli.command {
        Some(Command::Handoff(args)) => {
            assert_eq!(args.target.as_deref(), Some("feature/ui"));
            assert!(args.create);
            assert_eq!(args.max_detached, Some(3));
            assert_eq!(args.max_branch_worktrees, Some(4));
        }
        command => panic!("expected handoff command, got {command:?}"),
    }

    let cli = Cli::try_parse_from(["hz", "handoff"]).unwrap();
    match cli.command {
        Some(Command::Handoff(args)) => assert_eq!(args.target, None),
        command => panic!("expected handoff command, got {command:?}"),
    }
}

#[test]
fn creation_accepts_short_flags() {
    let cli = Cli::try_parse_from([
        "hz",
        "new",
        "-r",
        "/repo",
        "-p",
        "../wt",
        "-B",
        "main",
        "-b",
        "feature/ui",
        "--max-detached",
        "5",
        "--max-branch-worktrees",
        "6",
        "-j",
        "-d",
        "--no-setup",
        "handle",
    ])
    .unwrap();

    match cli.command {
        Some(Command::New(args)) => {
            assert_eq!(args.name.as_deref(), Some("handle"));
            assert_eq!(args.repo, Some(PathBuf::from("/repo")));
            assert_eq!(args.path, Some(PathBuf::from("../wt")));
            assert_eq!(args.base.as_deref(), Some("main"));
            assert_eq!(args.branch.as_deref(), Some("feature/ui"));
            assert_eq!(args.max_detached, Some(5));
            assert_eq!(args.max_branch_worktrees, Some(6));
            assert!(args.json);
            assert!(args.debug);
            assert!(!args.setup);
            assert!(args.no_setup);
        }
        command => panic!("expected new command, got {command:?}"),
    }
}

#[test]
fn fork_accepts_named_detached_options() {
    let cli = Cli::try_parse_from([
        "hz",
        "fork",
        "copy",
        "-r",
        "/repo",
        "-p",
        "/tmp/copy",
        "--no-diff",
        "--max-detached",
        "3",
        "-j",
        "--path-only",
    ])
    .unwrap();

    match cli.command {
        Some(Command::Fork(args)) => {
            assert_eq!(args.name.as_deref(), Some("copy"));
            assert_eq!(args.repo, Some(PathBuf::from("/repo")));
            assert_eq!(args.path, Some(PathBuf::from("/tmp/copy")));
            assert!(args.no_diff);
            assert_eq!(args.max_detached, Some(3));
            assert!(args.json);
            assert!(args.path_only);
        }
        command => panic!("expected fork command, got {command:?}"),
    }
}

#[test]
fn path_and_list_accept_short_flags() {
    let cli = Cli::try_parse_from(["hz", "path", "-r", "/repo", "-j", "target"]).unwrap();
    match cli.command {
        Some(Command::Path(args)) => {
            assert_eq!(args.target.as_deref(), Some("target"));
            assert_eq!(args.repo, Some(PathBuf::from("/repo")));
            assert!(args.json);
        }
        command => panic!("expected path command, got {command:?}"),
    }

    let cli = Cli::try_parse_from(["hz", "ls", "-r", "/repo", "-j"]).unwrap();
    match cli.command {
        Some(Command::List(args)) => {
            assert_eq!(args.repo, Some(PathBuf::from("/repo")));
            assert!(args.json);
        }
        command => panic!("expected list command, got {command:?}"),
    }

    let cli = Cli::try_parse_from(["hz", "pwd", "-r", "/repo", "-j"]).unwrap();
    match cli.command {
        Some(Command::Pwd(args)) => {
            assert_eq!(args.repo, Some(PathBuf::from("/repo")));
            assert!(args.json);
        }
        command => panic!("expected pwd command, got {command:?}"),
    }
}

#[test]
fn worktree_pwd_is_available_under_worktree_group() {
    let cli = Cli::try_parse_from(["hz", "worktree", "pwd", "--repo", "/repo"]).unwrap();

    match cli.command {
        Some(Command::Worktree {
            command: WorktreeCommand::Pwd(args),
        }) => assert_eq!(args.repo, Some(PathBuf::from("/repo"))),
        command => panic!("expected worktree pwd command, got {command:?}"),
    }
}

#[test]
fn current_worktree_entry_matches_current_path() {
    let mut current = test_entry(hz_command::WorktreeSource::Managed);
    current.path = PathBuf::from("/worktrees/current");

    let mut other = test_entry(hz_command::WorktreeSource::Managed);
    other.path = PathBuf::from("/worktrees/other");
    other.branch = Some("feature/other".to_owned());

    let entries = vec![other, current];

    let found = current_worktree_entry(&entries, Path::new("/worktrees/current"))
        .expect("current worktree should be found");

    assert_eq!(worktree_branch_or_handle(found), "feature/ui");
}

#[test]
fn hidden_completion_command_accepts_kind_and_repo() {
    let cli = Cli::try_parse_from(["hz", "__complete", "worktree-targets", "-r", "/repo"]).unwrap();

    match cli.command {
        Some(Command::Complete(args)) => {
            assert_eq!(args.kind, CompletionKind::WorktreeTargets);
            assert_eq!(args.repo, Some(PathBuf::from("/repo")));
        }
        command => panic!("expected complete command, got {command:?}"),
    }
}

// The shell integrations are hand-written to support auto-cd and dynamic
// worktree target completion. Keep their command/flag surface pinned to Clap so
// adding a CLI flag without updating bash/zsh/fish completions fails in tests.
#[test]
fn shell_completion_command_lists_match_clap() {
    let bash = hz_command::shell_integration(hz_command::Shell::Bash);
    let zsh = hz_command::shell_integration(hz_command::Shell::Zsh);
    let fish = hz_command::shell_integration(hz_command::Shell::Fish);

    let top_commands = clap_completion_subcommands(&[]);
    assert_eq!(bash_words_variable(bash, "_hz_top_commands"), top_commands);
    assert_eq!(
        zsh_described_commands(zsh, "_hz_complete_main"),
        top_commands
    );
    assert_eq!(
        fish_argument_words(fish, "not __fish_seen_subcommand_from"),
        top_commands
    );

    let worktree_commands = clap_completion_subcommands(&["worktree"]);
    assert_eq!(
        bash_words_variable(bash, "_hz_worktree_commands"),
        worktree_commands
    );
    assert_eq!(
        zsh_described_commands(zsh, "_hz_complete_worktree_subcommand"),
        worktree_commands
    );
    assert_eq!(
        fish_argument_words(fish, "__hz_needs_worktree_subcommand"),
        worktree_commands
    );

    let agent_commands = clap_completion_subcommands(&["agent"]);
    assert_eq!(
        bash_words_variable(bash, "_hz_agent_commands"),
        agent_commands
    );
    assert_eq!(
        zsh_described_commands(zsh, "_hz_complete_agent_subcommand"),
        agent_commands
    );
    assert_eq!(
        fish_argument_words(fish, "__hz_needs_agent_subcommand"),
        agent_commands
    );

    assert!(!bash.contains("_hz_ts_commands"));
    assert!(!zsh.contains("_hz_complete_ts_subcommand"));
    assert!(!fish.contains("__hz_needs_ts_subcommand"));
}

#[test]
fn shell_completion_option_flags_match_clap() {
    let bash = hz_command::shell_integration(hz_command::Shell::Bash);
    let zsh = hz_command::shell_integration(hz_command::Shell::Zsh);
    let fish = hz_command::shell_integration(hz_command::Shell::Fish);

    let root_flags = clap_completion_flags(&[]);
    assert_eq!(bash_root_flags(bash), root_flags, "bash options for hz");
    assert_eq!(zsh_root_flags(zsh), root_flags, "zsh options for hz");
    assert_eq!(
        fish_completion_flags(fish, FishCompletionContext::Root),
        root_flags,
        "fish options for hz"
    );

    for group in ["worktree", "wt", "agent"] {
        let expected = clap_completion_flags(&[group]);
        assert_eq!(
            bash_group_flags(bash, group),
            expected,
            "bash options for hz {group}"
        );
        assert_eq!(
            zsh_group_flags(zsh, group),
            expected,
            "zsh options for hz {group}"
        );
        assert_eq!(
            fish_completion_flags(fish, FishCompletionContext::Top(group)),
            expected,
            "fish options for hz {group}"
        );
    }

    for command in clap_completion_subcommands(&[]) {
        if matches!(command.as_str(), "worktree" | "wt" | "agent") {
            continue;
        }

        let expected = clap_completion_flags(&[&command]);
        assert_eq!(
            bash_command_flags(bash, &command),
            expected,
            "bash options for hz {command}"
        );
        assert_eq!(
            zsh_command_flags(zsh, &command),
            expected,
            "zsh options for hz {command}"
        );
        assert_eq!(
            fish_completion_flags(fish, FishCompletionContext::Top(&command)),
            expected,
            "fish options for hz {command}"
        );
    }

    for command in clap_completion_subcommands(&["worktree"]) {
        let expected = clap_completion_flags(&["worktree", &command]);
        assert_eq!(
            bash_command_flags(bash, &command),
            expected,
            "bash options for hz worktree {command}"
        );
        assert_eq!(
            zsh_command_flags(zsh, &command),
            expected,
            "zsh options for hz worktree {command}"
        );
        assert_eq!(
            fish_completion_flags(fish, FishCompletionContext::Worktree(&command)),
            expected,
            "fish options for hz worktree {command}"
        );
    }

    for command in clap_completion_subcommands(&["agent"]) {
        let expected = clap_completion_flags(&["agent", &command]);
        assert_eq!(
            bash_command_flags(bash, &command),
            expected,
            "bash options for hz agent {command}"
        );
        assert_eq!(
            zsh_command_flags(zsh, &command),
            expected,
            "zsh options for hz agent {command}"
        );
        assert_eq!(
            fish_completion_flags(fish, FishCompletionContext::Agent(&command)),
            expected,
            "fish options for hz agent {command}"
        );
    }
}

#[test]
fn init_install_and_lifecycle_commands_parse() {
    let cli = Cli::try_parse_from(["hz", "init", "-r", "/repo"]).unwrap();
    match cli.command {
        Some(Command::Init(args)) => {
            assert_eq!(args.shell, None);
            assert_eq!(args.repo, Some(PathBuf::from("/repo")));
        }
        command => panic!("expected init command, got {command:?}"),
    }

    let cli = Cli::try_parse_from(["hz", "init", "zsh"]).unwrap();
    match cli.command {
        Some(Command::Init(args)) => assert_eq!(args.shell, Some(ShellArg::Zsh)),
        command => panic!("expected init command, got {command:?}"),
    }

    let cli = Cli::try_parse_from(["hz", "install", "fish"]).unwrap();
    match cli.command {
        Some(Command::Install(args)) => assert_eq!(args.shell, ShellArg::Fish),
        command => panic!("expected install command, got {command:?}"),
    }

    let cli = Cli::try_parse_from(["hz", "setup", "-r", "/repo", "target"]).unwrap();
    match cli.command {
        Some(Command::Setup(args)) => {
            assert_eq!(args.target.as_deref(), Some("target"));
            assert_eq!(args.repo, Some(PathBuf::from("/repo")));
        }
        command => panic!("expected setup command, got {command:?}"),
    }

    let cli = Cli::try_parse_from(["hz", "cleanup"]).unwrap();
    match cli.command {
        Some(Command::Cleanup(args)) => assert_eq!(args.target, None),
        command => panic!("expected cleanup command, got {command:?}"),
    }

    let cli = Cli::try_parse_from([
        "hz",
        "update",
        "--target-version",
        "0.1.1",
        "--install-dir",
        "/tmp/hz-bin",
    ])
    .unwrap();
    match cli.command {
        Some(Command::Update(args)) => {
            assert_eq!(args.version.as_deref(), Some("0.1.1"));
            assert_eq!(args.install_dir, Some(PathBuf::from("/tmp/hz-bin")));
        }
        command => panic!("expected update command, got {command:?}"),
    }
}

#[test]
fn update_target_uses_invoked_binary_name_and_directory() {
    let cwd = env::current_dir().unwrap();

    assert_eq!(
        update_binary_name(OsStr::new("./target/debug/hz-dev")).unwrap(),
        OsString::from("hz-dev")
    );
    assert!(
        default_update_install_dir(OsStr::new("./target/debug/hz-dev"))
            .unwrap()
            .ends_with(Path::new("target/debug"))
    );
    assert_eq!(
        default_update_install_dir(OsStr::new("/usr/local/bin/hz-beta")).unwrap(),
        PathBuf::from("/usr/local/bin")
    );
    assert_eq!(
        absolute_path(PathBuf::from("bin")).unwrap(),
        cwd.join("bin")
    );
}

#[test]
fn update_detects_package_manager_install_dirs() {
    assert_eq!(
        managed_update_install(Path::new("/opt/homebrew/bin"), OsStr::new("hz")),
        Some(ManagedUpdateInstall::Homebrew)
    );
    assert_eq!(
        managed_update_install(Path::new("/Users/me/.cargo/bin"), OsStr::new("hz")),
        Some(ManagedUpdateInstall::Cargo)
    );
    assert_eq!(
        managed_update_install(
            Path::new("/Users/me/.local/share/mise/shims"),
            OsStr::new("hz")
        ),
        Some(ManagedUpdateInstall::Mise)
    );
    assert_eq!(
        managed_update_install(Path::new("/nix/store/abc-hz/bin"), OsStr::new("hz")),
        Some(ManagedUpdateInstall::Nix)
    );
    assert_eq!(
        classify_managed_update_path(Path::new("/usr/local/bin")),
        None
    );
}

#[test]
fn update_rejects_managed_install_dirs() {
    let error = check_update_install_dir(Path::new("/Users/me/.cargo/bin"), OsStr::new("hz"))
        .unwrap_err()
        .to_string();
    assert!(error.contains("Cargo-managed"));
    assert!(error.contains("--install-dir DIR"));
    assert!(!error.contains("--force-self-update"));

    assert!(check_update_install_dir(Path::new("hz-unmanaged-test-bin"), OsStr::new("hz")).is_ok());
}

#[test]
fn update_repo_respects_env_override() {
    assert_eq!(update_repo(None), OsString::from(RELEASE_REPO));
    assert_eq!(
        update_repo(Some(OsString::from("example/hz-fork"))),
        OsString::from("example/hz-fork")
    );
    assert_eq!(
        update_repo(Some(OsString::new())),
        OsString::from(RELEASE_REPO)
    );
}

#[test]
fn completion_candidates_are_deduplicated() {
    let mut candidates = vec!["local".to_owned()];

    push_completion_candidate(&mut candidates, Some("feature/ui".to_owned()));
    push_completion_candidate(&mut candidates, Some("feature/ui".to_owned()));
    push_completion_candidate(&mut candidates, Some(String::new()));
    push_completion_candidate(&mut candidates, None);

    assert_eq!(candidates, vec!["local", "feature/ui"]);
}

#[test]
fn worktree_completion_candidates_use_display_targets() {
    let mut branched = test_entry(hz_command::WorktreeSource::Managed);
    branched.id = "45aa44e4-9dd5-4e74-b7ae-82db4b365e78".to_owned();
    branched.handle = "45aa44e4-9dd5-4e74-b7ae-82db4b365e78".to_owned();
    branched.branch = Some("feat(tui)/diff".to_owned());

    let mut detached = test_entry(hz_command::WorktreeSource::Managed);
    detached.id = "de625fc0-3962-4680-be9c-37fca7a57aaf".to_owned();
    detached.handle = "tw61".to_owned();
    detached.branch = None;

    let mut candidates = Vec::new();
    push_worktree_completion_candidate(&mut candidates, &branched);
    push_worktree_completion_candidate(&mut candidates, &detached);

    assert_eq!(candidates, vec!["feat(tui)/diff", "tw61"]);
}

#[test]
fn removed_worktree_display_identifier_prefers_branch() {
    let worktree = hz_command::WorktreeEntry {
        id: "entry-id".to_owned(),
        handle: "generated-handle".to_owned(),
        repo: PathBuf::from("/repo"),
        path: PathBuf::from("/worktrees/entry-id"),
        branch: Some("feature/ui".to_owned()),
        base: None,
        source: hz_command::WorktreeSource::Managed,
        created_at_unix: 0,
        modified_at_unix: 0,
        status: hz_command::WorktreeStatus::Unknown,
    };

    assert_eq!(worktree_branch_or_handle(&worktree), "feature/ui");
}

#[test]
fn removal_candidates_report_duplicate_before_later_unknown_target() {
    let test_dir = test_dir("hz-removal-duplicate-order-test");
    let repo = test_dir.join("repo");
    let destination = test_dir.join("feature");
    init_committed_git_repo(&repo);
    git(["branch", "feature"], &repo);
    git(
        [
            "worktree",
            "add",
            "-q",
            destination.to_str().unwrap(),
            "feature",
        ],
        &repo,
    );
    let mut args = remove_args(false, true);
    args.targets = vec![
        "feature".to_owned(),
        "feature".to_owned(),
        "missing".to_owned(),
    ];
    args.repo = Some(repo.clone());

    let error = find_removal_candidates(&args).unwrap_err();

    assert_eq!(error.to_string(), "duplicate worktree target: feature");
    fs::remove_dir_all(test_dir).expect("test directory should be removed");
}

#[test]
fn unmanaged_json_removal_requires_force() {
    let args = remove_args(true, false);
    let worktree = test_entry(hz_command::WorktreeSource::Git);

    let error = should_confirm_unmanaged_removal(&args, &worktree).unwrap_err();

    assert_eq!(
        error.to_string(),
        "refusing to remove unmanaged worktree in --json mode without --force"
    );
}

#[test]
fn force_skips_unmanaged_removal_confirmation() {
    let args = remove_args(false, true);
    let worktree = test_entry(hz_command::WorktreeSource::Git);

    assert!(!should_confirm_unmanaged_removal(&args, &worktree).unwrap());
}

#[test]
fn repo_lifecycle_hooks_require_explicit_create_or_remove_flags() {
    let cli = Cli::try_parse_from(["hz", "new", "handle"]).unwrap();
    match cli.command {
        Some(Command::New(args)) => {
            assert!(!args.setup);
            assert!(!args.no_setup);
        }
        command => panic!("expected new command, got {command:?}"),
    }

    let cli = Cli::try_parse_from(["hz", "new", "--setup", "handle"]).unwrap();
    match cli.command {
        Some(Command::New(args)) => assert!(args.setup),
        command => panic!("expected new command, got {command:?}"),
    }

    let cli = Cli::try_parse_from(["hz", "rm", "--cleanup", "handle"]).unwrap();
    match cli.command {
        Some(Command::Remove(args)) => {
            assert!(args.cleanup);
            assert!(!args.no_cleanup);
        }
        command => panic!("expected remove command, got {command:?}"),
    }

    let cli = Cli::try_parse_from(["hz", "new", "--setup", "--no-setup", "handle"]).unwrap();
    match cli.command {
        Some(Command::New(args)) => {
            assert!(args.setup);
            assert!(args.no_setup);
        }
        command => panic!("expected new command, got {command:?}"),
    }

    let cli = Cli::try_parse_from(["hz", "rm", "--cleanup", "--no-cleanup", "handle"]).unwrap();
    match cli.command {
        Some(Command::Remove(args)) => {
            assert!(args.cleanup);
            assert!(args.no_cleanup);
        }
        command => panic!("expected remove command, got {command:?}"),
    }
}

#[test]
fn managed_removal_skips_confirmation() {
    let args = remove_args(false, false);
    let worktree = test_entry(hz_command::WorktreeSource::Managed);

    assert!(!should_confirm_unmanaged_removal(&args, &worktree).unwrap());
}

#[test]
fn cleanup_skips_git_removals_outside_hz_namespace() {
    assert!(should_run_cleanup_for_removal(&test_entry(
        hz_command::WorktreeSource::Managed
    )));
    assert!(!should_run_cleanup_for_removal(&test_entry(
        hz_command::WorktreeSource::Git
    )));
}

#[test]
fn cleanup_runs_when_user_managed_status_check_fails() {
    let repo = test_dir("hz-cleanup-status-error-test");
    fs::create_dir_all(repo.join(".hz")).unwrap();
    fs::write(repo.join(".hz").join("hz.toml"), "[worktree\n").unwrap();

    let mut worktree = test_entry(hz_command::WorktreeSource::Git);
    worktree.repo = repo.clone();
    worktree.path = repo.join("../agent-worktrees/entry");

    assert!(should_run_cleanup_for_removal(&worktree));

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn hz_namespace_git_removal_skips_confirmation_and_runs_cleanup() {
    let Some(home) = env::var_os("HOME").filter(|home| !home.is_empty()) else {
        return;
    };
    let args = remove_args(false, false);
    let mut worktree = test_entry(hz_command::WorktreeSource::Git);
    worktree.repo = PathBuf::from(&home).join("code/hz");
    worktree.path = PathBuf::from(home).join(".hz/worktrees/hz/entry-id");

    assert!(!should_confirm_unmanaged_removal_with_stdin(&args, &worktree, false).unwrap());
    assert!(should_run_cleanup_for_removal(&worktree));
}

#[test]
fn unmanaged_non_interactive_removal_requires_force() {
    let args = remove_args(false, false);
    let worktree = test_entry(hz_command::WorktreeSource::Git);

    let error = should_confirm_unmanaged_removal_with_stdin(&args, &worktree, false).unwrap_err();

    assert_eq!(
        error.to_string(),
        "refusing to prompt for unmanaged worktree removal without a terminal; use --force"
    );
}

#[test]
fn unmanaged_interactive_removal_confirms() {
    let args = remove_args(false, false);
    let worktree = test_entry(hz_command::WorktreeSource::Git);

    assert!(should_confirm_unmanaged_removal_with_stdin(&args, &worktree, true).unwrap());
}

#[test]
fn single_target_json_keeps_object_shape() {
    let removed = vec![test_entry(hz_command::WorktreeSource::Managed)];

    let output = removed_worktrees_json(1, &removed).unwrap();

    assert!(output.trim_start().starts_with('{'));
}

#[test]
fn multi_target_json_keeps_array_shape_when_one_is_removed() {
    let removed = vec![test_entry(hz_command::WorktreeSource::Managed)];

    let output = removed_worktrees_json(2, &removed).unwrap();

    assert!(output.trim_start().starts_with('['));
}

#[test]
fn single_target_json_uses_array_when_nothing_was_removed() {
    let output = removed_worktrees_json(1, &[]).unwrap();

    assert_eq!(output, "[]");
}

#[test]
fn agent_remove_json_always_uses_array_shape() {
    let removed = vec![test_entry(hz_command::WorktreeSource::Managed)];

    let output = removed_worktrees_json_array(&removed).unwrap();

    assert!(output.trim_start().starts_with('['));
}

#[derive(Clone, Copy)]
enum FishCompletionContext<'a> {
    Root,
    Top(&'a str),
    Worktree(&'a str),
    Agent(&'a str),
}

fn clap_completion_subcommands(path: &[&str]) -> BTreeSet<String> {
    let command = built_cli_command();
    let command = clap_find_command(&command, path);

    command
        .get_subcommands()
        .filter(|subcommand| !subcommand.is_hide_set() && subcommand.get_name() != "help")
        .flat_map(|subcommand| {
            let mut names = vec![subcommand.get_name().to_owned()];
            names.extend(subcommand.get_all_aliases().map(str::to_owned));
            names
        })
        .collect()
}

fn clap_completion_flags(path: &[&str]) -> BTreeSet<String> {
    let command = built_cli_command();
    let command = clap_find_command(&command, path);
    let mut flags = BTreeSet::new();

    for arg in command
        .get_arguments()
        .filter(|arg| !arg.is_positional() && !arg.is_hide_set())
    {
        if let Some(short) = arg.get_short() {
            flags.insert(format!("-{short}"));
        }
        if let Some(short_aliases) = arg.get_all_short_aliases() {
            flags.extend(short_aliases.into_iter().map(|short| format!("-{short}")));
        }
        if let Some(long) = arg.get_long() {
            flags.insert(format!("--{long}"));
        }
        if let Some(long_aliases) = arg.get_all_aliases() {
            flags.extend(long_aliases.into_iter().map(|long| format!("--{long}")));
        }
    }

    flags
}

fn built_cli_command() -> clap::Command {
    let mut command = <Cli as clap::CommandFactory>::command();
    command.build();
    command
}

fn clap_find_command<'a>(mut command: &'a clap::Command, path: &[&str]) -> &'a clap::Command {
    for name in path {
        command = command
            .get_subcommands()
            .find(|subcommand| clap_command_matches(subcommand, name))
            .unwrap_or_else(|| panic!("missing clap command path component: {name}"));
    }

    command
}

fn clap_command_matches(command: &clap::Command, name: &str) -> bool {
    command.get_name() == name || command.get_all_aliases().any(|alias| alias == name)
}

fn bash_words_variable(script: &str, variable: &str) -> BTreeSet<String> {
    let marker = format!("{variable}=\"");
    let value = script
        .split(&marker)
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap_or_else(|| panic!("missing bash variable: {variable}"));

    words(value)
}

fn zsh_described_commands(script: &str, function: &str) -> BTreeSet<String> {
    shell_function_body(script, function)
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let description = line.strip_prefix('\'')?;
            let (command, _) = description.split_once(':')?;
            Some(command.to_owned())
        })
        .collect()
}

fn fish_argument_words(script: &str, condition: &str) -> BTreeSet<String> {
    let line = script
        .lines()
        .find(|line| line.contains(condition) && line.contains(" -a \""))
        .unwrap_or_else(|| panic!("missing fish completion arguments for: {condition}"));
    let value = line
        .split(" -a \"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap_or_else(|| panic!("missing fish completion argument list for: {condition}"));

    words(value)
}

fn bash_root_flags(script: &str) -> BTreeSet<String> {
    shell_root_flags(script)
}

fn zsh_root_flags(script: &str) -> BTreeSet<String> {
    shell_root_flags(script)
}

fn shell_root_flags(script: &str) -> BTreeSet<String> {
    let body = shell_function_body(script, "_hz_completion");
    let root_branch = body.split("local cmd").next().unwrap_or(body);

    extract_shell_flags(root_branch)
}

fn bash_group_flags(script: &str, group: &str) -> BTreeSet<String> {
    shell_group_flags(script, group)
}

fn zsh_group_flags(script: &str, group: &str) -> BTreeSet<String> {
    shell_group_flags(script, group)
}

fn shell_group_flags(script: &str, group: &str) -> BTreeSet<String> {
    let body = shell_function_body(script, "_hz_completion");
    let marker = match group {
        "worktree" | "wt" | "agent" => {
            "if [[ \"$cmd\" == \"worktree\" || \"$cmd\" == \"wt\" || \"$cmd\" == \"agent\" ]]; then"
        }
        _ => panic!("unsupported shell group: {group}"),
    };
    let branch = shell_branch_body(body, marker);

    extract_shell_flags(branch)
}

fn bash_command_flags(script: &str, command: &str) -> BTreeSet<String> {
    shell_case_flags(script, "_hz_complete_command_args", command)
}

fn zsh_command_flags(script: &str, command: &str) -> BTreeSet<String> {
    shell_case_flags(script, "_hz_complete_command_options", command)
}

fn shell_case_flags(script: &str, function: &str, command: &str) -> BTreeSet<String> {
    let body = shell_function_body(script, function);
    let mut found = false;
    let mut selected = false;
    let mut selected_body = String::new();

    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed == ";;" {
            selected = false;
            continue;
        }

        if let Some(pattern) = shell_case_pattern(trimmed) {
            selected = pattern.split('|').any(|candidate| candidate == command);
            found |= selected;
            continue;
        }

        if selected {
            selected_body.push_str(line);
            selected_body.push('\n');
        }
    }

    if !found {
        panic!("missing {function} case arm for: {command}");
    }

    extract_shell_flags(&selected_body)
}

fn shell_case_pattern(line: &str) -> Option<&str> {
    let pattern = line.strip_suffix(')')?;
    if pattern
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || character == '-' || character == '|')
    {
        Some(pattern)
    } else {
        None
    }
}

fn shell_function_body<'a>(script: &'a str, function: &str) -> &'a str {
    let marker = format!("{function}() {{");
    script
        .split(&marker)
        .nth(1)
        .and_then(|rest| rest.split("\n}").next())
        .unwrap_or_else(|| panic!("missing shell function: {function}"))
}

fn shell_branch_body<'a>(body: &'a str, marker: &str) -> &'a str {
    body.split(marker)
        .nth(1)
        .and_then(|rest| rest.split("\n  fi\n\n").next())
        .unwrap_or_else(|| panic!("missing shell branch: {marker}"))
}

fn extract_shell_flags(text: &str) -> BTreeSet<String> {
    text.split(|character: char| {
        character.is_whitespace()
            || matches!(character, '"' | '\'' | '(' | ')' | ';' | ',' | '[' | ']')
    })
    .filter_map(normalize_shell_flag)
    .collect()
}

fn normalize_shell_flag(token: &str) -> Option<String> {
    if token == "--" {
        return None;
    }

    if let Some(long) = token.strip_prefix("--") {
        if is_long_completion_flag(long) {
            return Some(token.to_owned());
        }
    }

    let mut characters = token.chars();
    if characters.next() == Some('-') {
        if let (Some(short), None) = (characters.next(), characters.next()) {
            if short.is_ascii_alphanumeric() {
                return Some(token.to_owned());
            }
        }
    }

    None
}

fn is_long_completion_flag(flag: &str) -> bool {
    flag.chars()
        .next()
        .is_some_and(|character| character.is_ascii_alphanumeric())
        && flag
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
}

fn fish_completion_flags(script: &str, context: FishCompletionContext<'_>) -> BTreeSet<String> {
    let mut flags = BTreeSet::new();

    for line in script.lines().map(str::trim) {
        if !line.starts_with("complete -c hz ") || !fish_line_applies(line, context) {
            continue;
        }

        flags.extend(fish_line_flags(line));
    }

    flags
}

fn fish_line_applies(line: &str, context: FishCompletionContext<'_>) -> bool {
    let condition = fish_line_condition(line);

    // Fish entries without a condition are global flags and apply in every context.
    match context {
        FishCompletionContext::Root => condition
            .is_none_or(|condition| condition.starts_with("not __fish_seen_subcommand_from")),
        FishCompletionContext::Top(command) => condition.is_none_or(|condition| {
            fish_condition_contains(condition, "__hz_command_is", command)
                || fish_condition_contains(condition, "__hz_top_command_is", command)
        }),
        FishCompletionContext::Worktree(command) => condition
            .is_none_or(|condition| fish_condition_contains(condition, "__hz_command_is", command)),
        FishCompletionContext::Agent(command) => condition
            .is_none_or(|condition| fish_condition_contains(condition, "__hz_command_is", command)),
    }
}

fn fish_line_condition(line: &str) -> Option<&str> {
    line.split(" -n \"")
        .nth(1)
        .and_then(|condition| condition.split('"').next())
}

fn fish_condition_contains(condition: &str, prefix: &str, command: &str) -> bool {
    condition.strip_prefix(prefix).is_some_and(|rest| {
        rest.starts_with(char::is_whitespace)
            && rest
                .split_whitespace()
                .any(|candidate| candidate == command)
    })
}

#[test]
fn fish_condition_contains_requires_helper_word_boundary() {
    assert!(fish_condition_contains(
        "__hz_command_is remove rm",
        "__hz_command_is",
        "remove"
    ));
    assert!(fish_condition_contains(
        "__hz_command_is\tremove rm",
        "__hz_command_is",
        "remove"
    ));
    assert!(!fish_condition_contains(
        "__hz_command_is_extra remove rm",
        "__hz_command_is",
        "remove"
    ));
    assert!(!fish_condition_contains(
        "__hz_command_isremove rm",
        "__hz_command_is",
        "remove"
    ));
}

fn fish_line_flags(line: &str) -> BTreeSet<String> {
    let mut flags = BTreeSet::new();
    let mut tokens = line.split_whitespace();

    while let Some(token) = tokens.next() {
        match token {
            "-s" => {
                let short = tokens
                    .next()
                    .unwrap_or_else(|| panic!("missing fish short flag name in: {line}"));
                flags.insert(format!("-{}", short.trim_matches('"')));
            }
            "-l" => {
                let long = tokens
                    .next()
                    .unwrap_or_else(|| panic!("missing fish long flag name in: {line}"));
                flags.insert(format!("--{}", long.trim_matches('"')));
            }
            _ => {}
        }
    }

    flags
}

fn words(text: &str) -> BTreeSet<String> {
    text.split_whitespace().map(str::to_owned).collect()
}

fn remove_args(json: bool, force: bool) -> RemoveWorktreeArgs {
    RemoveWorktreeArgs {
        targets: vec!["target".to_owned()],
        repo: None,
        json,
        force,
        debug: false,
        cleanup: false,
        no_cleanup: false,
    }
}

fn test_entry(source: hz_command::WorktreeSource) -> hz_command::WorktreeEntry {
    hz_command::WorktreeEntry {
        id: "entry-id".to_owned(),
        handle: "generated-handle".to_owned(),
        repo: PathBuf::from("/repo"),
        path: PathBuf::from("/worktrees/entry-id"),
        branch: Some("feature/ui".to_owned()),
        base: None,
        source,
        created_at_unix: 0,
        modified_at_unix: 0,
        status: hz_command::WorktreeStatus::Unknown,
    }
}

fn test_dir(prefix: &str) -> PathBuf {
    let test_dir = env::temp_dir().join(format!(
        "{prefix}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos()
    ));
    fs::create_dir_all(&test_dir).expect("test directory should be created");
    test_dir
}

fn init_committed_git_repo(repo: &Path) {
    fs::create_dir_all(repo).expect("repo directory should be created");
    git(["init", "-q"], repo);
    git(["config", "user.email", "test@example.com"], repo);
    git(["config", "user.name", "Test"], repo);
    fs::write(repo.join("file.txt"), "base\n").expect("tracked file should be written");
    git(["add", "file.txt"], repo);
    git(["commit", "-q", "-m", "init"], repo);
}

fn git<const N: usize>(args: [&str; N], cwd: &Path) {
    let output = ProcessCommand::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .expect("git should run");
    assert!(
        output.status.success(),
        "git failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
