# Workspace benchmarks

These runners compare optimized hz and [Rift](https://github.com/anomalyco/rift)
**CLI commands**, including process startup and registry access, on Linux and
macOS. They use disposable workspaces and isolated registries. A supplied source
checkout is copied first and never initialized by either tool.

## Running locally

The optional benchmark shell provides Python, Git, GNU time, Rust, and Node 22:

```sh
nix develop .#benchmark -c just release
```

Build a pinned Rift checkout with `cargo build --release --locked -p rift-cli`
inside that shell. The CI comparison pins Rift to
`5afb72b492a4aa0c81c2fbfe39a709aebf3773c7` (0.0.12).
Run with absolute binary paths and the revisions actually built:

```sh
nix develop .#benchmark -c python3 benchmarks/compare.py \
  --hz /absolute/path/to/hz/build/release/hz \
  --rift /absolute/path/to/rift/target/release/rift \
  --hz-revision HZ_COMMIT --rift-revision RIFT_COMMIT \
  --output /tmp/hz-comparison-new --samples 7
```

The output directory must not already exist. Keep it on the filesystem being
measured. Rift's btrfs initialization converts its **disposable fixture** into a
subvolume. Runners leave source fixtures, logs, and JSON results for inspection;
remove that specific output directory when finished. Failed runs retain their
fixtures. Never pass a working project as the output directory.

## Workloads

Synthetic fixtures include staged, unstaged, and untracked changes:

- `small`: 100 source files and a packed Git repository.
- `development`: 2,000 source files, 20,000 dependency files, and 64 MiB of build
  artifacts, plus a packed Git repository.
- `large-files`: 100 source files and 256 MiB of build artifacts, plus Git.

Use `--workloads` to select a subset. Payload counts exclude SCM metadata and
workspace markers. Full creation compares `hz new --full` with
`rift create --copy-all`; filtered creation compares `hz new --filtered` with
Rift's default. Both disable hooks. The synthetic filtered payloads are
equivalent; arbitrary repositories can expose differences in filtering.

For real workspaces, `prepare.py` fetches a pinned revision into a fresh directory
and records its preparation and provenance in a JSON file beside it:

```sh
nix develop .#benchmark -c python3 benchmarks/prepare.py typescript /tmp/ts-fixture
nix develop .#benchmark -c python3 benchmarks/compare.py \
  --hz /absolute/path/to/hz/build/release/hz \
  --rift /absolute/path/to/rift/target/release/rift \
  --source /tmp/ts-fixture --modes full --samples 5 \
  --hz-revision HZ_COMMIT --rift-revision RIFT_COMMIT \
  --output /tmp/hz-typescript-new
```

Available public workloads:

- **TypeScript v5.9.3:** source, locked npm dependencies, and compiler output.
  Dependency lifecycle scripts are disabled; the compiler build runs explicitly.
- **Linux v6.12:** complete shallow Git source checkout, without a kernel build.

The exact revisions live in `prepare.py`. `--source` also accepts another
standalone Git checkout, including local changes. It preserves symlinks and
hardlink groups while preparing fixtures. Use `--modes full` for public projects;
filtered mode is rejected when it would omit tracked paths. Source fixtures must
remain idle while copied. LLVM, Rust, and Zig are possible future workloads;
they are not part of the current comparison.

## Measurements and validation

Each mode receives one warmup and randomized paired samples with a fixed seed.
Measurements include create, remove, and GC wall latency, process-tree CPU time,
and child peak RSS. Wall/CPU timings include the measurement launcher; GNU time
measures RSS separately to avoid counting the Python parent's memory. RSS units
are calibrated against the host's native resource accounting. Raw samples and
median/minimum/maximum summaries are retained. Initialization is a **single setup
observation**, not a sampled performance result.

Validation runs outside the timing window. Every sample checks payload names,
entry types, modes, sizes, empty directories, symlink targets, copied SCM hashes,
detached HEAD, and staged/unstaged diffs. Payload contents are hashed on warmup
and the final sample. Only deliberate SCM mutations (HEAD, its reflog, marker
exclusions, and hz's base reference) are excluded from the SCM hash comparison.
Git index auto-refresh is disabled so observers do not change the data checked
for subsequent children. Remove and GC check both disk and registry state. A
completed run sets `verified: true`; partial JSON is not a successful result.
This is not an exhaustive metadata or failure-injection suite.

Concurrent create bursts and hz restore are checked separately:

```sh
nix develop .#benchmark -c python3 benchmarks/concurrency.py \
  --hz /absolute/path/to/hz/build/release/hz \
  --rift /absolute/path/to/rift/target/release/rift \
  --workers 4 --rounds 3 --output /tmp/hz-concurrency-new
```

This uses the synthetic development fixture. Barrier-released bursts first run
without retries, recording successes and lock conflicts, then with bounded
retries for operation-lock/SQLite-busy errors only. Other errors and timeouts
fail validation. Every successful child is validated, removed, and collected;
the retry phase additionally restores and revalidates hz children. Rift has no
restore command. Concurrent wall times include scheduling and retry delays;
there is no concurrent CPU/RSS comparison. Restore timings are diagnostic
observations without a separate warmup.

## Filesystem matrix

The manual [benchmark workflow](../.github/workflows/benchmark.yml) runs pinned
TypeScript, Linux, or synthetic fixtures on btrfs, XFS, and macOS/APFS, followed
by concurrency/restore checks. For example:

```sh
gh workflow run benchmark.yml -f workload=typescript -f samples=5
```

Download the run's artifacts for raw JSON, logs, and fixture provenance. Shared
CI hosts are useful for portability and correctness evidence; their timings
are noisy and should not be compared across hosts as filesystem rankings.

## Interpreting results

These are warm-cache CLI latency measurements. Run on an otherwise idle host
and retain the recorded CPU, filesystem/mount options, binary hashes, and
revisions. Do not interpret command completion as total asynchronous device
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
