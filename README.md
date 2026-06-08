# hz

[![Quality](https://github.com/phongndo/hz/actions/workflows/quality.yml/badge.svg)](https://github.com/phongndo/hz/actions/workflows/quality.yml)
[![Release](https://github.com/phongndo/hz/actions/workflows/release.yml/badge.svg)](https://github.com/phongndo/hz/actions/workflows/release.yml)

`hz` is a fast terminal workflow for parallel development with agents. It
creates isolated workspaces, makes it fast to move between them, hands off
diffs safely, runs repo-local lifecycle hooks, and gives you a keyboard-first
review loop without leaving the terminal.

The long-term vision is an all-in-one, provider-agnostic workflow for
AI-assisted development: create isolated workspaces, run one or many agents,
track their output, review changes, merge or hand off the useful work, and clean
up safely from the terminal.

## Status

`hz` is pre-1.0. The current release is usable for local Git worktree, handoff,
lifecycle, shell integration, install/update, and diff review workflows. Built-
in agent execution, provider adapters, and the broader dashboard/runtime are on
the roadmap, not shipped yet. Prefer `--json` for scripts, and expect command
shapes to keep tightening before 1.0.

See [docs/roadmap.md](docs/roadmap.md) for the product direction.

## What hz does today

- Creates isolated Git worktrees for parallel human or AI-agent tasks.
- Installs shell integration so `hz new`, `hz cd`, and `hz handoff` can change
  your current directory.
- Applies uncommitted changes between linked worktrees without forcing a commit.
- Moves branch ownership between worktrees when both sides are clean.
- Runs repo-local setup and cleanup hooks for reproducible agent workspaces.
- Reviews worktree or patch diffs in a terminal UI, with plain output for pipes.
- Lists, finds, prunes, and removes managed and unmanaged worktrees.
- Installs and updates release binaries from GitHub releases.

## Quickstart

```sh
# one-time repo setup
hz init
git add .hz
git commit -m "Add hz lifecycle config"
hz install zsh      # or bash/fish; restart or source your shell rc file

# create an isolated workspace for a task/agent and cd into it
hz new fix-login

# run your terminal AI agent or manual workflow here
# ...edit files...

# hand the diff back to the linked local worktree and return there
hz handoff

# review and clean up
hz diff
hz rm -f fix-login    # force is needed if the source worktree still has handed-off changes
```

Without shell integration, use `hz path <target>` to print a worktree path and
`cd "$(hz path <target>)"` from your shell.

`hz init` writes repo-local lifecycle files under `.hz/`. Commit them before
starting the task flow above so the destination worktree is clean when
`hz handoff` applies changes back to it. For a throwaway first demo where you do
not need lifecycle hooks, skip `hz init`.

## Installation

Install the latest release with Homebrew:

```sh
brew install phongndo/tap/hz-cli
```

Or use the shell installer:

```sh
curl -fsSL https://raw.githubusercontent.com/phongndo/hz/main/scripts/install.sh | sh
```

The installer downloads the matching GitHub release archive for macOS or Linux,
verifies its SHA-256 file with `shasum` or `sha256sum`, and installs `hz` to
`~/.local/bin` by default. Set `HZ_ALLOW_UNVERIFIED=1` only when you explicitly
want to install without checksum verification.

```sh
curl -fsSL https://raw.githubusercontent.com/phongndo/hz/main/scripts/install.sh | HZ_VERSION=0.1.10 sh
curl -fsSL https://raw.githubusercontent.com/phongndo/hz/main/scripts/install.sh | HZ_INSTALL_DIR=/usr/local/bin sh
```

Verify the installed binary:

```sh
which -a hz
hz --version
```

The default curl install path is `~/.local/bin/hz`; if you set
`HZ_INSTALL_DIR`, the first `hz` on `PATH` should come from that directory.

Update an installer-managed binary in place:

```sh
hz update
hz update --target-version 0.1.10
```

`hz update` refuses to infer common package-manager-managed locations (for
example Homebrew, mise, Cargo, or Nix paths). Update those installs with their
package manager instead, pass `--install-dir` for an installer-managed target, or
use `--force-self-update` if you intentionally want to overwrite the detected
binary.

With mise, use the GitHub backend until `hz` has a mise registry shorthand:

```sh
mise use -g github:phongndo/hz@latest
hz --version
```

With Cargo, install the package `hz-cli`; it provides the `hz` binary:

```sh
cargo install --locked --git https://github.com/phongndo/hz --tag v0.1.10 hz-cli
```

After installing the binary, install shell integration for auto-cd and
completions:

```sh
hz install zsh
source ~/.zshrc
```

## Core workflows

### Worktrees

`hz new` creates a scratch Git worktree with a human-facing handle and a UUID
directory under `~/.hz/worktrees/<repo>/` by default. Pass a name or `--branch`
to create a branch-backed worktree:

```sh
hz new fix-login
hz new
hz ls
hz path fix-login
hz cd fix-login
hz setup fix-login
hz cleanup fix-login
hz cd
hz rm fix-login
```

`hz new` without a name generates a four-character lowercase alphanumeric handle
and leaves the worktree on a detached `HEAD`. Managed worktrees are registered
in `~/.hz/registry.json`.
`hz ls`, `hz cd`, and `hz rm` also detect unmanaged Git worktrees created by
other tools. Removing an unmanaged worktree outside `~/.hz/worktrees/<repo>/`
asks for confirmation because the path is not in `hz`'s worktree namespace. Add
`[worktree].user_managed_roots` in `.hz/hz.toml` for other directories that
should be treated as user-managed by `hz`.

Detached scratch worktrees are capped at 15 by default. Creating another
detached worktree auto-removes the oldest clean managed detached worktrees until
the cap is satisfied. Branch-backed, unmanaged, dirty, unknown, and current
worktrees are not auto-removed. If there are not enough removable worktrees,
`hz new` and `hz handoff --new` refuse to create another detached worktree. Set
`[worktree].max_detached` in `.hz/hz.toml`, or pass `--max-detached <count>` to
`hz new` or `hz handoff --new`; `0` disables auto-pruning.

Branch-backed worktrees are also capped at 15 by default. Creating another
branch-backed worktree auto-removes the oldest clean managed branch-backed
worktrees until the cap is satisfied. Removing a branch-backed worktree removes
only the checkout; the Git branch remains in the repo and can be checked out
again later. Detached, unmanaged, dirty, unknown, and current worktrees are not
auto-removed. Set `[worktree].max_branch_worktrees` in `.hz/hz.toml`, or pass
`--max-branch-worktrees <count>` to `hz new` or branch-backed
`hz handoff --new`; `0` disables auto-pruning.

Repo config can set the default base branch for new worktrees and additional
user-managed worktree roots:

```toml
# .hz/hz.toml
[worktree]
max_detached = 15
max_branch_worktrees = 15
default_base = "dev"
user_managed_roots = ["~/.codex/worktrees"]
```

With that config, `hz new feature/ui` behaves like
`hz new feature/ui --base dev`. Passing `--base main` still wins for deliberate
main-based work.

### Shell integration

`hz cd` prints a path for scripts. To make `hz new` and `hz cd` change the
current shell directory, run `hz install <shell>` once to update your shell rc
file:

```sh
hz install zsh
hz install bash
hz install fish
```

For zsh, this updates `~/.zshrc`. Restart your shell or run `source ~/.zshrc`
after install. With the integration loaded, plain `hz new ...` creates the
worktree and changes into it, and `hz cd` returns to the local repo root.
`--json`, `--path-only`, and help calls still pass through to the real binary
without changing directories.

The shell integration also installs completions for zsh, bash, and fish.
Completions include command aliases such as `hz cd`, `hz ls`, and `hz rm`,
nested `hz worktree ...` commands, command flags, shell names for
`hz init`/`hz shell`, and live worktree targets for commands that accept them.

### Handoff

`hz handoff` applies the current worktree's uncommitted diff to its linked
counterpart by default. From a linked worktree it applies the patch to `local`.
From `local`, pass a worktree handle such as `hz handoff fix-login` to apply the
local patch there. After a patch handoff, running `hz handoff` from `local`
defaults back to the last linked worktree. The destination must be clean unless
its current diff still matches the last patch handed off between that pair; in
that case `hz` safely replaces it with the source diff. Source changes are left
in place. With shell integration loaded, successful handoffs change into the
destination worktree unless `--json`, `--path-only`, or help is passed.

Use `hz handoff --new` to create a new detached destination worktree and apply
the current patch there. Use `hz handoff --new fix-login` to create a
branch-backed destination worktree named `fix-login`.

Use `hz handoff <worktree> --branch` to move branch ownership instead of
applying a patch. Branch handoff is clean-only on both sides.

### Diff review

`hz diff` opens a read-only terminal diff viewer when stdout is an interactive
terminal. It falls back to plain patch output for pipes, and `--stat` prints a
summary without opening the viewer.

```sh
hz diff
hz diff --staged
hz diff --unstaged
hz diff --no-untracked
hz diff --base main
hz diff main feature
hz diff --pr 123
hz diff --pr https://github.com/owner/repo/pull/123
hz diff --patch changes.diff
cat changes.diff | hz diff --patch -
hz diff --no-watch
hz diff --no-syntax
hz diff --stat
hz ts add ruby elixir
hz ts update --all
hz ts available --installed
hz ts rm ruby
```

The default view is all working tree changes against `HEAD`, including
untracked files. `hz diff --pr <number>` reviews a pull request from the current
repository's `origin` GitHub remote. `hz diff --pr <url>` reviews any GitHub pull
request URL without requiring a local repository. Positional revisions remain
literal, so refs named `pr` can still be compared normally. Patch mode reads an
existing unified diff from a file or stdin without requiring a Git repository.
The viewer uses split mode on wide terminals and unified mode on narrower
terminals, and switches as the terminal is resized.
GitHub PR fetching uses `curl`; set `GH_TOKEN` or `GITHUB_TOKEN` for private
repositories or higher GitHub rate limits.
Working tree diffs live-reload as files or Git state change; use `--no-watch` to
disable filesystem watching. Use `b` to toggle the file sidebar, drag its
divider to resize it, `s` to toggle split/unified, `j/k` to scroll, `h/l` or
`←/→` to scroll long lines horizontally, `n/p` for files, `]/[` for hunks, `f`
to filter files, `/` to grep changed diff content, `r` to reload, `?` to show
keybindings, and `q` to quit. When a grep filter is active, `n`/`p` move between
grep matches. Active filters stay visible in the bottom bar. In filter prompts,
`Enter` closes the prompt and keeps the filter, `Esc` clears active filters, and
`Ctrl-U` clears the current input.

Syntax highlighting is Tree-sitter based. Common languages are bundled for
zero-config highlighting; `hz ts add <language>` installs extra languages.
`hz ts add` seeds a shipped parser-release lockfile, verifies the downloaded
release bundle against that lock before loading, and records the resulting
parser checksum. `hz diff` never downloads parsers while rendering and verifies
recorded parser checksums before loading user-cache parser libraries. Use
`--no-syntax` to force plain diff text, and `hz ts rm <language>`, `hz ts ls`,
`hz ts doctor`, or `hz ts clean` to maintain the parser cache. `hz ts update
<language>` refreshes cached parsers, and `hz ts available --installed` /
`--enabled` filters the
language list. Repo-backed diffs highlight full old/new file sides and map spans
back to diff lines; patch input and unavailable file contents fall back to
hunk-local highlighting. Highlighting stays lazy and cached, and falls back to
plain diff text for missing languages, missing queries, or very large
hunks/lines.

Syntax mode, diff styling, colorscheme, and performance limits are user-local in
`~/.config/hz/config.toml`; repo `.hz/hz.toml` does not control parser loading.
The default colorscheme is the built-in, read-only `system` scheme: terminal
foreground/background and ANSI syntax colors stay system-driven, while hz owns
the hunk-style diff red/green accents and changed-line backgrounds. Put
Base16/Tinted scheme files in
`~/.config/hz/colorscheme/` and set `colorscheme = "name"` for cross-tool
compatibility with editors such as Neovim. Built-in colorscheme names are
resolved before user files, so custom files cannot replace the default `system`
scheme; layer `bg`, `addition_bg`, `deletion_bg`, and related overrides in
`config.toml`, or use a new colorscheme name for a full custom scheme.
Built-in diff colorschemes include
`hz-dark`, `catppuccin-mocha`, `gruvbox-dark`, `tokyonight`, and `dracula`. Set
`transparent_background = true` to let the terminal background show through diff
and inline backgrounds for non-system colorschemes. `hz ts path` prints the cache,
registry, user config, and colorscheme paths.

### Repo lifecycle

`hz init` initializes repo-local lifecycle config:

```text
.hz/hz.toml
.hz/environment/setup
.hz/environment/cleanup
```

`.hz/hz.toml` declares the commands `hz` should run:

```toml
[worktree]
max_detached = 15
max_branch_worktrees = 15
# user_managed_roots = ["~/.codex/worktrees"]

[lifecycle]
setup = [".hz/environment/setup"]
cleanup = [".hz/environment/cleanup"]
```

`.hz/environment/setup` and `.hz/environment/cleanup` are executable script
files. Edit them to contain the repo setup and cleanup commands an agent
worktree should run. `hz new` runs the configured setup command after creating a
worktree, and `hz rm` runs the configured cleanup command before removing one.
Use `--no-setup` or `--no-cleanup` to bypass a hook for one command. Use
`hz setup [target]` or `hz cleanup [target]` to run a hook manually. Hook stdout
is forwarded to stderr so `--json` and `--path-only` output stays
machine-readable.

Lifecycle config is read from the target worktree. Commit `.hz/hz.toml` and any
referenced scripts before relying on `hz new` to run setup in newly created
worktrees.

### Config and display

`hz ls` display is configurable from `.hz/hz.toml`:

```toml
[list]
headers = "auto" # auto | always | never
columns = ["marker", "target", "status", "modified", "path"]
compact_columns = ["marker", "target", "status"]

[color]
mode = "auto" # auto | always | never
scheme = "terminal"
```

Columns can include `marker`, `target`, `branch`, `handle`, `status`, `base`,
`modified`, and `path`. Color defaults to terminal-native ANSI colors so the
user's terminal color scheme decides the actual palette. Custom `hz ls` color
schemes can opt into different ANSI color names while still letting the terminal
theme provide the actual colors.

See [docs/config.md](docs/config.md) for the full config reference.

For compatibility, `hz init <shell>` still installs shell integration, but
`hz install <shell>` is the documented command for shell setup.

## Architecture

```text
hz-cli       command parsing, terminal output, install/update, and CLI UX
hz-command   command facade shared by CLI and future TUI/runtime callers
hz-core      shared errors and common models
hz-git       low-level Git integration boundary
hz-worktree  worktree domain boundary: new, path, list, handoff, remove
hz-diff      diff loading and rendering boundary
hz-syntax    tree-sitter syntax highlighting and parser cache management
hz-tui       ratatui/crossterm diff review UI boundary
hz-bench     local benchmark fixture generation
```

The command crate is the main extension seam. CLI, TUI, and future runtime code
should call `hz-command` instead of duplicating workflow logic. Provider or
agent-runtime integrations can sit beside these crates without forcing the
existing command surface to depend on plugin machinery.

## Development

Install the repo Rust toolchain:

```sh
rustup show
```

Or enter the Nix development shell:

```sh
nix develop
```

Run local checks:

```sh
just setup
just check
just build
just smoke
```

For contribution guidelines, dev-shell details, and CI expectations, see
[CONTRIBUTING.md](CONTRIBUTING.md).

## CI and releases

`.github/workflows/quality.yml` runs rust-analyzer diagnostics, formatter,
Clippy, workspace tests, a full workspace build, and installer/update smoke
tests against a local release fixture.
`.github/workflows/release.yml` builds release archives with SHA-256 checksum
files for supported macOS and Linux targets when a `v*.*.*` tag is pushed.

## License

MIT. See [LICENSE](LICENSE).
