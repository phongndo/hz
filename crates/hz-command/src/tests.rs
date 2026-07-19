use super::*;
use tempfile::TempDir;

#[test]
fn config_init_creates_new_workspace_lifecycle_files_once() {
    let temp = TempDir::new().unwrap();
    let first = init_config(InitConfig {
        at: temp.path().to_path_buf(),
    })
    .unwrap();
    let second = init_config(InitConfig {
        at: temp.path().to_path_buf(),
    })
    .unwrap();

    assert!(first.config_created);
    assert!(first.postcreate_created);
    assert!(first.preremove_created);
    assert!(!second.config_created);
    assert!(!second.postcreate_created);
    assert!(!second.preremove_created);
    let config = HzConfig::load(temp.path()).unwrap();
    assert!(config.lifecycle.postcreate.is_none());
    assert!(config.lifecycle.preremove.is_none());
}

#[test]
fn config_failure_precedes_workspace_activation() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("project");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join(".hz"), "blocks configuration").unwrap();
    let mut manager = hz_workspace::Manager::open(temp.path().join("workspaces.sqlite")).unwrap();

    let result = manager.init_with_setup(
        InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::Copy,
        },
        |at| {
            init_config(InitConfig {
                at: at.to_path_buf(),
            })
            .map_err(hz_workspace::Error::from)
        },
    );

    assert!(result.is_err());
    assert!(!root.join(MARKER_FILE).exists());
    assert!(matches!(
        manager.current(&root),
        Err(hz_workspace::Error::WorkspaceNotInitialized(path)) if path == std::fs::canonicalize(root).unwrap()
    ));
}

#[test]
fn legacy_hooks_remain_disabled_during_config_migration() {
    let temp = TempDir::new().unwrap();
    let hz = temp.path().join(".hz");
    std::fs::create_dir(&hz).unwrap();
    std::fs::write(
        hz.join("hz.toml"),
        r#"[worktree]
auto_prune = true
max_detached = 10
max_branch_worktrees = 10

[list]
headers = "auto"
columns = ["marker", "target", "status", "modified", "path"]

[color]
mode = "auto"
scheme = "terminal"

[lifecycle]
setup = [".hz/environment/setup"]
cleanup = [".hz/environment/cleanup"]
"#,
    )
    .unwrap();

    let initialized = init_config(InitConfig {
        at: temp.path().to_path_buf(),
    })
    .unwrap();
    let config = HzConfig::load(temp.path()).unwrap();

    assert!(!initialized.config_created);
    assert!(config.lifecycle.postcreate.is_none());
    assert!(config.lifecycle.preremove.is_none());
}

#[test]
fn current_hook_names_take_precedence_over_legacy_names() {
    let temp = TempDir::new().unwrap();
    let hz = temp.path().join(".hz");
    std::fs::create_dir(&hz).unwrap();
    std::fs::write(
        hz.join("hz.toml"),
        "[lifecycle]\nsetup = [\"old\"]\npostcreate = [\"new\"]\n",
    )
    .unwrap();

    let config = HzConfig::load(temp.path()).unwrap();

    assert_eq!(
        config.lifecycle.postcreate.as_deref(),
        Some(["new".to_owned()].as_slice())
    );
}

