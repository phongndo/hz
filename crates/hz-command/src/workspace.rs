use std::{
    env, io,
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
};

use hz_core::{HzError, HzResult};
use hz_workspace::{
    CreatedWorkspace, DoctorReport, GarbageCollection, InitializedWorkspace, ListWorkspaces,
    Manager, RemoveMode, RemovedWorkspaces, Workspace,
};
use serde::Serialize;

use crate::{HzConfig, InitConfig, InitializedConfig, init_config};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkspaceInitialization {
    pub initialized: InitializedWorkspace,
    pub config: InitializedConfig,
}

pub fn workspace_manager() -> HzResult<Manager> {
    match env::var_os("HZ_DATABASE") {
        Some(path) if !path.is_empty() => Manager::open(PathBuf::from(path)),
        _ => Manager::open_default(),
    }
    .map_err(HzError::from)
}

pub fn init_workspace(input: hz_workspace::InitWorkspace) -> HzResult<WorkspaceInitialization> {
    let mut manager = workspace_manager()?;
    let (initialized, config) = manager
        .init_with_setup(input, |at| {
            init_config(InitConfig {
                at: at.to_path_buf(),
            })
            .map_err(hz_workspace::Error::from)
        })
        .map_err(HzError::from)?;
    Ok(WorkspaceInitialization {
        initialized,
        config,
    })
}

pub fn create_workspace(
    input: hz_workspace::CreateWorkspace,
    run_hooks: bool,
    capture_hook_output: bool,
) -> HzResult<CreatedWorkspace> {
    let mut manager = workspace_manager()?;
    let source = manager.current(&input.from).map_err(HzError::from)?;
    let config = if run_hooks {
        HzConfig::load(&source.path)?
    } else {
        HzConfig::default()
    };
    let created = manager.create(input).map_err(HzError::from)?;
    if run_hooks {
        if let Some(command) = config.lifecycle.postcreate.as_deref() {
            run_lifecycle(
                command,
                "postcreate",
                &source.path,
                &created.workspace,
                capture_hook_output,
            )?;
        }
    }
    Ok(created)
}

pub fn adopt_workspace(at: impl AsRef<Path>) -> HzResult<Workspace> {
    workspace_manager()?.adopt(at).map_err(HzError::from)
}

pub fn current_workspace(at: impl AsRef<Path>) -> HzResult<Workspace> {
    workspace_manager()?.current(at).map_err(HzError::from)
}

pub fn resolve_workspace(
    at: impl AsRef<Path>,
    target: Option<&str>,
    include_trash: bool,
) -> HzResult<Workspace> {
    workspace_manager()?
        .resolve_target(at, target, include_trash)
        .map_err(HzError::from)
}

pub fn list_workspaces(input: ListWorkspaces) -> HzResult<Vec<Workspace>> {
    workspace_manager()?.list(input).map_err(HzError::from)
}

pub fn workspace_ancestors(at: impl AsRef<Path>, target: Option<&str>) -> HzResult<Vec<Workspace>> {
    workspace_manager()?
        .ancestors(at, target)
        .map_err(HzError::from)
}

pub fn pin_workspaces(
    at: impl AsRef<Path>,
    targets: &[String],
    pinned: bool,
) -> HzResult<Vec<Workspace>> {
    workspace_manager()?
        .set_pinned(at, targets, pinned)
        .map_err(HzError::from)
}

pub fn remove_workspace(
    at: impl AsRef<Path>,
    target: Option<&str>,
    mode: RemoveMode,
    force_root: bool,
    run_hooks: bool,
    capture_hook_output: bool,
) -> HzResult<RemovedWorkspaces> {
    let mut manager = workspace_manager()?;
    let selected = manager
        .resolve_target(at.as_ref(), target, false)
        .map_err(HzError::from)?;
    if selected.parent_id.is_none() && mode == RemoveMode::Subtree && !force_root {
        return Err(HzError::Usage(format!(
            "root workspace removal requires --force: {}",
            selected.path.display()
        )));
    }
    if run_hooks && mode == RemoveMode::Subtree {
        let config = HzConfig::load(&selected.path)?;
        if let Some(command) = config.lifecycle.preremove.as_deref() {
            run_lifecycle(
                command,
                "preremove",
                &selected.path,
                &selected,
                capture_hook_output,
            )?;
        }
    }
    manager
        .remove(at, target, mode, force_root)
        .map_err(HzError::from)
}

pub fn restore_workspace(at: impl AsRef<Path>, target: &str) -> HzResult<Vec<Workspace>> {
    workspace_manager()?
        .restore(at, target)
        .map_err(HzError::from)
}

pub fn trashed_workspaces(at: impl AsRef<Path>) -> HzResult<Vec<Workspace>> {
    workspace_manager()?.trashed(at).map_err(HzError::from)
}

pub fn gc_workspaces() -> HzResult<GarbageCollection> {
    workspace_manager()?.gc().map_err(HzError::from)
}

