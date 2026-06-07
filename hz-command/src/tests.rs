use super::*;
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
};

#[test]
fn init_repo_creates_config_and_lifecycle_scripts_once() {
    let test_dir = test_repo("hz-repo-init-test");

    let init = init_repo(InitRepo {
        repo: Some(test_dir.clone()),
    })
    .unwrap();

    assert!(init.config_created);
    assert!(init.setup_created);
    assert!(init.cleanup_created);
    assert_eq!(
        fs::read_to_string(&init.config_path).unwrap(),
        default_config()
    );
    assert!(
        fs::read_to_string(&init.setup_path)
            .unwrap()
            .contains("Add repo setup commands here.")
    );
    assert!(
        fs::read_to_string(&init.cleanup_path)
            .unwrap()
            .contains("Add repo cleanup commands here.")
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_ne!(
            fs::metadata(&init.setup_path).unwrap().permissions().mode() & 0o111,
            0
        );
        assert_ne!(
            fs::metadata(&init.cleanup_path)
                .unwrap()
                .permissions()
                .mode()
                & 0o111,
            0
        );
    }

    let second = init_repo(InitRepo {
        repo: Some(test_dir.clone()),
    })
    .unwrap();
    assert!(!second.config_created);
    assert!(!second.setup_created);
    assert!(!second.cleanup_created);

    fs::remove_dir_all(test_dir).unwrap();
}

#[test]
fn init_repo_uses_main_worktree_for_linked_worktree() {
    let test_dir = test_repo("hz-repo-init-linked-test");
    commit_initial(&test_dir);
    let linked_dir = test_dir.with_file_name(format!(
        "{}-linked",
        test_dir.file_name().unwrap().to_string_lossy()
    ));
    let linked_arg = linked_dir.to_str().unwrap();
    git(
        &["worktree", "add", "-q", "--detach", linked_arg, "HEAD"],
        &test_dir,
    );

    let init = init_repo(InitRepo {
        repo: Some(linked_dir.clone()),
    })
    .unwrap();

    assert_eq!(
        fs::canonicalize(&init.repo).unwrap(),
        fs::canonicalize(&test_dir).unwrap()
    );
    assert_eq!(
        fs::canonicalize(init.config_path.parent().unwrap()).unwrap(),
        fs::canonicalize(test_dir.join(".hz")).unwrap()
    );
    assert!(init.config_created);
    assert!(!linked_dir.join(".hz").join("hz.toml").exists());

    git(&["worktree", "remove", "-f", linked_arg], &test_dir);
    fs::remove_dir_all(test_dir).unwrap();
}

#[test]
fn init_repo_creates_hz_config_when_root_hz_toml_exists() {
    let test_dir = test_repo("hz-repo-init-root-config-test");
    fs::write(
        test_dir.join("hz.toml"),
        "[worktree]\ndefault_base = \"dev\"\n",
    )
    .unwrap();

    let init = init_repo(InitRepo {
        repo: Some(test_dir.clone()),
    })
    .unwrap();

    assert!(init.config_created);
    assert!(init.config_path.exists());
    assert!(init.setup_created);
    assert!(init.cleanup_created);

    let config = load_repo_config(LoadRepoConfig {
        repo: Some(test_dir.clone()),
    })
    .unwrap();
    assert_eq!(config.default_base(), None);

    fs::remove_dir_all(test_dir).unwrap();
}

#[test]
fn lifecycle_setup_runs_configured_script_in_worktree() {
    let test_dir = test_repo("hz-lifecycle-test");
    fs::create_dir_all(test_dir.join(".hz").join("environment")).unwrap();
    fs::write(
        test_dir.join(".hz").join(CONFIG_FILE),
        "[lifecycle]\nsetup = [\".hz/environment/setup\"]\n",
    )
    .unwrap();
    fs::write(
        test_dir.join(".hz").join("environment").join("setup"),
        "#!/usr/bin/env sh\nset -eu\nprintf '%s' \"$HZ_TARGET:$HZ_LIFECYCLE\" > lifecycle.out\n",
    )
    .unwrap();
    make_executable(&test_dir.join(".hz").join("environment").join("setup")).unwrap();

    let run = run_lifecycle(RunLifecycle {
        target: None,
        repo: Some(test_dir.clone()),
        kind: LifecycleKind::Setup,
    })
    .unwrap();

    assert!(run.configured);
    assert_eq!(run.target, "local");
    assert_eq!(
        fs::read_to_string(test_dir.join("lifecycle.out")).unwrap(),
        "local:setup"
    );

    fs::remove_dir_all(test_dir).unwrap();
}

