# Development

## Principles

Hz treats data safety and performance as product behavior. Prefer designs with one clear owner,
direct data flow, bounded work, and few states. Preserve ordering and failure behavior at system
boundaries. Measure costs before adding complexity to improve them.

## Workflow

Enter the pinned development environment and use the repository commands:

```sh
nix develop
just run --version
just build
just test
just fmt
just lint
just lsp-check
just check
```

The default build is a debug build. Use `just profile=release build` for a release build and
`just bench` for the benchmark executable.

Tests use GoogleTest and CTest. Benchmarks use Google Benchmark. Conan supplies C++ dependencies,
while Nix supplies the compiler and development tools.

## Verification

`just check` runs formatting, the build, tests, clang-tidy, and clangd diagnostics. The CI scripts
under `scripts/ci` provide the same entry points used by GitHub Actions.
