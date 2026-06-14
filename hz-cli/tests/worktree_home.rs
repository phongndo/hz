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
fn worktree_defaults_use_child_home() {
    let test_dir = temp_test_dir("default-home");
    let _cleanup = CleanupDir(test_dir.clone());
    let home = test_dir.join("home");
    let repo = test_dir.join(test_dir.file_name().expect("test dir should have name"));
    fs::create_dir_all(&home).expect("home should be created");
    fs::create_dir_all(&repo).expect("repo should be created");
    initialize_repo(&repo);

    let repo_name = repo.file_name().expect("repo should have name");
    let child_worktree_root = home.join(".hz").join("worktrees").join(repo_name);
    let parent_worktree_root = std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(".hz").join("worktrees").join(repo_name));
    if let Some(path) = &parent_worktree_root {
        assert!(!path.exists(), "test repo name should be unique");
    }

    let output = Command::new(hz())
        .env("HOME", &home)
        .args(["new", "--repo", repo.to_str().unwrap(), "--no-setup"])
        .output()
        .expect("hz should run");

    assert!(
        output.status.success(),
        "hz new failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if let Some(path) = parent_worktree_root.filter(|path| path.exists()) {
        let _ = fs::remove_dir_all(&path);
        panic!("worktree leaked into parent HOME: {}", path.display());
    }
    assert!(
        child_worktree_root.exists(),
        "worktree should be created under child HOME"
    );
}

fn initialize_repo(repo: &Path) {
    git(repo, &["init", "-q"]);
    git(repo, &["config", "user.email", "test@example.com"]);
    git(repo, &["config", "user.name", "Test"]);
    fs::write(repo.join("file.txt"), "initial content\n").expect("file should be written");
    git(repo, &["add", "file.txt"]);
    git(repo, &["commit", "-q", "-m", "init"]);
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
        "hz-cli-worktree-{name}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos()
    ))
}

struct CleanupDir(PathBuf);

impl Drop for CleanupDir {
    fn drop(&mut self) {
        if self.0.exists() {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