#[test]
fn lifecycle_command_streams_stdout_to_sink() {
    let test_dir = test_repo("hz-lifecycle-stdout-test");
    fs::create_dir_all(test_dir.join(".hz").join("environment")).unwrap();
    let script = test_dir.join(".hz").join("environment").join("setup");
    fs::write(&script, "#!/usr/bin/env sh\nprintf 'hello stdout'\n").unwrap();
    make_executable(&script).unwrap();
    let mut stdout = Vec::new();

    run_lifecycle_command(
        &test_dir,
        &test_dir,
        "local",
        LifecycleKind::Setup,
        &[".hz/environment/setup".to_owned()],
        &mut stdout,
    )
    .unwrap();

    assert_eq!(String::from_utf8(stdout).unwrap(), "hello stdout");
    fs::remove_dir_all(test_dir).unwrap();
}

#[test]
fn repo_config_loads_hz_directory_config() {
    let test_dir = test_repo("hz-config-test");
    fs::create_dir_all(test_dir.join(".hz")).unwrap();
    fs::write(
        test_dir.join(".hz").join("hz.toml"),
        "[worktree]\ndefault_base = \"dev\"\n",
    )
    .unwrap();

    let config = load_repo_config(LoadRepoConfig {
        repo: Some(test_dir.clone()),
    })
    .unwrap();

    assert_eq!(config.default_base(), Some("dev"));

    fs::remove_dir_all(test_dir).unwrap();
}

#[test]
fn repo_config_ignores_root_hz_toml() {
    let test_dir = test_repo("hz-config-root-test");
    fs::write(
        test_dir.join("hz.toml"),
        "[worktree]\ndefault_base = \"dev\"\n",
    )
    .unwrap();

    let config = load_repo_config(LoadRepoConfig {
        repo: Some(test_dir.clone()),
    })
    .unwrap();

    assert_eq!(config.default_base(), None);

    fs::remove_dir_all(test_dir).unwrap();
}

#[test]
fn create_worktree_defaults_base_from_repo_config() {
    let test_dir = test_repo("hz-create-default-base-test");
    fs::create_dir_all(test_dir.join(".hz")).unwrap();
    fs::write(
        test_dir.join(".hz").join("hz.toml"),
        "[worktree]\ndefault_base = \"dev\"\n",
    )
    .unwrap();

    let input = create_worktree_with_config_defaults(CreateWorktree {
        name: Some("feature/ui".to_owned()),
        repo: Some(test_dir.clone()),
        path: None,
        base: None,
        branch: None,
        max_detached_worktrees: None,
    })
    .unwrap();

    assert_eq!(input.base.as_deref(), Some("dev"));

    fs::remove_dir_all(test_dir).unwrap();
}

#[test]
fn create_worktree_keeps_explicit_base_over_repo_config() {
    let test_dir = test_repo("hz-create-explicit-base-test");
    fs::create_dir_all(test_dir.join(".hz")).unwrap();
    fs::write(
        test_dir.join(".hz").join("hz.toml"),
        "[worktree]\ndefault_base = \"dev\"\n",
    )
    .unwrap();

    let input = create_worktree_with_config_defaults(CreateWorktree {
        name: Some("feature/ui".to_owned()),
        repo: Some(test_dir.clone()),
        path: None,
        base: Some("main".to_owned()),
        branch: None,
        max_detached_worktrees: None,
    })
    .unwrap();

    assert_eq!(input.base.as_deref(), Some("main"));

    fs::remove_dir_all(test_dir).unwrap();
}

