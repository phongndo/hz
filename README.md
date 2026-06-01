# hz

`hz` is planned as a headless-first CLI for coordinating parallel agent work
across Git worktrees. It currently ships the core worktree, handoff, lifecycle,
shell integration, install, and update flows for the headless CLI while leaving
room for a future TUI/runtime layer.

## Crates

```text
hz-cli       command parsing and CLI argument shape
hz-command   command facade shared by CLI and future TUI/runtime callers
hz-core      shared errors and common models
hz-git       low-level git integration boundary
hz-worktree  worktree domain boundary: new, path, handoff
hz-diff      diff domain boundary
hz-tui       ratatui/crossterm UI boundary
```

The command crate is the main extension seam for now. CLI and TUI code should
call `hz-command` instead of duplicating workflow logic. If agent providers or a
runtime become necessary later, they can sit beside these crates without forcing
the existing command surface to depend on plugin machinery.

Some command handlers still intentionally return `not implemented yet` until
their real behavior is added.

## Installation

Install the latest release with the shell installer:

```sh
curl -fsSL https://raw.githubusercontent.com/phongndo/hz/main/scripts/install.sh | sh
```

The installer downloads the matching GitHub release archive for macOS or Linux,
verifies its SHA-256 file with `shasum` or `sha256sum`, and installs `hz` to
`~/.local/bin` by default. Set `HZ_ALLOW_UNVERIFIED=1` only when you explicitly
want to install without checksum verification.

```sh
curl -fsSL https://raw.githubusercontent.com/phongndo/hz/main/scripts/install.sh | HZ_VERSION=0.1.2 sh
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
hz update --target-version 0.1.2
```

With mise, use the GitHub backend until `hz` has a mise registry shorthand:

```sh
mise use -g github:phongndo/hz@latest
hz --version
```

With Cargo, install the package `hz-cli`; it provides the `hz` binary:

```sh
cargo install --locked --git https://github.com/phongndo/hz --tag v0.1.2 hz-cli
```

After installing the binary, install shell integration for auto-cd and
completions:

```sh
hz install zsh
source ~/.zshrc
```

## Worktrees

`hz new` creates a detached scratch Git worktree with a human-facing handle and
a UUID directory under `~/.hz/worktrees/<repo>/` by default. Pass a name or
`--branch` to create a branch-backed worktree:

```sh
hz new fix-login
hz new
hz ls
hz path fix-login
hz cd fix-login
hz handoff
hz setup fix-login
hz cleanup fix-login
hz cd
hz rm fix-login
```

`hz new` without a name generates a four-character lowercase alphanumeric handle
and leaves the worktree on a detached HEAD. Managed worktrees are registered in
`~/.config/hz/registry.json` or
`$XDG_CONFIG_HOME/hz/registry.json`. `hz ls`, `hz cd`, and `hz rm` also detect
unmanaged Git worktrees created by other tools. Removing an unmanaged worktree
asks for confirmation because the path is not managed by hz. `hz ls` includes
the local worktree and marks the current worktree. Interactive terminals use
Unicode markers such as `●` and `⌂`; non-terminal output and `HZ_ASCII=1 hz ls`
use ASCII fallbacks such as `@` and `~`.

Detached scratch worktrees are capped at 15 by default. Creating another
detached worktree auto-removes the oldest clean managed detached worktrees until
the cap is satisfied. Branch-backed, unmanaged, dirty, unknown, and current
worktrees are not auto-removed. If there are not enough removable worktrees,
`hz new` and `hz handoff --new` refuse to create another detached worktree. Set
`[worktree].max_detached` in `.hz/hz.toml`, or pass `--max-detached <count>` to
`hz new` or `hz handoff --new`; `0` disables auto-pruning.

Repo config can set the default base branch for new worktrees:

```toml
# .hz/hz.toml
[worktree]
max_detached = 15
default_base = "dev"
```

With that config, `hz new feature/ui` behaves like
`hz new feature/ui --base dev`. Passing `--base main` still wins for deliberate
main-based work.

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

`hz handoff` applies the current worktree's uncommitted diff to its linked
counterpart by default. From a linked worktree it applies the patch to `local`.
From `local`, pass a worktree handle such as `hz handoff fix-login` to apply the
local patch there. After a patch handoff, running `hz handoff` from `local`
defaults back to the last linked worktree. The destination must be clean unless
its current diff still matches the last patch handed off between that pair; in
that case hz safely replaces it with the source diff. Source changes are left in
place. With shell integration loaded, successful handoffs change into the
destination worktree unless `--json`, `--path-only`, or help is passed.

Use `hz handoff --new` to create a new detached destination worktree and apply
the current patch there. Use `hz handoff --new fix-login` to create a
branch-backed destination worktree named `fix-login`.

Use `hz handoff <worktree> --branch` to move branch ownership instead
of applying a patch. Branch handoff is clean-only on both sides.

## Repo lifecycle

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

## Development

Install the repo Rust toolchain:

```sh
rustup show
```

Or enter the Nix development shell:

```sh
nix develop
```

Inside interactive `nix develop`, the shell enters zsh with a repo-local
`ZDOTDIR` under `target/dev-zdotdir`, so user shell aliases, functions, and PATH
rewrites do not override the dev environment. `hz` resolves through the
repo-local `target/dev-bin/hz` shim before any user-installed binary on `PATH`.
The shim builds `hz-cli` on first use when `target/debug/hz` is missing, then
runs the local development binary. It does not fall back to `~/.local/bin/hz` or
another installed `hz`. The dev zsh rc file also loads `hz shell zsh`, so auto-cd
and completion behavior use the local binary by default.

Verify the active binary:

```sh
type -a hz
whence -p hz
hz --version
```

Run the local checks:

```sh
just setup
just check
just build
just smoke
just hz --help
hz --help
```

`just hz ...` is useful for commands that print output, but it cannot change the
current shell directory. Use plain `hz new` or `hz cd` inside interactive
`nix develop` to exercise auto-cd behavior from the development binary without
editing your shell rc file.

Use `hz install zsh` only when you want to update your real shell rc file for an
installed `hz` binary. `just smoke-zsh` verifies the zsh integration in an
isolated shell, including branch names such as `fix(scope)/name` that zsh would
otherwise treat as globs. `just smoke-curl-install` downloads the published
installer with curl, installs into a temporary directory, and runs the installed
binary.

Bash cannot run unquoted branch names containing parentheses, such as
`fix(scope)/name`, because bash parses `(` as syntax before `hz` can receive the
argument. Quote those names in bash or use the zsh integration.

```sh
cargo check --workspace --all-targets --all-features --locked
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-targets --all-features --locked
cargo build --workspace --all-targets --all-features --locked
rust-analyzer diagnostics .
```

## CI

`.github/workflows/quality.yml` runs rust-analyzer diagnostics, formatter,
Clippy, workspace tests, and a full workspace build.

`.github/workflows/pr-template.yml` requires pull requests to keep the template
sections, including Motivation, and mark at least one verification command plus
the CodeRabbit and Greptile review checklist items. `.coderabbit.yaml` and
`.greptile/` configure repository-specific AI review behavior when those GitHub
apps are installed.

The same quality gate is also available through Nix:

```sh
nix develop -c cargo check --workspace --all-targets --all-features --locked
nix develop -c cargo fmt --all --check
nix develop -c cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
nix develop -c cargo test --workspace --all-targets --all-features --locked
nix develop -c cargo build --workspace --all-targets --all-features --locked
nix develop -c rust-analyzer diagnostics .
```
