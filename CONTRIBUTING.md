# Contributing to hz

Thanks for helping make `hz` a production-grade terminal workflow for parallel
AI agents. Keep changes small, explicit, and grounded in the current crate
boundaries.

## Principles

- Headless first: every workflow should be scriptable before it needs an
  interactive UI.
- Git native: prefer Git worktrees, diffs, branches, and plain files over hidden
  state.
- Safe by default: do not remove or overwrite user work without a clean check or
  explicit confirmation.
- Provider agnostic: agent/runtime integrations should not leak into core
  worktree behavior.
- Boring code wins: preserve public CLI behavior unless a change intentionally
  documents a compatibility break.

## Setup

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
The shim builds `hz-cli` only when `target/debug/hz` is missing, then runs the
local development binary. It does not fall back to `~/.local/bin/hz` or another
installed `hz`, and it does not rebuild on every completion or command. After
editing Rust code, run `cargo build -p hz-cli --locked` when you want the shim to
pick up changes. Set `HZ_DEV_AUTO_BUILD=1` only if you explicitly want the shim
to rebuild when source files are newer than its dev stamp.

The dev zsh rc file also loads `hz shell zsh`, so auto-cd and completion
behavior use the local binary by default.

Verify the active binary:

```sh
type -a hz
whence -p hz
hz --version
```

## Local checks

Use the cheapest useful check first while developing:

```sh
just setup
just install-hooks
just check
just build
just smoke
just hz --help
hz --help
```

`just install-hooks` configures Git to use the repo's versioned hooks from
`.githooks`. The pre-commit hook runs `just check` before each commit.

The full local quality gate is:

```sh
rust-analyzer diagnostics .
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-targets --all-features --locked
cargo build --workspace --all-targets --all-features --locked
cargo run -p hz-bench --locked -- cmd --hz target/debug/hz --worktrees 2 --warmup 0 --iterations 1
```

The same checks are available through Nix:

```sh
nix develop -c rust-analyzer diagnostics .
nix develop -c cargo fmt --all --check
nix develop -c cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
nix develop -c cargo test --workspace --all-targets --all-features --locked
nix develop -c cargo build --workspace --all-targets --all-features --locked
```

`just hz ...` is useful for commands that print output, but it cannot change the
current shell directory. Use plain `hz new` or `hz cd` inside interactive
`nix develop` to exercise auto-cd behavior from the development binary without
editing your shell rc file.

Use `hz install zsh` only when you want to update your real shell rc file for an
installed `hz` binary. `just smoke-zsh` verifies the zsh integration in an
isolated shell, including branch names such as `fix(scope)/name` that zsh would
otherwise treat as globs. `just smoke` also runs the installer/update smoke
against a temporary local release fixture. `just smoke-curl-install` exercises
the published curl install path when you want live release coverage.

Bash cannot run unquoted branch names containing parentheses, such as
`fix(scope)/name`, because bash parses `(` as syntax before `hz` can receive the
argument. Quote those names in bash or use the zsh integration.

## Pull requests

- Fill out the PR template, including motivation, risk, and verification.
- Keep each PR focused on one behavior, command path, or documentation goal.
- Update README/docs when changing user-facing commands, config, install flows,
  or shell behavior.
- Add or update focused tests for command parsing, shell integration, Git
  safety checks, and lifecycle behavior when those paths change.

`.github/workflows/pr-template.yml` requires pull requests to keep the template
sections and mark at least one verification command.

## CI

`.github/workflows/quality.yml` runs rust-analyzer diagnostics, formatter,
Clippy, workspace tests, a full workspace build, and a headless `hz-bench`
smoke.
