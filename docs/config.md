# hz config

`hz` uses repo-local config for workflow defaults that should travel with a
repository.

```text
.hz/
  hz.toml
  environment/
    setup
    cleanup
```

Run `hz init` to create the repo config and environment scripts.

## Precedence

For implemented keys, explicit CLI flags win over repo config:

```text
CLI flag > .hz/hz.toml > built-in default
```

`hz` reads repo config only from `.hz/hz.toml`.

## Worktree

```toml
[worktree]
auto_prune = true
max_detached = 10
max_branch_worktrees = 10
default_base = "dev"
user_managed_roots = ["~/.codex/worktrees", "../agent-worktrees"]
```

`auto_prune` defaults to `true`: creating worktrees deletes the oldest clean
managed worktrees when a configured limit would be exceeded. Set it to `false`
to keep managed worktrees instead of deleting them automatically. Per-command
limit flags still win over repo config.

`max_detached` caps unpinned managed detached scratch worktrees for the repo.
Creating another detached worktree auto-removes the oldest clean unpinned
managed detached worktrees until the cap is satisfied. Set it to `0` to disable
auto-pruning.

`max_branch_worktrees` caps unpinned managed branch-backed worktrees for the
repo. Creating another branch-backed worktree auto-removes the oldest clean
unpinned managed branch-backed worktrees until the cap is satisfied. Only the
checkout is removed; the Git branch remains available to check out later. Set it
to `0` to disable auto-pruning.

Pinned worktrees do not count toward these auto-prune limits. Use
`hz pin <target...>` to make managed worktrees persistent, and
`hz unpin <target...>` to make them eligible for auto-prune again.
`hz ls --pinned` shows pinned worktrees; `hz ls --unpinned` shows unpinned
worktrees.

`default_base` is the branch or revision used when `hz new` is called without
`--base`.

```sh
hz new feature/ui
# behaves like:
hz new feature/ui --base dev
```

Passing `--base` always overrides `default_base`.

`user_managed_roots` adds directories whose Git worktrees at or under them
should be treated as user-managed by `hz`, even when they are not in the
registry. This keeps `hz rm` from prompting for those paths and runs the cleanup
lifecycle for them when `hz rm --cleanup` is used. The default `~/.hz/worktrees/<repo>/` root is always
included.

Relative roots are resolved from the repository root. `~/` expands to `$HOME`.
Configured roots are not auto-pruned; auto-pruning still only removes clean
registry-managed worktrees covered by the configured limits.

### Ignored local files

`hz` also reads an optional root `.worktreeinclude` file when creating managed
worktrees. List ignored local setup files or Gitignore-style patterns that a new
worktree should receive:

```text
# .worktreeinclude
.env
.env.local
config/secrets.json
```

Only files already ignored by Git and matched by `.worktreeinclude` are copied.
Tracked files and other untracked files are not copied. Source symlinks are
skipped, and existing destination files are not overwritten.

## List

```toml
[list]
headers = "auto"
columns = ["marker", "target", "status", "modified", "path"]
compact_columns = ["marker", "target", "status"]
```

`headers` controls the `hz ls` header row:

```text
auto    show headers in normal-width output, hide them in compact output
always  always show headers
never   never show headers
```

`columns` controls normal-width `hz ls` output. `compact_columns` controls
narrow terminal output.

Supported columns:

```text
marker    current/local marker
target    branch when present, otherwise generated handle
branch    Git branch, or -
handle    hz handle, or -
status    clean/dirty/unknown status
base      creation base, or -
modified  last modified time
path      worktree path
```

Examples:

```toml
# Dense agent workflow
[list]
headers = "never"
columns = ["marker", "target", "status"]
compact_columns = ["marker", "target", "status"]
```

```toml
# Branch-heavy workflow
[list]
headers = "always"
columns = ["marker", "branch", "base", "status", "modified", "path"]
```

## Color

```toml
[color]
mode = "auto"
scheme = "terminal"
```

`mode` controls ANSI color:

```text
auto    color only when stdout is a terminal
always  always emit ANSI color
never   never emit ANSI color
```

The default uses terminal-native ANSI colors. That means `hz` follows the
user's terminal color scheme instead of forcing a custom palette.

Custom schemes are opt-in and use ANSI color names:

```toml
[color]
mode = "auto"
scheme = "blueprint"

[color.schemes.blueprint]
header = "cyan"
target = "blue"
branch = "blue"
handle = "magenta"
base = "white"
modified = "white"
path = "white"
clean = "green"
dirty = "yellow"
unknown = "red"
current = "green"
local = "cyan"
```

Supported color names are `black`, `red`, `green`, `yellow`, `blue`,
`magenta`, `cyan`, and `white`. Unknown color names are ignored.

## Diff review

Diff review and syntax highlighting are configured by the separate
[`dx`](https://github.com/phongndo/dx) CLI. Repo-local `.hz/hz.toml` only controls
workspace isolation, list display, color, and lifecycle behavior for `hz`.

## Lifecycle

```toml
[lifecycle]
setup = [".hz/environment/setup"]
cleanup = [".hz/environment/cleanup"]
```

Lifecycle commands are argv arrays. Relative executable paths are resolved from
the target worktree. `hz new --setup` runs `setup` after creating a worktree,
and `hz rm --cleanup` runs `cleanup` before removing one.