#[cfg(unix)]
#[test]
fn shell_completions_route_nested_scm_targets_and_forward_at() {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    let shells = [
        (
            Shell::Bash,
            "bash",
            r#"
source "$HZ_INTEGRATION"
COMP_WORDS=(hz path --at "$HZ_AT" ""); COMP_CWORD=4; _hz_complete
COMP_WORDS=(hz git handoff --at "$HZ_AT" ""); COMP_CWORD=5; _hz_complete
COMP_WORDS=(hz git status --at "$HZ_AT" ""); COMP_CWORD=5; _hz_complete
COMP_WORDS=(hz hg status --at "$HZ_AT" ""); COMP_CWORD=5; _hz_complete
COMP_WORDS=(hz --machine git handoff --at "$HZ_AT" ""); COMP_CWORD=6; _hz_complete
COMP_WORDS=(hz restore --at "$HZ_AT" ""); COMP_CWORD=4; _hz_complete
"#,
        ),
        (
            Shell::Zsh,
            "zsh",
            r#"
source "$HZ_INTEGRATION"
_arguments() { return 0 }
_values() { return 0 }
compadd() { return 0 }
words=(hz path --at "$HZ_AT" ""); CURRENT=5; _hz_complete
words=(hz git handoff --at "$HZ_AT" ""); CURRENT=6; _hz_complete
words=(hz git status --at "$HZ_AT" ""); CURRENT=6; _hz_complete
words=(hz hg status --at "$HZ_AT" ""); CURRENT=6; _hz_complete
words=(hz --machine git handoff --at "$HZ_AT" ""); CURRENT=7; _hz_complete
words=(hz restore --at "$HZ_AT" ""); CURRENT=5; _hz_complete
"#,
        ),
        (
            Shell::Fish,
            "fish",
            r#"
source "$HZ_INTEGRATION"
complete -C "hz path --at $HZ_AT " >/dev/null
complete -C "hz git handoff --at $HZ_AT " >/dev/null
complete -C "hz git status --at $HZ_AT " >/dev/null
complete -C "hz hg status --at $HZ_AT " >/dev/null
complete -C "hz --machine git handoff --at $HZ_AT " >/dev/null
complete -C "hz restore --at $HZ_AT " >/dev/null
"#,
        ),
    ];

    for (shell, executable, command) in shells {
        if Command::new(executable).arg("--version").output().is_err() {
            continue;
        }
        let temp = TempDir::new().unwrap();
        let bin = temp.path().join("bin");
        let args = temp.path().join("args");
        let at = temp.path().join("family");
        let integration = temp.path().join("integration");
        std::fs::create_dir(&bin).unwrap();
        std::fs::create_dir(&at).unwrap();
        std::fs::write(
            bin.join("hz"),
            "#!/bin/sh\nprintf 'CALL\\n' >> \"$HZ_ARGS\"\nprintf '%s\\n' \"$@\" >> \"$HZ_ARGS\"\nprintf 'alpha\\n'\n",
        )
        .unwrap();
        std::fs::set_permissions(bin.join("hz"), std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(&integration, shell_integration(shell)).unwrap();
        let path = std::env::join_paths(std::iter::once(bin.clone()).chain(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        )))
        .unwrap();

        let output = Command::new(executable)
            .args(["-c", command])
            .env("PATH", path)
            .env("HOME", temp.path())
            .env("HZ_ARGS", &args)
            .env("HZ_AT", &at)
            .env("HZ_INTEGRATION", &integration)
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "{executable} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let workspace_call = format!(
            "CALL\n__complete\nworkspace-targets\n--at\n{}\n",
            at.display()
        );
        let trash_call = format!("CALL\n__complete\ntrash-targets\n--at\n{}\n", at.display());
        assert_eq!(
            std::fs::read_to_string(&args).unwrap(),
            format!(
                "{workspace_call}{workspace_call}{workspace_call}{workspace_call}{workspace_call}{trash_call}"
            ),
            "unexpected {executable} completion calls"
        );
    }
}

