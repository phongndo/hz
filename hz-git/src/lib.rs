use std::{
    ffi::OsString,
    fs,
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
    process::{self, Command},
    time::{SystemTime, UNIX_EPOCH},
};

use hz_core::{HzError, HzResult};

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

pub fn add_worktree(
    repo: &Path,
    path: &Path,
    branch: Option<&str>,
    base: Option<&str>,
) -> HzResult<()> {
    let mut command = Command::new("git");
    command.arg("-C").arg(repo).args(["worktree", "add"]);
    if let Some(branch) = branch {
        command.args(["-b", branch]);
    } else {
        command.arg("--detach");
    }
    command.arg("--").arg(path);

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
    let output = worktree_list_output(repo)?;

    Ok(parse_worktree_list(&output.stdout))
}

pub fn main_worktree(repo: &Path) -> HzResult<PathBuf> {
    let output = worktree_list_output(repo)?;
    parse_main_worktree_path(&output.stdout).ok_or_else(|| empty_worktree_list_error(repo))
}

fn worktree_list_output(repo: &Path) -> HzResult<process::Output> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["worktree", "list", "--porcelain", "-z"])
        .output()?;

    if !output.status.success() {
        return Err(git_error("failed to list git worktrees", &output));
    }

    Ok(output)
}

pub fn worktree_state(path: &Path) -> HzResult<GitWorktreeState> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["status", "--porcelain=v1", "-z", "--untracked-files=all"])
        .output()?;

    if !output.status.success() {
        return Err(git_error("failed to read git worktree status", &output));
    }

    if output.stdout.is_empty() {
        return Ok(GitWorktreeState {
            dirty: false,
            modified_at_unix: 0,
        });
    }

    Ok(GitWorktreeState {
        dirty: true,
        modified_at_unix: status_paths_modified_at(path, &output.stdout),
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

pub fn current_head(repo: &Path) -> HzResult<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--verify", "HEAD"])
        .output()?;

    if !output.status.success() {
        return Err(git_error("failed to read current git HEAD", &output));
    }

    let head = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if head.is_empty() {
        return Err(HzError::Usage("git HEAD was empty".to_owned()));
    }

    Ok(head)
}

pub fn remote_url(repo: &Path, remote: &str) -> HzResult<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["remote", "get-url", remote])
        .output()?;

    if !output.status.success() {
        return Err(git_error("failed to read git remote URL", &output));
    }

    let url = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if url.is_empty() {
        return Err(HzError::Usage(format!("git remote {remote} URL was empty")));
    }

    Ok(url)
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

pub fn delete_branch(repo: &Path, branch: &str) -> HzResult<()> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["branch", "-D", "--"])
        .arg(branch)
        .output()?;

    if !output.status.success() {
        return Err(git_error("failed to delete git branch", &output));
    }

    Ok(())
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

pub fn switch_detached_at(repo: &Path, rev: &str) -> HzResult<()> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["switch", "--detach"])
        .arg("--")
        .arg(rev)
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
    let temp_index = create_temp_index(&index_path)?;

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

    apply_patch_command(repo, patch, true, false)?;
    apply_patch_command(repo, patch, false, false)?;
    Ok(true)
}

pub fn apply_patch_reverse(repo: &Path, patch: &[u8]) -> HzResult<bool> {
    if patch.iter().all(|byte| byte.is_ascii_whitespace()) {
        return Ok(false);
    }

    apply_patch_command(repo, patch, true, true)?;
    apply_patch_command(repo, patch, false, true)?;
    Ok(true)
}