#[test]
fn lifecycle_is_noop_without_configured_command() {
    let test_dir = test_repo("hz-lifecycle-noop-test");

    let run = run_lifecycle(RunLifecycle {
        target: None,
        repo: Some(test_dir.clone()),
        kind: LifecycleKind::Cleanup,
    })
    .unwrap();

    assert!(!run.configured);
    assert_eq!(run.target, "local");

    fs::remove_dir_all(test_dir).unwrap();
}

#[test]
fn lifecycle_target_is_consistent_for_created_and_found_worktrees() {
    let created = CreatedWorktree {
        id: "id".to_owned(),
        name: "handle".to_owned(),
        handle: "handle".to_owned(),
        repo: PathBuf::from("/repo"),
        path: PathBuf::from("/repo/../worktrees/handle"),
        branch: Some("feature/login".to_owned()),
        base: None,
        source: WorktreeSource::Managed,
        warnings: Vec::new(),
    };
    let found = WorktreeEntry {
        id: "id".to_owned(),
        handle: "handle".to_owned(),
        repo: PathBuf::from("/repo"),
        path: PathBuf::from("/repo/../worktrees/handle"),
        branch: Some("feature/login".to_owned()),
        base: None,
        source: WorktreeSource::Managed,
        created_at_unix: 0,
        modified_at_unix: 0,
        status: WorktreeStatus::Unknown,
    };

    assert_eq!(created_worktree_target(&created), "feature/login");
    assert_eq!(worktree_target(&found), "feature/login");

    let detached = CreatedWorktree {
        branch: None,
        ..created
    };
    assert_eq!(created_worktree_target(&detached), "handle");
}

#[test]
fn github_pull_request_url_parses() {
    assert_eq!(
        github_pull_request_from_url("https://github.com/owner/repo/pull/123/files?plain=1"),
        Some(GitHubPullRequest {
            owner: "owner".to_owned(),
            repo: "repo".to_owned(),
            number: 123,
        })
    );
    assert_eq!(
        github_pull_request_from_url("github.com/owner/repo/pull/456"),
        Some(GitHubPullRequest {
            owner: "owner".to_owned(),
            repo: "repo".to_owned(),
            number: 456,
        })
    );

    assert_eq!(
        github_pull_request_from_url("https://example.com/owner/repo/pull/1"),
        None
    );
    assert_eq!(
        github_pull_request_from_url("https://github.com/owner/repo/issues/1"),
        None
    );
    assert_eq!(
        github_pull_request_from_url("https://github.com/owner/repo/pull/0"),
        None
    );
}

#[test]
fn github_remote_url_parses_common_git_url_forms() {
    for remote in [
        "git@github.com:owner/repo.git",
        "ssh://git@github.com/owner/repo.git",
        "https://github.com/owner/repo.git",
        "https://github.com/owner/repo",
        "https://x-access-token:secret@github.com/owner/repo.git",
        "https://user:password@github.com/owner/repo",
    ] {
        assert_eq!(
            github_repo_from_remote_url(remote),
            Some(("owner".to_owned(), "repo".to_owned()))
        );
    }

    assert_eq!(
        github_repo_from_remote_url("https://example.com/owner/repo.git"),
        None
    );
    assert_eq!(
        github_repo_from_remote_url("https://github.com/owner"),
        None
    );
    assert_eq!(
        github_repo_from_remote_url("https://token@example.com/owner/repo.git"),
        None
    );
    assert_eq!(
        github_repo_from_remote_url("https://example.com/path@github.com/owner/repo.git"),
        None
    );
}