#[cfg(unix)]
#[test]
fn zsh_completion_dispatches_after_global_options() {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    if Command::new("zsh").arg("--version").output().is_err() {
        return;
    }
    let temp = TempDir::new().unwrap();
    let bin = temp.path().join("bin");
    let calls = temp.path().join("calls");
    let values = temp.path().join("values");
    let integration = temp.path().join("integration");
    std::fs::create_dir(&bin).unwrap();
    std::fs::write(
        bin.join("hz"),
        "#!/bin/sh\nprintf '%s\\n' \"$@\" >> \"$HZ_CALLS\"\n",
    )
    .unwrap();
    std::fs::set_permissions(bin.join("hz"), std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(&integration, shell_integration(Shell::Zsh)).unwrap();
    let path = std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))
    .unwrap();

    let output = Command::new("zsh")
        .args([
            "-c",
            r#"
source "$HZ_INTEGRATION"
_arguments() { return 0 }
_values() { printf '%s\n' "$@" >> "$HZ_VALUES" }
compadd() { return 0 }
words=(hz --machine git ""); CURRENT=4; _hz_complete
words=(hz --machine path ""); CURRENT=4; _hz_complete
"#,
        ])
        .env("PATH", path)
        .env("HOME", temp.path())
        .env("HZ_CALLS", &calls)
        .env("HZ_VALUES", &values)
        .env("HZ_INTEGRATION", &integration)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "zsh failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(values).unwrap(),
        "git command\nstatus\nhandoff\n"
    );
    assert_eq!(
        std::fs::read_to_string(calls).unwrap(),
        "__complete\nworkspace-targets\n"
    );
}

#[cfg(unix)]
#[test]
fn bash_target_completion_preserves_whitespace_and_glob_characters() {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    if Command::new("bash").arg("--version").output().is_err() {
        return;
    }
    let temp = TempDir::new().unwrap();
    let bin = temp.path().join("bin");
    let integration = temp.path().join("integration");
    std::fs::create_dir(&bin).unwrap();
    std::fs::write(
        bin.join("hz"),
        "#!/bin/sh\nprintf '%s\\n' 'parser fix' 'parser * fix' other\n",
    )
    .unwrap();
    std::fs::set_permissions(bin.join("hz"), std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(&integration, shell_integration(Shell::Bash)).unwrap();
    let path = std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))
    .unwrap();

    let output = Command::new("bash")
        .args([
            "-c",
            r#"
source "$HZ_INTEGRATION"
COMP_WORDS=(hz path "parser"); COMP_CWORD=2; _hz_complete
[[ "${#COMPREPLY[@]}" -eq 2 ]]
[[ "${COMPREPLY[0]}" == "parser fix" ]]
[[ "${COMPREPLY[1]}" == "parser * fix" ]]
"#,
        ])
        .env("PATH", path)
        .env("HOME", temp.path())
        .env("HZ_INTEGRATION", integration)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "bash failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
#[test]
fn shell_wrappers_insert_path_only_before_argument_terminators() {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    let shells = [
        (
            Shell::Bash,
            "bash",
            "source \"$HZ_INTEGRATION\"; hz new -- -scratch",
        ),
        (
            Shell::Zsh,
            "zsh",
            "source \"$HZ_INTEGRATION\"; eval 'hz new -- -scratch'",
        ),
        (
            Shell::Fish,
            "fish",
            "source \"$HZ_INTEGRATION\"; hz new -- -scratch",
        ),
    ];

    for (shell, executable, command) in shells {
        if Command::new(executable).arg("--version").output().is_err() {
            continue;
        }
        let temp = TempDir::new().unwrap();
        let bin = temp.path().join("bin");
        let target = temp.path().join("target");
        let args = temp.path().join("args");
        let integration = temp.path().join("integration");
        std::fs::create_dir(&bin).unwrap();
        std::fs::create_dir(&target).unwrap();
        std::fs::write(
            bin.join("hz"),
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$HZ_ARGS\"\nprintf '%s\\n' \"$HZ_TARGET\"\n",
        )
        .unwrap();
        std::fs::set_permissions(bin.join("hz"), std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(&integration, shell_integration(shell)).unwrap();
        let path = std::env::join_paths(std::iter::once(bin.clone()).chain(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        )))
        .unwrap();

        let output = Command::new(executable)
            .args(["-c", command])
            .env("PATH", path)
            .env("HOME", temp.path())
            .env("HZ_ARGS", &args)
            .env("HZ_TARGET", &target)
            .env("HZ_INTEGRATION", &integration)
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "{executable} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            std::fs::read_to_string(args).unwrap(),
            "new\n--path-only\n--\n-scratch\n"
        );
    }
}
