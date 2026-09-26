#!/usr/bin/env python3
"""Regression checks for benchmark observers."""
from pathlib import Path
import sys
import tempfile
import unittest

sys.dont_write_bytecode = True
from compare import copy_fixture, fixture, git


class GitObserverTests(unittest.TestCase):
    def test_diff_preserves_copied_index(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / 'source'
            child = Path(directory) / 'child'
            fixture(source, 'small')
            copy_fixture(source, child)
            index = child / '.git/index'
            before = index.read_bytes()
            self.assertIn('unstaged changes', git(child, 'diff', '--binary', '--no-ext-diff', '--no-textconv'))
            self.assertIn('staged changes', git(child, 'diff', '--cached', '--binary', '--no-ext-diff', '--no-textconv'))
            self.assertEqual(before, index.read_bytes(), 'validation must leave copied index bytes unchanged')


if __name__ == '__main__':
    unittest.main()
