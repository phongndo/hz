# hz design

This document is the specification for hz's workspace model. It describes what
hz guarantees, what it deliberately does not do, and why. Command syntax lives
in [cli.md](cli.md) and configuration in [config.md](config.md); this document
owns the model those describe.

## Purpose

hz makes fast, isolated copies of a working directory so that several people or
agents can work on the same project at once without sharing a checkout. The
copies are ordinary directories. Source control is a layer on top, not the
substrate.

## Principles

1. **One behaviour on every filesystem.** hz has the same semantics, commands,
   and guarantees everywhere. Filesystems differ only in speed and in whether
   copy-on-write is available at all. hz does not use filesystem-specific
   snapshot mechanisms (btrfs subvolumes, ZFS datasets, APFS snapshots), even
   where they would be faster, because each one carries its own privilege
   rules, layout constraints, and failure modes that would leak into the model.
2. **No silent degradation.** A root is registered in exactly one copy mode.
   `hz init` proves that copy-on-write cloning works in that directory before
   registering it; if it does not, `hz init` fails and names `hz init --copy` as
   the explicit alternative. hz never falls back from cloning to byte copying
   on its own.
3. **The hot path touches only the filesystem.** Creating and removing a
   workspace never invokes a source-control subprocess. Hooks are opt-in.
4. **Workspaces work the moment they exist.** The default copy is complete,
   including dependency and build artifacts, so an agent can run tests
   immediately. Filtering is an explicit choice.
5. **Removal is recoverable and constant-time.** `hz rm` renames; `hz gc`
   unlinks.

## Workspace

A workspace is a directory that contains an identity marker, `.hz-workspace`,
holding a ULID. The ULID is the workspace's stable identity: it names the
workspace's physical directory and its registry row, and it survives handle
changes and moves.

Workspaces form a logical tree. A **root** is a directory the user registered
with `hz init`; it is left where it is. A **child** is created by `hz new` from
any active workspace in the family. The registry records each child's
immediate parent, but all descendants of a root share one flat storage
directory adjacent to the root:

```text
~/code/app/                                   root
~/code/.hz-workspaces/<root-handle>-<last-six-root-id-characters>/<id>/     children, any depth
~/code/.hz-workspaces/<root-handle>-<last-six-root-id-characters>/.trash/<id>/
```

Adjacent storage keeps source and destination on the same filesystem, which
cloning requires. `--into DIR` overrides the location; it must also be on the
same filesystem. Registered workspace directories must not physically contain
one another. Initialization, creation, and adoption reject overlapping paths,
including reserved trash locations, so removing one cannot erase another.

Handles (`parser-fix`) are for people and may be reused after removal. IDs are
for the filesystem and the registry and are never reused.

### Registry

One SQLite database per user (`$XDG_DATA_HOME/hz/hz.sqlite` or the platform
equivalent) records every workspace: ID, root ID, parent ID, handle, path,
state, copy mode, pin flag, and timestamps. The marker in the directory and the
row in the registry must agree; a mismatch is reported by `hz doctor` and never
silently repaired, because a path alone cannot prove that the directory now at
that path is the workspace that was registered there.

Lifecycle states are `creating`, `active`, and `trashed`. Unregistering a root
removes its row. Create, remove, and restore record intent before filesystem
changes; the row also retains the trash path during an interrupted restore.
`hz doctor --fix` reconciles these intermediate states. GC first renames trash
to `<id>.deleting` and preserves its marker until the final unlink so interrupted
deletion can resume without guessing ownership. If removal was interrupted
before its rename, GC requires repair first instead of forgetting the directory.

The C++ rewrite does not migrate older registries. An incompatible schema is
rejected without changing its contents. Use the matching older binary to manage
existing workspaces, or select a fresh `HZ_DATA_DIR` and explicitly initialize
roots there.

Lifecycle mutations, configuration writes, Git handoff, and doctor acquire one
exclusive operation lock per registry. An overlapping operation fails with a
retryable conflict instead of acting on stale state. The lock is released on
exit, including crashes; hooks inherit no lock descriptor. Status, listing, and
path queries remain available while hooks run. This deliberately serializes
creates too; concurrency can be refined after measurement.

