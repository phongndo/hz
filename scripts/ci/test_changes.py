#!/usr/bin/env python3
"""Tests for the CI change classifier."""

from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path

MODULE_PATH = Path(__file__).with_name("changes.py")
SPEC = importlib.util.spec_from_file_location("ci_changes", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
changes = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(changes)


class ClassifyPathsTests(unittest.TestCase):
    def selected(self, *paths: str) -> set[str]:
        result = changes.classify_paths(paths)
        return {lane for lane, enabled in result.items() if enabled}

    def test_docs_only_selects_no_expensive_lane(self) -> None:
        self.assertEqual(self.selected("README.md", "docs/cli.md"), set())

    def test_rust_selects_correctness_and_runtime_lanes(self) -> None:
        self.assertEqual(
            self.selected("crates/hz-cli/src/main.rs"),
            {"rust", "integration", "performance"},
        )

    def test_shell_scripts_select_integration(self) -> None:
        self.assertEqual(self.selected("scripts/install.sh"), {"integration"})

    def test_workflow_or_ci_change_selects_every_lane(self) -> None:
        expected = set(changes.LANES)
        self.assertEqual(self.selected(".github/workflows/quality.yml"), expected)
        self.assertEqual(self.selected("scripts/ci/rust"), expected)

    def test_repository_tooling_selects_rust_and_workflow_lint(self) -> None:
        self.assertEqual(self.selected("hk.pkl"), {"rust", "workflows"})

    def test_unknown_path_fails_safe(self) -> None:
        self.assertEqual(self.selected("new-build-input.txt"), set(changes.LANES))


if __name__ == "__main__":
    unittest.main()