pub fn doctor_workspaces(fix: bool) -> HzResult<DoctorReport> {
    workspace_manager()?.doctor(fix).map_err(HzError::from)
}

fn run_lifecycle(
    argv: &[String],
    kind: &str,
    source: &Path,
    workspace: &Workspace,
    capture_output: bool,
) -> HzResult<()> {
    let (program, args) = argv
        .split_first()
        .ok_or_else(|| HzError::Usage(format!("{kind} command cannot be empty")))?;
    if program.is_empty() {
        return Err(HzError::Usage(format!(
            "{kind} command program cannot be empty"
        )));
    }
    let program = if program.contains('/') || program.contains('\\') {
        workspace.path.join(program)
    } else {
        PathBuf::from(program)
    };
    let root = root_path_for(workspace)?;
    let mut command = Command::new(&program);
    command
        .args(args)
        .current_dir(&workspace.path)
        .env("HZ_ROOT", &root)
        .env("HZ_SOURCE", source)
        .env("HZ_WORKSPACE", &workspace.path)
        .env("HZ_WORKSPACE_ID", &workspace.id)
        .env(
            "HZ_PARENT_ID",
            workspace.parent_id.as_deref().unwrap_or_default(),
        )
        .env("HZ_LIFECYCLE", kind)
        // Compatibility aliases for lifecycle scripts written for Hz 0.7.
        .env("HZ_REPO", &root)
        .env("HZ_WORKTREE", &workspace.path)
        .env("HZ_TARGET", &workspace.handle);

    if capture_output {
        let output = command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .map_err(|error| lifecycle_start_error(kind, workspace, error))?;
        if !output.status.success() {
            return Err(HzError::Usage(lifecycle_failure(
                kind,
                workspace,
                output.status,
                Some(&output.stdout),
                Some(&output.stderr),
            )));
        }
        return Ok(());
    }

    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|error| lifecycle_start_error(kind, workspace, error))?;
    if let Some(mut stdout) = child.stdout.take() {
        io::copy(&mut stdout, &mut io::stderr())?;
    }
    let status = child.wait()?;
    if !status.success() {
        return Err(HzError::Usage(lifecycle_failure(
            kind, workspace, status, None, None,
        )));
    }
    Ok(())
}

fn lifecycle_start_error(kind: &str, workspace: &Workspace, error: io::Error) -> HzError {
    HzError::Usage(format!(
        "failed to start {kind} command at {}: {error}",
        workspace.path.display()
    ))
}

fn lifecycle_failure(
    kind: &str,
    workspace: &Workspace,
    status: ExitStatus,
    stdout: Option<&[u8]>,
    stderr: Option<&[u8]>,
) -> String {
    let mut message = format!(
        "{kind} command failed at {} with status {status}; workspace remains active",
        workspace.path.display()
    );
    for (name, bytes) in [("stdout", stdout), ("stderr", stderr)] {
        let Some(bytes) = bytes else {
            continue;
        };
        let output = String::from_utf8_lossy(bytes);
        let output = output.trim();
        if !output.is_empty() {
            message.push_str(&format!("; {name}: {output}"));
        }
    }
    message
}

fn root_path_for(workspace: &Workspace) -> HzResult<PathBuf> {
    if workspace.id == workspace.root_id {
        return Ok(workspace.path.clone());
    }
    let manager = workspace_manager()?;
    let ancestors = manager
        .ancestors(&workspace.path, None)
        .map_err(HzError::from)?;
    ancestors
        .last()
        .map(|root| root.path.clone())
        .ok_or_else(|| HzError::Usage("workspace root is missing".to_owned()))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn lifecycle_exports_legacy_environment_aliases() {
        let temp = TempDir::new().unwrap();
        let workspace = Workspace {
            id: "root-id".to_owned(),
            root_id: "root-id".to_owned(),
            parent_id: None,
            handle: "root".to_owned(),
            path: temp.path().to_path_buf(),
            original_path: None,
            storage_path: None,
            state: hz_workspace::WorkspaceState::Active,
            materializer: hz_workspace::Materializer::RegisteredRoot,
            strategy: "copy".to_owned(),
            copy_mode: hz_workspace::CopyMode::All,
            pinned: false,
            created_at_unix_ms: 0,
            updated_at_unix_ms: 0,
        };
        let output = temp.path().join("environment");
        let command = vec![
            "sh".to_owned(),
            "-c".to_owned(),
            "printf '%s\\n' \"$HZ_REPO\" \"$HZ_WORKTREE\" \"$HZ_TARGET\" > environment".to_owned(),
        ];

        run_lifecycle(&command, "postcreate", temp.path(), &workspace, true).unwrap();

        assert_eq!(
            std::fs::read_to_string(output).unwrap(),
            format!(
                "{}\n{}\nroot\n",
                temp.path().display(),
                temp.path().display()
            )
        );
    }
}