Recovery covers interrupted processes. It is not a transaction across SQLite
and the filesystem during power loss. Keep the workspace tree stable during
copying, removal, and GC; hz does not supervise processes that still use it.

## Materialization

Creating a child is one operation everywhere: **walk the source tree and clone
each entry**.

```text
for each entry under source (excluding the marker and any filter matches):
  directory     → mkdir, metadata replayed after its contents
  regular file  → clone primitive (or byte copy in copy mode)
  hard link     → recreated as a hard link within the copy, keyed by (dev, ino)
  symlink       → recreated with the same target, never followed
  anything else → error; the copy is abandoned and removed
then replay mode, ownership where permitted, xattrs, ACLs, and timestamps
```

The per-file clone primitive is the only platform-specific part:

| Platform | Clone primitive | Byte copy (`--copy` mode) |
| --- | --- | --- |
| Linux | `ioctl(FICLONE)` | `copy_file_range`, then read/write |
| macOS | `clonefile(2)` with `CLONE_NOFOLLOW` and `CLONE_ACL` | `copyfile(3)` without `COPYFILE_CLONE` |
| Windows (design only) | `FSCTL_DUPLICATE_EXTENTS_TO_FILE` | `CopyFileEx` |

Linux and macOS are implemented; Windows is not implemented or verified.
The corresponding clone primitives work on btrfs, XFS (reflink-enabled, the default since
xfsprogs 5.1), OpenZFS 2.3+ (2.2.x needs `zfs_bclone_enabled=1`), bcachefs,
APFS, and ReFS including Dev Drive. It does not work on ext4, tmpfs, NFS, NTFS,
or FAT; those need `--copy`.

User xattrs are copied before final read-only permissions are restored. Linux
access ACLs are replayed with xattrs; Darwin ACLs are copied separately for
files and directories, including byte-copy mode.

Clones share data blocks with the source until either side writes. On
compressed filesystems (btrfs `compress=zstd`, ZFS `compression=`) the clone
references the compressed extents as they are, so compression neither slows
cloning nor breaks sharing. Per-file compression settings are xattrs
(`btrfs.compression`, `bcachefs.compression`) and are carried by the xattr
replay, not by the clone itself.

Cost is O(entries) in the source tree on every platform. The walker is
single-threaded. Byte copying also scales with data size.

### Why not snapshots

A btrfs subvolume snapshot of the same tree takes about 3 ms. hz still does
not use it, for these reasons:

- It only works if the source is already a subvolume. Converting a live
  repository into one means a staged copy and a rename swap of the user's
  directory, and afterwards the directory can no longer be `mv`ed across the
  filesystem.
- Deleting a subvolume in constant time needs root or the
  `user_subvol_rm_allowed` mount option; without it, deletion is the same
  recursive unlink a plain directory needs.
- Nested subvolumes are excluded from snapshots and appear as empty stubs.
- Snapshots cannot be filtered.
- None of this exists on XFS, APFS, or ReFS, so hz would have two models.

The one property snapshots have that cloning lacks is a point-in-time copy of
`.git`. hz compensates with the source-control checks below.

### Copy modes

Each root is registered as `cow` or `copy` at `hz init`, and all its children
inherit that mode. `hz init` verifies `cow` by cloning a probe file in the root
directory. `hz ls` shows the mode.

Each create is `full` (default) or `filtered`. Filtered creates skip
regenerable artifacts matched by path component at any depth: `node_modules`,
`.pnpm-store`, `target`, `.venv`, `venv`, `.tox`, `.nox`, `__pycache__`,
`.pytest_cache`, `.mypy_cache`, `.ruff_cache`, `.next`, `.nuxt`, `.svelte-kit`,
`.turbo`, `.vite`, `.parcel-cache`, `.cache`, `dist`, `build`, `coverage`, and
`.yarn/{cache,unplugged,install-state.gz,build-state.yml}`. Nothing under a
source-control directory is filtered except a live `fsmonitor--daemon.ipc`
socket, which cannot be copied and would have no daemon behind it. A project
may set `filtered` as its default in `.hz/hz.toml`. `--full` overrides that default.
The Git fsmonitor socket is omitted even in full mode.

