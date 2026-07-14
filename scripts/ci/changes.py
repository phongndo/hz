#!/usr/bin/env python3
"""Classify a Git diff into the CI lanes that it can affect."""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
from collections.abc import Iterable

LANES = ("rust", "integration", "performance", "workflows")
ZERO_SHA = "0" * 40


def classify_paths(paths: Iterable[str]) -> dict[str, bool]:
    result = {lane: False for lane in LANES}

    for raw_path in paths:
        path = raw_path.removeprefix("./")

        # CI orchestration validates every lane. This prevents a broken
        # classifier or conditional job from validating only itself.
        if path.startswith(".github/workflows/") or path.startswith("scripts/ci/"):
            return {lane: True for lane in LANES}

        if path.startswith("crates/"):
            result["rust"] = True
            result["integration"] = True
            result["performance"] = True
            continue

        if path.startswith("scripts/") or path.startswith(".hz/"):
            result["integration"] = True
            continue

        if path.startswith("benchmarks/"):
            result["performance"] = True
            continue

        if path in {"Cargo.toml", "Cargo.lock", "rust-toolchain.toml"} or (
            path.startswith("crates/") and path.endswith("/Cargo.toml")
        ):
            result["rust"] = True
            result["integration"] = True
            result["performance"] = True
            continue

        if path in {"flake.nix", "flake.lock", "hk.pkl", "Justfile", "justfile"}:
            result["rust"] = True
            result["workflows"] = True
            continue

        if path.startswith(".github/"):
            result["workflows"] = True
            continue

        # Known documentation and repository metadata cannot affect a binary.
        if (
            path.startswith("docs/")
            or path in {"README.md", "CONTRIBUTING.md", "LICENSE", ".gitignore"}
        ):
            continue

        # Unknown paths get the safe fallback. New build inputs must not
        # silently bypass CI until this classifier learns about them.
        return {lane: True for lane in LANES}

    return result


def changed_paths(base: str | None, head: str) -> list[str] | None:
    if not base or base == ZERO_SHA:
        return None

    for revision in (base, head):
        check = subprocess.run(
            ["git", "cat-file", "-e", f"{revision}^{{commit}}"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
        )
        if check.returncode != 0:
            return None

    completed = subprocess.run(
        [
            "git",
            "diff",
            "--name-only",
            "--diff-filter=ACDMRTUXB",
            f"{base}...{head}",
        ],
        text=True,
        capture_output=True,
        check=False,
    )
    if completed.returncode != 0:
        print(completed.stderr, file=sys.stderr, end="")
        return None
    return [line for line in completed.stdout.splitlines() if line]


def write_outputs(result: dict[str, bool], output_path: str | None) -> None:
    lines = [f"{lane}={'true' if result[lane] else 'false'}" for lane in LANES]
    if output_path:
        with open(output_path, "a", encoding="utf-8") as output:
            output.write("\n".join(lines) + "\n")
    else:
        print("\n".join(lines))


def write_summary(paths: list[str] | None, result: dict[str, bool]) -> None:
    summary_path = os.environ.get("GITHUB_STEP_SUMMARY")
    if not summary_path:
        return

    selected = ", ".join(lane for lane in LANES if result[lane]) or "none (docs only)"
    diff = "full validation fallback" if paths is None else f"{len(paths)} changed path(s)"
    with open(summary_path, "a", encoding="utf-8") as summary:
        summary.write("## CI impact\n\n")
        summary.write(f"- Diff: {diff}\n")
        summary.write(f"- Selected lanes: {selected}\n")
        if paths:
            summary.write("\n<details><summary>Changed paths</summary>\n\n```text\n")
            summary.write("\n".join(paths))
            summary.write("\n```\n</details>\n")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--base", default=os.environ.get("BASE_SHA"))
    parser.add_argument("--head", default=os.environ.get("HEAD_SHA", "HEAD"))
    parser.add_argument("--output", default=os.environ.get("GITHUB_OUTPUT"))
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    paths = changed_paths(args.base, args.head)
    if paths is not None:
        subprocess.run(
            ["git", "diff", "--check", f"{args.base}...{args.head}", "--", "."],
            check=True,
        )
    result = {lane: True for lane in LANES} if paths is None else classify_paths(paths)

    if paths is None:
        print("Unable to establish a complete base diff; selecting every CI lane.")
    else:
        print(f"Classified {len(paths)} changed path(s).")
        for path in paths:
            print(f"  {path}")

    write_outputs(result, args.output)
    write_summary(paths, result)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