pub fn hash_bytes(repo: &Path, bytes: &[u8]) -> HzResult<String> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["hash-object", "--stdin"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    child
        .stdin
        .as_mut()
        .ok_or_else(|| HzError::Usage("failed to open git hash-object stdin".to_owned()))?
        .write_all(bytes)?;
    let output = child.wait_with_output()?;

    if !output.status.success() {
        return Err(git_error("failed to hash bytes", &output));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn apply_patch_command(repo: &Path, patch: &[u8], check: bool, reverse: bool) -> HzResult<()> {
    let mut command = Command::new("git");
    command.arg("-C").arg(repo).arg("apply");
    if check {
        command.arg("--check");
    }
    if reverse {
        command.arg("--reverse");
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

    Ok(parse_untracked_paths(&output.stdout))
}

pub fn ignored_paths_matching(repo: &Path, pathspecs: &[String]) -> HzResult<Vec<PathBuf>> {
    if pathspecs.is_empty() {
        return Ok(Vec::new());
    }

    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args([
            "ls-files",
            "--others",
            "--ignored",
            "--exclude-standard",
            "-z",
            "--",
        ])
        .args(pathspecs)
        .output()?;

    if !output.status.success() {
        return Err(git_error("failed to list ignored files", &output));
    }

    Ok(parse_untracked_paths(&output.stdout))
}

pub fn git_path(repo: &Path, path: &str) -> HzResult<PathBuf> {
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

fn create_temp_index(index_path: &Path) -> HzResult<PathBuf> {
    for attempt in 0..16 {
        let temp_path = temp_index_path(index_path, attempt)?;
        let mut temp_file = match create_private_temp_file(&temp_path) {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        };

        initialize_temp_index(index_path, &temp_path, &mut temp_file)?;
        return Ok(temp_path);
    }

    Err(HzError::Usage(
        "failed to create a unique temporary git index".to_owned(),
    ))
}

fn initialize_temp_index(
    index_path: &Path,
    temp_path: &Path,
    temp_file: &mut fs::File,
) -> HzResult<()> {
    let copy_result = (|| -> HzResult<()> {
        if index_path.exists() {
            let mut index_file = fs::File::open(index_path)?;
            std::io::copy(&mut index_file, temp_file)?;
        }
        temp_file.sync_all()?;
        Ok(())
    })();
    if let Err(error) = copy_result {
        let _ = fs::remove_file(temp_path);
        return Err(error);
    }
    Ok(())
}

fn create_private_temp_file(path: &Path) -> std::io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn temp_index_path(index_path: &Path, attempt: u32) -> HzResult<PathBuf> {
    let parent = index_path.parent().ok_or_else(|| {
        HzError::Usage(format!(
            "git index path has no parent: {}",
            index_path.display()
        ))
    })?;
    Ok(parent.join(format!(
        ".hz-git-index-{}-{}-{}.tmp",
        process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| HzError::Usage(format!("system time before unix epoch: {error}")))?
            .as_nanos(),
        attempt
    )))
}

fn parse_untracked_paths(output: &[u8]) -> Vec<PathBuf> {
    output
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(path_from_git_bytes)
        .collect()
}

fn status_paths_modified_at(repo: &Path, status: &[u8]) -> u64 {
    status_paths(status)
        .into_iter()
        .filter_map(|path| path_modified_at(&repo.join(path)))
        .max()
        .unwrap_or_else(|| path_modified_at(repo).unwrap_or(0))
}

fn status_paths(status: &[u8]) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let mut fields = status
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty());

    while let Some(field) = fields.next() {
        if field.len() < 4 || field[2] != b' ' {
            continue;
        }

        let status = &field[..2];
        paths.push(path_from_git_bytes(&field[3..]));

        if status.iter().any(|byte| matches!(byte, b'R' | b'C')) {
            let _ = fields.next();
        }
    }

    paths
}

#[cfg(unix)]
fn path_from_git_bytes(bytes: &[u8]) -> PathBuf {
    use std::os::unix::ffi::OsStringExt;

    PathBuf::from(OsString::from_vec(bytes.to_vec()))
}

#[cfg(not(unix))]
fn path_from_git_bytes(bytes: &[u8]) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
}

fn path_modified_at(path: &Path) -> Option<u64> {
    let metadata = fs::symlink_metadata(path).ok()?;
    metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
}

