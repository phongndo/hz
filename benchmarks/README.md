# Benchmarks

`hz-bench` contains headless benchmark utilities for the workspace CLI. It does
not benchmark the old diff/TUI product; diff review benchmarks live with
[`dx`](https://github.com/phongndo/dx).

Build `hz`, then run the command benchmark:

```sh
cargo build -p hz-cli --locked
cargo run -p hz-bench -- cmd --hz target/debug/hz --worktrees 12 --iterations 10
```

The benchmark creates an isolated temporary Git repo and HOME, creates synthetic
`hz` worktrees, then measures end-to-end CLI latency for commands such as
`hz list`, `hz path`, shell generation, and dynamic completion candidate lookup.

Use JSON for machine-readable results:

```sh
cargo run -p hz-bench -- cmd \
  --hz target/debug/hz \
  --worktrees 50 \
  --iterations 25 \
  --json
```

Use `--mutating` when you also want to measure create/remove latency:

```sh
cargo run -p hz-bench -- cmd --hz target/debug/hz --mutating
```

By default, fixtures are removed after the run. Pass `--keep <new-dir>` to keep
the generated repo and isolated HOME for inspection.
