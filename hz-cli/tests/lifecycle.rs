#![cfg(unix)]

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::{SystemTime, UNIX_EPOCH},
};

fn hz() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_hz"))
}

#[test]
fn lifecycle_hooks_run_only_when_explicitly_requested() {
    let test_dir = temp_test_dir("hook-opt-in");
    let home = test_dir.join("home");
    let repo = test_dir.join("repo");
    let worktrees = test_dir.join("worktrees");
    let lifecycle_log = repo.join("lifecycle.log");
    fs::create_dir_all(&home).expect("home should be created");
    fs::create_dir_all(&repo).expect("repo should be created");
    fs::create_dir_all(&worktrees).expect("worktrees dir should be created");
    initialize_repo_with_lifecycle(&repo);

    run_hz(
        &home,
        &[
            "new",
            "no-setup",
            "--repo",
            repo.to_str().unwrap(),
            "--path",
            worktrees.join("no-setup").to_str().unwrap(),
        ],
    );
    assert!(!lifecycle_log.exists(), "setup ran without --setup");

    run_hz(
        &home,
        &[
            "new",
            "with-setup",
            "--repo",
            repo.to_str().unwrap(),
            "--path",
            worktrees.join("with-setup").to_str().unwrap(),
            "--setup",
        ],
    );
    assert_eq!(
        fs::read_to_string(&lifecycle_log).expect("lifecycle log should exist"),
        "with-setup:setup\n"
    );

    run_hz(
        &home,
        &[
            "new",
            "setup-optout",
            "--repo",
            repo.to_str().unwrap(),
            "--path",
            worktrees.join("setup-optout").to_str().unwrap(),
            "--setup",
            "--no-setup",
        ],
    );
    assert_eq!(
        fs::read_to_string(&lifecycle_log).expect("lifecycle log should exist"),
        "with-setup:setup\n"
    );

    run_hz(
        &home,
        &[
            "rm",
            "no-setup",
            "--repo",
            repo.to_str().unwrap(),
            "--force",
        ],
    );
    assert_eq!(
        fs::read_to_string(&lifecycle_log).expect("lifecycle log should exist"),
        "with-setup:setup\n"
    );

    run_hz(
        &home,
        &[
            "rm",
            "with-setup",
            "--repo",
            repo.to_str().unwrap(),
            "--force",
            "--cleanup",
        ],
    );
    assert_eq!(
        fs::read_to_string(&lifecycle_log).expect("lifecycle log should exist"),
        "with-setup:setup\nwith-setup:cleanup\n"
    );

    run_hz(
        &home,
        &[
            "rm",
            "setup-optout",
            "--repo",
            repo.to_str().unwrap(),
            "--force",
            "--cleanup",
            "--no-cleanup",
        ],
    );
    assert_eq!(
        fs::read_to_string(&lifecycle_log).expect("lifecycle log should exist"),
        "with-setup:setup\nwith-setup:cleanup\n"
    );

    fs::remove_dir_all(test_dir).expect("test dir should be removed");
}

#[test]
fn cleanup_runs_for_user_managed_git_worktree_when_requested() {
    let test_dir = temp_test_dir("user-managed-cleanup");
    let home = test_dir.join("home");
    let repo = test_dir.join("repo");
    let worktree = repo.join("agent-worktrees/external-cleanup");
    let lifecycle_log = repo.join("lifecycle.log");
    fs::create_dir_all(&home).expect("home should be created");
    fs::create_dir_all(&repo).expect("repo should be created");
    initialize_repo_with_lifecycle(&repo);
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "external-cleanup",
            worktree.to_str().unwrap(),
            "HEAD",
        ],
    );

    run_hz(
        &home,
        &[
            "rm",
            "external-cleanup",
            "--repo",
            repo.to_str().unwrap(),
            "--force",
            "--cleanup",
        ],
    );

    assert_eq!(
        fs::read_to_string(&lifecycle_log).expect("lifecycle log should exist"),
        "external-cleanup:cleanup\n"
    );
    assert!(!worktree.exists(), "git worktree should be removed");

    fs::remove_dir_all(test_dir).expect("test dir should be removed");
}

