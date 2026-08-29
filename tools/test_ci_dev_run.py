from __future__ import annotations

import os
import runpy
import tempfile
import unittest
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]


def load_runner() -> dict[str, Any]:
    return runpy.run_path(str(ROOT / "scripts" / "dev-run"), run_name="light_dev_run")


def seed_configuration_inputs(root: Path, runner: dict[str, Any]) -> None:
    for relative in runner["CONFIGURE_INPUTS"]:
        path = root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(relative, encoding="utf-8")


def write_current_configuration(root: Path, fingerprint: str) -> None:
    build = root / "build" / "dev"
    (build / "conan").mkdir(parents=True, exist_ok=True)
    (build / "build.ninja").touch()
    (build / "conan" / "conan_toolchain.cmake").touch()
    (build / "CMakeCache.txt").write_text(
        "CMAKE_BUILD_TYPE:STRING=Dev\n"
        "LIGHT_BUILD_TESTS:BOOL=OFF\n"
        "LIGHT_BUILD_BENCHMARKS:BOOL=OFF\n"
        f"CMAKE_HOME_DIRECTORY:INTERNAL={root.resolve()}\n",
        encoding="utf-8",
    )
    (build / ".light-configure-signature").write_text(
        fingerprint + "\n", encoding="ascii"
    )


class DevelopmentRunnerContractTest(unittest.TestCase):
    def test_just_and_dev_shell_light_delegate_to_the_shared_runner(self) -> None:
        justfile = (ROOT / "Justfile").read_text(encoding="utf-8")
        flake = (ROOT / "flake.nix").read_text(encoding="utf-8")
        self.assertIn('exec ./scripts/dev-run "$@"', justfile)
        self.assertIn('exec "$runner" "$@"', flake)

    def test_worktree_namespaces_are_stable_and_distinct(self) -> None:
        runner = load_runner()
        runtime_directory = runner["runtime_directory"]
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            first = root / "first"
            second = root / "second"
            first.mkdir()
            second.mkdir()
            self.assertEqual(
                runtime_directory(first, 1234), runtime_directory(first, 1234)
            )
            self.assertNotEqual(
                runtime_directory(first, 1234), runtime_directory(second, 1234)
            )
            self.assertIn("light-dev-1234-", runtime_directory(first, 1234).name)

    def test_cached_configuration_builds_only_the_checkout_light_target(self) -> None:
        runner = load_runner()
        prepare = runner["prepare"]
        fingerprint_for = runner["configuration_fingerprint"]
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary) / "checkout"
            runtime = Path(temporary) / "runtime"
            build = root / "build" / "dev"
            seed_configuration_inputs(root, runner)
            environment = {"PATH": os.environ.get("PATH", "")}
            write_current_configuration(root, fingerprint_for(root, environment))
            (build / "light").write_bytes(b"current checkout light")

            calls: list[list[str]] = []
            prepare.__globals__["runtime_directory"] = lambda _root: runtime
            prepare.__globals__["run_checked"] = lambda arguments, _root, _environment: (
                calls.append(list(arguments))
            )
            binary, arguments, execution_environment = prepare(
                root, ["argument with spaces", "--version"], environment
            )

            self.assertEqual(
                calls,
                [["cmake", "--build", str(build), "--target", "light"]],
            )
            self.assertEqual(binary, build / "light")
            self.assertEqual(
                arguments,
                [str(build / "light"), "argument with spaces", "--version"],
            )
            self.assertEqual(execution_environment, environment)

    def test_missing_configuration_runs_setup_once_before_incremental_build(
        self,
    ) -> None:
        runner = load_runner()
        prepare = runner["prepare"]
        fingerprint_for = runner["configuration_fingerprint"]
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary) / "checkout"
            runtime = Path(temporary) / "runtime"
            build = root / "build" / "dev"
            seed_configuration_inputs(root, runner)
            environment = {"PATH": os.environ.get("PATH", "")}
            calls: list[list[str]] = []

            def run_checked(
                arguments: list[str], _root: Path, _environment: dict[str, str]
            ) -> None:
                calls.append(list(arguments))
                if arguments[0].endswith("scripts/ci/configure"):
                    write_current_configuration(
                        root, fingerprint_for(root, environment)
                    )
                else:
                    (build / "light").write_bytes(b"configured light")

            prepare.__globals__["runtime_directory"] = lambda _root: runtime
            prepare.__globals__["run_checked"] = run_checked
            prepare(root, ["--version"], environment)
            prepare(root, ["--version"], environment)

            configure_calls = [
                call for call in calls if call[0].endswith("scripts/ci/configure")
            ]
            build_calls = [call for call in calls if call[0] == "cmake"]
            self.assertEqual(len(configure_calls), 1)
            self.assertEqual(len(build_calls), 2)
            self.assertTrue(
                all(call[-2:] == ["--target", "light"] for call in build_calls)
            )

    def test_configuration_fingerprint_tracks_inputs_and_environment(self) -> None:
        runner = load_runner()
        fingerprint_for = runner["configuration_fingerprint"]
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            seed_configuration_inputs(root, runner)
            initial = fingerprint_for(root, {"CC": "clang"})
            changed_environment = fingerprint_for(root, {"CC": "other-clang"})
            (root / "conan.lock").write_text("changed", encoding="utf-8")
            changed_input = fingerprint_for(root, {"CC": "clang"})

            self.assertNotEqual(initial, changed_environment)
            self.assertNotEqual(initial, changed_input)


if __name__ == "__main__":
    unittest.main()
