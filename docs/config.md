# hz configuration

Workspace-local configuration lives at `.hz/hz.toml` and is copied into child
workspaces. `hz init` creates it while registering a root; `hz config init`
creates only the configuration files.

```text
.hz/
  hz.toml
  environment/
    postcreate
    preremove
```

## Lifecycle

```toml
[lifecycle]
postcreate = [".hz/environment/postcreate"]
preremove = [".hz/environment/preremove"]
```

The generated entries are commented out so the default create/remove path does
not spawn hook processes. Uncomment only the hooks the project needs. Commands
are argv arrays, not shell strings; relative executable paths are resolved from
the workspace in which the hook runs.

`postcreate` runs after filesystem cloning, marker creation, and registry
activation. Failure leaves the new workspace active so it can be
inspected or removed normally.

`preremove` runs before a selected workspace subtree moves into trash. Either
hook can be skipped with the command's `--no-hooks` flag.

Lifecycle processes receive:

```text
HZ_ROOT          root workspace path
HZ_SOURCE        immediate source path
HZ_WORKSPACE     selected or created workspace path
HZ_WORKSPACE_ID  stable workspace ULID
HZ_PARENT_ID     immediate parent ID, or empty for a root
HZ_LIFECYCLE     postcreate or preremove
HZ_REPO          compatibility alias for HZ_ROOT
HZ_WORKTREE      compatibility alias for HZ_WORKSPACE
HZ_TARGET        compatibility alias for the workspace handle
```

Legacy `setup` and `cleanup` entries are ignored when reading pre-0.8
configuration because those hooks were opt-in. Rename only the entries that
should run by default to `postcreate` and `preremove`.

## Copy filtering

The default mode takes a complete COW snapshot. `hz new --filtered` omits
built-in regenerable artifacts such as `node_modules`, `target`, virtual
environments, framework caches, `dist`, `build`, and `coverage`.

Source-control metadata is copied as ordinary filesystem state. Workspace
creation does not invoke source-control subprocesses; it only ensures local Git
or Mercurial ignore rules protect the `.hz-workspace` identity marker.
