# hz CLI reference

`hz` has two command surfaces:

- Human commands optimize for terminal use, shell integration, and readable
  output.
- `--json` and `--machine` optimize the same commands for agents and scripts.
  `--json` prints JSON for one command. `--machine` forces JSON, avoids shell
  auto-cd, and fails instead of prompting when a safe non-interactive answer is
  required.

## Usage

```sh
hz [command] [options]
hz worktree <command> [options]
hz wt <command> [options]
```

Running `hz` without a command prints help.

## Human commands

```sh
hz worktree new [name]                  # Create a managed worktree
hz worktree fork [name]                 # Fork the current worktree state
hz worktree path [target]               # Print a worktree path; alias: cd
hz worktree list                        # List worktrees; alias: ls
hz worktree pwd                         # Print current target: local, branch, or handle
hz worktree remove <target...>          # Remove worktrees; alias: rm
hz worktree pin <target...>             # Keep worktrees out of auto-prune
hz worktree unpin <target...>           # Make worktrees eligible for auto-prune
hz worktree handoff [target]            # Apply changes between linked worktrees
hz init                                 # Create repo-local .hz config and lifecycle files
hz install <zsh|bash|fish>              # Install shell integration
hz shell <zsh|bash|fish>                # Print shell integration
hz update                               # Update a curl-installed hz binary
```

`hz wt <command>` is a shorter alias for `hz worktree <command>`.

Most commands that return data accept `--json` (`-j`). With shell integration
loaded, `hz worktree new`, `hz worktree fork`, `hz worktree cd`, and
`hz worktree handoff` may change the current directory unless `--json`,
`--machine`, `--path-only`, or help is passed.

## Machine-readable mode

```sh
hz --machine worktree new [name]            # Create a worktree and print JSON
hz --machine worktree fork [name]           # Fork the current state and print JSON
hz --machine worktree path [target]         # Print a target path as JSON; alias: cd
hz --machine worktree list                  # List worktrees as JSON; alias: ls
hz --machine worktree pwd                   # Print current target/repo/path as JSON
hz --machine worktree remove <target...>    # Remove worktrees and print a JSON array; alias: rm
hz --machine worktree pin <target...>       # Pin worktrees and print JSON
hz --machine worktree unpin <target...>     # Unpin worktrees and print JSON
hz --machine worktree handoff [target]      # Handoff changes and print JSON
```

`--machine` is a global flag, so it can be passed before or after the command:
`hz --machine worktree list` and `hz worktree list --machine` are equivalent.

Use this surface when another program needs stable stdout:

```sh
hz --machine worktree new fix-login --repo .
hz --machine worktree list --repo .
hz --machine worktree handoff fix-login --repo .
hz --machine worktree remove fix-login --repo . --force
```

Safety behavior is unchanged. For example, `hz --machine worktree remove`
refuses to remove an unmanaged worktree without `--force` instead of asking for
confirmation. It always returns an array, even when one target was requested.
Lifecycle hook stdout is forwarded to stderr so JSON stdout remains parseable.

For compatibility, `hz agent ...` remains as a machine-readable alias for the
same worktree commands.

## Common options

| Option | Commands | Description |
| --- | --- | --- |
| `-r`, `--repo <path>` | worktree, init | Repository to operate on |
| `-p`, `--path <path>` | `worktree new`, `worktree fork` | Destination path for the worktree |
| `-B`, `--base <rev>` | `worktree new` | Base revision for the new worktree |
| `-b`, `--branch <name>` | `worktree new` | Create or use a branch-backed worktree |
| `--max-detached <n>` | `worktree new`, `worktree fork`, `worktree handoff --new` | Override detached worktree cap |
| `--max-branch-worktrees <n>` | `worktree new`, branch `worktree handoff --new` | Override branch-backed worktree cap |
| `--pinned`, `--unpinned` | `worktree list` | Filter listed worktrees by pin state |
| `-j`, `--json` | data-producing worktree commands | Print JSON |
| `--machine` | worktree commands | Force JSON and avoid shell side effects |
| `-f`, `--force`, `--yes` | `worktree remove` | Skip removal confirmation and pass force to Git |
| `--setup`, `--no-setup` | `worktree new` | Run or suppress setup lifecycle |
| `--cleanup`, `--no-cleanup` | `worktree remove` | Run or suppress cleanup lifecycle |

See [config.md](config.md) for repo-local defaults and display settings.
