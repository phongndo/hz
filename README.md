# hz

`hz` creates fast, isolated copy-on-write workspaces for parallel humans and
agents. Each workspace is an independent directory containing the entire working
tree and its source-control metadata. Ancestry is logical, not a Git worktree
relationship.

hz is pre-1.0. This C++23 rewrite targets Linux and macOS. Windows is a design
consideration, not an implemented platform. Older registries are not migrated.

## Quickstart

```sh
cd ~/code/app
hz init                       # probes copy-on-write support
# Use hz init --copy explicitly on filesystems without cloning.

hz config init                # optional: creates commented .hz/hz.toml
hz install zsh                # or bash/fish; restart or source your rc
hz new parser-fix             # creates a child and enters it

# Work in the independent workspace, then inspect or hand changes back.
hz pwd
hz ls --tree
hz git status
hz git handoff root           # destination must be clean

hz rm parser-fix              # moves the child and descendants to trash
hz restore parser-fix         # recover them before GC
hz rm parser-fix
hz gc                         # permanently deletes all registered trash
```

Without shell integration:

```sh
cd "$(hz new parser-fix --path-only)"
cd "$(hz path root)"
```

## Workspace model

Every managed workspace has a stable ULID in `.hz-workspace`. A per-user SQLite
registry records its path, handle, immediate parent, root, copy mode, and
lifecycle state. Children normally live beside the root:

```text
~/code/app/
~/code/.hz-workspaces/app-<last-six-root-id-characters>/<workspace-id>/
~/code/.hz-workspaces/app-<last-six-root-id-characters>/.trash/<workspace-id>/
```

Creation uses the fastest strategy the filesystem allows. A root that is a
btrfs subvolume is copied with a constant-time snapshot. Otherwise hz clones
each file with Linux `FICLONE`, or the whole tree with macOS `clonefile`, then
refreshes Git's cached stat data so the first Git command does not reread
every file. Clones share data blocks until either side writes. `hz init --copy`
explicitly selects byte copies instead; hz never falls back silently. `--into`
can select another storage directory on the same filesystem.

Copies are full by default, including build and dependency artifacts.
`hz new --filtered` skips built-in regenerable artifacts; `.hz/hz.toml` can make
filtering the default, and `--full` overrides it.

Git children have detached HEADs and their own metadata. Linked worktree sources,
reftable repositories, Git locks, and operations in progress are refused. Default
creation and removal invoke no SCM processes. Configured lifecycle hooks are
explicit opt-ins. `hz git status`, `hz git handoff`, and `hz hg status` invoke SCM
commands only when requested.

Removal renames directories into trash; GC performs recursive deletion later.
Pins protect workspaces from removal. `hz adopt PATH` follows a manual move, and
`hz doctor --fix` repairs interrupted operations where ownership can be proven.
Root removal requires `--force`, preserves the directory, and removes its marker
and registry row.

See [the design](docs/design.md) for guarantees and filesystem support,
[the CLI reference](docs/cli.md) for commands, and
[configuration](docs/config.md) for filtering and hooks.

## Automation

`--machine` emits JSON and bypasses shell navigation:

```sh
hz --machine new parser-fix
hz --machine list
hz --machine git status parser-fix
```

`--json`, help, and path-only calls also bypass shell navigation. Hook output
goes to stderr so it does not corrupt machine output.

## Development

hz uses C++23, CMake, Ninja, and Clang inside the Nix development shell:

```sh
nix develop -c just setup
nix develop -c just test
nix develop -c just check
nix flake check -L
```

`nix build` produces the release binary and runs the tests. CI checks Linux and
macOS packages plus Linux btrfs, XFS, ZFS, and ext4. See
[CONTRIBUTING.md](CONTRIBUTING.md) for the development workflow.

## License

MIT
