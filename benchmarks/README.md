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

## Comparing Hz, Rift, and Git

Build release binaries, then run the comparison harness with a Rift binary:

```sh
cargo build --release -p hz-cli --locked
cargo run --release -p hz-bench -- compare \
  --hz target/release/hz \
  --rift /path/to/release/rift \
  --repo-files 10000 \
  --iterations 10
```

The harness creates three equivalent isolated repositories and registries on
the same filesystem, rotates tool order each round, and separately measures:

- `hz new --no-hooks` and `hz rm --no-hooks`
- `rift create --copy-all --no-hooks` and `rift remove`
- local `git clone` and recursive deletion

It reports latency distributions and median Hz speedups. Add `--filtered` to
compare `hz new --filtered --no-hooks` with Rift's default filtered creation.
Use `--artifact-files N` to put tracked files under `target/`; Git still checks
out those files, so its result is intentionally not semantically equivalent in
filtered mode. Use `--json` for machine-readable results.

The setup uses release binaries, identical committed inputs, isolated homes and
databases, rotating order, and separate creation/removal timers. The operations
still have meaningful semantic differences: Hz preserves source-control and
dirty filesystem state, Rift detaches Git `HEAD`, and Git clone only reproduces
committed Git state. Hz and Rift removal are recoverable constant-time moves to
trash, while the Git baseline is an irreversible recursive delete.
