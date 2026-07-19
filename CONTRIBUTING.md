# Contributing to hz

Thanks for helping make `hz` a production-grade terminal workflow for parallel
AI agents. Keep changes small, explicit, and grounded in the current crate
boundaries.

## Principles

- Headless first: every workflow should be scriptable before it needs an
  interactive UI.
- Workspace first: identity, ancestry, storage, and lifecycle must not depend on
  a particular source control.
- Safe by default: destructive operations move complete logical subtrees to
  recoverable trash before garbage collection.
- Source-control agnostic: Git, Mercurial, Jujutsu, and future integrations must
  sit behind capabilities rather than leak into core workspace behavior.
- Boring code wins: prefer explicit state transitions, repairable metadata, and
  stable machine output.

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
just hooks
just check
just hk-check
just ci-check
just ci-rust
just ci-integration
just ci-performance
just ci-workflows
just test
just build
just smoke
just hz --help
hz --help
```

`just hooks` validates the hk config. Git hooks are managed globally by
[hk](https://hk.jdx.dev) (`hk-pre-commit` runs `cargo fmt --check` and
`cargo clippy` before each commit). The repo's hook steps are defined in
[hk.pkl](./hk.pkl).

The full local quality gate is:

```sh
rust-analyzer diagnostics .
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-targets --all-features --locked
cargo build --workspace --all-targets --all-features --locked
cargo run -p hz-bench --locked -- cmd --hz target/debug/hz --workspaces 2 --warmup 0 --iterations 1
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
isolated shell, including handles containing shell glob characters. `just smoke`
also runs the installer/update smoke
against a temporary local release fixture. `just smoke-curl-install` exercises
the published curl install path when you want live release coverage.

Bash cannot run unquoted handles containing parentheses because it parses `(`
as syntax before `hz` can receive the argument. Quote those handles in Bash or
use the Zsh integration.

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

[docs/ci.md](docs/ci.md) documents the required lanes, scheduled platform
coverage, and release qualification model. Pull requests and pushes to `main`
run change-aware correctness, MSRV, integration, performance, and workflow-lint
lanes. The stable `CI gate` job is the branch-protection check; it fails if any
selected lane fails or is cancelled.
