use std::{
    path::Path,
    process::{Command, Output},
};

use hz_core::{HzError, HzResult};
use hz_scm::{SourceControl, SourceStatus};

#[derive(Debug, Default, Clone, Copy)]
pub struct MercurialSourceControl;

impl SourceControl for MercurialSourceControl {
    fn kind(&self) -> &'static str {
        "hg"
    }

    fn status(&self, workspace: &Path) -> HzResult<SourceStatus> {
        if !workspace.join(".hg").exists() {
            return Ok(SourceStatus::Unknown);
        }
        let output = Command::new("hg")
            .arg("--cwd")
            .arg(workspace)
            .args(["status", "-mardu", "-0"])
            .output()?;
        if !output.status.success() {
            return Err(hg_error("failed to read Mercurial status", &output));
        }
        Ok(if status_has_changes(&output.stdout) {
            SourceStatus::Dirty
        } else {
            SourceStatus::Clean
        })
    }
}

pub fn revision(repository: &Path) -> HzResult<Option<String>> {
    revision_optional(repository)
}

fn revision_optional(repository: &Path) -> HzResult<Option<String>> {
    if !repository.join(".hg").exists() {
        return Ok(None);
    }
    let output = Command::new("hg")
        .arg("--cwd")
        .arg(repository)
        .args(["id", "-i"])
        .output()?;
    if !output.status.success() {
        return Err(hg_error("failed to read Mercurial revision", &output));
    }
    let revision = String::from_utf8_lossy(&output.stdout)
        .trim()
        .trim_end_matches('+')
        .to_owned();
    Ok((!revision.is_empty()).then_some(revision))
}

fn status_has_changes(output: &[u8]) -> bool {
    output
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .any(|record| record.get(2..).is_none_or(|path| path != b".hz-workspace"))
}

fn hg_error(context: &str, output: &Output) -> HzError {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if stderr.is_empty() {
        HzError::Usage(format!(
            "{context}: Mercurial exited with {}",
            output.status
        ))
    } else {
        HzError::Usage(format!("{context}: {stderr}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_includes_unknown_files_but_ignores_the_workspace_marker() {
        assert!(!status_has_changes(b"? .hz-workspace\0"));
        assert!(status_has_changes(b"? source.rs\0"));
        assert!(status_has_changes(b"? .hz-workspace\0M tracked.txt\0"));
    }
}
