#![cfg(unix)]

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
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

fn initialize_repo_with_lifecycle(repo: &Path) {
    git(repo, &["init", "-q"]);
    git(repo, &["config", "user.email", "test@example.com"]);
    git(repo, &["config", "user.name", "Test"]);
    fs::create_dir_all(repo.join(".hz/environment")).expect("lifecycle dir should be created");
    fs::write(
        repo.join(".hz/hz.toml"),
        "[lifecycle]\nsetup = [\".hz/environment/setup\"]\ncleanup = [\".hz/environment/cleanup\"]\n",
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
    let output = Command::new(hz())
        .env("HOME", home)
        .args(args)
        .output()
        .expect("hz should run");
    assert!(
        output.status.success(),
        "hz failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
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

fn temp_test_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "hz-cli-lifecycle-{name}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos()
    ))
}
