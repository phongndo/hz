# hz

`hz` is planned as a headless-first CLI for coordinating parallel agent work
across Git worktrees. The repository currently contains scaffolding only:
command shapes, crate boundaries, and placeholder domain types.

## Commands

```sh
cargo run -p hz-cli -- worktree new <name>
cargo run -p hz-cli -- worktree switch <name-or-path>
cargo run -p hz-cli -- worktree handoff <from> <to>
cargo run -p hz-cli -- diff --stat
cargo run -p hz-cli -- tui
```

Shortcuts are available for the worktree commands:

```sh
cargo run -p hz-cli -- new <name>
cargo run -p hz-cli -- switch <name-or-path>
cargo run -p hz-cli -- handoff <from> <to>
```

The intended default path policy for named worktrees is:

```text
~/.hz/worktrees/<repo-name>/<worktree-name>
```

Use `--path <path>` with `worktree new` when a worktree should live somewhere
else. This behavior is not implemented yet.

## JSON

Headless commands reserve `--json` for future structured output:

```sh
cargo run -p hz-cli -- worktree new agent-a --json
cargo run -p hz-cli -- worktree switch agent-a --json
cargo run -p hz-cli -- worktree handoff agent-a agent-b --json
```

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

```sh
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```
