#!/usr/bin/env python3
"""Tests for exact-SHA workflow qualification."""

from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path

MODULE_PATH = Path(__file__).with_name("verify-workflow-run.py")
SPEC = importlib.util.spec_from_file_location("verify_workflow_run", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
verifier = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(verifier)


class SuccessfulRunsTests(unittest.TestCase):
    def test_accepts_only_an_exact_successful_trusted_run(self) -> None:
        expected = {
            "head_sha": "abc",
            "head_branch": "main",
            "event": "push",
            "status": "completed",
            "conclusion": "success",
        }
        payload = {
            "workflow_runs": [
                expected,
                {**expected, "head_sha": "other"},
                {**expected, "head_branch": "feature"},
                {**expected, "event": "pull_request"},
                {**expected, "conclusion": "failure"},
            ]
        }

        self.assertEqual(
            verifier.successful_runs(payload, sha="abc", branch="main", event="push"),
            [expected],
        )

    def test_rejects_malformed_api_payload(self) -> None:
        with self.assertRaises(ValueError):
            verifier.successful_runs({}, sha="abc", branch="main", event="push")


if __name__ == "__main__":
    unittest.main()
