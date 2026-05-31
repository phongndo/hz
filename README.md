# hz

`hz` is planned as a headless-first CLI for coordinating parallel agent work
across Git worktrees. The repository currently contains scaffolding only:
command shapes, crate boundaries, and placeholder domain types.

## Crates

```text
crates/hz-cli       command parsing and CLI argument shape
crates/hz-command   command facade shared by CLI and future TUI/runtime callers
crates/hz-core      shared errors and common models
crates/hz-git       low-level git integration boundary
crates/hz-worktree  worktree domain boundary: new, path, handoff
crates/hz-diff      diff domain boundary
crates/hz-tui       ratatui/crossterm UI boundary
```

The command crate is the main extension seam for now. CLI and TUI code should
call `hz-command` instead of duplicating workflow logic. If agent providers or a
runtime become necessary later, they can sit beside these crates without forcing
the existing command surface to depend on plugin machinery.

Some command handlers still intentionally return `not implemented yet` until
their real behavior is added.

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
hz.toml
.hz/setup
.hz/cleanup
```

`hz.toml` declares the commands `hz` should run:

```toml
[lifecycle]
setup = [".hz/setup"]
cleanup = [".hz/cleanup"]
```

`.hz/setup` and `.hz/cleanup` are executable script files. Edit them with the
repo setup and cleanup commands an agent worktree should run. `hz new` runs the
configured setup command after creating a worktree, and `hz rm` runs the
configured cleanup command before removing one. Use `--no-setup` or
`--no-cleanup` to bypass a hook for one command. Use `hz setup [target]` or
`hz cleanup [target]` to run a hook manually. Hook stdout is forwarded to stderr
so `--json` and `--path-only` output stays machine-readable.

Lifecycle config is read from the target worktree. Commit `hz.toml` and any
referenced scripts before relying on `hz new` to run setup in newly created
worktrees.

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

Run the local development binary without typing `target/debug/hz`:

```sh
just setup
hz install zsh
just check
just build
just hz --help
hz new test-branch
```

`just hz ...` is useful for commands that print output, but it cannot change the
current shell directory. Use `hz install zsh` once, then call `hz new` or
`hz cd` directly for auto-cd behavior.

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
