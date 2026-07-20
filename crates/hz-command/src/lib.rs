mod config;
mod shell;
mod source_control;
mod workspace;

#[cfg(test)]
mod tests;

pub use config::{
    HzConfig, InitConfig, InitializedConfig, LifecycleConfig, config_path, init_config,
};
pub use shell::{
    Shell, ShellInit, install_shell_integration, shell_init_comment, shell_init_line,
    shell_integration,
};
pub use source_control::{
    GitHandoff, GitWorkspaceStatus, MercurialWorkspaceStatus, git_handoff, git_status,
    mercurial_status,
};
pub use workspace::{
    WorkspaceInitialization, adopt_workspace, create_workspace, current_workspace,
    doctor_workspaces, gc_workspaces, init_workspace, list_workspaces, pin_workspaces,
    remove_workspace, remove_workspace_with_navigation, resolve_workspace, restore_workspace,
    trashed_workspaces, workspace_ancestors, workspace_manager,
};

pub use hz_workspace::{
    CopyMode, CreateWorkspace, CreatedWorkspace, DoctorIssue, DoctorReport, GarbageCollection,
    InitOutcome, InitStrategy, InitWorkspace, InitializedWorkspace, ListScope, ListWorkspaces,
    MARKER_FILE, Materializer, RemoveMode, RemovedWorkspaces, Workspace, WorkspaceState,
};