#[test]
fn github_remote_error_redacts_url_userinfo() {
    let repo = test_repo("hz-github-pr-redact-origin-test");
    git(
        &[
            "remote",
            "add",
            "origin",
            "https://user:secret-token@example.com/owner/repo.git",
        ],
        &repo,
    );

    let error = github_pull_request_from_target(Some(&repo), "42")
        .expect_err("non-GitHub origin should fail");
    let message = error.to_string();

    assert!(message.contains("https://<redacted>@example.com/owner/repo.git"));
    assert!(!message.contains("secret-token"));
    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn github_curl_config_includes_timeouts_and_escapes_values() {
    let config = github_curl_config(
        "https://github.com/owner/repo/pull/1.diff",
        Some("tok\"en\n"),
    );

    assert!(config.contains("connect-timeout = \"10\"\n"));
    assert!(config.contains("max-time = \"60\"\n"));
    assert!(config.contains("header = \"User-Agent: hz\"\n"));
    assert!(config.contains("header = \"Authorization: Bearer tok\\\"en\\n\"\n"));
    assert!(config.contains("url = \"https://github.com/owner/repo/pull/1.diff\"\n"));
}

#[test]
fn github_pull_request_number_uses_origin_remote() {
    let repo = test_repo("hz-github-pr-origin-test");
    git(
        &["remote", "add", "origin", "git@github.com:owner/repo.git"],
        &repo,
    );

    let pull_request = github_pull_request_from_target(Some(&repo), "42")
        .expect("pull request should be inferred from origin");

    assert_eq!(
        pull_request,
        GitHubPullRequest {
            owner: "owner".to_owned(),
            repo: "repo".to_owned(),
            number: 42,
        }
    );

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn zsh_init_line_is_rc_file_friendly() {
    assert_eq!(shell_init_line(Shell::Zsh), r#"eval "$(hz shell zsh)""#);
}

#[test]
fn shell_rc_paths_respect_zdotdir_and_ignore_empty_xdg_config_home() {
    let home = Some(PathBuf::from("/home/user"));

    assert_eq!(
        shell_rc_path_from_env(
            Shell::Zsh,
            home.clone(),
            Some(PathBuf::from("/tmp/zdotdir")),
            None,
        )
        .unwrap(),
        PathBuf::from("/tmp/zdotdir/.zshrc")
    );
    assert_eq!(
        shell_rc_path_from_env(Shell::Fish, home, None, Some(PathBuf::new())).unwrap(),
        PathBuf::from("/home/user/.config/fish/config.fish")
    );
}

#[test]
fn shell_rc_paths_do_not_fall_back_to_empty_home() {
    assert_eq!(
        shell_rc_path_from_env(
            Shell::Zsh,
            Some(PathBuf::new()),
            Some(PathBuf::from("/tmp/zdotdir")),
            None,
        )
        .unwrap(),
        PathBuf::from("/tmp/zdotdir/.zshrc")
    );
    assert_eq!(
        shell_rc_path_from_env(
            Shell::Fish,
            Some(PathBuf::new()),
            None,
            Some(PathBuf::from("/tmp/config")),
        )
        .unwrap(),
        PathBuf::from("/tmp/config/fish/config.fish")
    );
    assert!(shell_rc_path_from_env(Shell::Bash, Some(PathBuf::new()), None, None).is_err());
}

#[test]
fn zsh_integration_wraps_new_and_cd() {
    let script = shell_integration(Shell::Zsh);
    let hzlocal_completion = script
        .split("_hzlocal_completion() {")
        .nth(1)
        .and_then(|completion| completion.split("\n}").next())
        .expect("hzlocal completion function should exist");

    assert!(script.contains("command hz \"$@\" --path-only"));
    assert!(script.contains("alias hz='noglob _hz'"));
    assert!(script.contains("_hz() {"));
    assert!(script.contains("alias hzcd='noglob _hzcd'"));
    assert!(script.contains("_hzcd() {"));
    assert!(script.contains("alias hzlocal='noglob _hzlocal'"));
    assert!(script.contains("_hzlocal() {"));
    assert!(script.contains("handoff)"));
    assert!(script.contains("--json|--path-only|--help|-h|-j"));
    assert!(script.contains("builtin cd \"$hz_target_path\" || return"));
    assert!(script.contains("command hz __complete worktree-targets \"${complete_args[@]}\""));
    assert!(script.contains("command hz __complete removable-worktrees \"${complete_args[@]}\""));
    assert!(script.contains("complete_args=(-r \"$repo\")"));
    assert!(script.contains("compdef _hz_completion hz _hz"));
    assert!(script.contains("compdef _hzcd_completion hzcd _hzcd"));
    assert!(script.contains("compdef _hzlocal_completion hzlocal _hzlocal"));
    assert!(script.contains("compadd -- -h --help -V --version"));
    assert!(script.contains("if [[ \"$PREFIX\" == -* ]]; then"));
    assert!(script.contains("_hz_complete_command_options \"$cmd\""));
    assert!(script.contains("_hz_complete_command_positionals \"$cmd\""));
    assert!(script.contains("_hz_complete_option_value \"$cmd\""));
    assert!(script.contains("_hz_git_refs"));
    assert!(script.contains("--branch)"));
    assert!(!script.contains("-b|--branch"));
    assert!(script.contains("compinit -C"));
    assert!(script.contains("shift words"));
    assert!(script.contains("shift 2 words"));
    assert!(script.contains("'rm:remove one or more worktrees'"));
    assert!(script.contains("'install:install shell integration'"));
    assert!(script.contains("'update:update hz from GitHub releases'"));
    assert!(script.contains("'diff:review a git diff'"));
    assert!(script.contains("'ts:manage diff syntax highlighting languages'"));
    assert!(script.contains("'add:install and enable syntax highlighting languages'"));
    assert!(script.contains("'update:update cached syntax highlighting parsers'"));
    assert!(script.contains("--installed --enabled"));
    assert!(script.contains("--all -h --help"));
    assert!(!script.contains("tui:open the terminal UI"));
    assert!(script.contains("--no-setup"));
    assert!(script.contains("--no-cleanup"));
    assert!(script.contains("--max-detached"));
    assert!(script.contains("--force-self-update"));
    assert!(script.contains("--pr"));
    assert!(script.contains("--patch"));
    assert!(script.contains("--staged"));
    assert!(script.contains("--unstaged"));
    assert!(script.contains("--no-untracked"));
    assert!(script.contains("--no-watch"));
    assert!(script.contains("--no-syntax"));
    assert!(hzlocal_completion.contains("_hz_complete_command_options cd"));
    assert!(!hzlocal_completion.contains("_hz_complete_command_positionals cd"));
}

#[test]
fn fish_integration_passes_json_short_flag_through() {
    let script = shell_integration(Shell::Fish);

    assert!(script.contains("case --json --path-only --help -h -j"));
    assert!(script.contains("or return"));
    assert!(script.contains("command hz __complete worktree-targets -r \"$repo\""));
    assert!(script.contains("command hz __complete removable-worktrees -r \"$repo\""));
    assert!(script.contains("__hz_command_is"));
    assert!(script.contains("__hz_top_command_is update"));
    assert!(script.contains("__hz_diff_position_is_revision"));
    assert!(script.contains("__hz_complete_git_refs"));
    assert!(script.contains("complete -c hz -n \"__hz_command_is remove rm\""));
    assert!(script.contains("init install setup cleanup shell update"));
    assert!(script.contains("ts tree-sitter"));
    assert!(script.contains("__hz_needs_ts_subcommand"));
    assert!(script.contains("add update rm remove list ls available clean path doctor"));
    assert!(script.contains("not __fish_seen_subcommand_from new path cd list ls"));
    assert!(script.contains("-l installed"));
    assert!(script.contains("-l enabled"));
    assert!(script.contains("-l all"));
    assert!(script.contains("-l no-setup"));
    assert!(script.contains("-l no-cleanup"));
    assert!(script.contains("-l max-detached"));
    assert!(script.contains("-l target-version"));
    assert!(script.contains("-l force-self-update"));
    assert!(script.contains("-l pr"));
    assert!(script.contains("-l patch"));
    assert!(script.contains("-l staged"));
    assert!(script.contains("-l unstaged"));
    assert!(script.contains("-l no-untracked"));
    assert!(script.contains("-l no-watch"));
    assert!(script.contains("-l no-syntax"));
    assert!(!script.contains("tui"));
}

#[test]
fn bash_integration_registers_completion() {
    let script = shell_integration(Shell::Bash);
    let worktree_completion = script
        .split("if [[ \"$cmd\" == \"worktree\" || \"$cmd\" == \"wt\" ]]; then")
        .nth(1)
        .and_then(|completion| completion.split("if [[ \"$cmd\" == \"ts\"").next())
        .expect("worktree completion branch should exist");
    let ts_completion = script
        .split("if [[ \"$cmd\" == \"ts\" || \"$cmd\" == \"tree-sitter\" ]]; then")
        .nth(1)
        .and_then(|completion| {
            completion
                .split("_hz_complete_command_args \"$cmd\"")
                .next()
        })
        .expect("tree-sitter completion branch should exist");

    assert!(script.contains("complete -F _hz_completion hz"));
    assert!(script.contains("_hz_dynamic_reply worktree-targets"));
    assert!(script.contains("_hz_dynamic_reply removable-worktrees"));
    assert!(script.contains("command hz __complete \"$command\" -r \"$repo\""));
    assert!(script.contains("for ((index = 1; index < COMP_CWORD; index++))"));
    assert!(script.contains("_hz_complete_option_value"));
    assert!(script.contains("_hz_git_ref_reply"));
    assert!(script.contains("--branch)"));
    assert!(!script.contains("-b|--branch"));
    assert!(script.contains("init install setup cleanup shell update"));
    assert!(script.contains("ts tree-sitter"));
    assert!(script.contains("_hz_complete_ts_args"));
    assert!(script.contains("add update rm remove list ls available clean path doctor"));
    assert!(script.contains("--installed --enabled"));
    assert!(script.contains("--all -h --help"));
    assert!(script.contains("--no-setup"));
    assert!(script.contains("--no-cleanup"));
    assert!(script.contains("--max-detached"));
    assert!(script.contains("--target-version"));
    assert!(script.contains("--force-self-update"));
    assert!(script.contains("--pr"));
    assert!(script.contains("--patch"));
    assert!(script.contains("--staged"));
    assert!(script.contains("--unstaged"));
    assert!(script.contains("--no-untracked"));
    assert!(script.contains("--no-watch"));
    assert!(script.contains("--no-syntax"));
    assert!(
        worktree_completion.contains("_hz_complete_command_args \"${COMP_WORDS[2]}\" \"$current\"")
    );
    assert!(ts_completion.contains("_hz_complete_ts_args \"${COMP_WORDS[2]}\" \"$current\""));
    assert!(!script.contains("tui"));
}

#[test]
fn installs_line_once() {
    let test_dir = env::temp_dir().join(format!(
        "hz-init-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos()
    ));
    let rc_file = test_dir.join(".zshrc");

    assert!(install_line(&rc_file, shell_init_line(Shell::Zsh)).unwrap());
    assert!(!install_line(&rc_file, shell_init_line(Shell::Zsh)).unwrap());

    let contents = fs::read_to_string(&rc_file).unwrap();
    assert_eq!(contents.matches(shell_init_line(Shell::Zsh)).count(), 1);
    assert_eq!(contents.matches(shell_init_comment()).count(), 1);

    fs::remove_dir_all(test_dir).unwrap();
}

#[test]
fn creates_backup_before_first_rc_file_install() {
    let test_dir = shell_install_test_dir("hz-init-backup-test");
    let rc_file = test_dir.join(".zshrc");
    let original = "export PATH=\"$HOME/bin:$PATH\"\n";
    fs::write(&rc_file, original).unwrap();

    assert!(install_line(&rc_file, shell_init_line(Shell::Zsh)).unwrap());

    let contents = fs::read_to_string(&rc_file).unwrap();
    assert_eq!(
        contents,
        format!(
            "{}{}\n{}\n",
            original,
            shell_init_comment(),
            shell_init_line(Shell::Zsh)
        )
    );
    assert_eq!(
        fs::read_to_string(shell_rc_backup_path(&rc_file).unwrap()).unwrap(),
        original
    );
    assert_no_shell_rc_temp_files(&test_dir);

    fs::remove_dir_all(test_dir).unwrap();
}

#[test]
fn does_not_overwrite_existing_rc_file_backup() {
    let test_dir = shell_install_test_dir("hz-init-existing-backup-test");
    let rc_file = test_dir.join(".zshrc");
    let backup_file = shell_rc_backup_path(&rc_file).unwrap();
    fs::write(&rc_file, "alias ll='ls -l'\n").unwrap();
    fs::write(&backup_file, "user backup\n").unwrap();

    assert!(install_line(&rc_file, shell_init_line(Shell::Zsh)).unwrap());

    assert_eq!(fs::read_to_string(&backup_file).unwrap(), "user backup\n");
    assert_no_shell_rc_temp_files(&test_dir);

    fs::remove_dir_all(test_dir).unwrap();
}

#[cfg(unix)]
#[test]
fn installs_line_without_replacing_symlinked_rc_file() {
    let test_dir = shell_install_test_dir("hz-init-symlink-test");
    let target_dir = test_dir.join("dotfiles");
    fs::create_dir_all(&target_dir).unwrap();
    let rc_file = test_dir.join(".zshrc");
    let target_file = target_dir.join("zshrc");
    fs::write(&target_file, "# managed dotfile\n").unwrap();
    std::os::unix::fs::symlink(&target_file, &rc_file).unwrap();

    assert!(install_line(&rc_file, shell_init_line(Shell::Zsh)).unwrap());

    assert!(
        fs::symlink_metadata(&rc_file)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        fs::read_to_string(&target_file).unwrap(),
        format!(
            "# managed dotfile\n{}\n{}\n",
            shell_init_comment(),
            shell_init_line(Shell::Zsh)
        )
    );
    assert_eq!(
        fs::read_to_string(shell_rc_backup_path(&rc_file).unwrap()).unwrap(),
        "# managed dotfile\n"
    );
    assert_no_shell_rc_temp_files(&test_dir);

    fs::remove_dir_all(test_dir).unwrap();
}

#[test]
fn does_not_duplicate_existing_bare_line() {
    let test_dir = env::temp_dir().join(format!(
        "hz-init-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos()
    ));
    let rc_file = test_dir.join(".zshrc");
    fs::create_dir_all(&test_dir).unwrap();
    fs::write(&rc_file, format!("{}\n", shell_init_line(Shell::Zsh))).unwrap();

    assert!(!install_line(&rc_file, shell_init_line(Shell::Zsh)).unwrap());

    let contents = fs::read_to_string(&rc_file).unwrap();
    assert_eq!(contents.matches(shell_init_line(Shell::Zsh)).count(), 1);
    assert_eq!(contents.matches(shell_init_comment()).count(), 0);

    fs::remove_dir_all(test_dir).unwrap();
}

fn shell_install_test_dir(prefix: &str) -> PathBuf {
    let test_dir = env::temp_dir().join(format!(
        "{prefix}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos()
    ));
    fs::create_dir_all(&test_dir).unwrap();
    test_dir
}

fn assert_no_shell_rc_temp_files(path: &Path) {
    assert!(!fs::read_dir(path).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".tmp")
    }));
}

fn test_repo(prefix: &str) -> PathBuf {
    let test_dir = env::temp_dir().join(format!(
        "{prefix}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos()
    ));
    fs::create_dir_all(&test_dir).unwrap();
    let status = ProcessCommand::new("git")
        .arg("init")
        .arg("-q")
        .arg(&test_dir)
        .status()
        .unwrap();
    assert!(status.success());
    test_dir
}

fn commit_initial(repo: &Path) {
    git(&["config", "user.email", "test@example.com"], repo);
    git(&["config", "user.name", "Test"], repo);
    fs::write(repo.join("file.txt"), "base\n").unwrap();
    git(&["add", "file.txt"], repo);
    git(&["commit", "-q", "-m", "init"], repo);
}

fn git(args: &[&str], cwd: &Path) {
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
