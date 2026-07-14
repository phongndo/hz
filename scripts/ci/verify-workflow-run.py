#!/usr/bin/env python3
"""Require a successful trusted workflow run for an exact source commit."""

from __future__ import annotations

import argparse
import json
import sys
from typing import Any


def successful_runs(
    payload: dict[str, Any], *, sha: str, branch: str, event: str
) -> list[dict[str, Any]]:
    runs = payload.get("workflow_runs")
    if not isinstance(runs, list):
        raise ValueError("GitHub response does not contain workflow_runs")

    return [
        run
        for run in runs
        if run.get("head_sha") == sha
        and run.get("head_branch") == branch
        and run.get("event") == event
        and run.get("status") == "completed"
        and run.get("conclusion") == "success"
    ]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--sha", required=True)
    parser.add_argument("--branch", required=True)
    parser.add_argument("--event", default="push")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        payload = json.load(sys.stdin)
        matches = successful_runs(
            payload, sha=args.sha, branch=args.branch, event=args.event
        )
    except (json.JSONDecodeError, ValueError) as error:
        print(f"::error::Could not inspect CI workflow runs: {error}", file=sys.stderr)
        return 2

    if not matches:
        print(
            "::error::Refusing to publish: "
            f"{args.sha} has no successful CI push run on {args.branch}.",
            file=sys.stderr,
        )
        return 1

    latest = max(matches, key=lambda run: run.get("run_started_at", ""))
    print(f"Qualified by {latest.get('html_url', 'a successful CI run')}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
