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

Include mutating create, trash, and shell-navigation removal measurements with:

```sh
cargo run -p hz-bench -- cmd \
  --hz target/debug/hz \
  --workspaces 12 \
  --iterations 10 \
  --mutating
```

Add `--remove-depth N` to create a logical chain and measure removal of all
`N` workspaces as one subtree:

```sh
cargo run --release -p hz-bench -- cmd \
  --hz target/release/hz \
  --workspaces 0 \
  --iterations 10 \
  --mutating \
  --remove-depth 100
```

Use `--keep DIR` to preserve a fixture for inspection and `--json` for machine
results. CI uses `--portable` for runner compatibility, while native APFS,
btrfs, and reflink performance should be measured without it. Reports include
minimum, median, average, p95, maximum, and raw samples.

To evaluate how `hz new` scales with repository entries, generate an included
file set and omit pre-created child workspaces:

```sh
cargo run --release -p hz-bench -- cmd \
  --hz target/release/hz \
  --workspaces 0 \
  --repo-files 10000 \
  --file-bytes 1024 \
  --iterations 10 \
  --mutating
```

To evaluate filtered creation, add regenerable files under `target/` and use the
same filter for fixture setup and measured creates:

```sh
cargo run --release -p hz-bench -- cmd \
  --hz target/release/hz \
  --workspaces 0 \
  --repo-files 100 \
  --artifact-files 10000 \
  --filtered \
  --iterations 10 \
  --mutating
```

## Comparing Rift

Rift defaults to filtered copying, while Hz defaults to an exact copy. Compare
equivalent modes rather than default command lines:

- `hz new --filtered --no-hooks` versus `rift create --no-hooks`
- `hz new --no-hooks` versus `rift create --copy-all --no-hooks`

Use release binaries, identical fixtures on the same filesystem, isolated
registries and homes, alternating tool order, and separate creation and removal
timers. Git results are not semantically identical: Rift detaches the created
checkout, while Hz preserves source-control state as ordinary filesystem data.
