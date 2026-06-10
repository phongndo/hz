use std::{
    ffi::OsString,
    fs,
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
    process::{self, Command},
    time::{SystemTime, UNIX_EPOCH},
};

use hz_core::{HzError, HzResult};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceControl {
    Git,
    Jj,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorktreeLabelKind {
    Branch,
    WorkspaceName,
}

impl SourceControl {
    pub fn supports_branch_handoff(self) -> bool {
        matches!(self, Self::Git)
    }

    pub fn worktree_label_kind(self) -> WorktreeLabelKind {
        match self {
            Self::Git => WorktreeLabelKind::Branch,
            Self::Jj => WorktreeLabelKind::WorkspaceName,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceWorktree {
    pub path: PathBuf,
    pub branch: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceWorktreeState {
    pub dirty: bool,
    pub modified_at_unix: u64,
}

pub type GitWorktree = SourceWorktree;
pub type GitWorktreeState = SourceWorktreeState;

trait SourceControlBackend: Sync {
    fn kind(&self) -> SourceControl;
    fn repository_root(&self, repo: Option<&Path>) -> HzResult<PathBuf>;
    fn add_worktree(
        &self,
        repo: &Path,
        path: &Path,
        branch: Option<&str>,
        base: Option<&str>,
    ) -> HzResult<()>;
    fn remove_worktree_with_force(&self, repo: &Path, path: &Path, force: bool) -> HzResult<()>;
    fn list_worktrees(&self, repo: &Path) -> HzResult<Vec<GitWorktree>>;
    fn main_worktree(&self, repo: &Path) -> HzResult<PathBuf>;
    fn worktree_state(&self, path: &Path) -> HzResult<GitWorktreeState>;
    fn current_branch(&self, repo: &Path) -> HzResult<Option<String>>;
    fn current_head(&self, repo: &Path) -> HzResult<String>;
    fn remote_url(&self, repo: &Path, remote: &str) -> HzResult<String>;
    fn branch_exists(&self, repo: &Path, branch: &str) -> HzResult<bool>;
    fn delete_branch(&self, repo: &Path, branch: &str) -> HzResult<()>;
    fn switch_branch(&self, repo: &Path, branch: &str) -> HzResult<()>;
    fn switch_detached(&self, repo: &Path) -> HzResult<()>;
    fn switch_detached_at(&self, repo: &Path, rev: &str) -> HzResult<()>;
    fn diff_patch(&self, repo: &Path) -> HzResult<Vec<u8>>;
    fn apply_patch_command(
        &self,
        repo: &Path,
        patch: &[u8],
        check: bool,
        reverse: bool,
    ) -> HzResult<()>;
    fn hash_bytes(&self, repo: &Path, bytes: &[u8]) -> HzResult<String>;
    fn scm_path(&self, repo: &Path, path: &str) -> HzResult<PathBuf>;
}

// Source-control-specific behavior lives behind this backend table so adding a
// new system is localized to a backend implementation and registration here.
struct GitBackend;
struct JjBackend;

static GIT_BACKEND: GitBackend = GitBackend;
static JJ_BACKEND: JjBackend = JjBackend;

struct BackendRoot {
    backend: &'static dyn SourceControlBackend,
    root: PathBuf,
}

fn backend_for_hint(repo: Option<&Path>) -> HzResult<&'static dyn SourceControlBackend> {
    Ok(backend_root_for_hint(repo)?
        .map(|candidate| candidate.backend)
        .unwrap_or(&GIT_BACKEND))
}

fn backend_for_path(path: &Path) -> HzResult<&'static dyn SourceControlBackend> {
    backend_for_hint(Some(path))
}

fn backend_root_for_hint(repo: Option<&Path>) -> HzResult<Option<BackendRoot>> {
    let jj_root = jj_repository_root(repo)?;
    let git_root = git_repository_root_if_present(repo)?;

    Ok(nearest_backend_root(jj_root, git_root))
}

fn nearest_backend_root(
    jj_root: Option<PathBuf>,
    git_root: Option<PathBuf>,
) -> Option<BackendRoot> {
    match (jj_root, git_root) {
        (Some(jj_root), Some(git_root)) => {
            if same_path(&jj_root, &git_root) || path_depth(&jj_root) > path_depth(&git_root) {
                Some(BackendRoot {
                    backend: &JJ_BACKEND,
                    root: jj_root,
                })
            } else {
                Some(BackendRoot {
                    backend: &GIT_BACKEND,
                    root: git_root,
                })
            }
        }
        (Some(root), None) => Some(BackendRoot {
            backend: &JJ_BACKEND,
            root,
        }),
        (None, Some(root)) => Some(BackendRoot {
            backend: &GIT_BACKEND,
            root,
        }),
        (None, None) => None,
    }
}

fn path_depth(path: &Path) -> usize {
    fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .components()
        .count()
}

impl SourceControlBackend for GitBackend {
    fn kind(&self) -> SourceControl {
        SourceControl::Git
    }

    fn repository_root(&self, repo: Option<&Path>) -> HzResult<PathBuf> {
        git_repository_root(repo)
    }

    fn add_worktree(
        &self,
        repo: &Path,
        path: &Path,
        branch: Option<&str>,
        base: Option<&str>,
    ) -> HzResult<()> {
        git_add_worktree(repo, path, branch, base)
    }

    fn remove_worktree_with_force(&self, repo: &Path, path: &Path, force: bool) -> HzResult<()> {
        git_remove_worktree_with_force(repo, path, force)
    }

    fn list_worktrees(&self, repo: &Path) -> HzResult<Vec<GitWorktree>> {
        git_list_worktrees(repo)
    }

    fn main_worktree(&self, repo: &Path) -> HzResult<PathBuf> {
        git_main_worktree(repo)
    }

    fn worktree_state(&self, path: &Path) -> HzResult<GitWorktreeState> {
        git_worktree_state(path)
    }

    fn current_branch(&self, repo: &Path) -> HzResult<Option<String>> {
        git_current_branch(repo)
    }

    fn current_head(&self, repo: &Path) -> HzResult<String> {
        git_current_head(repo)
    }

    fn remote_url(&self, repo: &Path, remote: &str) -> HzResult<String> {
        git_remote_url(repo, remote)
    }

    fn branch_exists(&self, repo: &Path, branch: &str) -> HzResult<bool> {
        git_branch_exists(repo, branch)
    }

    fn delete_branch(&self, repo: &Path, branch: &str) -> HzResult<()> {
        git_delete_branch(repo, branch)
    }

    fn switch_branch(&self, repo: &Path, branch: &str) -> HzResult<()> {
        git_switch_branch(repo, branch)
    }

    fn switch_detached(&self, repo: &Path) -> HzResult<()> {
        git_switch_detached(repo)
    }

    fn switch_detached_at(&self, repo: &Path, rev: &str) -> HzResult<()> {
        git_switch_detached_at(repo, rev)
    }

    fn diff_patch(&self, repo: &Path) -> HzResult<Vec<u8>> {
        git_diff_patch(repo)
    }

    fn apply_patch_command(
        &self,
        repo: &Path,
        patch: &[u8],
        check: bool,
        reverse: bool,
    ) -> HzResult<()> {
        git_apply_patch_command(repo, patch, check, reverse, GitApplyTarget::Repository)
    }

    fn hash_bytes(&self, repo: &Path, bytes: &[u8]) -> HzResult<String> {
        git_hash_bytes(repo, bytes)
    }

    fn scm_path(&self, repo: &Path, path: &str) -> HzResult<PathBuf> {
        git_scm_path(repo, path)
    }
}

impl SourceControlBackend for JjBackend {
    fn kind(&self) -> SourceControl {
        SourceControl::Jj
    }

    fn repository_root(&self, repo: Option<&Path>) -> HzResult<PathBuf> {
        jj_repository_root(repo)?
            .ok_or_else(|| HzError::Usage("failed to find jj repository root".to_owned()))
    }

    fn add_worktree(
        &self,
        repo: &Path,
        path: &Path,
        branch: Option<&str>,
        base: Option<&str>,
    ) -> HzResult<()> {
        add_jj_workspace(repo, path, branch, base)
    }

    fn remove_worktree_with_force(&self, repo: &Path, path: &Path, force: bool) -> HzResult<()> {
        remove_jj_workspace_with_force(repo, path, force)
    }

    fn list_worktrees(&self, repo: &Path) -> HzResult<Vec<GitWorktree>> {
        list_jj_workspaces(repo)
    }

    fn main_worktree(&self, repo: &Path) -> HzResult<PathBuf> {
        self.repository_root(Some(repo))
    }

    fn worktree_state(&self, path: &Path) -> HzResult<GitWorktreeState> {
        jj_worktree_state(path)
    }

    fn current_branch(&self, repo: &Path) -> HzResult<Option<String>> {
        current_jj_bookmark(repo)
    }

    fn current_head(&self, repo: &Path) -> HzResult<String> {
        current_jj_commit(repo)
    }

    fn remote_url(&self, repo: &Path, remote: &str) -> HzResult<String> {
        git_remote_url(repo, remote)
    }

    fn branch_exists(&self, repo: &Path, branch: &str) -> HzResult<bool> {
        jj_bookmark_exists(repo, branch)
    }

    fn delete_branch(&self, repo: &Path, branch: &str) -> HzResult<()> {
        delete_jj_bookmark(repo, branch)
    }

    fn switch_branch(&self, _repo: &Path, _branch: &str) -> HzResult<()> {
        Err(jj_branch_handoff_error())
    }

    fn switch_detached(&self, _repo: &Path) -> HzResult<()> {
        Err(jj_branch_handoff_error())
    }

    fn switch_detached_at(&self, _repo: &Path, _rev: &str) -> HzResult<()> {
        Err(jj_branch_handoff_error())
    }

    fn diff_patch(&self, repo: &Path) -> HzResult<Vec<u8>> {
        jj_diff_patch(repo)
    }

    fn apply_patch_command(
        &self,
        repo: &Path,
        patch: &[u8],
        check: bool,
        reverse: bool,
    ) -> HzResult<()> {
        git_apply_patch_command(
            repo,
            patch,
            check,
            reverse,
            GitApplyTarget::CurrentDirectory,
        )
    }

    fn hash_bytes(&self, _repo: &Path, bytes: &[u8]) -> HzResult<String> {
        Ok(sha256_hex(bytes))
    }

    fn scm_path(&self, repo: &Path, path: &str) -> HzResult<PathBuf> {
        Ok(repo.join(".jj").join(path))
    }
}

pub fn repository_root(repo: Option<&Path>) -> HzResult<PathBuf> {
    if let Some(candidate) = backend_root_for_hint(repo)? {
        Ok(candidate.root)
    } else {
        git_repository_root(repo)
    }
}

pub fn source_control(repo: &Path) -> HzResult<SourceControl> {
    Ok(backend_for_path(repo)?.kind())
}

fn git_repository_root(repo: Option<&Path>) -> HzResult<PathBuf> {
    let output = git_repository_root_output(repo)?;
    if !output.status.success() {
        return Err(git_error("failed to find git repository root", &output));
    }

    parse_git_repository_root(&output.stdout)
}

fn git_repository_root_if_present(repo: Option<&Path>) -> HzResult<Option<PathBuf>> {
    let output = match git_repository_root_output(repo) {
        Ok(output) => output,
        Err(HzError::Io(error)) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };

    if output.status.success() {
        Ok(Some(parse_git_repository_root(&output.stdout)?))
    } else {
        Ok(None)
    }
}

fn git_repository_root_output(repo: Option<&Path>) -> HzResult<process::Output> {
    let mut command = Command::new("git");
    if let Some(repo) = repo {
        command.arg("-C").arg(repo);
    }
    command.args(["rev-parse", "--show-toplevel"]);

    Ok(command.output()?)
}

fn parse_git_repository_root(stdout: &[u8]) -> HzResult<PathBuf> {
    let root = String::from_utf8_lossy(stdout).trim().to_owned();
    if root.is_empty() {
        return Err(HzError::Usage("git repository root was empty".to_owned()));
    }

    Ok(PathBuf::from(root))
}

fn jj_repository_root(repo: Option<&Path>) -> HzResult<Option<PathBuf>> {
    for args in [vec!["root"], vec!["workspace", "root"]] {
        let output = jj_output(repo, &args)?;
        let Some(output) = output else {
            return Ok(None);
        };
        if !output.status.success() {
            continue;
        }

        let root = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if root.is_empty() {
            return Err(HzError::Usage("jj repository root was empty".to_owned()));
        }
        return Ok(Some(PathBuf::from(root)));
    }

    Ok(None)
}

pub fn add_worktree(
    repo: &Path,
    path: &Path,
    branch: Option<&str>,
    base: Option<&str>,
) -> HzResult<()> {
    backend_for_path(repo)?.add_worktree(repo, path, branch, base)
}

fn git_add_worktree(
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

fn add_jj_workspace(
    repo: &Path,
    path: &Path,
    branch: Option<&str>,
    base: Option<&str>,
) -> HzResult<()> {
    let mut command = jj_command(Some(repo));
    command.args(["workspace", "add"]);
    if let Some(branch) = branch {
        command.args(["--name", branch]);
    }
    if let Some(base) = base {
        command.args(["--revision", base]);
    }
    command.arg("--").arg(path);

    let output = command.output()?;
    if !output.status.success() {
        return Err(jj_error("failed to add jj workspace", &output));
    }

    Ok(())
}

pub fn remove_worktree(repo: &Path, path: &Path) -> HzResult<()> {
    remove_worktree_with_force(repo, path, false)
}

pub fn remove_worktree_with_force(repo: &Path, path: &Path, force: bool) -> HzResult<()> {
    backend_for_path(repo)?.remove_worktree_with_force(repo, path, force)
}

fn git_remove_worktree_with_force(repo: &Path, path: &Path, force: bool) -> HzResult<()> {
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

fn remove_jj_workspace_with_force(repo: &Path, path: &Path, force: bool) -> HzResult<()> {
    if !force {
        let state = worktree_state(path)?;
        if state.dirty {
            return Err(HzError::Usage(format!(
                "failed to remove jj workspace: working copy has uncommitted changes: {}",
                path.display()
            )));
        }
    }

    let workspace = jj_workspace_for_path(repo, path)?;
    if path.exists() {
        fs::remove_dir_all(path)?;
    }

    let output = jj_command(Some(repo))
        .args(["workspace", "forget"])
        .arg(&workspace.name)
        .output()?;
    if !output.status.success() {
        return Err(jj_error("failed to forget jj workspace", &output));
    }

    Ok(())
}

pub fn list_worktrees(repo: &Path) -> HzResult<Vec<GitWorktree>> {
    backend_for_path(repo)?.list_worktrees(repo)
}

fn git_list_worktrees(repo: &Path) -> HzResult<Vec<GitWorktree>> {
    let output = worktree_list_output(repo)?;

    Ok(parse_worktree_list(&output.stdout))
}

pub fn main_worktree(repo: &Path) -> HzResult<PathBuf> {
    backend_for_path(repo)?.main_worktree(repo)
}

fn git_main_worktree(repo: &Path) -> HzResult<PathBuf> {
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
    backend_for_path(path)?.worktree_state(path)
}

fn git_worktree_state(path: &Path) -> HzResult<GitWorktreeState> {
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

fn jj_worktree_state(path: &Path) -> HzResult<GitWorktreeState> {
    let output = jj_command(Some(path))
        .args(["diff", "--name-only"])
        .output()?;
    if !output.status.success() {
        return Err(jj_error("failed to read jj workspace status", &output));
    }

    let paths = parse_jj_paths(&output.stdout);
    if paths.is_empty() {
        return Ok(GitWorktreeState {
            dirty: false,
            modified_at_unix: 0,
        });
    }

    Ok(GitWorktreeState {
        dirty: true,
        modified_at_unix: paths
            .into_iter()
            .filter_map(|path_name| path_modified_at(&path.join(path_name)))
            .max()
            .unwrap_or_else(|| path_modified_at(path).unwrap_or(0)),
    })
}

pub fn current_branch(repo: &Path) -> HzResult<Option<String>> {
    backend_for_path(repo)?.current_branch(repo)
}

fn git_current_branch(repo: &Path) -> HzResult<Option<String>> {
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

fn current_jj_bookmark(repo: &Path) -> HzResult<Option<String>> {
    Ok(jj_bookmarks_for_revision(repo, "@")?
        .into_iter()
        .next()
        .filter(|bookmark| !bookmark.is_empty()))
}

pub fn current_head(repo: &Path) -> HzResult<String> {
    backend_for_path(repo)?.current_head(repo)
}

fn git_current_head(repo: &Path) -> HzResult<String> {
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

fn current_jj_commit(repo: &Path) -> HzResult<String> {
    let output = jj_command(Some(repo))
        .args(["log", "-r", "@", "--no-graph", "-T", "commit_id ++ \"\\n\""])
        .output()?;
    if !output.status.success() {
        return Err(jj_error("failed to read current jj commit", &output));
    }

    let head = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if head.is_empty() {
        return Err(HzError::Usage("jj commit id was empty".to_owned()));
    }

    Ok(head)
}

pub fn remote_url(repo: &Path, remote: &str) -> HzResult<String> {
    backend_for_path(repo)?.remote_url(repo, remote)
}

fn git_remote_url(repo: &Path, remote: &str) -> HzResult<String> {
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
    backend_for_path(repo)?.branch_exists(repo, branch)
}

fn git_branch_exists(repo: &Path, branch: &str) -> HzResult<bool> {
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

fn jj_bookmark_exists(repo: &Path, bookmark: &str) -> HzResult<bool> {
    Ok(jj_bookmarks(repo)?
        .iter()
        .any(|candidate| candidate == bookmark))
}

pub fn delete_branch(repo: &Path, branch: &str) -> HzResult<()> {
    backend_for_path(repo)?.delete_branch(repo, branch)
}

fn delete_jj_bookmark(repo: &Path, branch: &str) -> HzResult<()> {
    let output = jj_command(Some(repo))
        .args(["bookmark", "delete"])
        .arg(branch)
        .output()?;
    if !output.status.success() {
        return Err(jj_error("failed to delete jj bookmark", &output));
    }

    Ok(())
}

fn git_delete_branch(repo: &Path, branch: &str) -> HzResult<()> {
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
    backend_for_path(repo)?.switch_branch(repo, branch)
}

fn git_switch_branch(repo: &Path, branch: &str) -> HzResult<()> {
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
    backend_for_path(repo)?.switch_detached(repo)
}

fn git_switch_detached(repo: &Path) -> HzResult<()> {
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
    backend_for_path(repo)?.switch_detached_at(repo, rev)
}

fn git_switch_detached_at(repo: &Path, rev: &str) -> HzResult<()> {
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
    backend_for_path(repo)?.diff_patch(repo)
}

fn git_diff_patch(repo: &Path) -> HzResult<Vec<u8>> {
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

fn jj_diff_patch(repo: &Path) -> HzResult<Vec<u8>> {
    let output = jj_command(Some(repo)).args(["diff", "--git"]).output()?;
    if !output.status.success() {
        return Err(jj_error("failed to create jj patch", &output));
    }

    Ok(output.stdout)
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
    backend_for_path(repo)?.hash_bytes(repo, bytes)
}

fn git_hash_bytes(repo: &Path, bytes: &[u8]) -> HzResult<String> {
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
    backend_for_path(repo)?.apply_patch_command(repo, patch, check, reverse)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GitApplyTarget {
    Repository,
    CurrentDirectory,
}

fn git_apply_patch_command(
    repo: &Path,
    patch: &[u8],
    check: bool,
    reverse: bool,
    target: GitApplyTarget,
) -> HzResult<()> {
    let mut command = Command::new("git");
    match target {
        GitApplyTarget::Repository => {
            command.arg("-C").arg(repo).arg("apply");
        }
        GitApplyTarget::CurrentDirectory => {
            command.current_dir(repo).arg("apply");
        }
    }
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

pub fn git_path(repo: &Path, path: &str) -> HzResult<PathBuf> {
    scm_path(repo, path)
}

pub fn scm_path(repo: &Path, path: &str) -> HzResult<PathBuf> {
    backend_for_path(repo)?.scm_path(repo, path)
}

fn git_scm_path(repo: &Path, path: &str) -> HzResult<PathBuf> {
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct JjWorkspace {
    name: String,
    path: PathBuf,
}

fn list_jj_workspaces(repo: &Path) -> HzResult<Vec<GitWorktree>> {
    let current = jj_repository_root(Some(repo))?;
    let mut workspaces = jj_workspaces(repo)?;
    if let Some(current) = current {
        workspaces.sort_by_key(|workspace| {
            (
                !same_path(&workspace.path, &current),
                workspace.name.clone(),
            )
        });
    }

    Ok(workspaces
        .into_iter()
        .map(|workspace| GitWorktree {
            path: workspace.path,
            branch: Some(workspace.name),
        })
        .collect())
}

fn jj_workspaces(repo: &Path) -> HzResult<Vec<JjWorkspace>> {
    let output = jj_command(Some(repo))
        .args([
            "workspace",
            "list",
            "-T",
            "name ++ \"\\t\" ++ root ++ \"\\n\"",
        ])
        .output()?;
    if !output.status.success() {
        return Err(jj_error("failed to list jj workspaces", &output));
    }

    Ok(parse_jj_workspace_list(&output.stdout))
}

fn jj_workspace_for_path(repo: &Path, path: &Path) -> HzResult<JjWorkspace> {
    jj_workspaces(repo)?
        .into_iter()
        .find(|workspace| same_path(&workspace.path, path))
        .ok_or_else(|| {
            HzError::Usage(format!(
                "jj workspace not found for path: {}",
                path.display()
            ))
        })
}

fn parse_jj_workspace_list(output: &[u8]) -> Vec<JjWorkspace> {
    String::from_utf8_lossy(output)
        .lines()
        .filter_map(|line| {
            let (name, path) = line.split_once('\t')?;
            let name = name.trim();
            let path = path.trim();
            if name.is_empty() || path.is_empty() {
                return None;
            }
            Some(JjWorkspace {
                name: name.to_owned(),
                path: PathBuf::from(path),
            })
        })
        .collect()
}

fn jj_bookmarks_for_revision(repo: &Path, revision: &str) -> HzResult<Vec<String>> {
    let output = jj_command(Some(repo))
        .args(["bookmark", "list", "-r", revision, "-T", "name ++ \"\\n\""])
        .output()?;
    if !output.status.success() {
        return Err(jj_error("failed to list jj bookmarks", &output));
    }

    Ok(parse_jj_names(&output.stdout))
}

fn jj_bookmarks(repo: &Path) -> HzResult<Vec<String>> {
    let output = jj_command(Some(repo))
        .args(["bookmark", "list", "-T", "name ++ \"\\n\""])
        .output()?;
    if !output.status.success() {
        return Err(jj_error("failed to list jj bookmarks", &output));
    }

    Ok(parse_jj_names(&output.stdout))
}

fn parse_jj_names(output: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(output)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

fn parse_jj_paths(output: &[u8]) -> Vec<PathBuf> {
    String::from_utf8_lossy(output)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .collect()
}

fn jj_command(repo: Option<&Path>) -> Command {
    let mut command = Command::new("jj");
    if let Some(repo) = repo {
        command.arg("-R").arg(repo);
    }
    command
}

fn jj_output(repo: Option<&Path>, args: &[&str]) -> HzResult<Option<process::Output>> {
    match jj_command(repo).args(args).output() {
        Ok(output) => Ok(Some(output)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn same_path(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }

    fs::canonicalize(left)
        .ok()
        .zip(fs::canonicalize(right).ok())
        .is_some_and(|(left, right)| left == right)
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex_encode(&Sha256::digest(bytes))
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
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

fn jj_error(context: &str, output: &std::process::Output) -> HzError {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let detail = stderr.trim();
    if detail.is_empty() {
        HzError::Usage(context.to_owned())
    } else {
        HzError::Usage(format!("{context}: {detail}"))
    }
}

fn jj_branch_handoff_error() -> HzError {
    HzError::Usage("branch handoff is not supported for this source-control backend".to_owned())
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
    fn backend_detection_prefers_colocated_jj_root() {
        let selected =
            nearest_backend_root(Some(PathBuf::from("/repo")), Some(PathBuf::from("/repo")))
                .expect("backend should be selected");

        assert_eq!(selected.backend.kind(), SourceControl::Jj);
        assert_eq!(selected.root, PathBuf::from("/repo"));
    }

    #[test]
    fn backend_detection_prefers_nearest_nested_git_root() {
        let selected = nearest_backend_root(
            Some(PathBuf::from("/repo")),
            Some(PathBuf::from("/repo/vendor/nested")),
        )
        .expect("backend should be selected");

        assert_eq!(selected.backend.kind(), SourceControl::Git);
        assert_eq!(selected.root, PathBuf::from("/repo/vendor/nested"));
    }

    #[test]
    fn backend_detection_prefers_nearest_nested_jj_root() {
        let selected = nearest_backend_root(
            Some(PathBuf::from("/repo/subproject")),
            Some(PathBuf::from("/repo")),
        )
        .expect("backend should be selected");

        assert_eq!(selected.backend.kind(), SourceControl::Jj);
        assert_eq!(selected.root, PathBuf::from("/repo/subproject"));
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

    #[test]
    fn parses_jj_workspace_list_template_output() {
        assert_eq!(
            parse_jj_workspace_list(b"main\t/repo\nfeature\t/workspaces/feature\n"),
            vec![
                JjWorkspace {
                    name: "main".to_owned(),
                    path: PathBuf::from("/repo"),
                },
                JjWorkspace {
                    name: "feature".to_owned(),
                    path: PathBuf::from("/workspaces/feature"),
                },
            ]
        );
    }

    #[test]
    fn parses_jj_name_only_output() {
        assert_eq!(
            parse_jj_names(b"main\n\nfeature\n"),
            vec!["main".to_owned(), "feature".to_owned()]
        );
    }

    #[test]
    fn sha256_hash_is_stable_for_jj_patch_handoffs() {
        assert_eq!(
            sha256_hex(b"patch"),
            "a4895eb44afc336fecbba6e520cd67e178dace0276655d102fceffa8e5f70570"
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
