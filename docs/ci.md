# Continuous integration

`hz` separates fast pull-request qualification from broader scheduled coverage
and release publishing. Third-party GitHub Actions are pinned to immutable
commits, checkout credentials are not persisted, and workflows use the minimum
permissions they need.

## Pull requests and main

[quality.yml](../.github/workflows/quality.yml) runs for pull requests, merge
queues, and pushes to `main`. The change classifier selects only affected lanes:

- **Rust correctness** — formatting, Clippy with warnings denied, and all
  workspace tests with all targets and features.
- **MSRV** — `cargo check` on the declared Rust 1.85 minimum.
- **Integration** — CLI/shell generation, zsh behavior, offline installer and
  updater smoke tests, and the distribution archive contract.
- **Performance** — a deterministic, small end-to-end `hz-bench` exercise.
- **Workflow lint** — `actionlint` for GitHub Actions and CI orchestration.

Documentation-only changes skip expensive lanes. Unknown files and changes to
CI orchestration deliberately select every lane. The classifier has unit tests
and falls back to full validation when it cannot establish a complete Git diff.
It also runs `git diff --check`.

Use **CI gate** as the required branch-protection check. It has a stable name
and verifies that every selected conditional job succeeded; requiring a
conditional lane directly can leave pull requests waiting when that lane is
correctly skipped.

Equivalent local commands are:

```sh
just ci-rust
just ci-integration
just ci-performance
just ci-workflows
# Or all four:
just ci-check
```

## Extended validation

[extended.yml](../.github/workflows/extended.yml) runs daily and on manual
dispatch. It adds rust-analyzer diagnostics, native tests on Intel and Arm Linux
and macOS, platform binary smoke tests, and a larger release-mode benchmark.
These jobs provide broad signal without delaying every pull request.

## Releases

[release.yml](../.github/workflows/release.yml) publishes only an exact source
commit that:

1. is reachable from the default branch,
2. has a version matching the release tag, and
3. has a successful trusted `push` run of `quality.yml` for that exact SHA.

The reusable [build-dist.yml](../.github/workflows/build-dist.yml) builds four
native archives from the qualified SHA. Each archive contains `hz`, `README.md`,
`LICENSE`, and `REVISION`, and is published with a SHA-256 checksum. Publishing
verifies the number and checksums of all downloaded artifacts before creating
or updating the GitHub release.

A manual release must run from the current default-branch tip. The optional
`version` input is an additional guard, not a version override.

## Repository settings

Configure the `main` branch ruleset to require pull requests and the
`CI / CI gate` check. Require `PR Template / Required PR fields` as well if PR
metadata is policy. Enable the merge queue only with the existing
`merge_group` trigger. Configure the `release` GitHub Environment with any
additional branch or reviewer protections desired for publishing.
