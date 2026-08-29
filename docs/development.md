# Development

## Principles

Light treats data safety and performance as product behavior. Prefer designs with one clear owner,
direct data flow, bounded work, and few states. Preserve ordering and failure behavior at system
boundaries. Measure costs before adding complexity to improve them.

## Workflow

Enter the pinned development environment and use the shared development runner:

```sh
nix develop
just run --version

# The shell command is an ergonomic alias for the same runner.
light --version
```

`just run [args...]` is the canonical explicit entry point. Both forms fingerprint the checkout's
configuration and toolchain inputs, configure `build/dev` only when they change, incrementally build
only the `light` target, and execute that checkout's exact binary with unchanged arguments.
Concurrent invocations for one worktree are serialized without sharing state across worktrees. The
`dev` profile uses `-O1`, debug symbols, enabled assertions, and frame pointers.

Common verification commands:

```sh
just build
just test
just fmt
just lint
just lsp-check
just check
```

Verification defaults to a debug build. Release remains explicit for packaging and performance
measurement: use `just profile=release build`, `just bench`, or `nix build .#light`.

Tests use GoogleTest and CTest. Benchmarks use Google Benchmark. Conan supplies C++ dependencies,
while Nix supplies the compiler and development tools.

## Verification

`just check` runs formatting, the build, tests, clang-tidy, and clangd diagnostics. The CI scripts
under `scripts/ci` provide the same entry points used by GitHub Actions.
