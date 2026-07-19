use std::{
    fs,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

#[test]
fn top_level_help_exposes_workspace_and_source_control_namespaces() {
    let output = Command::new(env!("CARGO_BIN_EXE_hz"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("new"));
    assert!(stdout.contains("ancestors"));
    assert!(stdout.contains("restore"));
    assert!(stdout.contains("git"));
    assert!(stdout.contains("hg"));
}

#[test]
fn machine_errors_are_structured_json() {
    let path = std::env::temp_dir().join(format!(
        "hz-machine-error-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&path).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_hz"))
        .env("HZ_DATABASE", path.join("workspaces.sqlite"))
        .args(["--machine", "list", "--at"])
        .arg(&path)
        .output()
        .unwrap();
    fs::remove_dir_all(path).unwrap();
    assert!(!output.status.success());
    let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["status"], "error");
    assert_eq!(error["error"]["code"], "workspace_not_initialized");
}

#[test]
fn machine_parse_errors_are_structured_json() {
    let output = Command::new(env!("CARGO_BIN_EXE_hz"))
        .args(["new", "--machine", "--not-a-real-option"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["status"], "error");
    assert_eq!(error["error"]["code"], "usage");
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("--not-a-real-option")
    );
}

#[test]
fn unknown_workspace_machine_errors_keep_their_stable_code() {
    let path = std::env::temp_dir().join(format!(
        "hz-unknown-workspace-error-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let root = path.join("project");
    fs::create_dir_all(&root).unwrap();
    let database = path.join("workspaces.sqlite");
    let initialized = Command::new(env!("CARGO_BIN_EXE_hz"))
        .env("HZ_DATABASE", &database)
        .current_dir(&root)
        .args(["init", "--here", "--copy"])
        .output()
        .unwrap();
    assert!(
        initialized.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&initialized.stderr)
    );

    let output = Command::new(env!("CARGO_BIN_EXE_hz"))
        .env("HZ_DATABASE", &database)
        .args(["--machine", "path", "missing", "--at"])
        .arg(&root)
        .output()
        .unwrap();
    fs::remove_dir_all(path).unwrap();

    assert!(!output.status.success());
    let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["code"], "unknown_workspace");
}

#[test]
fn removed_git_worktree_creation_command_is_not_accepted() {
    let output = Command::new(env!("CARGO_BIN_EXE_hz"))
        .args(["git", "new"])
        .output()
        .unwrap();
    assert!(!output.status.success());
}

#[test]
fn machine_removal_does_not_validate_unused_ancestors() {
    let path = std::env::temp_dir().join(format!(
        "hz-machine-remove-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let root = path.join("project");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("file.txt"), "root").unwrap();
    let database = path.join("workspaces.sqlite");
    let initialized = Command::new(env!("CARGO_BIN_EXE_hz"))
        .env("HZ_DATABASE", &database)
        .args(["init", "--here", "--copy"])
        .arg(&root)
        .output()
        .unwrap();
    assert!(
        initialized.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&initialized.stderr)
    );
    let created = Command::new(env!("CARGO_BIN_EXE_hz"))
        .env("HZ_DATABASE", &database)
        .args(["--machine", "new", "child", "--from"])
        .arg(&root)
        .arg("--no-hooks")
        .output()
        .unwrap();
    assert!(
        created.status.success(),
        "create failed: {}",
        String::from_utf8_lossy(&created.stderr)
    );
    let created: serde_json::Value = serde_json::from_slice(&created.stdout).unwrap();
    let child = created["workspace"]["path"].as_str().unwrap();
    fs::remove_file(root.join(".hz-workspace")).unwrap();

    let removed = Command::new(env!("CARGO_BIN_EXE_hz"))
        .env("HZ_DATABASE", &database)
        .args(["--machine", "rm", "--at"])
        .arg(child)
        .arg("--no-hooks")
        .output()
        .unwrap();

    assert!(
        removed.status.success(),
        "remove failed: {}",
        String::from_utf8_lossy(&removed.stderr)
    );
    assert!(!std::path::Path::new(child).exists());
    fs::remove_dir_all(path).unwrap();
}

#[cfg(unix)]
#[test]
fn removing_children_skips_the_preserved_workspace_preremove_hook() {
    let path = std::env::temp_dir().join(format!(
        "hz-remove-children-hook-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let root = path.join("project");
    fs::create_dir_all(&root).unwrap();
    let database = path.join("workspaces.sqlite");
    let initialized = Command::new(env!("CARGO_BIN_EXE_hz"))
        .env("HZ_DATABASE", &database)
        .args(["init", "--here", "--copy"])
        .arg(&root)
        .output()
        .unwrap();
    assert!(
        initialized.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&initialized.stderr)
    );
    let created = Command::new(env!("CARGO_BIN_EXE_hz"))
        .env("HZ_DATABASE", &database)
        .args(["--machine", "new", "child", "--from"])
        .arg(&root)
        .arg("--no-hooks")
        .output()
        .unwrap();
    assert!(
        created.status.success(),
        "create failed: {}",
        String::from_utf8_lossy(&created.stderr)
    );
    let created: serde_json::Value = serde_json::from_slice(&created.stdout).unwrap();
    let child = created["workspace"]["path"].as_str().unwrap().to_owned();
    fs::write(
        root.join(".hz/hz.toml"),
        "[lifecycle]\npreremove = [\"false\"]\n",
    )
    .unwrap();

    let removed = Command::new(env!("CARGO_BIN_EXE_hz"))
        .env("HZ_DATABASE", &database)
        .args(["--machine", "rm", "--children", "--at"])
        .arg(&root)
        .output()
        .unwrap();

    assert!(
        removed.status.success(),
        "remove failed: {}",
        String::from_utf8_lossy(&removed.stderr)
    );
    assert!(root.exists());
    assert!(!std::path::Path::new(&child).exists());
    fs::remove_dir_all(path).unwrap();
}

#[cfg(windows)]
#[test]
fn removing_the_current_workspace_requires_an_external_windows_cwd() {
    let path = std::env::temp_dir().join(format!(
        "hz-windows-current-removal-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let root = path.join("project");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("file.txt"), "root").unwrap();
    let database = path.join("workspaces.sqlite");
    let initialized = Command::new(env!("CARGO_BIN_EXE_hz"))
        .env("HZ_DATABASE", &database)
        .current_dir(&root)
        .args(["init", "--here", "--copy"])
        .output()
        .unwrap();
    assert!(
        initialized.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&initialized.stderr)
    );
    let created = Command::new(env!("CARGO_BIN_EXE_hz"))
        .env("HZ_DATABASE", &database)
        .current_dir(&root)
        .args(["--machine", "new", "child", "--no-hooks"])
        .output()
        .unwrap();
    assert!(
        created.status.success(),
        "create failed: {}",
        String::from_utf8_lossy(&created.stderr)
    );
    let created: serde_json::Value = serde_json::from_slice(&created.stdout).unwrap();
    let child = created["workspace"]["path"].as_str().unwrap();

    let blocked = Command::new(env!("CARGO_BIN_EXE_hz"))
        .env("HZ_DATABASE", &database)
        .current_dir(child)
        .args(["--machine", "rm", "--no-hooks"])
        .output()
        .unwrap();
    assert!(!blocked.status.success());
    assert!(String::from_utf8_lossy(&blocked.stderr).contains("change to another workspace"));
    assert!(std::path::Path::new(child).exists());

    let removed = Command::new(env!("CARGO_BIN_EXE_hz"))
        .env("HZ_DATABASE", &database)
        .current_dir(&root)
        .args(["--machine", "rm", "child", "--no-hooks"])
        .output()
        .unwrap();
    assert!(
        removed.status.success(),
        "remove failed: {}",
        String::from_utf8_lossy(&removed.stderr)
    );
    assert!(!std::path::Path::new(child).exists());
    fs::remove_dir_all(path).unwrap();
}

#[cfg(unix)]
#[test]
fn failing_machine_hooks_keep_stderr_as_one_json_document() {
    use std::os::unix::fs::PermissionsExt;

    let path = std::env::temp_dir().join(format!(
        "hz-machine-hook-error-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let root = path.join("project");
    fs::create_dir_all(&root).unwrap();
    let database = path.join("workspaces.sqlite");
    let initialized = Command::new(env!("CARGO_BIN_EXE_hz"))
        .env("HZ_DATABASE", &database)
        .current_dir(&root)
        .args(["init", "--here", "--copy"])
        .output()
        .unwrap();
    assert!(
        initialized.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&initialized.stderr)
    );

    fs::write(
        root.join(".hz/hz.toml"),
        "[lifecycle]\npostcreate = [\".hz/environment/postcreate\"]\n",
    )
    .unwrap();
    let hook = root.join(".hz/environment/postcreate");
    fs::write(
        &hook,
        "#!/usr/bin/env sh\nprintf 'hook stdout'\nprintf 'hook stderr' >&2\nexit 23\n",
    )
    .unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_hz"))
        .env("HZ_DATABASE", &database)
        .current_dir(&root)
        .args(["--machine", "new", "child"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["status"], "error");
    assert_eq!(error["error"]["code"], "usage");
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("hook stderr")
    );
    fs::remove_dir_all(path).unwrap();
}
