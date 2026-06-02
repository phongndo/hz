# Benchmarks

`hz diff` performance work starts with deterministic fixtures that can be reused
by hz benchmarks and by competitor runs against Hunk or other diff viewers.

Generate the standard fixture suite into `target/`:

```sh
cargo run -p hz-bench -- fixtures --out target/bench-fixtures --force
```

Generate one scenario:

```sh
cargo run -p hz-bench -- fixtures \
  --out target/bench-fixtures \
  --scenario many-small-files \
  --force
```

Add the opt-in larger stress fixture:

```sh
cargo run -p hz-bench -- fixtures --out target/bench-fixtures --stress --force
```

Each scenario directory contains:

```text
repo/            git repository with benchmark working-tree state
patch.diff       primary all-changes patch, including synthetic untracked files
head.patch       same all-changes patch for HEAD-vs-worktree mode
unstaged.patch   unstaged patch, including synthetic untracked files
staged.patch     staged patch
pair/            before/after files for direct file comparison benchmarks
manifest.json    scenario metadata and expected text stats
```

Patch-mode benchmarks can bypass Git setup and isolate parser/viewer costs:

```sh
hz diff --patch target/bench-fixtures/balanced-changeset/patch.diff >/dev/null
hz diff --patch target/bench-fixtures/balanced-changeset/patch.diff --stat >/dev/null
```

Standard scenarios:

- `many-small-files`
- `balanced-changeset`
- `large-single-file`
- `many-untracked-small`
- `few-untracked-large`
- `minified-one-line`
- `binary-files`
- `staged-unstaged`

The opt-in `huge-mixed-stress` scenario is intentionally larger and should be
used for max-size, memory, and scroll-latency work rather than default local
smoke checks.
