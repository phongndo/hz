# hz CLI

Workspace lifecycle uses top-level commands. Source-control operations use
namespaces such as `hz git`. Global `--at DIR` changes the workspace context;
`--json` (`-j`) selects JSON and `--machine` also bypasses shell navigation.

Targets accept handles, IDs, unambiguous ID prefixes, or paths. With no target,
commands that allow omission use the workspace containing the current directory.
`root` and `local` select that workspace family's root.

## Registration and creation

```sh
hz init [PATH] [--copy]
hz new [NAME] [--from TARGET] [--into DIR] [--filtered|--full] [--no-hooks]
```

`init` registers exactly the selected directory, writes its marker, and probes
copy-on-write support. It rejects nested roots. Repeating it on a registered
workspace is idempotent; it does not change the family's copy mode. `--copy`
explicitly selects byte copying for a new root. Configuration creation is a
separate `hz config init` command.

`new` copies the current workspace, or `--from TARGET`, into an independent
child. The handle is generated when omitted. Both full and filtered creation
walk the source tree; full is the default. `--into DIR` must be outside the
source, outside other registered workspaces and trash, and on the same filesystem. See [design.md](design.md) for the model and
[config.md](config.md) for filtering defaults and hooks.

## Navigation and listing

```sh
hz pwd
hz path|cd [TARGET]
hz ancestors [TARGET]
hz list|ls [--tree] [--all] [--trash]
```

`path` prints the selected directory; shell integration makes `cd` navigate to
it. `ancestors` lists the chain from root to immediate parent. `ls` shows the
current family, or every family when outside a workspace. `--all` always shows
every family; `--trash` includes removed workspaces.

## Retention and recovery

```sh
hz pin TARGET...
hz unpin TARGET...
hz remove|rm [TARGET] [--children] [--force] [--no-hooks]
hz restore TARGET
hz gc
hz adopt PATH
hz doctor [--fix]
```

Removal renames the selected logical subtree into trash, deepest first.
`--children` preserves the selected workspace. Pins block removal even with
`--force`. Removing a root requires `--force` and removes its marker and registry
row while preserving the root directory; its descendants move to trash.

`restore` reverses a removal, including descendants removed in that operation.
A child's parent must still be registered and active, and its old handle and
path must be free. For a reused trash handle, the current family's most recently
removed match wins; use a full ID to choose another instance. Descendants of an
unregistered root cannot be restored through this command.

`gc` permanently deletes all registered trash across families. It is the only
recursive removal step. `adopt` records a manual move of an active workspace
whose marker still identifies it, rejecting paths that overlap other workspaces. `doctor --fix` repairs interrupted operations
where filesystem evidence proves the action safe. Unresolved findings return a
nonzero status. Missing active markers are reported, not recreated; an explicit
`hz init PATH` can restore a root marker after the directory is verified.

## Source control

```sh
hz git status [TARGET]
hz git handoff [TARGET] [--from SOURCE] [--3way]
hz hg status [TARGET]
```

These commands explicitly invoke Git or Mercurial. Status reports the selected
workspace. Handoff applies the source's net changes since creation, including
commits, staged/unstaged edits, and untracked files, to a clean destination. The
default destination is its immediate parent. It leaves changes uncommitted and
does not move the destination's branch. `--3way` allows Git's three-way fallback,
which can stage the applied changes. A failed preflight leaves the destination
unchanged; review the destination after a successful handoff.

## Configuration and shell integration

```sh
hz config init [TARGET]
hz install zsh|bash|fish
hz shell zsh|bash|fish
```

`install` adds the integration to the shell startup file once. `shell` prints
it for explicit evaluation. With integration, `init`, `new`, `cd`, `restore`,
and `git handoff` navigate to the returned directory. `rm` navigates to a
surviving ancestor only when the shell was inside a removed workspace.

`--json`, `--machine`, `--path-only`, and help calls never navigate. `init`,
`new`, `rm`, `restore`, and `git handoff` accept `--path-only`; an empty removal
path means the shell should stay where it is.

Overlapping mutations using the same registry return a conflict; retry after
the other operation completes. Hooks cannot start another mutation while their
parent operation holds the lock.

## Machine output

Data commands emit JSON when requested, with structured workspace records and
structured operation errors. Hooks write to stderr. Argument parsing errors
and help use CLI11's ordinary output. Shell-script and completion commands emit
text intended for shells.
