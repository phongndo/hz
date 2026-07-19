use std::{
    fs,
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
    process::{self, Command, Output, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

#[cfg(unix)]
use std::ffi::OsString;

use hz_core::{HzError, HzResult};
use hz_scm::{SourceControl, SourceStatus};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitStatus {
    pub dirty: bool,
    pub modified_at_unix: u64,
}

const WORKSPACE_MARKER: &str = ".hz-workspace";

pub fn status(path: &Path) -> HzResult<GitStatus> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["status", "--porcelain=v1", "-z", "--untracked-files=all"])
        .output()?;
    if !output.status.success() {
        return Err(git_error("failed to read Git status", &output));
    }
    let paths = status_paths(&output.stdout)
        .into_iter()
        .filter(|candidate| candidate != Path::new(WORKSPACE_MARKER))
        .collect::<Vec<_>>();
    Ok(GitStatus {
        dirty: !paths.is_empty(),
        modified_at_unix: status_paths_modified_at(path, &paths),
    })
}

pub fn current_branch(repository: &Path) -> HzResult<Option<String>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(["branch", "--show-current"])
        .output()?;
    if !output.status.success() {
        return Err(git_error("failed to read current Git branch", &output));
    }
    let branch = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    Ok((!branch.is_empty()).then_some(branch))
}

pub fn current_head(repository: &Path) -> HzResult<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(["rev-parse", "--verify", "HEAD"])
        .output()?;
    if !output.status.success() {
        return Err(git_error("failed to read current Git HEAD", &output));
    }
    let head = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if head.is_empty() {
        Err(HzError::Usage("Git HEAD was empty".to_owned()))
    } else {
        Ok(head)
    }
}

/// Build a binary patch containing tracked, staged, and untracked state.
pub fn diff_patch(repository: &Path) -> HzResult<Vec<u8>> {
    let untracked = untracked_pathspecs(repository)?;
    if untracked.is_empty() {
        return diff_patch_with_index(repository, None);
    }

    let index_path = git_path(repository, "index")?;
    let temp_index = create_temp_index(repository, &index_path)?;
    let result = (|| {
        prepare_untracked_files(repository, &temp_index, &untracked)?;
        diff_patch_with_index(repository, Some(&temp_index))
    })();
    let _ = fs::remove_file(&temp_index);
    result
}

fn prepare_untracked_files(repository: &Path, index: &Path, pathspecs: &[Vec<u8>]) -> HzResult<()> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(repository)
        .arg("--literal-pathspecs")
        .env("GIT_INDEX_FILE", index)
        .args(["add", "-N", "--pathspec-from-file=-", "--pathspec-file-nul"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| HzError::Usage("failed to open Git pathspec input".to_owned()))?;
    let write_result = (|| -> std::io::Result<()> {
        for pathspec in pathspecs {
            stdin.write_all(pathspec)?;
            stdin.write_all(&[0])?;
        }
        Ok(())
    })();
    drop(stdin);
    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Err(git_error(
            "failed to prepare untracked files for diff",
            &output,
        ));
    }
    write_result?;
    Ok(())
}

pub fn apply_patch(repository: &Path, patch: &[u8]) -> HzResult<bool> {
    if patch.iter().all(|byte| byte.is_ascii_whitespace()) {
        return Ok(false);
    }
    apply_patch_command(repository, patch, true)?;
    apply_patch_command(repository, patch, false)?;
    Ok(true)
}

fn apply_patch_command(repository: &Path, patch: &[u8], check: bool) -> HzResult<()> {
    let mut command = Command::new("git");
    command.arg("-C").arg(repository).arg("apply");
    if check {
        command.arg("--check");
    }
    command.arg("--binary");
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .as_mut()
        .ok_or_else(|| HzError::Usage("failed to open git apply stdin".to_owned()))?
        .write_all(patch)?;
    let output = child.wait_with_output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(git_error("failed to apply Git patch", &output))
    }
}

fn diff_patch_with_index(repository: &Path, index: Option<&Path>) -> HzResult<Vec<u8>> {
    let base = diff_base(repository)?;
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(repository)
        .args(["diff", "--no-color", "--binary"])
        .arg(&base)
        .args(["--", ":(exclude,top).hz-workspace"]);
    if let Some(index) = index {
        command.env("GIT_INDEX_FILE", index);
    }
    let output = command.output()?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(git_error("failed to create Git patch", &output))
    }
}

fn diff_base(repository: &Path) -> HzResult<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(["rev-parse", "--verify", "--quiet", "HEAD"])
        .output()?;
    match output.status.code() {
        Some(0) => Ok("HEAD".to_owned()),
        Some(1) => empty_tree(repository),
        _ => Err(git_error("failed to resolve Git HEAD", &output)),
    }
}

