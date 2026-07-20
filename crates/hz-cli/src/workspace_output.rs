use std::{collections::HashMap, env};

use serde::Serialize;

use crate::{
    CliResult,
    args::{
        AdoptArgs, AncestorsArgs, ConfigInitArgs, DoctorArgs, GitHandoffArgs, InitWorkspaceArgs,
        JsonArgs, ListWorkspaceArgs, NewWorkspaceArgs, PathWorkspaceArgs, PwdWorkspaceArgs,
        RemoveWorkspaceArgs, RestoreWorkspaceArgs, SourceStatusArgs, TargetsArgs,
    },
    write_stdout,
};

pub(crate) fn init_workspace(args: InitWorkspaceArgs, machine: bool) -> CliResult<()> {
    let initialized = hz_command::init_workspace(hz_command::InitWorkspace {
        at: args.path.unwrap_or(env::current_dir()?),
        here: args.here,
        strategy: if args.copy {
            hz_command::InitStrategy::Copy
        } else {
            hz_command::InitStrategy::CopyOnWrite
        },
    })?;
    if machine || args.json {
        json(&initialized)
    } else if args.path_only {
        write_stdout(format_args!(
            "{}\n",
            initialized.initialized.workspace.path.display()
        ))
    } else {
        write_stdout(format_args!(
            "initialized {}  {}\n",
            initialized.initialized.workspace.handle,
            initialized.initialized.workspace.path.display()
        ))
    }
}

pub(crate) fn new_workspace(args: NewWorkspaceArgs, machine: bool) -> CliResult<()> {
    let created = hz_command::create_workspace(
        hz_command::CreateWorkspace {
            from: args.from.unwrap_or(env::current_dir()?),
            handle: args.name,
            into: args.into,
            copy_mode: if args.filtered {
                hz_command::CopyMode::Filtered
            } else {
                hz_command::CopyMode::All
            },
        },
        !args.no_hooks,
        machine,
    )?;
    if machine || args.json {
        json(&created)
    } else if args.path_only {
        write_stdout(format_args!("{}\n", created.workspace.path.display()))
    } else {
        write_stdout(format_args!(
            "created {}  {}\n",
            created.workspace.handle,
            created.workspace.path.display()
        ))
    }
}

pub(crate) fn path_workspace(args: PathWorkspaceArgs, machine: bool) -> CliResult<()> {
    let at = args.at.unwrap_or(env::current_dir()?);
    let target = args.target.as_deref().or(Some("root"));
    let workspace = hz_command::resolve_workspace(at, target, false)?;
    if machine || args.json {
        json(&workspace)
    } else {
        write_stdout(format_args!("{}\n", workspace.path.display()))
    }
}

pub(crate) fn pwd_workspace(args: PwdWorkspaceArgs, machine: bool) -> CliResult<()> {
    let workspace = hz_command::current_workspace(args.at.unwrap_or(env::current_dir()?))?;
    if machine || args.json {
        json(&workspace)
    } else {
        write_stdout(format_args!("{}\n", workspace.handle))
    }
}

pub(crate) fn list_workspaces(args: ListWorkspaceArgs, machine: bool) -> CliResult<()> {
    let workspaces = hz_command::list_workspaces(hz_command::ListWorkspaces {
        of: args.at,
        scope: if args.roots {
            hz_command::ListScope::Roots
        } else if args.children {
            hz_command::ListScope::Children
        } else {
            hz_command::ListScope::Family
        },
        pinned: if args.pinned {
            Some(true)
        } else if args.unpinned {
            Some(false)
        } else {
            None
        },
    })?;
    if machine || args.json {
        return json(&workspaces);
    }
    render_list(&workspaces, args.tree)
}

pub(crate) fn ancestors(args: AncestorsArgs, machine: bool) -> CliResult<()> {
    let workspaces = hz_command::workspace_ancestors(
        args.at.unwrap_or(env::current_dir()?),
        args.target.as_deref(),
    )?;
    if machine || args.json {
        json(&workspaces)
    } else {
        for workspace in workspaces {
            write_stdout(format_args!(
                "{}\t{}\n",
                workspace.handle,
                workspace.path.display()
            ))?;
        }
        Ok(())
    }
}

