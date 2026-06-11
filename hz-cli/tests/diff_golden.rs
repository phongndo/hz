use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

fn hz() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_hz"))
}

#[test]
fn diff_renders_unborn_head_worktree() {
    let test_dir = temp_test_dir("unborn-head");
    let repo = test_dir.join("repo");
    fs::create_dir_all(&repo).expect("repo should be created");
    git(&repo, &["init", "-q"]);
    fs::write(repo.join("new.txt"), "new\n").expect("file should be written");

    let output = Command::new(hz())
        .args(["diff", "--no-watch", "--no-syntax", "-r"])
        .arg(&repo)
        .output()
        .expect("hz diff should run");

    assert!(
        output.status.success(),
        "hz diff failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("diff should be utf-8");
    assert!(stdout.contains("diff --git a/new.txt b/new.txt"));
    assert!(stdout.contains("new file mode"));
    assert!(stdout.contains("+new"));

    fs::remove_dir_all(test_dir).expect("test dir should be removed");
}

#[test]
fn diff_stat_escapes_terminal_control_characters_in_paths() {
    let test_dir = temp_test_dir("stat-escape");
    let repo = test_dir.join("repo");
    fs::create_dir_all(&repo).expect("repo should be created");
    git(&repo, &["init", "-q"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "Test"]);
    fs::write(repo.join("base.txt"), "base\n").expect("base file should be written");
    git(&repo, &["add", "base.txt"]);
    git(&repo, &["commit", "-q", "-m", "init"]);

    let evil_name = format!("evil{}]52;c;AAAA{}.txt", '\u{1b}', '\u{7}');
    fs::write(repo.join(&evil_name), "new\n").expect("evil file should be written");

    let output = Command::new(hz())
        .args(["diff", "--stat", "-r"])
        .arg(&repo)
        .output()
        .expect("hz diff --stat should run");

    assert!(
        output.status.success(),
        "hz diff --stat failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.stdout.contains(&0x1b));
    assert!(!output.stdout.contains(&0x07));
    let stdout = String::from_utf8(output.stdout).expect("stat should be utf-8");
    assert!(stdout.contains("\\u{1b}]52;c;AAAA\\u{7}.txt"));

    fs::remove_dir_all(test_dir).expect("test dir should be removed");
}

fn temp_test_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "hz-cli-diff-{name}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos()
    ))
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
