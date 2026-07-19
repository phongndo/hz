# hz benchmarks

`hz-bench cmd` measures end-to-end CLI latency against a synthetic initialized
workspace family.

```sh
cargo build -p hz-cli --locked
cargo run -p hz-bench -- cmd --hz target/debug/hz --workspaces 12 --iterations 10
```

The fixture creates an isolated Git repository and HOME, initializes the native
COW strategy, creates child workspaces, and measures commands such as `hz list`,
`hz path`, shell generation, and dynamic target completion. Use `--portable`
only when testing command overhead on a filesystem without COW; reports record
which materialization mode was used.

Include mutating create/trash measurements with:

```sh
cargo run -p hz-bench -- cmd \
  --hz target/debug/hz \
  --workspaces 12 \
  --iterations 10 \
  --mutating
```

Use `--keep DIR` to preserve a fixture for inspection and `--json` for machine
results. CI uses `--portable` for runner compatibility, while native APFS,
btrfs, and reflink performance should be measured without it.
