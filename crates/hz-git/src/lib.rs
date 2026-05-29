use std::{
    env, fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use hz_core::{HzError, HzResult};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitRepository {
    pub root: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitWorktreeSpec {
    pub repo: Option<PathBuf>,
    pub path: Option<PathBuf>,
    pub base: Option<String>,
    pub branch: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitDiffSpec {
    pub repo: Option<PathBuf>,
    pub base: Option<String>,
    pub stat: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitWorktree {
    pub path: PathBuf,
    pub branch: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitWorktreeState {
    pub dirty: bool,
    pub modified_at_unix: u64,
}

pub fn repository_root(repo: Option<&Path>) -> HzResult<PathBuf> {
    let mut command = Command::new("git");
    if let Some(repo) = repo {
        command.arg("-C").arg(repo);
    }
    command.args(["rev-parse", "--show-toplevel"]);

    let output = command.output()?;
    if !output.status.success() {
        return Err(git_error("failed to find git repository root", &output));
    }

    let root = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if root.is_empty() {
        return Err(HzError::Usage("git repository root was empty".to_owned()));
    }

    Ok(PathBuf::from(root))
}

pub fn add_worktree(repo: &Path, path: &Path, branch: &str, base: Option<&str>) -> HzResult<()> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(repo)
        .args(["worktree", "add", "-b", branch])
        .arg("--")
        .arg(path);

    if let Some(base) = base {
        command.arg(base);
    }

    let output = command.output()?;
    if !output.status.success() {
        return Err(git_error("failed to add git worktree", &output));
    }

    Ok(())
}

pub fn remove_worktree(repo: &Path, path: &Path) -> HzResult<()> {
    remove_worktree_with_force(repo, path, false)
}

pub fn remove_worktree_with_force(repo: &Path, path: &Path, force: bool) -> HzResult<()> {
    let mut command = Command::new("git");
    command.arg("-C").arg(repo).args(["worktree", "remove"]);
    if force {
        command.arg("--force");
    }
    command.arg("--").arg(path);

    let output = command.output()?;

    if !output.status.success() {
        return Err(git_error("failed to remove git worktree", &output));
    }

    Ok(())
}

pub fn list_worktrees(repo: &Path) -> HzResult<Vec<GitWorktree>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["worktree", "list", "--porcelain"])
        .output()?;

    if !output.status.success() {
        return Err(git_error("failed to list git worktrees", &output));
    }

    Ok(parse_worktree_list(&String::from_utf8_lossy(
        &output.stdout,
    )))
}

pub fn main_worktree(repo: &Path) -> HzResult<PathBuf> {
    list_worktrees(repo)?
        .into_iter()
        .next()
        .map(|worktree| worktree.path)
        .ok_or_else(|| HzError::Usage("git worktree list was empty".to_owned()))
}

pub fn worktree_state(path: &Path) -> HzResult<GitWorktreeState> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["status", "--porcelain", "--untracked-files=normal"])
        .output()?;

    if !output.status.success() {
        return Err(git_error("failed to read git worktree status", &output));
    }

    let status = String::from_utf8_lossy(&output.stdout);
    if status.trim().is_empty() {
        return Ok(GitWorktreeState {
            dirty: false,
            modified_at_unix: 0,
        });
    }

    Ok(GitWorktreeState {
        dirty: true,
        modified_at_unix: status_paths_modified_at(path, &status),
    })
}

pub fn current_branch(repo: &Path) -> HzResult<Option<String>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["branch", "--show-current"])
        .output()?;

    if !output.status.success() {
        return Err(git_error("failed to read current git branch", &output));
    }

    let branch = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if branch.is_empty() {
        Ok(None)
    } else {
        Ok(Some(branch))
    }
}

pub fn branch_exists(repo: &Path, branch: &str) -> HzResult<bool> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["show-ref", "--verify", "--quiet"])
        .arg(format!("refs/heads/{branch}"))
        .output()?;

    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(git_error("failed to check git branch", &output)),
    }
}

pub fn switch_branch(repo: &Path, branch: &str) -> HzResult<()> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["switch", "--"])
        .arg(branch)
        .output()?;

    if !output.status.success() {
        return Err(git_error("failed to switch git branch", &output));
    }

    Ok(())
}

pub fn switch_detached(repo: &Path) -> HzResult<()> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["switch", "--detach"])
        .output()?;

    if !output.status.success() {
        return Err(git_error("failed to detach git worktree", &output));
    }

    Ok(())
}