#[test]
fn fork_no_diff_leaves_dirty_changes_behind() {
    let test_dir = temp_test_dir("fork-no-diff");
    let home = test_dir.join("home");
    let repo = test_dir.join("repo");
    let destination = test_dir.join("forked");
    fs::create_dir_all(&home).expect("home should be created");
    fs::create_dir_all(&repo).expect("repo should be created");
    initialize_repo_with_lifecycle(&repo);

    fs::write(repo.join(".hz/hz.toml"), "# changed\n").expect("tracked file should be changed");
    fs::write(repo.join("untracked.txt"), "untracked\n").expect("untracked file should be written");

    run_hz(
        &home,
        &[
            "fork",
            "clean-copy",
            "--repo",
            repo.to_str().unwrap(),
            "--path",
            destination.to_str().unwrap(),
            "--no-diff",
        ],
    );

    assert_eq!(
        fs::read_to_string(destination.join(".hz/hz.toml"))
            .expect("forked tracked file should exist"),
        "[worktree]\nuser_managed_roots = [\"agent-worktrees\"]\n\n[lifecycle]\nsetup = [\".hz/environment/setup\"]\ncleanup = [\".hz/environment/cleanup\"]\n"
    );
    assert!(!destination.join("untracked.txt").exists());
    assert_eq!(
        fs::read_to_string(repo.join(".hz/hz.toml")).expect("source tracked file should exist"),
        "# changed\n"
    );
    assert!(repo.join("untracked.txt").exists());
    assert!(
        !repo.join("lifecycle.log").exists(),
        "fork ran lifecycle hooks"
    );

    fs::remove_dir_all(test_dir).expect("test dir should be removed");
}

#[test]
fn fork_copies_dirty_changes_by_default() {
    let test_dir = temp_test_dir("fork-with-diff");
    let home = test_dir.join("home");
    let repo = test_dir.join("repo");
    let destination = test_dir.join("forked");
    fs::create_dir_all(&home).expect("home should be created");
    fs::create_dir_all(&repo).expect("repo should be created");
    initialize_repo_with_lifecycle(&repo);

    fs::write(repo.join(".hz/hz.toml"), "# changed\n").expect("tracked file should be changed");
    fs::write(repo.join("untracked.txt"), "untracked\n").expect("untracked file should be written");

    run_hz(
        &home,
        &[
            "fork",
            "dirty-copy",
            "--repo",
            repo.to_str().unwrap(),
            "--path",
            destination.to_str().unwrap(),
        ],
    );

    assert_eq!(
        fs::read_to_string(destination.join(".hz/hz.toml"))
            .expect("forked tracked file should exist"),
        "# changed\n"
    );
    assert_eq!(
        fs::read_to_string(destination.join("untracked.txt"))
            .expect("forked untracked file should exist"),
        "untracked\n"
    );
    assert_eq!(
        fs::read_to_string(repo.join(".hz/hz.toml")).expect("source tracked file should exist"),
        "# changed\n"
    );
    assert!(repo.join("untracked.txt").exists());
    assert!(
        !repo.join("lifecycle.log").exists(),
        "fork ran lifecycle hooks"
    );

    fs::remove_dir_all(test_dir).expect("test dir should be removed");
}

#[test]
fn fork_clean_worktree_reports_unchanged_json() {
    let test_dir = temp_test_dir("fork-clean-json");
    let home = test_dir.join("home");
    let repo = test_dir.join("repo");
    let destination = test_dir.join("forked");
    fs::create_dir_all(&home).expect("home should be created");
    fs::create_dir_all(&repo).expect("repo should be created");
    initialize_repo_with_lifecycle(&repo);

    let output = run_hz_output(
        &home,
        None,
        &[
            "fork",
            "clean-json",
            "--repo",
            repo.to_str().unwrap(),
            "--path",
            destination.to_str().unwrap(),
            "--json",
        ],
    );
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("fork output should be json");

    assert_eq!(json["changed"], false);
    assert_eq!(json["worktree"]["handle"], "clean-json");
    assert_eq!(
        json["worktree"]["path"],
        fs::canonicalize(&destination)
            .expect("destination should exist")
            .to_string_lossy()
            .as_ref()
    );
    assert_eq!(
        git_stdout(&repo, &["rev-parse", "HEAD"]),
        git_stdout(&destination, &["rev-parse", "HEAD"])
    );
    assert!(!destination.join("untracked.txt").exists());
    assert!(
        !repo.join("lifecycle.log").exists(),
        "fork ran lifecycle hooks"
    );

    fs::remove_dir_all(test_dir).expect("test dir should be removed");
}