fn empty_tree(repository: &Path) -> HzResult<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(["hash-object", "-t", "tree", "--stdin"])
        .stdin(Stdio::null())
        .output()?;
    if !output.status.success() {
        return Err(git_error("failed to resolve Git's empty tree", &output));
    }
    let object = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if object.is_empty() {
        Err(HzError::Usage("Git's empty tree ID was empty".to_owned()))
    } else {
        Ok(object)
    }
}

fn untracked_pathspecs(repository: &Path) -> HzResult<Vec<Vec<u8>>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(["ls-files", "--others", "--exclude-standard", "-z"])
        .output()?;
    if output.status.success() {
        Ok(parse_nul_paths(&output.stdout)
            .into_iter()
            .filter(|path| path.as_slice() != WORKSPACE_MARKER.as_bytes())
            .collect())
    } else {
        Err(git_error("failed to list untracked files", &output))
    }
}

fn git_path(repository: &Path, path: &str) -> HzResult<PathBuf> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(["rev-parse", "--git-path", path])
        .output()?;
    if !output.status.success() {
        return Err(git_error("failed to resolve Git path", &output));
    }
    let path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if path.is_empty() {
        return Err(HzError::Usage("Git path was empty".to_owned()));
    }
    let path = PathBuf::from(path);
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(repository.join(path))
    }
}

fn create_temp_index(repository: &Path, index_path: &Path) -> HzResult<PathBuf> {
    for attempt in 0..16 {
        let temp_path = temp_index_path(index_path, attempt)?;
        let mut temp_file = match create_private_temp_file(&temp_path) {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        };
        let result = if index_path.exists() {
            initialize_temp_index(index_path, &mut temp_file)
        } else {
            drop(temp_file);
            initialize_empty_temp_index(repository, index_path, &temp_path)
        };
        if let Err(error) = result {
            let _ = fs::remove_file(&temp_path);
            return Err(error);
        }
        return Ok(temp_path);
    }
    Err(HzError::Usage(
        "failed to create a unique temporary Git index".to_owned(),
    ))
}

fn initialize_temp_index(index_path: &Path, temp_file: &mut fs::File) -> HzResult<()> {
    let mut source = fs::File::open(index_path)?;
    std::io::copy(&mut source, temp_file)?;
    temp_file.sync_all()?;
    Ok(())
}

fn initialize_empty_temp_index(
    repository: &Path,
    index_path: &Path,
    temp_path: &Path,
) -> HzResult<()> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repository)
        .env("GIT_INDEX_FILE", index_path)
        .args(["read-tree", "--empty", "--index-output"])
        .arg(temp_path)
        .output()?;
    if !output.status.success() {
        return Err(git_error(
            "failed to initialize a temporary Git index",
            &output,
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(temp_path, fs::Permissions::from_mode(0o600))?;
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
            "Git index path has no parent: {}",
            index_path.display()
        ))
    })?;
    Ok(parent.join(format!(
        ".hz-git-index-{}-{}-{attempt}.tmp",
        process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| HzError::Usage(format!("system time before Unix epoch: {error}")))?
            .as_nanos(),
    )))
}

fn parse_nul_paths(output: &[u8]) -> Vec<Vec<u8>> {
    output
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(<[u8]>::to_vec)
        .collect()
}

fn status_paths_modified_at(repository: &Path, paths: &[PathBuf]) -> u64 {
    if paths.is_empty() {
        return 0;
    }
    paths
        .iter()
        .filter_map(|path| path_modified_at(&repository.join(path)))
        .max()
        .unwrap_or_else(|| path_modified_at(repository).unwrap_or(0))
}

fn status_paths(porcelain: &[u8]) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let mut fields = porcelain
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty());
    while let Some(field) = fields.next() {
        if field.len() < 4 || field[2] != b' ' {
            continue;
        }
        let state = &field[..2];
        paths.push(path_from_git_bytes(&field[3..]));
        if state.iter().any(|byte| matches!(byte, b'R' | b'C')) {
            let _ = fields.next();
        }
    }
    paths
}

fn path_modified_at(path: &Path) -> Option<u64> {
    fs::symlink_metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
}

#[cfg(unix)]
fn path_from_git_bytes(path: &[u8]) -> PathBuf {
    use std::os::unix::ffi::OsStringExt;
    PathBuf::from(OsString::from_vec(path.to_vec()))
}

#[cfg(not(unix))]
fn path_from_git_bytes(path: &[u8]) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(path).into_owned())
}

