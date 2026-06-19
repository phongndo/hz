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
```

Running `hz` without a command prints help.

## Human commands

```sh
hz new [name]                  # Create a managed worktree
hz fork [name]                 # Fork the current worktree state
hz path [target]               # Print a worktree path; alias: hz cd
hz list                        # List worktrees; alias: hz ls
hz pwd                         # Print current target: local, branch, or handle
hz remove <target...>          # Remove worktrees; alias: hz rm
hz handoff [target]            # Apply changes between linked worktrees
hz setup [target]              # Run configured setup lifecycle
hz cleanup [target]            # Run configured cleanup lifecycle
hz init                        # Create repo-local .hz config and lifecycle files
hz install <zsh|bash|fish>     # Install shell integration
hz shell <zsh|bash|fish>       # Print shell integration
hz update                      # Update a curl-installed hz binary
```

`hz worktree <command>` and `hz wt <command>` are explicit namespaces for the
worktree commands: `new`, `fork`, `path`, `list`, `pwd`, `remove`, and
`handoff`.

Most commands that return data accept `--json` (`-j`). With shell integration
loaded, `hz new`, `hz fork`, `hz cd`, and `hz handoff` may change the current
directory unless `--json`, `--machine`, `--path-only`, or help is passed.

## Machine-readable mode

```sh
hz --machine new [name]            # Create a worktree and print JSON
hz --machine fork [name]           # Fork the current state and print JSON
hz --machine path [target]         # Print a target path as JSON; alias: cd
hz --machine list                  # List worktrees as JSON; alias: ls
hz --machine pwd                   # Print current target/repo/path as JSON
hz --machine remove <target...>    # Remove worktrees and print a JSON array; alias: rm
hz --machine handoff [target]      # Handoff changes and print JSON
hz setup [target] --json           # Run setup lifecycle and print JSON
hz cleanup [target] --json         # Run cleanup lifecycle and print JSON
```

`--machine` is a global flag, so it can be passed before or after the command:
`hz --machine list` and `hz list --machine` are equivalent.

Use this surface when another program needs stable stdout:

```sh
hz --machine new fix-login --repo .
hz --machine list --repo .
hz --machine handoff fix-login --repo .
hz --machine remove fix-login --repo . --force
```

Safety behavior is unchanged. For example, `hz --machine remove` refuses to remove
an unmanaged worktree without `--force` instead of asking for confirmation. It
always returns an array, even when one target was requested. Lifecycle hook
stdout is forwarded to stderr so JSON stdout remains parseable.

For compatibility, `hz agent ...` remains as a machine-readable alias for the
same worktree and lifecycle commands.

## Common options

| Option | Commands | Description |
| --- | --- | --- |
| `-r`, `--repo <path>` | worktree, lifecycle, init | Repository to operate on |
| `-p`, `--path <path>` | `new`, `fork` | Destination path for the worktree |
| `-B`, `--base <rev>` | `new` | Base revision for the new worktree |
| `-b`, `--branch <name>` | `new` | Create or use a branch-backed worktree |
| `--max-detached <n>` | `new`, `fork`, `handoff --new` | Override detached worktree cap |
| `--max-branch-worktrees <n>` | `new`, branch `handoff --new` | Override branch-backed worktree cap |
| `-j`, `--json` | data-producing human commands | Print JSON |
| `--machine` | worktree and lifecycle commands | Force JSON and avoid shell side effects |
| `-f`, `--force`, `--yes` | `remove` | Skip removal confirmation and pass force to Git |
| `--setup`, `--no-setup` | `new` | Run or suppress setup lifecycle |
| `--cleanup`, `--no-cleanup` | `remove` | Run or suppress cleanup lifecycle |

See [config.md](config.md) for repo-local defaults and display settings.
