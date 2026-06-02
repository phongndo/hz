# hz roadmap

`hz` is being built toward an all-in-one terminal workflow for parallel AI agent
development. The current foundation is Git worktree orchestration: isolated
workspaces, safe diff handoff, lifecycle hooks, shell integration, and terminal
diff review.

This roadmap is directional, not a compatibility promise. Pre-1.0 work should
prefer the smallest useful surface that can stay scriptable and provider
agnostic.

## Product vision

The goal is a terminal command center where a developer can:

1. Split a repository into multiple safe workspaces.
2. Launch one or many AI agents against those workspaces.
3. Track status, logs, diffs, and ownership from one place.
4. Review and hand off useful changes without losing local context.
5. Clean up completed or abandoned attempts confidently.

## Current foundation

- Worktree creation, discovery, listing, path lookup, and removal.
- Detached scratch worktree caps with safe auto-pruning.
- Patch and branch handoff between linked worktrees.
- Repo-local setup and cleanup hooks.
- zsh, bash, and fish integration for auto-cd and completions.
- Terminal diff review with plain output for non-interactive use.
- Installer, update command, release packaging, and CI quality gates.

## Near-term direction

- Generic agent command runner that can start any terminal agent command in a
  managed worktree without baking provider choices into core crates.
- Run/session registry for active, completed, and failed agent attempts.
- Status and log views that work headlessly first and can power a richer TUI.
- Review queue for comparing multiple agent outputs before handoff or merge.
- Config profiles for common repo setup, cleanup, base branch, and display
  conventions.

## Later possibilities

- Provider adapters for tools that need structured launch, resume, or metadata
  support.
- A dashboard-style TUI for monitoring multiple agents and diffs at once.
- Remote or container-backed workers when local worktrees are not enough.
- Policy hooks for sandboxing, secrets, and repository-specific guardrails.
- Merge assistance that keeps Git history explicit and reviewable.

## Design constraints

- Do not make an agent provider mandatory for core worktree workflows.
- Do not hide Git state behind a database when plain Git can remain the source
  of truth.
- Do not delete or overwrite user changes without clean checks, confirmation,
  or an explicit force path.
- Do not require the TUI for automation; JSON and plain text output should stay
  first-class.