#[test]
fn fork_uses_current_linked_worktree_state_when_repo_points_at_main() {
    let test_dir = temp_test_dir("fork-linked-current-with-repo");
    let home = test_dir.join("home");
    let repo = test_dir.join("repo");
    let linked = test_dir.join("linked");
    let destination = test_dir.join("forked");
    fs::create_dir_all(&home).expect("home should be created");
    fs::create_dir_all(&repo).expect("repo should be created");
    initialize_repo_with_lifecycle(&repo);
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            linked.to_str().unwrap(),
            "HEAD",
        ],
    );

    fs::write(repo.join(".hz/hz.toml"), "# main\n").expect("main file should be changed");
    fs::write(linked.join(".hz/hz.toml"), "# linked\n").expect("linked file should be changed");
    fs::write(linked.join("linked.txt"), "linked\n")
        .expect("linked untracked file should be written");

    run_hz_output(
        &home,
        Some(&linked),
        &[
            "fork",
            "linked-copy",
            "--repo",
            repo.to_str().unwrap(),
            "--path",
            destination.to_str().unwrap(),
        ],
    );

    assert_eq!(
        fs::read_to_string(destination.join(".hz/hz.toml"))
            .expect("forked tracked file should exist"),
        "# linked\n"
    );
    assert_eq!(
        fs::read_to_string(destination.join("linked.txt"))
            .expect("forked untracked file should exist"),
        "linked\n"
    );
    assert_eq!(
        fs::read_to_string(repo.join(".hz/hz.toml")).expect("main tracked file should exist"),
        "# main\n"
    );

    fs::remove_dir_all(test_dir).expect("test dir should be removed");
}

fn initialize_repo_with_lifecycle(repo: &Path) {
    git(repo, &["init", "-q"]);
    git(repo, &["config", "user.email", "test@example.com"]);
    git(repo, &["config", "user.name", "Test"]);
    fs::create_dir_all(repo.join(".hz/environment")).expect("lifecycle dir should be created");
    fs::write(
        repo.join(".hz/hz.toml"),
        "[worktree]\nuser_managed_roots = [\"agent-worktrees\"]\n\n[lifecycle]\nsetup = [\".hz/environment/setup\"]\ncleanup = [\".hz/environment/cleanup\"]\n",
    )
    .expect("config should be written");
    write_hook(repo, "setup");
    write_hook(repo, "cleanup");
    git(repo, &["add", ".hz"]);
    git(repo, &["commit", "-q", "-m", "init"]);
}

fn write_hook(repo: &Path, name: &str) {
    let path = repo.join(".hz/environment").join(name);
    fs::write(
        &path,
        "#!/usr/bin/env sh\nset -eu\nprintf '%s:%s\\n' \"$HZ_TARGET\" \"$HZ_LIFECYCLE\" >> \"$HZ_REPO/lifecycle.log\"\n",
    )
    .expect("hook should be written");
    let mut permissions = fs::metadata(&path)
        .expect("hook metadata should be available")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).expect("hook should be executable");
}

fn run_hz(home: &Path, args: &[&str]) {
    run_hz_output(home, None, args);
}

fn run_hz_output(home: &Path, current_dir: Option<&Path>, args: &[&str]) -> Output {
    let mut command = Command::new(hz());
    command.env("HOME", home).args(args);
    if let Some(current_dir) = current_dir {
        command.current_dir(current_dir);
    }
    let output = command.output().expect("hz should run");
    assert!(
        output.status.success(),
        "hz failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .expect("git should run");
    assert!(
        output.status.success(),
        "git failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_stdout(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .expect("git should run");
    assert!(
        output.status.success(),
        "git failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("git output should be utf-8")
}

fn temp_test_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "hz-cli-lifecycle-{name}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos()
    ))
}