The filter matches names, not `.gitignore` rules, so a source directory that
happens to be called `build` or `dist` is also skipped in filtered mode.

## Removal and recovery

`hz rm` moves the selected workspace and its logical subtree into the family's
`.trash` directory with one `rename` per workspace, deepest first, and marks
the rows `trashed`. It does not walk or unlink files. Pinned workspaces refuse
removal. `--children` keeps the selected workspace and trashes its descendants.
Removing a root requires `--force`; the root directory stays in place and only
its marker and registry row are removed. Its trashed descendants remain
available to GC, but cannot be restored without their registered parent.

`hz restore` renames a trashed subtree back and reactivates it, provided its
parent is active and its original handles and paths are free. `hz gc`
recursively unlinks everything in trash; this is the only O(files) removal
step and it is deferred by design. Rename operations require atomic
no-replace support; hz reports an error if the filesystem cannot provide it.
If both trash and deletion directories exist, doctor leaves both for inspection.
Invalid trash markers are reported and never silently accepted.

## Source control

Source-control metadata is copied as ordinary filesystem state. That makes each
child a fully independent repository with the parent's index, staged, unstaged,
untracked, and ignored files intact.

Git-specific behaviour on create:

- **HEAD is detached** in the child by writing the resolved commit SHA to
  `.git/HEAD`. The child is on no branch, so work inside it cannot move or push
  the parent's branch by accident; an agent that wants a branch creates one.
  An unborn HEAD is left as it is. `refs/hz/base` records the creation commit;
  an unborn-base marker records when there was no commit yet. Reftable sources
  are refused because editing their HEAD would require invoking Git.
- **Sources mid-operation are refused.** A walk-and-clone copy is not
  point-in-time, so hz refuses to create from a source whose `.git` contains
  `index.lock`, `HEAD.lock`, `MERGE_HEAD`, `CHERRY_PICK_HEAD`, `REVERT_HEAD`,
  `BISECT_LOG`, `rebase-merge`, or `rebase-apply`. Retry once the operation
  finishes. Top-level Git locks and locks under `refs` are also refused, as is
  a pending cherry-pick/revert sequence. These checks cannot prevent a concurrent
  Git process starting after the check; keep the source quiescent during copying.
- **Linked worktrees are refused as sources.** A linked worktree's `.git` is a
  file pointing into another repository's `worktrees/` entry; copying it would
  make two directories claim the same entry.
- `.hz-workspace` is added to `.git/info/exclude` so the marker never shows as
  untracked.

Mercurial receives the equivalent marker protection via `.hg/hgrc`.

Everything else is explicit: `hz git status`, `hz git handoff`, and `hz hg
status` run source-control commands only when asked. `hz git handoff` applies a
workspace's net changes since its stored creation base to another clean Git
workspace, defaulting to its parent. This includes commits, staged and unstaged
edits, and untracked files. The destination receives uncommitted changes; no
branch is moved. An unborn creation base means the empty tree even after the
child has made its first commit.

## Hooks

`hz config init` explicitly creates `.hz/hz.toml`; `hz init` does not generate
configuration or scripts. `.hz/hz.toml` may define `postcreate` and `preremove` commands as argv arrays.
They are disabled by default so that the default create and remove path spawns
no processes. `postcreate` runs in the new workspace after it is active; its
failure is reported but leaves the workspace active. `preremove` runs before
the rename into trash. `--no-hooks` skips both. Hooks receive `HZ_ROOT`,
`HZ_SOURCE`, `HZ_WORKSPACE`, `HZ_WORKSPACE_ID`, `HZ_PARENT_ID`,
`HZ_HANDLE`, and `HZ_LIFECYCLE`.

## Machine interface

`--machine` forces JSON output, disables shell navigation, and is the interface
for agents and editors. Every command's JSON includes the workspace ID, handle,
path, state, and copy mode where relevant.

## Non-goals

- Filesystem-native snapshots, for the reasons above.
- Running or supervising agents. hz produces directories; what runs in them is
  the caller's concern.
- Replacing source-control commands. hz never commits, branches, or merges on
  its own.
- Cross-filesystem workspaces. Cloning needs one filesystem; `--into` must stay
  on it.
