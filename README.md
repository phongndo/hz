# hz

`hz` creates fast, isolated copy-on-write workspaces for parallel humans and
agents. Workspaces are independent directories arranged in a logical ancestry
tree; source control is an integration, not the workspace substrate.

> Hz is pre-1.0. Native copy-on-write currently targets macOS and Linux;
> `hz init --copy` provides an explicit portable fallback.

## Quickstart

```sh
cd ~/code/app
hz init

git add .hz
# Commit the generated lifecycle configuration when this is a Git repository.

hz install zsh                 # or bash/fish; restart or source your rc
hz new parser-fix              # creates a child and enters it

# Work in the independent workspace, then inspect or hand changes back.
hz pwd
hz ls --tree
hz git status
hz git handoff root

hz rm parser-fix               # moves its subtree to trash
hz gc                          # physically deletes trash
```

Without shell integration:

```sh
cd "$(hz new parser-fix --path-only)"
cd "$(hz path root)"
```

## Workspace model

Every managed workspace has a stable ULID in `.hz-workspace`. A central SQLite
registry records its path, handle, root, immediate parent, materialization
strategy, source-control integrations, and lifecycle state.

```text
app (root)
├── parser-fix
│   └── parser-follow-up
└── schema-work
```

Ancestry is logical. Descendants normally share one flat storage directory next
to the root:

```text
~/code/app/
~/code/.hz-workspaces/app-01ABCDEF/<workspace-id>/
~/code/.hz-workspaces/app-01ABCDEF/.trash/<workspace-id>/
```

The adjacent location keeps source and destination on the same filesystem.
Handles are for people; IDs name physical directories and remain stable.

## Commands

Workspace lifecycle occupies the top-level command namespace:

```text
hz init [PATH] [--here] [--copy]
hz new [NAME] [--from PATH] [--into DIR] [--filtered] [--no-hooks]
hz list|ls [--tree] [--children] [--roots]
hz pwd
hz path|cd [TARGET]
hz ancestors [TARGET]
hz remove|rm [TARGET] [--children] [--force] [--no-hooks]
hz pin <TARGET...>
hz unpin <TARGET...>
hz restore <TARGET>
hz gc
hz adopt PATH
hz doctor [--fix]
```

Targets may be handles, IDs, unambiguous ID prefixes, or paths. `root` and
`local` both select the current workspace family's root.

`hz new` snapshots the selected workspace's current filesystem state. Creating
from a child records that child as the immediate parent while continuing to use
the root family's storage.

### Copy modes

Creation uses native copy-on-write by default. APFS clones and btrfs subvolume
snapshots take the fastest whole-tree path; an ordinary btrfs root is instead
reflink-imported into its first child. These paths share existing data blocks,
so only blocks changed afterward require additional storage.

Use `--filtered` to omit heavyweight regenerable artifacts such as
`node_modules`, `target`, virtual environments, framework caches, `dist`,
`build`, and `coverage`. Filtering requires walking the tree and can be slower
than a native whole-directory snapshot.

### Removal and recovery

Removing a child moves its full logical subtree into same-filesystem trash with
atomic renames; the removal hot path does not walk or unlink workspace files.
`--children` preserves the selected workspace and removes only descendants.
Removing a root requires `--force`; its directory remains in place while its
marker is removed.

`hz restore` reverses a logical removal. `hz gc` physically deletes trash.
After manually moving a workspace, `hz adopt PATH` updates its registered path.
`hz doctor --fix` reconciles interrupted create/remove/restore operations. It
reports missing active markers without recreating them because a registered
path alone cannot prove ownership of the directory now at that path. After
independently verifying the directory, `hz init --here` explicitly restores it.

## Source control

Source-control operations live below their source-control namespace. Git and
Mercurial are the first built-in integrations:

```sh
hz git status [TARGET]
hz git handoff [TARGET]
hz hg status [TARGET]
```

Workspace lifecycle operations never invoke source-control subprocesses. Hz
adds repository-local Git or Mercurial ignore protection for the
`.hz-workspace` identity marker, then treats source-control metadata as ordinary
filesystem state copied by the selected workspace strategy. Other metadata is
not normalized, so linked checkout semantics remain the source-control tool's
responsibility.

`hz git status`, `hz git handoff`, and `hz hg status` are explicit operations
layered over a selected workspace. `hz git handoff` applies the current
workspace's patch to another clean Git workspace and defaults to the immediate
parent. Additional source-control behavior can be added without changing
workspace identity or storage.

## Filesystem support

| Platform | Strategy |
| --- | --- |
| macOS/APFS | `clonefile`, with per-entry cloning for filtered copies |
| Linux/btrfs | writable snapshots or filtered reflink imports |
| Linux/XFS and compatible filesystems | per-file `FICLONE` reflinks |
| Windows | explicit portable copying with `hz init --copy` |

`hz init` verifies that the selected root can support native copy-on-write. On
btrfs, an ordinary live root remains in place because it cannot be snapshotted
atomically without risking concurrent writes; child workspaces are
reflink-imported into subvolumes. Roots that are already subvolumes use writable
snapshots for unfiltered children. There is no silent byte-copy fallback;
`hz init --copy` explicitly opts a workspace family into portable byte copying
when COW is not available.

## Lifecycle configuration

`hz init` registers the root and creates:

```text
.hz/
  hz.toml
  environment/
    postcreate
    preremove
```

Hooks are disabled by default to keep creation and removal on the filesystem-only
hot path. Enable them explicitly when needed:

```toml
[lifecycle]
postcreate = [".hz/environment/postcreate"]
preremove = [".hz/environment/preremove"]
```

Post-create hooks run in the active destination with `HZ_ROOT`, `HZ_SOURCE`,
`HZ_WORKSPACE`, `HZ_WORKSPACE_ID`, `HZ_PARENT_ID`, and `HZ_LIFECYCLE`. A failing
post-create hook leaves the workspace active and reports its path. Use
`--no-hooks` to skip configured hooks.

Use `hz config init [PATH]` when only the configuration files are needed.

## Shell and machine integration

```sh
hz install zsh
hz install bash
hz install fish
```

The shell wrapper lets `hz init`, `hz new`, `hz cd`, `hz rm`, `hz restore`, and
`hz git handoff` change the caller's directory. `--json`, `--machine`, help, and
path-only calls never change directories.

For automation, `--machine` forces JSON output and bypasses shell navigation:

```sh
hz --machine new parser-fix
hz --machine list
hz --machine git status parser-fix
```

## Development

```sh
cargo test --workspace --locked
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
./scripts/smoke-zsh
```

The primary implementation boundaries are:

```text
crates/hz-workspace  workspace identity, ancestry, SQLite, COW lifecycle
crates/hz-scm        explicit source-control status boundary and shared types
crates/hz-git        explicit Git status and handoff operations
crates/hz-hg         explicit Mercurial status operations
crates/hz-command    configuration, hooks, and command orchestration
crates/hz-cli        CLI, output, completion, and shell navigation
```

## License

MIT
