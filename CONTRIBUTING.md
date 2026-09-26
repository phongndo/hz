# Contributing to hz

Keep changes small, explicit, and consistent with [docs/design.md](docs/design.md),
which is the specification for the workspace model.

## Principles

- Headless first: every workflow should be scriptable before it needs an
  interactive UI.
- Workspace first: identity, ancestry, storage, and lifecycle must not depend on
  a particular source control or filesystem.
- Safe by default: destructive operations move complete logical subtrees to
  recoverable trash before garbage collection.
- Boring code wins: prefer explicit state transitions, repairable metadata, and
  stable machine output.

## Setup

Everything runs inside the Nix development shell, which provides the compiler,
CMake, Ninja, the libraries, and clang tooling:

```sh
nix develop
just setup
```

`just setup` configures `build/debug` with a compile database that `.clangd`
points at. The shell puts `build/debug` first on `PATH`, so `hz` inside the
shell is the development binary.

## Layout

```text
CMakeLists.txt        version number, targets, dependencies
CMakePresets.json     debug and release presets under build/<preset>
flake.nix             package, checks, and development shell
src/hz/               hz_core: workspace model, registry, materialization
src/cli/              the hz executable: argument parsing and output
shell/                zsh, bash, and fish integration scripts
tests/unit/           Catch2 unit tests for hz_core
tests/CMakeLists.txt  unit test registration and black-box CLI tests
docs/                 design, CLI, and configuration references
```

Dependencies come from nixpkgs and are found with CMake config packages:
CLI11, toml++, nlohmann_json, SQLite, and Catch2. Add a dependency in both
`flake.nix` and `CMakeLists.txt`.

## Local checks

```sh
just build       # compile
just test        # build, then ctest
just fmt         # clang-format in place
just check       # clang-format --dry-run and clang-tidy (warnings fail)
just release     # optimized build under build/release
nix flake check  # what CI runs: release build plus tests
```

Git hooks are managed by [hk](https://hk.jdx.dev); the steps are in
[hk.pkl](./hk.pkl) and run clang-format before each commit. `just hooks`
validates the configuration.

## Conventions

- C++23, no compiler extensions. Warnings are enabled through the
  `hz_warnings` target and should stay clean.
- Errors are exceptions caught at the CLI command boundary for structured
  output; `main` handles unexpected failures. Library code does not print.
- The tree walker and workspace semantics are shared. Keep platform differences
  in clone, metadata, filesystem, and process helpers.
- Behaviour changes update `docs/design.md` in the same change.

## Pull requests

- Keep each pull request focused on one behaviour change.
- Include tests for new behaviour: unit tests for `hz_core`, black-box CLI
  tests for command output and filesystem effects.
- CI runs `nix flake check` on Linux and macOS, plus filesystem tests on Linux
  btrfs, XFS, ZFS, and ext4. Shell tests exercise bash, zsh, and fish.
