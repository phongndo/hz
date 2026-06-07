#![allow(unused_imports)]

use crate::*;
use std::{
    collections::HashMap,
    env, fs,
    io::{self, ErrorKind, Read, Write},
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, Stdio},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use hz_core::{HzError, HzResult};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleKind {
    Setup,
    Cleanup,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunLifecycle {
    pub target: Option<String>,
    pub repo: Option<PathBuf>,
    pub kind: LifecycleKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LifecycleRun {
    pub repo: PathBuf,
    pub path: PathBuf,
    pub target: String,
    pub kind: LifecycleKind,
    pub configured: bool,
}

pub fn run_lifecycle(input: RunLifecycle) -> HzResult<LifecycleRun> {
    let target = input.target.unwrap_or_else(|| "local".to_owned());
    if target == "local" {
        let local = hz_worktree::local(LocalWorktree { repo: input.repo })?;
        return run_lifecycle_for_path(&local.repo, &local.path, "local", input.kind);
    }

    let worktree = hz_worktree::find(FindWorktree {
        target: target.clone(),
        repo: input.repo,
    })?;

    run_lifecycle_for_entry(&worktree, input.kind)
}

pub fn run_lifecycle_for_entry(
    entry: &WorktreeEntry,
    kind: LifecycleKind,
) -> HzResult<LifecycleRun> {
    run_lifecycle_for_path(&entry.repo, &entry.path, &worktree_target(entry), kind)
}

pub(crate) fn run_lifecycle_for_path(
    repo: &Path,
    path: &Path,
    target: &str,
    kind: LifecycleKind,
) -> HzResult<LifecycleRun> {
    let config = HzConfig::load(path)?;
    let Some(command) = config.lifecycle_command(kind) else {
        return Ok(LifecycleRun {
            repo: repo.to_path_buf(),
            path: path.to_path_buf(),
            target: target.to_owned(),
            kind,
            configured: false,
        });
    };

    let mut stderr = io::stderr();
    run_lifecycle_command(repo, path, target, kind, command, &mut stderr)?;
    Ok(LifecycleRun {
        repo: repo.to_path_buf(),
        path: path.to_path_buf(),
        target: target.to_owned(),
        kind,
        configured: true,
    })
}

pub(crate) fn run_lifecycle_command(
    repo: &Path,
    worktree: &Path,
    target: &str,
    kind: LifecycleKind,
    argv: &[String],
    stdout: &mut impl Write,
) -> HzResult<()> {
    let (program, args) = argv
        .split_first()
        .ok_or_else(|| HzError::Usage(format!("{} command cannot be empty", kind.label())))?;
    if program.is_empty() {
        return Err(HzError::Usage(format!(
            "{} command program cannot be empty",
            kind.label()
        )));
    }

    let program = lifecycle_program(worktree, program)?;
    let mut child = ProcessCommand::new(&program)
        .args(args)
        .current_dir(worktree)
        .env("HZ_REPO", repo)
        .env("HZ_WORKTREE", worktree)
        .env("HZ_TARGET", target)
        .env("HZ_LIFECYCLE", kind.label())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;
    let Some(mut child_stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(HzError::Usage(
            "failed to open lifecycle command stdout".to_owned(),
        ));
    };
    if let Err(error) = io::copy(&mut child_stdout, stdout) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error.into());
    }
    let status = child.wait()?;

    if !status.success() {
        return Err(HzError::Usage(format!(
            "{} command failed with status {}",
            kind.label(),
            status
        )));
    }

    Ok(())
}

pub(crate) fn lifecycle_program(worktree: &Path, program: &str) -> HzResult<PathBuf> {
    if looks_like_path(program) {
        let path = worktree.join(program);
        if !path.exists() {
            return Err(HzError::Usage(format!(
                "lifecycle command not found in worktree: {}",
                path.display()
            )));
        }
        return Ok(path);
    }

    Ok(PathBuf::from(program))
}

pub(crate) fn looks_like_path(program: &str) -> bool {
    program.contains('/') || program.contains('\\') || program == "." || program == ".."
}

pub(crate) fn worktree_target(entry: &WorktreeEntry) -> String {
    branch_or_handle(entry.branch.as_deref(), &entry.handle)
}

pub(crate) fn created_worktree_target(created: &CreatedWorktree) -> String {
    branch_or_handle(created.branch.as_deref(), &created.handle)
}

pub(crate) fn branch_or_handle(branch: Option<&str>, handle: &str) -> String {
    branch.unwrap_or(handle).to_owned()
}

impl LifecycleKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            LifecycleKind::Setup => "setup",
            LifecycleKind::Cleanup => "cleanup",
        }
    }
}