pub fn diff_patch(repo: &Path) -> HzResult<Vec<u8>> {
    let untracked = untracked_paths(repo)?;
    if untracked.is_empty() {
        return diff_patch_with_index(repo, None);
    }

    let index_path = git_path(repo, "index")?;
    let temp_index = temp_index_path()?;
    if index_path.exists() {
        fs::copy(&index_path, &temp_index)?;
    } else {
        fs::File::create(&temp_index)?.sync_all()?;
    }

    let result = (|| {
        let mut add = Command::new("git");
        add.arg("-C")
            .arg(repo)
            .env("GIT_INDEX_FILE", &temp_index)
            .args(["add", "-N", "--"])
            .args(&untracked);
        let output = add.output()?;
        if !output.status.success() {
            return Err(git_error(
                "failed to prepare untracked files for diff",
                &output,
            ));
        }

        diff_patch_with_index(repo, Some(&temp_index))
    })();

    let _ = fs::remove_file(&temp_index);
    result
}

pub fn apply_patch(repo: &Path, patch: &[u8]) -> HzResult<bool> {
    if patch.iter().all(|byte| byte.is_ascii_whitespace()) {
        return Ok(false);
    }

    apply_patch_command(repo, patch, true)?;
    apply_patch_command(repo, patch, false)?;
    Ok(true)
}

fn apply_patch_command(repo: &Path, patch: &[u8], check: bool) -> HzResult<()> {
    let mut command = Command::new("git");
    command.arg("-C").arg(repo).arg("apply");
    if check {
        command.arg("--check");
    }
    command.arg("--binary");

    let mut child = command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    child
        .stdin
        .as_mut()
        .ok_or_else(|| HzError::Usage("failed to open git apply stdin".to_owned()))?
        .write_all(patch)?;
    let output = child.wait_with_output()?;

    if !output.status.success() {
        return Err(git_error("failed to apply git patch", &output));
    }

    Ok(())
}

fn diff_patch_with_index(repo: &Path, index: Option<&Path>) -> HzResult<Vec<u8>> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(repo)
        .args(["diff", "--binary", "HEAD"]);
    if let Some(index) = index {
        command.env("GIT_INDEX_FILE", index);
    }
    let output = command.output()?;

    if !output.status.success() {
        return Err(git_error("failed to create git patch", &output));
    }

    Ok(output.stdout)
}

fn untracked_paths(repo: &Path) -> HzResult<Vec<PathBuf>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["ls-files", "--others", "--exclude-standard", "-z"])
        .output()?;

    if !output.status.success() {
        return Err(git_error("failed to list untracked files", &output));
    }

    Ok(output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| PathBuf::from(String::from_utf8_lossy(path).into_owned()))
        .collect())
}

fn git_path(repo: &Path, path: &str) -> HzResult<PathBuf> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--git-path", path])
        .output()?;

    if !output.status.success() {
        return Err(git_error("failed to resolve git path", &output));
    }

    let path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if path.is_empty() {
        return Err(HzError::Usage("git path was empty".to_owned()));
    }

    let path = PathBuf::from(path);
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(repo.join(path))
    }
}

fn temp_index_path() -> HzResult<PathBuf> {
    Ok(env::temp_dir().join(format!(
        "hz-git-index-{}.tmp",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| HzError::Usage(format!("system time before unix epoch: {error}")))?
            .as_nanos()
    )))
}

fn status_paths_modified_at(repo: &Path, status: &str) -> u64 {
    status
        .lines()
        .filter_map(status_path)
        .filter_map(|path| path_modified_at(&repo.join(path)))
        .max()
        .unwrap_or_else(|| path_modified_at(repo).unwrap_or(0))
}

fn status_path(line: &str) -> Option<&str> {
    let path = line.get(3..)?;
    path.rsplit_once(" -> ")
        .map_or(Some(path), |(_, renamed)| Some(renamed))
}

fn path_modified_at(path: &Path) -> Option<u64> {
    let metadata = fs::symlink_metadata(path).ok()?;
    let modified_at = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())?;

    if !metadata.is_dir() {
        return Some(modified_at);
    }

    let child_modified_at = fs::read_dir(path)
        .ok()?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| path_modified_at(&entry.path()))
        .max()
        .unwrap_or(0);

    Some(modified_at.max(child_modified_at))
}

