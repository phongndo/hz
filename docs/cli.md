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
hz git <command> [options]
```

Running `hz` without a command prints help.

## Human commands

```sh
hz git new [name]                  # Create a managed worktree
hz git fork [name]                 # Fork the current worktree state
hz git path [target]               # Print a worktree path; alias: cd
hz git list                        # List worktrees; alias: ls
hz git pwd                         # Print current target: local, branch, or handle
hz git remove <target...>          # Remove worktrees; alias: rm
hz git pin <target...>             # Keep worktrees out of auto-prune
hz git unpin <target...>           # Make worktrees eligible for auto-prune
hz git handoff [target]            # Apply changes between linked worktrees
hz init                                 # Create repo-local .hz config and lifecycle files
hz install <zsh|bash|fish>              # Install shell integration
hz shell <zsh|bash|fish>                # Print shell integration
hz update                               # Update a curl-installed hz binary
```

Most commands that return data accept `--json` (`-j`). With shell integration
loaded, `hz git new`, `hz git fork`, `hz git cd`, and
`hz git handoff` may change the current directory unless `--json`,
`--machine`, `--path-only`, or help is passed.

## Machine-readable mode

```sh
hz --machine git new [name]            # Create a worktree and print JSON
hz --machine git fork [name]           # Fork the current state and print JSON
hz --machine git path [target]         # Print a target path as JSON; alias: cd
hz --machine git list                  # List worktrees as JSON; alias: ls
hz --machine git pwd                   # Print current target/repo/path as JSON
hz --machine git remove <target...>    # Remove worktrees and print a JSON array; alias: rm
hz --machine git pin <target...>       # Pin worktrees and print JSON
hz --machine git unpin <target...>     # Unpin worktrees and print JSON
hz --machine git handoff [target]      # Handoff changes and print JSON
```

`--machine` is a global flag, so it can be passed before or after the command:
`hz --machine git list` and `hz git list --machine` are equivalent.

Use this surface when another program needs stable stdout:

```sh
hz --machine git new fix-login --repo .
hz --machine git list --repo .
hz --machine git handoff fix-login --repo .
hz --machine git remove fix-login --repo . --force
```

Safety behavior is unchanged. For example, `hz --machine git remove`
refuses to remove an unmanaged worktree without `--force` instead of asking for
confirmation. It always returns an array, even when one target was requested.
Lifecycle hook stdout is forwarded to stderr so JSON stdout remains parseable.

## Common options

| Option | Commands | Description |
| --- | --- | --- |
| `-r`, `--repo <path>` | git, init | Repository to operate on |
| `-p`, `--path <path>` | `git new`, `git fork` | Destination path for the worktree |
| `-B`, `--base <rev>` | `git new` | Base revision for the new worktree |
| `-b`, `--branch <name>` | `git new` | Create or use a branch-backed worktree |
| `--max-detached <n>` | `git new`, `git fork`, `git handoff --new` | Override detached worktree cap |
| `--max-branch-worktrees <n>` | `git new`, branch `git handoff --new` | Override branch-backed worktree cap |
| `--pinned`, `--unpinned` | `git list` | Filter listed worktrees by pin state |
| `-j`, `--json` | data-producing Git commands | Print JSON |
| `--machine` | Git commands | Force JSON and avoid shell side effects |
| `-f`, `--force`, `--yes` | `git remove` | Skip removal confirmation and pass force to Git |
| `--setup`, `--no-setup` | `git new` | Run or suppress setup lifecycle |
| `--cleanup`, `--no-cleanup` | `git remove` | Run or suppress cleanup lifecycle |

See [config.md](config.md) for repo-local defaults and display settings.