pub(crate) fn remove_workspace(args: RemoveWorkspaceArgs, machine: bool) -> CliResult<()> {
    let at = args.at.unwrap_or(env::current_dir()?);
    let mode = if args.children {
        hz_command::RemoveMode::Children
    } else {
        hz_command::RemoveMode::Subtree
    };
    if args.path_only && !machine && !args.json {
        let (_, destination) = hz_command::remove_workspace_with_navigation(
            &at,
            args.target.as_deref(),
            mode,
            args.force,
            !args.no_hooks,
            false,
        )?;
        return write_stdout(format_args!("{}\n", destination.display()));
    }

    let removed = hz_command::remove_workspace(
        &at,
        args.target.as_deref(),
        mode,
        args.force,
        !args.no_hooks,
        machine,
    )?;
    if machine || args.json {
        return json(&removed);
    }
    for workspace in &removed.removed {
        write_stdout(format_args!("trashed {}\n", workspace.handle))?;
    }
    if removed.root_unregistered {
        write_stdout(format_args!(
            "unregistered {}\n",
            removed.selected.path.display()
        ))?;
    }
    Ok(())
}

pub(crate) fn pin_workspaces(args: TargetsArgs, machine: bool, pinned: bool) -> CliResult<()> {
    let workspaces = hz_command::pin_workspaces(
        args.at.unwrap_or(env::current_dir()?),
        &args.targets,
        pinned,
    )?;
    if machine || args.json {
        json(&workspaces)
    } else {
        for workspace in workspaces {
            write_stdout(format_args!(
                "{} {}\n",
                if pinned { "pinned" } else { "unpinned" },
                workspace.handle
            ))?;
        }
        Ok(())
    }
}

pub(crate) fn restore_workspace(args: RestoreWorkspaceArgs, machine: bool) -> CliResult<()> {
    let restored =
        hz_command::restore_workspace(args.at.unwrap_or(env::current_dir()?), &args.target)?;
    if machine || args.json {
        json(&restored)
    } else if args.path_only {
        let selected = restored
            .iter()
            .find(|workspace| {
                workspace.handle == args.target || workspace.id.starts_with(&args.target)
            })
            .or_else(|| restored.first())
            .ok_or_else(|| hz_core::HzError::Usage("restore returned no workspace".to_owned()))?;
        write_stdout(format_args!("{}\n", selected.path.display()))
    } else {
        for workspace in restored {
            write_stdout(format_args!(
                "restored {}  {}\n",
                workspace.handle,
                workspace.path.display()
            ))?;
        }
        Ok(())
    }
}

pub(crate) fn gc(args: JsonArgs, machine: bool) -> CliResult<()> {
    let result = hz_command::gc_workspaces()?;
    if machine || args.json {
        json(&result)
    } else {
        for path in result.deleted {
            write_stdout(format_args!("{}\n", path.display()))?;
        }
        Ok(())
    }
}

pub(crate) fn adopt(args: AdoptArgs, machine: bool) -> CliResult<()> {
    let workspace = hz_command::adopt_workspace(args.path)?;
    if machine || args.json {
        json(&workspace)
    } else {
        write_stdout(format_args!(
            "adopted {}  {}\n",
            workspace.handle,
            workspace.path.display()
        ))
    }
}

pub(crate) fn doctor(args: DoctorArgs, machine: bool) -> CliResult<()> {
    let report = hz_command::doctor_workspaces(args.fix)?;
    if machine || args.json {
        json(&report)
    } else if report.issues.is_empty() {
        write_stdout(format_args!("workspace registry is healthy\n"))
    } else {
        for issue in report.issues {
            write_stdout(format_args!(
                "{}{}: {} ({})\n",
                if issue.fixed { "fixed " } else { "" },
                issue.workspace_id,
                issue.message,
                issue.path.display()
            ))?;
        }
        Ok(())
    }
}

pub(crate) fn git_status(args: SourceStatusArgs, machine: bool) -> CliResult<()> {
    let status = hz_command::git_status(
        args.at.unwrap_or(env::current_dir()?),
        args.target.as_deref(),
    )?;
    if machine || args.json {
        json(&status)
    } else {
        write_stdout(format_args!(
            "{}\t{:?}\t{}\n",
            status.workspace.handle,
            status.status,
            status.branch.as_deref().unwrap_or("detached")
        ))
    }
}

