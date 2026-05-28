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
crates/hz-worktree  worktree domain boundary: new, switch, handoff
crates/hz-diff      diff domain boundary
crates/hz-tui       ratatui/crossterm UI boundary
```

The command crate is the main extension seam for now. CLI and TUI code should
call `hz-command` instead of duplicating workflow logic. If agent providers or a
runtime become necessary later, they can sit beside these crates without forcing
the existing command surface to depend on plugin machinery.

The command handlers intentionally return `not implemented yet` until the first
real behavior is added.

## Development

Install the repo Rust toolchain:

```sh
rustup show
```

Or enter the Nix development shell:

```sh
nix develop
```

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

The same quality gate is also available through Nix:

```sh
nix develop -c cargo check --workspace --all-targets --all-features --locked
nix develop -c cargo fmt --all --check
nix develop -c cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
nix develop -c cargo test --workspace --all-targets --all-features --locked
nix develop -c cargo build --workspace --all-targets --all-features --locked
nix develop -c rust-analyzer diagnostics .
```
