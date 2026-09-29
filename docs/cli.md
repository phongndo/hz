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
       [--label KEY=VALUE]...
```

`init` registers exactly the selected directory, writes its marker, and probes
copy-on-write support. It rejects nested roots. Repeating it on a registered
workspace is idempotent; it does not change the family's copy mode. `--copy`
explicitly selects byte copying for a new root. Configuration creation is a
separate `hz config init` command.

`new` copies the current workspace, or `--from TARGET`, into an independent
child. The handle is generated when omitted. Both full and filtered creation
walk the source tree; full is the default. `--into DIR` must be outside the
source, outside other registered workspaces and trash, and on the same filesystem. `--label`
attaches [labels](#labels). See [design.md](design.md) for the model and
[config.md](config.md) for filtering defaults and hooks.

## Navigation and listing

```sh
hz pwd
hz path|cd [TARGET]
hz ancestors [TARGET]
hz list|ls [--tree] [--all] [--trash] [--label KEY[=VALUE]]...
```

`path` prints the selected directory; shell integration makes `cd` navigate to
it. `ancestors` lists the chain from root to immediate parent. `ls` shows the
current family, or every family when outside a workspace. `--all` always shows
every family; `--trash` includes removed workspaces. `--label` keeps only
workspaces carrying that label, or that key with any value; repeated selectors
must all match.

## Labels

```sh
hz label TARGET [KEY=VALUE]... [--unset KEY]...
```

Labels are key-value pairs that tools attach to workspaces they manage, such
as the ID of the session or task that owns one, so they can find it again with
`hz ls --all --label KEY=VALUE`. hz gives them no meaning. Children do not
inherit them, and they survive removal and restore.

`label` sets and unsets the given labels, then prints every label of the
workspace as `KEY=VALUE` lines; with no changes it only prints them. Keys are
1-128 letters, digits, `.`, `_`, `/`, or `-`, starting with a letter or digit.
Values are at most 4096 bytes of UTF-8 without control characters; the first
`=` separates key from value.

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

Mutations using the same registry take turns and may fail as `busy`; see [concurrency in the design](design.md#registry).

## Machine output

Data commands emit one JSON object on stdout when requested. Every object
carries `"api_version": 1`. Within a version, fields are only added; removing
or renaming a field, changing its type or meaning, or changing an exit status
increments it. Hooks write to stderr. Argument parsing errors and help use
CLI11's ordinary output. Shell-script and completion commands emit text
intended for shells.

Workspace records contain `id`, `handle`, `root_id`, `parent_id` (null for a
root), `path`, `location` (the trash path while trashed), `state`, `mode`,
`filtered`, `pinned`, `root`, `labels`, and `created_at` and `updated_at` in
Unix milliseconds.

A failure prints `{"error": {"kind", "message", "retryable"}}` and exits with
the status of its kind. `message` is for people; match on `kind` or the status.
`retryable` is true when the same request may succeed once another hz process
finishes, without the caller changing anything.

| Status | Kind | Meaning |
| --- | --- | --- |
| 0 | | Success |
| 1 | `internal` | Unexpected failure |
| 2 | | Invalid command line |
| 10 | `io` | An operating-system call failed |
| 11 | `cow_unavailable` | The filesystem cannot clone here |
| 12 | `unsupported_entry` | A socket, FIFO, or device cannot be copied |
| 13 | `invalid_path` | The path cannot be used |
| 14 | `invalid_argument` | A name, flag, or value is not acceptable |
| 15 | `not_found` | No workspace matches |
| 16 | `ambiguous` | More than one workspace matches |
| 17 | `conflict` | The request contradicts current state |
| 18 | `busy` | Another hz process holds the registry or workspace; retryable |
| 19 | `inconsistent` | The registry and filesystem disagree; also `doctor` with unresolved findings |
| 20 | `unsafe_source` | Source control is mid-operation in the source |
| 21 | `hook_failed` | A lifecycle hook failed |
| 22 | `registry` | The registry database failed |

A failed postcreate hook leaves the new workspace active but reports only the
error; a caller that labeled it can find it with `hz ls --label`.