fn parse_worktree_list(output: &[u8]) -> Vec<GitWorktree> {
    let mut worktrees = Vec::new();
    let mut path = None;
    let mut branch = None;

    for field in output.split(|byte| *byte == 0) {
        if field.is_empty() {
            if let Some(path) = path.take() {
                worktrees.push(GitWorktree { path, branch });
                branch = None;
            }
            continue;
        }

        if let Some(value) = field.strip_prefix(b"worktree ") {
            path = Some(path_from_git_bytes(value));
        } else if let Some(value) = field.strip_prefix(b"branch refs/heads/") {
            branch = Some(String::from_utf8_lossy(value).into_owned());
        }
    }

    if let Some(path) = path {
        worktrees.push(GitWorktree { path, branch });
    }

    worktrees
}

fn parse_main_worktree_path(output: &[u8]) -> Option<PathBuf> {
    output
        .split(|byte| *byte == 0)
        .find_map(|field| field.strip_prefix(b"worktree ").map(path_from_git_bytes))
}

fn empty_worktree_list_error(repo: &Path) -> HzError {
    HzError::Usage(format!(
        "git worktree list returned no entries for {}; unexpected repository state",
        repo.display()
    ))
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
        process::Command,
        time::{SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn parses_porcelain_worktree_list() {
        let output = b"worktree /repo\0HEAD abc\0branch refs/heads/main\0\0worktree /repo-feature\0HEAD def\0branch refs/heads/feature\0\0";

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
    fn parses_main_worktree_path_from_porcelain_list() {
        let output = b"worktree /repo\0HEAD abc\0branch refs/heads/main\0\0worktree /repo-feature\0HEAD def\0branch refs/heads/feature\0\0";

        assert_eq!(
            parse_main_worktree_path(output),
            Some(PathBuf::from("/repo"))
        );
        assert_eq!(parse_main_worktree_path(b""), None);
    }

    #[test]
    fn status_paths_read_nul_porcelain_records() {
        assert_eq!(
            status_paths(b" M src/lib.rs\0?? nested/file.txt\0R  new-name.rs\0old-name.rs\0"),
            vec![
                PathBuf::from("src/lib.rs"),
                PathBuf::from("nested/file.txt"),
                PathBuf::from("new-name.rs")
            ]
        );
    }

    #[test]
    fn status_paths_preserve_newlines_in_paths() {
        assert_eq!(
            status_paths(b" M line\nbreak.txt\0"),
            vec![PathBuf::from("line\nbreak.txt")]
        );
    }

    #[test]
    fn untracked_paths_read_nul_records() {
        assert_eq!(
            parse_untracked_paths(b"line\nbreak.txt\0nested/file.txt\0"),
            vec![
                PathBuf::from("line\nbreak.txt"),
                PathBuf::from("nested/file.txt")
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn untracked_paths_preserve_non_utf8_paths() {
        use std::os::unix::ffi::OsStringExt;

        assert_eq!(
            parse_untracked_paths(b"invalid-\xff.txt\0"),
            vec![PathBuf::from(OsString::from_vec(
                b"invalid-\xff.txt".to_vec()
            ))]
        );
    }

    #[test]
    fn temp_index_is_removed_when_initialization_fails() {
        let test_dir = env::temp_dir().join(format!(
            "hz-git-temp-index-cleanup-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time should be after unix epoch")
                .as_nanos()
        ));
        let index_path = test_dir.join("index-directory");
        let temp_path = test_dir.join("temp-index");
        fs::create_dir_all(&index_path).expect("index directory should be created");
        let mut temp_file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .expect("temp index should be created");

        let result = initialize_temp_index(&index_path, &temp_path, &mut temp_file);

        assert!(result.is_err());
        assert!(!temp_path.exists());
        fs::remove_dir_all(test_dir).expect("test directory should be removed");
    }

    #[test]
    fn temp_index_paths_are_adjacent_to_source_index() {
        let index = PathBuf::from("/repo/.git/worktrees/feature/index");
        let temp = temp_index_path(&index, 0).expect("temp index path should resolve");

        assert_eq!(temp.parent(), index.parent());
        assert!(
            temp.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".hz-git-index-")
        );
    }

    #[test]
    fn worktree_state_reads_concrete_untracked_files() {
        let test_dir = env::temp_dir().join(format!(
            "hz-git-status-untracked-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time should be after unix epoch")
                .as_nanos()
        ));
        let repo = test_dir.join("repo");
        let nested_file = repo.join("nested").join("file.txt");
        fs::create_dir_all(nested_file.parent().unwrap())
            .expect("test directory should be created");

        git(["init", "-q", repo.to_str().unwrap()], &test_dir);
        fs::write(&nested_file, "untracked\n").expect("untracked file should be written");

        let state = worktree_state(&repo).expect("worktree state should be read");

        assert!(state.dirty);
        assert_eq!(
            state.modified_at_unix,
            path_modified_at(&nested_file).expect("file mtime should be read")
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

    #[test]
    fn reverse_patch_restores_worktree() {
        let test_dir = env::temp_dir().join(format!(
            "hz-git-reverse-patch-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time should be after unix epoch")
                .as_nanos()
        ));
        let repo = test_dir.join("repo");
        fs::create_dir_all(&test_dir).expect("test directory should be created");

        git(["init", "-q", repo.to_str().unwrap()], &test_dir);
        git(["config", "user.email", "test@example.com"], &repo);
        git(["config", "user.name", "Test"], &repo);
        fs::write(repo.join("file.txt"), "base\n").expect("tracked file should be written");
        git(["add", "file.txt"], &repo);
        git(["commit", "-q", "-m", "init"], &repo);

        fs::write(repo.join("file.txt"), "base\nchanged\n")
            .expect("tracked file should be changed");
        let patch = diff_patch(&repo).expect("patch should be created");
        assert!(apply_patch_reverse(&repo, &patch).expect("patch should reverse"));

        assert_eq!(fs::read_to_string(repo.join("file.txt")).unwrap(), "base\n");
        assert!(!worktree_state(&repo).unwrap().dirty);

        fs::remove_dir_all(test_dir).expect("test directory should be removed");
    }

    #[test]
    fn hash_bytes_changes_when_input_changes() {
        let test_dir = env::temp_dir().join(format!(
            "hz-git-hash-bytes-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time should be after unix epoch")
                .as_nanos()
        ));
        let repo = test_dir.join("repo");
        fs::create_dir_all(&test_dir).expect("test directory should be created");

        git(["init", "-q", repo.to_str().unwrap()], &test_dir);

        assert_ne!(
            hash_bytes(&repo, b"one").unwrap(),
            hash_bytes(&repo, b"two").unwrap()
        );

        fs::remove_dir_all(test_dir).expect("test directory should be removed");
    }

    #[test]
    fn add_worktree_without_branch_creates_detached_worktree() {
        let test_dir = env::temp_dir().join(format!(
            "hz-git-detached-worktree-test-{}",
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

        add_worktree(&repo, &destination, None, None).expect("detached worktree should be added");

        assert_eq!(current_branch(&destination).unwrap(), None);
        let destination = fs::canonicalize(&destination).unwrap();
        assert!(list_worktrees(&repo).unwrap().into_iter().any(|worktree| {
            fs::canonicalize(worktree.path).unwrap() == destination && worktree.branch.is_none()
        }));

        fs::remove_dir_all(test_dir).expect("test directory should be removed");
    }

    #[test]
    fn switch_detached_at_restores_specific_head() {
        let test_dir = env::temp_dir().join(format!(
            "hz-git-detached-head-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time should be after unix epoch")
                .as_nanos()
        ));
        let repo = test_dir.join("repo");
        fs::create_dir_all(&test_dir).expect("test directory should be created");

        git(["init", "-q", repo.to_str().unwrap()], &test_dir);
        git(["config", "user.email", "test@example.com"], &repo);
        git(["config", "user.name", "Test"], &repo);
        fs::write(repo.join("file.txt"), "base\n").expect("tracked file should be written");
        git(["add", "file.txt"], &repo);
        git(["commit", "-q", "-m", "init"], &repo);
        let first = current_head(&repo).expect("first HEAD should be read");

        fs::write(repo.join("file.txt"), "base\nchanged\n")
            .expect("tracked file should be changed");
        git(["commit", "-q", "-am", "change"], &repo);

        switch_detached_at(&repo, &first).expect("worktree should detach at first commit");

        assert_eq!(current_branch(&repo).unwrap(), None);
        assert_eq!(current_head(&repo).unwrap(), first);

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
