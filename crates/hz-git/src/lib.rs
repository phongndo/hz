use std::{
    path::{Path, PathBuf},
    process::Command,
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
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["worktree", "remove", "--"])
        .arg(path)
        .output()?;

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
}
