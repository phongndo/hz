# hz

[![Quality](https://github.com/phongndo/hz/actions/workflows/quality.yml/badge.svg)](https://github.com/phongndo/hz/actions/workflows/quality.yml)
[![Release](https://github.com/phongndo/hz/actions/workflows/release.yml/badge.svg)](https://github.com/phongndo/hz/actions/workflows/release.yml)

`hz` is a fast terminal workflow for parallel development with agents. It
creates isolated Git workspaces, makes it fast to move between them, hands off
diffs safely, and runs repo-local lifecycle hooks.

Diff review has moved to [`dx`](https://github.com/phongndo/dx) so workspace
isolation and diff review can evolve as separate tools.

## Status

`hz` is pre-1.0. The current release is usable for local Git worktree, handoff,
lifecycle, shell integration, and install/update workflows. Prefer `--json` for
scripts, and expect command shapes to keep tightening before 1.0.

See [docs/roadmap.md](docs/roadmap.md) for the product direction.

## What hz does today

- Creates isolated Git worktrees for parallel human or AI-agent tasks.
- Forks the current worktree state into a detached scratch worktree.
- Installs shell integration so `hz new`, `hz fork`, `hz cd`, and `hz handoff`
  can change your current directory.
- Applies uncommitted changes between linked worktrees without forcing a commit.
- Moves branch ownership between worktrees when both sides are clean.
- Runs repo-local setup and cleanup hooks on explicit opt-in commands for reproducible agent workspaces.
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

# review with dx if desired, then clean up
dx
hz rm -f fix-login    # force is needed if the source worktree still has handed-off changes
```

Without shell integration, use `hz path <target>` to print a worktree path and
`cd "$(hz path <target>)"` from your shell.

`hz init` writes repo-local lifecycle files under `.hz/`. Commit them before
starting the task flow above so the destination worktree is clean when
`hz handoff` applies changes back to it. For a throwaway first demo where you do
not need lifecycle hooks, skip `hz init`.

## Installation

Install the latest release with the shell installer:

```sh
curl -fsSL https://raw.githubusercontent.com/phongndo/hz/main/scripts/install.sh | sh
```

The curl installer is the only supported install path for now. Homebrew, mise,
Cargo, and other package-manager installs are deprecated; reinstall with the
command above if you used one of those paths before.

The installer downloads the matching GitHub release archive for macOS or Linux,
verifies its SHA-256 file with `shasum` or `sha256sum`, and installs `hz` to
`~/.local/bin` by default. Set `HZ_ALLOW_UNVERIFIED=1` only when you explicitly
want to install without checksum verification.

```sh
curl -fsSL https://raw.githubusercontent.com/phongndo/hz/main/scripts/install.sh | HZ_VERSION=0.5.0 sh
curl -fsSL https://raw.githubusercontent.com/phongndo/hz/main/scripts/install.sh | HZ_INSTALL_DIR=/usr/local/bin sh
```

Verify the installed binary:

```sh
which -a hz
hz --version
```

The default curl install path is `~/.local/bin/hz`; if you set
`HZ_INSTALL_DIR`, the first `hz` on `PATH` should come from that directory.

Update a curl-installed binary in place:

```sh
hz update
hz update --target-version 0.5.0
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
hz fork
hz new
hz ls
hz pwd
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

`hz pwd` prints the current worktree target (`local`, a branch name, or a
detached handle). Pass `--json` to include the target, repo, and path.

`hz fork` creates a new detached worktree at the current `HEAD` and applies the
current worktree diff there, including untracked files. The source worktree is
left unchanged. Pass an optional handle to name the detached fork, or
`--no-diff` to fork only the current `HEAD` without copying local changes:

```sh
hz fork
hz fork alt-try
hz fork --no-diff
```

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

Auto-pruning is enabled by default. Set `[worktree].auto_prune = false` to opt
into keeping managed worktrees instead of deleting them at the configured
limits. Dirty and current worktrees are still protected from auto-removal.

Repo config can set the default base branch for new worktrees and additional
user-managed worktree roots:

```toml
# .hz/hz.toml
[worktree]
auto_prune = true
max_detached = 15
max_branch_worktrees = 15
default_base = "dev"
user_managed_roots = ["~/.codex/worktrees"]
```

With that config, `hz new feature/ui` behaves like
`hz new feature/ui --base dev`. Passing `--base main` still wins for deliberate
main-based work.

Ignored local files are not present in a fresh Git worktree. To copy selected
ignored setup files into new managed `hz` worktrees, add a root
`.worktreeinclude` file using Gitignore-style patterns:

```text
# .worktreeinclude
.env
.env.local
config/secrets.json
```

`hz` copies only ignored files that match `.worktreeinclude`; tracked files and
other untracked files are left alone. Source symlinks are skipped, and existing
destination files are never overwritten.

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
after install. With the integration loaded, plain `hz new ...` or `hz fork ...`
creates the worktree and changes into it, and `hz cd` returns to the local repo
root.
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

Diff review now lives in the separate [`dx`](https://github.com/phongndo/dx) CLI.
Use `dx` from any `hz` worktree when you want the terminal review UI or plain
diff output:

```sh
dx
dx --staged
dx --base main
dx --pr 123
dx --patch changes.diff
```

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
worktree should run. `hz new --setup` runs the configured setup command after
creating a worktree, and `hz rm --cleanup` runs the configured cleanup command
before removing one. Use `--no-setup` or `--no-cleanup` to explicitly suppress a
hook when using aliases or wrapper scripts. Use `hz setup [target]` or
`hz cleanup [target]` to run a hook manually. Hook stdout
is forwarded to stderr so `--json` and `--path-only` output stays
machine-readable.

Lifecycle config is read from the target worktree. Commit `.hz/hz.toml` and any
referenced scripts before opting in to `hz new --setup` for newly created
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
hz-bench     headless command benchmark utilities
hz-command   command facade shared by CLI and future runtime callers
hz-core      shared errors and common models
hz-git       low-level Git integration boundary
hz-worktree  worktree domain boundary: new, path, list, handoff, remove
```

The command crate is the main extension seam. CLI and future runtime code should
call `hz-command` instead of duplicating workflow logic. Provider or agent-runtime
integrations can sit beside these crates without forcing the existing command
surface to depend on plugin machinery.

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
Clippy, workspace tests, a full workspace build, a headless `hz-bench` smoke,
and installer/update smoke tests against a local release fixture.
`.github/workflows/release.yml` builds release archives with SHA-256 checksum
files for supported macOS and Linux targets when a `v*.*.*` tag is pushed.

## License

MIT. See [LICENSE](LICENSE).
