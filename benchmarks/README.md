# Workspace benchmarks

`compare.py` compares optimized hz and Rift **CLI commands**, including process
startup and registry access. It creates disposable Git repositories and isolated
registries beneath a new output directory; it never initializes a supplied
project. Python 3, Git, GNU time, and util-linux `findmnt` are required. The runner
currently supports Linux.

Build hz with the declared environment:

```sh
nix develop -c just release
```

Build a pinned [anomalyco/rift](https://github.com/anomalyco/rift) checkout with
`cargo build --release --locked -p rift-cli` using a temporary Rust/C toolchain.
Then run, substituting absolute paths and the revisions actually built:

```sh
nix shell nixpkgs#time -c nix develop -c python3 benchmarks/compare.py \
  --hz /absolute/path/to/hz/build/release/hz \
  --rift /absolute/path/to/rift/target/release/rift \
  --hz-revision HZ_COMMIT --rift-revision RIFT_COMMIT \
  --output /tmp/hz-comparison-new --samples 9
```

The output directory must not already exist. Keep it on the filesystem being
measured. Rift's btrfs initialization converts its **disposable fixture** into a
subvolume. The runner leaves source fixtures and `results.json` for inspection;
remove that specific output directory when finished. Failed runs also retain
their fixtures. Never pass a working project as the output directory.

## Workloads and measurements

- `small`: 100 source files and a packed Git repository.
- `development`: 2,000 source files, 20,000 dependency files, and 64 MiB of build
  artifacts, plus a packed Git repository.
- `large-files`: 100 source files and 256 MiB of build artifacts, plus Git.

Each fixture also contains staged, unstaged, and untracked changes. Reported
payload counts exclude SCM metadata and workspace markers. Use `--workloads` to
select a subset.

Full creation compares `hz new --full` with `rift create --copy-all`. Filtered
creation compares `hz new --filtered` with Rift's default. Both disable hooks.
These synthetic fixtures have equivalent filtered payloads; arbitrary real
repositories can expose differences in the tools' filters.

Each mode receives one warmup and randomized paired samples with a fixed seed.
Measurements include create, remove, and GC wall latency, process-tree CPU time,
and child peak RSS. Wall/CPU timings include the small measurement launcher;
GNU time measures RSS separately to avoid counting the Python parent's memory.
Raw samples and median/minimum/maximum summaries are retained. Initialization
is a **single setup observation**, not a sampled performance result.

Validation happens outside the timing window: payload names and sizes every
sample, payload hashes on the warmup and final sample, copied SCM hashes every
sample except intentionally changed HEAD/reflog/exclusion/base-reference files,
and detached HEAD plus staged/unstaged diffs. Remove and GC check both disk and
registry state. This is not an exhaustive metadata or failure-injection suite.

## Interpreting results

These are warm-cache synthetic CLI latency measurements. Run on an otherwise
idle host and retain the recorded CPU, filesystem/mount options, binary hashes,
and revisions. Do not interpret command completion as total asynchronous device
work or space reclamation. Do not add independently calculated medians to claim
a total lifecycle or CPU median; calculate the total per sample first.

On btrfs, Rift full creation uses a native subvolume snapshot; hz uses the
filesystem-independent per-file walker specified in [the design](../docs/design.md).
Filtered creation walks files in both tools. Report these modes separately.
Rift GC may fall back to walking when native subvolume deletion is not permitted
by the mount. Initialization cost and source conversion also differ.

Performance comparisons must preserve hz's identity checks, metadata handling,
explicit copy mode, and interrupted-operation recovery. A faster command that
omits payload or loses state is not an equivalent result.