fn git_error(context: &str, output: &Output) -> HzError {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if stderr.is_empty() {
        HzError::Usage(format!("{context}: Git exited with {}", output.status))
    } else {
        HzError::Usage(format!("{context}: {stderr}"))
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct GitSourceControl;

impl SourceControl for GitSourceControl {
    fn kind(&self) -> &'static str {
        "git"
    }

    fn status(&self, workspace: &Path) -> HzResult<SourceStatus> {
        if !workspace.join(".git").exists() {
            return Ok(SourceStatus::Unknown);
        }
        Ok(if status(workspace)?.dirty {
            SourceStatus::Dirty
        } else {
            SourceStatus::Clean
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn git<const N: usize>(args: [&str; N], cwd: &Path) {
        let output = Command::new("git")
            .current_dir(cwd)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn repository() -> (TempDir, PathBuf) {
        let temp = TempDir::new().unwrap();
        let repo = temp.path().join("repo");
        git(["init", "-q", repo.to_str().unwrap()], temp.path());
        git(["config", "user.email", "test@example.com"], &repo);
        git(["config", "user.name", "Test"], &repo);
        fs::write(repo.join("file.txt"), "base\n").unwrap();
        git(["add", "file.txt"], &repo);
        git(["commit", "-q", "-m", "init"], &repo);
        (temp, repo)
    }

    #[test]
    fn status_and_patches_ignore_untracked_and_tracked_workspace_markers() {
        let (_temp, repo) = repository();
        fs::write(repo.join(WORKSPACE_MARKER), "workspace-id\n").unwrap();

        assert!(!status(&repo).unwrap().dirty);
        assert!(diff_patch(&repo).unwrap().is_empty());

        git(["add", WORKSPACE_MARKER], &repo);
        git(["commit", "-q", "-m", "track marker"], &repo);
        fs::write(repo.join(WORKSPACE_MARKER), "child-workspace-id\n").unwrap();
        git(["add", WORKSPACE_MARKER], &repo);

        assert!(!status(&repo).unwrap().dirty);
        assert!(diff_patch(&repo).unwrap().is_empty());
    }

    #[test]
    fn status_preserves_untracked_and_modified_paths() {
        let (_temp, repo) = repository();
        fs::write(repo.join("file.txt"), "changed\n").unwrap();
        fs::write(repo.join("new.txt"), "new\n").unwrap();
        assert!(status(&repo).unwrap().dirty);
    }

    #[test]
    fn patch_contains_tracked_and_untracked_state() {
        let (temp, repo) = repository();
        let destination = temp.path().join("destination");
        git(
            [
                "clone",
                "-q",
                "--no-hardlinks",
                repo.to_str().unwrap(),
                destination.to_str().unwrap(),
            ],
            temp.path(),
        );
        fs::write(repo.join("file.txt"), "changed\n").unwrap();
        fs::write(repo.join("new.txt"), "new\n").unwrap();
        let patch = diff_patch(&repo).unwrap();
        assert!(apply_patch(&destination, &patch).unwrap());
        assert_eq!(
            fs::read_to_string(destination.join("file.txt")).unwrap(),
            "changed\n"
        );
        assert_eq!(
            fs::read_to_string(destination.join("new.txt")).unwrap(),
            "new\n"
        );
    }

    #[test]
    fn patch_supports_an_unborn_head() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        git(["init", "-q", source.to_str().unwrap()], temp.path());
        git(["init", "-q", destination.to_str().unwrap()], temp.path());
        fs::write(source.join("staged.txt"), "staged\n").unwrap();
        git(["add", "staged.txt"], &source);
        fs::write(source.join("untracked.txt"), "untracked\n").unwrap();

        let patch = diff_patch(&source).unwrap();
        assert!(apply_patch(&destination, &patch).unwrap());
        assert_eq!(
            fs::read_to_string(destination.join("staged.txt")).unwrap(),
            "staged\n"
        );
        assert_eq!(
            fs::read_to_string(destination.join("untracked.txt")).unwrap(),
            "untracked\n"
        );
    }

    #[test]
    fn patch_ignores_forced_git_color_configuration() {
        let (temp, repo) = repository();
        let destination = temp.path().join("destination");
        git(
            [
                "clone",
                "-q",
                "--no-hardlinks",
                repo.to_str().unwrap(),
                destination.to_str().unwrap(),
            ],
            temp.path(),
        );
        git(["config", "color.ui", "always"], &repo);
        fs::write(repo.join("file.txt"), "changed\n").unwrap();

        let patch = diff_patch(&repo).unwrap();
        assert!(!patch.contains(&0x1b));
        assert!(apply_patch(&destination, &patch).unwrap());
    }

    #[test]
    fn status_parser_preserves_newlines() {
        assert_eq!(
            status_paths(b" M line\nbreak.txt\0"),
            vec![PathBuf::from("line\nbreak.txt")]
        );
    }

    #[cfg(unix)]
    #[test]
    fn nul_path_parser_preserves_non_utf8_paths() {
        assert_eq!(
            parse_nul_paths(b"invalid-\xff.txt\0"),
            vec![b"invalid-\xff.txt".to_vec()]
        );
    }
}
