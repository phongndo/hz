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
max_detached = 15
default_base = "dev"
user_managed_roots = ["~/.codex/worktrees", "../agent-worktrees"]
```

`max_detached` caps managed detached scratch worktrees for the repo. Creating
another detached worktree auto-removes the oldest clean managed detached
worktrees until the cap is satisfied. Set it to `0` to disable auto-pruning.

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
lifecycle for them. The default `~/.hz/worktrees/<repo>/` root is always
included.

Relative roots are resolved from the repository root. `~/` expands to `$HOME`.
Configured roots are not auto-pruned; auto-pruning still only removes clean
registry-managed detached worktrees.

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

## Syntax highlighting

`hz diff` syntax highlighting is configured outside repo-local `.hz/hz.toml`
because extra parser downloads and display preferences are user-local concerns.
Managed parser state is stored in:

```text
~/.config/hz/tree-sitter.json
```

Human-editable user settings are stored in one file:

```text
~/.config/hz/config.toml
```

Custom Base16/Tinted color schemes live next to it:

```text
~/.config/hz/colorscheme/<name>.toml
~/.config/hz/colorscheme/<name>.yaml
```

Common languages are bundled; use these commands for extra languages and cache
diagnostics:

```sh
hz ts add ruby elixir
hz ts update --all
hz ts available --installed
hz ts rm ruby
hz ts ls
hz ts doctor
hz ts clean
```

`hz ts add` writes hz's shipped Tree-sitter parser-release lockfile into the
user cache, verifies the downloaded release bundle against that lock before
loading native parser code, and records the installed parser checksum for later
loads.

The interactive diff viewer only uses bundled parsers or already-enabled,
already-installed Tree-sitter parsers with matching checksum records. It does not
download parsers while rendering. `hz diff --no-syntax` disables syntax for a run.
`hz ts rm` removes a language from the enabled set and deletes its cached parser
library when present; `hz ts clean` purges the parser cache while keeping the
enabled-language config.

User-local settings support language mode, colorscheme, diff styling, and
performance limits:

```toml
# ~/.config/hz/config.toml
mode = "enabled"                  # enabled, builtin, or all
colorscheme = "system"            # built-in name, ansi, or Base16 table
transparent_background = false     # keep diff backgrounds by default

# Optional Ghostty-style overrides layered on the colorscheme.
# With system, omit bg/fg to keep the terminal foreground/background:
# bg = "#111315"
# fg = "#c8d0d8"
# addition_bg = "#1f3025"          # full-line + background
# deletion_bg = "#372526"          # full-line - background
# addition_inline_bg = "#284b32"   # paired word-diff background
# deletion_inline_bg = "#5a3030"   # paired word-diff background
# search_match_fg = "#111315"      # grep match foreground; like a Search highlight group
# search_match_bg = "#e5c07b"      # grep match background; defaults come from the colorscheme

[diff]
line_background = "subtle"         # none, subtle, or strong
gutter_background = "delta"        # base or delta
inline_background = "strong"       # none, subtle, or strong
sign_style = "bold"                # normal or bold
context_expand = 20                # lines per click, or "full"

[limits]
max_source_kib = 1024
max_line_kib = 8
cache_entries = 512
queue_entries = 512
prefetch_viewports = 1
```

Modes:

```text
enabled  core bundled languages plus languages enabled with `hz ts add`
builtin  all bundled parsers that have highlight queries; ignores user cache
all      all bundled parsers plus trusted cached parsers; never downloads
```

Built-in colorschemes are:

```text
system              default, read-only system scheme
default             alias for system
hz-dark             built-in dark diff colorscheme; terminal-dark is an alias
hz-light            built-in light diff colorscheme; terminal-light is an alias
minimal             sparse ANSI-color colorscheme
ansi                use the active terminal ANSI palette
catppuccin-mocha    packaged Catppuccin Mocha-inspired palette
gruvbox-dark        packaged Gruvbox Dark-inspired palette
tokyonight          packaged Tokyo Night-inspired palette
dracula             packaged Dracula-inspired palette
```

The default `system` colorscheme is built in and read-only. It keeps the
terminal foreground/background and ANSI syntax colors, but hz owns the diff
colors: fixed red/green accents and fixed changed-line backgrounds make diffs
readable regardless of the terminal palette. User colorscheme files in
`~/.config/hz/colorscheme/` are only loaded for non-built-in names, so a file
such as `system.toml` cannot replace the default. To customize colors, layer
overrides in `config.toml`, or choose a new colorscheme name and set
`colorscheme = "name"`. `addition_bg` and `deletion_bg` paint the full `+`/`-`
changed lines, while `addition_inline_bg`/`deletion_inline_bg` paint paired
word-level changes. Packaged non-system colorschemes also use subtle blended
red/green changed-line backgrounds and stronger inline word backgrounds. Set
`transparent_background = true` if the diff viewer should avoid setting
add/delete/inline/gutter backgrounds and let the terminal background show
through. The default is `false` so diff backgrounds stay visible.

Color overrides can be top-level, matching Ghostty-style config layering, or in
a `[colors]` table. Top-level values win if both are set:

```toml
colorscheme = "system"

bg = "#111315"
fg = "#c8d0d8"
addition_bg = "#1f3025"
deletion_bg = "#372526"

[colors]
keyword = "#c99bea"
string = "ansi-10"
```

Supported string values are `#rrggbb`, `rrggbb`, `ansi-N`/`ansi:N`, an ANSI
index such as `"10"`, `default`, `reset`, `none`, or named ANSI colors such as
`red`, `bright-green`, and `dark-gray`. Diff override keys include `bg`, `fg`,
`header`, `file`, `hunk`, `notice`, `muted`, `gutter_bg`, `empty_diff`,
`addition_fg`, `addition_gutter_bg`, `addition_bg`, `addition_inline_bg`,
`deletion_fg`, `deletion_gutter_bg`, `deletion_bg`, `deletion_inline_bg`,
`search_match_fg`, and `search_match_bg`.
Syntax override keys include `attribute`, `comment`, `constant`, `constructor`,
`function`, `keyword`, `label`, `module`, `number`, `operator`, `property`,
`punctuation`, `string`, `tag`, `type`, and `variable`.

For Neovim or cross-tool colorscheme compatibility, share a Base16/Tinted scheme
file and point `hz` at it:

```toml
mode = "enabled"

[colorscheme]
source = "base16"
path = "~/.config/tinted-theming/schemes/catppuccin-mocha.yaml"
```

Or put the scheme in `~/.config/hz/colorscheme/mocha.yaml` and reference it by
name:

```toml
colorscheme = "mocha"
```

Base16 loading is local-only and expects a static scheme file with `base00`
through `base0F` colors. `hz` does not execute or parse Neovim colorscheme Lua.
The older `theme` key and `[theme]` table are still accepted as legacy aliases;
new config should use `colorscheme`.

## Lifecycle

```toml
[lifecycle]
setup = [".hz/environment/setup"]
cleanup = [".hz/environment/cleanup"]
```

Lifecycle commands are argv arrays. Relative executable paths are resolved from
the target worktree. `hz new` runs `setup` after creating a worktree, and
`hz rm` runs `cleanup` before removing one.
