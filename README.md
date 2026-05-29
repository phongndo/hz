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

`hz new` creates a Git worktree with a human-facing branch/handle and a UUID
directory under `~/.hz/worktrees/<repo>/` by default:

```sh
hz new fix-login
hz new
hz ls
hz path fix-login
hz cd fix-login
hz handoff fix-login
hz cd
hz rm fix-login
```

`hz new` without a name generates a `word-word` branch/handle. Managed
worktrees are registered in `~/.config/hz/registry.json` or
`$XDG_CONFIG_HOME/hz/registry.json`. `hz ls`, `hz cd`, and `hz rm` also detect
unmanaged Git worktrees created by other tools. Removing an unmanaged worktree
asks for confirmation because the path is not managed by hz.

`hz cd` prints a path for scripts. To make `hz new` and `hz cd` change the
current shell directory, run `hz init <shell>` once to update your shell rc
file:

```sh
hz init zsh
hz init bash
hz init fish
```

For zsh, this updates `~/.zshrc`. Restart your shell or run `source ~/.zshrc`
after init. With the integration loaded, plain `hz new ...` creates the
worktree and changes into it, and `hz cd` returns to the local repo root.
`--json`, `--path-only`, and help calls still pass through to the real binary
without changing directories.

`hz handoff` applies the current worktree's uncommitted diff to the other side
by default. From a linked worktree it applies the patch to `local`. From
`local`, pass a worktree handle such as `hz handoff fix-login` to apply the
local patch there. The destination must be clean, and source changes are left in
place.

Use `hz handoff --branch <branch-or-worktree>` to move branch ownership instead
of applying a patch. Branch handoff is clean-only on both sides.

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
just dev-init-zsh
just check
just build
just hz --help
hz new test-branch
```

`just hz ...` is useful for commands that print output, but it cannot change the
current shell directory. Use `just dev-init-zsh` once, then call `hz new` or
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