pub(crate) fn mercurial_status(args: SourceStatusArgs, machine: bool) -> CliResult<()> {
    let status = hz_command::mercurial_status(
        args.at.unwrap_or(env::current_dir()?),
        args.target.as_deref(),
    )?;
    if machine || args.json {
        json(&status)
    } else {
        write_stdout(format_args!(
            "{}\t{:?}\t{}\n",
            status.workspace.handle,
            status.status,
            status.revision.as_deref().unwrap_or("unknown")
        ))
    }
}

pub(crate) fn git_handoff(args: GitHandoffArgs, machine: bool) -> CliResult<()> {
    let handoff = hz_command::git_handoff(
        args.at.unwrap_or(env::current_dir()?),
        args.target.as_deref(),
    )?;
    if machine || args.json {
        json(&handoff)
    } else if args.path_only {
        write_stdout(format_args!("{}\n", handoff.to.path.display()))
    } else {
        write_stdout(format_args!(
            "handed off {} -> {}{}\n",
            handoff.from.handle,
            handoff.to.handle,
            if handoff.changed { "" } else { " (no changes)" }
        ))
    }
}

pub(crate) fn config_init(args: ConfigInitArgs, machine: bool) -> CliResult<()> {
    let initialized = hz_command::init_config(hz_command::InitConfig {
        at: args.path.unwrap_or(env::current_dir()?),
    })?;
    if machine || args.json {
        json(&initialized)
    } else {
        write_stdout(format_args!("{}\n", initialized.config_path.display()))
    }
}

fn render_list(workspaces: &[hz_command::Workspace], tree: bool) -> CliResult<()> {
    let current = env::current_dir()
        .ok()
        .and_then(|cwd| std::fs::canonicalize(cwd).ok());
    let parents = workspaces
        .iter()
        .map(|workspace| (workspace.id.clone(), workspace.parent_id.clone()))
        .collect::<HashMap<_, _>>();
    let ordered = if tree {
        tree_order(workspaces)
    } else {
        workspaces.iter().collect()
    };
    for workspace in ordered {
        let marker = if current
            .as_ref()
            .is_some_and(|current| current.starts_with(&workspace.path))
        {
            "*"
        } else {
            " "
        };
        let indent = if tree {
            "  ".repeat(depth(&workspace.id, &parents))
        } else {
            String::new()
        };
        write_stdout(format_args!(
            "{marker} {indent}{}{}\t{}\n",
            workspace.handle,
            if workspace.pinned { " [pinned]" } else { "" },
            workspace.path.display()
        ))?;
    }
    Ok(())
}

fn tree_order(workspaces: &[hz_command::Workspace]) -> Vec<&hz_command::Workspace> {
    fn append<'a>(
        parent: Option<&str>,
        workspaces: &'a [hz_command::Workspace],
        output: &mut Vec<&'a hz_command::Workspace>,
    ) {
        for workspace in workspaces.iter().filter(|workspace| {
            workspace.parent_id.as_deref() == parent
                || (parent.is_none()
                    && workspace.parent_id.as_ref().is_some_and(|parent_id| {
                        !workspaces
                            .iter()
                            .any(|candidate| &candidate.id == parent_id)
                    }))
        }) {
            if output.iter().any(|candidate| candidate.id == workspace.id) {
                continue;
            }
            output.push(workspace);
            append(Some(&workspace.id), workspaces, output);
        }
    }

    let mut output = Vec::with_capacity(workspaces.len());
    append(None, workspaces, &mut output);
    output
}

fn depth(id: &str, parents: &HashMap<String, Option<String>>) -> usize {
    let mut depth = 0;
    let mut current = parents.get(id).cloned().flatten();
    while let Some(parent) = current {
        if !parents.contains_key(&parent) {
            break;
        }
        depth += 1;
        current = parents.get(&parent).cloned().flatten();
    }
    depth
}

fn json(value: &impl Serialize) -> CliResult<()> {
    write_stdout(format_args!("{}\n", serde_json::to_string_pretty(value)?))
}
