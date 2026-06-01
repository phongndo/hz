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
default_base = "dev"
```

`default_base` is the branch or revision used when `hz new` is called without
`--base`.

```sh
hz new feature/ui
# behaves like:
hz new feature/ui --base dev
```

Passing `--base` always overrides `default_base`.

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

## Lifecycle

```toml
[lifecycle]
setup = [".hz/environment/setup"]
cleanup = [".hz/environment/cleanup"]
```

Lifecycle commands are argv arrays. Relative executable paths are resolved from
the target worktree. `hz new` runs `setup` after creating a worktree, and
`hz rm` runs `cleanup` before removing one.
