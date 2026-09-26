# hz configuration

Workspace-local configuration lives at `.hz/hz.toml` and is copied into children.
`hz init` only registers the root. Run `hz config init [TARGET]` explicitly to
create a commented configuration in a registered workspace. Existing files are
left intact; hook scripts are supplied by the project.

## Copy filtering

```toml
[create]
filtered = true
```

Copies are full by default. `filtered = true` changes the default for children
created from this workspace; `hz new --full` and `hz new --filtered` override it.
The exact filter and filesystem guarantees live in [design.md](design.md).

## Lifecycle

```toml
[lifecycle]
postcreate = [["./scripts/setup"], ["npm", "install", "--prefer-offline"]]
preremove = [["./scripts/teardown"]]
```

Each entry is an argv array, executed without a shell. A string entry is also
accepted as one executable with no arguments, not as a shell command. Relative
executable paths containing `/` resolve from the workspace where the hook runs;
plain executable names use `PATH`.

The generated entries are commented out. With no configured hooks, creation
and removal spawn no processes. `--no-hooks` skips configured hooks.

`postcreate` runs in the new workspace after it is active. Failure reports the
workspace path and leaves it available for inspection or removal.

`preremove` runs in each selected descendant, deepest first, before any workspace
moves into trash. A failure cancels removal, though side effects of hooks that
already ran remain. Unregistering a root leaves its directory in place and does
not run a removal hook in the root itself.

Hook output goes to stderr, preserving JSON and path-only output on stdout.
Hooks receive EOF on stdin and these environment variables:

```text
HZ_ROOT          root workspace path
HZ_SOURCE        immediate source/parent path
HZ_WORKSPACE     workspace where the hook runs
HZ_WORKSPACE_ID  stable workspace ULID
HZ_PARENT_ID     immediate parent ID, or empty for a root
HZ_HANDLE        workspace handle
HZ_LIFECYCLE     postcreate or preremove
```

Legacy registry and configuration migration is not provided by the C++ rewrite.
