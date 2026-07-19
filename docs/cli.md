# hz CLI

Workspace lifecycle is source-control-neutral and uses top-level commands.
Source-control-specific operations use namespaces such as `hz git`.

## Workspace lifecycle

### `hz init [PATH] [--here] [--copy]`

Registers a root, writes `.hz-workspace`, validates native copy-on-write support,
and creates `.hz/` lifecycle configuration. Without `--here`, Hz selects an
existing managed ancestor; otherwise it initializes the requested directory.
`--copy` explicitly selects portable byte-copy materialization for filesystems
without native COW support.

### `hz new [NAME]`

Creates a child snapshot of the nearest managed workspace.

```sh
hz new parser-fix
hz new --from ~/code/app
hz new parser-fix --into /same-filesystem/workspaces
hz new parser-fix --filtered
hz new parser-fix --no-hooks
```

Default creation takes a complete COW snapshot for the lowest creation latency
and shares existing data blocks. `--filtered` omits known regenerable dependency,
build, and cache artifacts at the cost of walking the source tree.

### Navigation

```sh
hz pwd
hz path parser-fix
hz cd parser-fix
hz path root
hz ancestors
hz ls
hz ls --tree
hz ls --children
hz ls --roots
```

`cd` is a shell-integrated alias of `path`. `root` and `local` select the current
family root.

### Retention

```sh
hz pin parser-fix
hz unpin parser-fix
hz rm parser-fix
hz rm parser-fix --children
hz rm --force                 # unregister current root
hz restore parser-fix
hz gc
hz adopt /new/path/to/workspace
hz doctor --fix
```

Removal uses same-filesystem renames into trash and does not walk workspace
files. `hz gc` performs the slower physical unlinking later. On Windows, the
calling shell must change outside a workspace before removing that workspace so
its directory handle is not locked. `--children`
preserves the selected workspace. Root unregistration requires `--force` and
preserves the root directory.

## Git

```sh
hz git status [TARGET]
hz git handoff [TARGET]
```

Status is evaluated only when explicitly requested. Handoff applies the current
workspace patch to a clean destination and defaults to the immediate parent.
Workspace creation does not invoke Git; it only ensures the repository-local
exclude rules protect the `.hz-workspace` identity marker.

## Mercurial

```sh
hz hg status [TARGET]
```

Status is evaluated only when explicitly requested. Workspace creation does not
invoke Mercurial; it adds a repository-local ignore for the `.hz-workspace`
identity marker and otherwise treats `.hg` as ordinary filesystem state.

## Configuration and utilities

```sh
hz config init [PATH]
hz install zsh|bash|fish
hz shell zsh|bash|fish
hz update
```

## Machine output

Data-producing commands accept `--json`. Global `--machine` forces JSON and
prevents shell wrappers from changing directory.