fn parse_worktree_list(output: &str) -> Vec<GitWorktree> {
    let mut worktrees = Vec::new();
    let mut path = None;
    let mut branch = None;

    for line in output.lines() {
        if line.is_empty() {
            if let Some(path) = path.take() {
                worktrees.push(GitWorktree { path, branch });
                branch = None;
            }
            continue;
        }

        if let Some(value) = line.strip_prefix("worktree ") {
            path = Some(PathBuf::from(value));
        } else if let Some(value) = line.strip_prefix("branch refs/heads/") {
            branch = Some(value.to_owned());
        }
    }

    if let Some(path) = path {
        worktrees.push(GitWorktree { path, branch });
    }

    worktrees
}

fn git_error(context: &str, output: &std::process::Output) -> HzError {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let detail = stderr.trim();
    if detail.is_empty() {
        HzError::Usage(context.to_owned())
    } else {
        HzError::Usage(format!("{context}: {detail}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        env,
        fs::File,
        process::Command,
        thread,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn parses_porcelain_worktree_list() {
        let output = "\
worktree /repo
HEAD abc
branch refs/heads/main

worktree /repo-feature
HEAD def
branch refs/heads/feature

";

        let worktrees = parse_worktree_list(output);

        assert_eq!(
            worktrees,
            vec![
                GitWorktree {
                    path: PathBuf::from("/repo"),
                    branch: Some("main".to_owned())
                },
                GitWorktree {
                    path: PathBuf::from("/repo-feature"),
                    branch: Some("feature".to_owned())
                }
            ]
        );
    }

    #[test]
    fn status_path_reads_final_path_for_renames() {
        assert_eq!(status_path(" M src/lib.rs"), Some("src/lib.rs"));
        assert_eq!(
            status_path("R  old-name.rs -> new-name.rs"),
            Some("new-name.rs")
        );
    }

    #[test]
    fn directory_modified_at_uses_newest_descendant() {
        let test_dir = env::temp_dir().join(format!(
            "hz-git-mtime-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time should be after unix epoch")
                .as_nanos()
        ));
        let nested_dir = test_dir.join("nested");
        let nested_file = nested_dir.join("file.txt");

        fs::create_dir_all(&nested_dir).expect("test directory should be created");
        File::create(&nested_file).expect("nested file should be created");
        let shallow_directory_modified_at =
            path_modified_at(&nested_dir).expect("directory mtime should be read");
        thread::sleep(Duration::from_secs(1));
        fs::write(&nested_file, "updated").expect("nested file should be updated");

        let file_modified_at = path_modified_at(&nested_file).expect("file mtime should be read");
        let directory_modified_at =
            path_modified_at(&nested_dir).expect("directory tree mtime should be read");

        assert!(
            directory_modified_at >= file_modified_at,
            "directory tree mtime should include nested files"
        );
        assert!(
            directory_modified_at > shallow_directory_modified_at,
            "directory tree mtime should reflect edits to existing nested files"
        );

        fs::remove_dir_all(test_dir).expect("test directory should be removed");
    }

    #[test]
    fn patch_diff_includes_modified_and_untracked_files() {
        let test_dir = env::temp_dir().join(format!(
            "hz-git-patch-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time should be after unix epoch")
                .as_nanos()
        ));
        let repo = test_dir.join("repo");
        let destination = test_dir.join("destination");
        fs::create_dir_all(&test_dir).expect("test directory should be created");

        git(["init", "-q", repo.to_str().unwrap()], &test_dir);
        git(["config", "user.email", "test@example.com"], &repo);
        git(["config", "user.name", "Test"], &repo);
        fs::write(repo.join("file.txt"), "base\n").expect("tracked file should be written");
        git(["add", "file.txt"], &repo);
        git(["commit", "-q", "-m", "init"], &repo);
        git(
            [
                "worktree",
                "add",
                "-q",
                "--detach",
                destination.to_str().unwrap(),
                "HEAD",
            ],
            &repo,
        );

        fs::write(repo.join("file.txt"), "base\nchanged\n")
            .expect("tracked file should be changed");
        fs::write(repo.join("new.txt"), "new\n").expect("untracked file should be written");

        let patch = diff_patch(&repo).expect("patch should be created");
        assert!(apply_patch(&destination, &patch).expect("patch should apply"));

        assert_eq!(
            fs::read_to_string(destination.join("file.txt")).unwrap(),
            "base\nchanged\n"
        );
        assert_eq!(
            fs::read_to_string(destination.join("new.txt")).unwrap(),
            "new\n"
        );

        fs::remove_dir_all(test_dir).expect("test directory should be removed");
    }

    fn git<const N: usize>(args: [&str; N], cwd: &Path) {
        let output = Command::new("git")
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
}
