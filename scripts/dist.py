#!/usr/bin/env python3
"""Release tooling: the tag to release, the Homebrew formula for a release's archives, and an
end-to-end check of scripts/install.sh and `hz update` against a built archive."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import re
import shutil
import stat
import subprocess
import tarfile
import tempfile
from pathlib import Path
from typing import Optional

ROOT = Path(__file__).resolve().parents[1]
REPOSITORY = "phongndo/hz"
TARGETS = {
    ("Darwin", "arm64"): "aarch64-apple-darwin",
    ("Linux", "aarch64"): "aarch64-unknown-linux-musl",
    ("Linux", "x86_64"): "x86_64-unknown-linux-musl",
}
# hz init's exit status where the filesystem cannot clone (docs/cli.md).
COW_UNAVAILABLE = 11


def release_tag() -> str:
    """v + the version in CMakeLists.txt's project(), which every build reports."""
    match = re.search(
        r"project\(\s*hz\s+VERSION\s+([0-9]+\.[0-9]+\.[0-9]+)\b",
        (ROOT / "CMakeLists.txt").read_text(),
    )
    if match is None:
        raise SystemExit("CMakeLists.txt has no project(hz VERSION x.y.z)")
    return f"v{match[1]}"


def checksum(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def read_checksum(path: Path) -> str:
    match = re.fullmatch(r"([0-9a-f]{64})  (\S+)\n?", path.read_text())
    if match is None or match[2] != path.name.removesuffix(".sha256"):
        raise SystemExit(f"not a checksum file for its archive: {path}")
    return match[1]


def write_checksum(archive: Path) -> None:
    Path(f"{archive}.sha256").write_text(f"{checksum(archive)}  {archive.name}\n")


def formula(checksums: Path, base_url: str) -> str:
    """The formula is hz-cli because homebrew-core's hz is another project's command."""
    tag = release_tag()

    def source(target: str, indent: str) -> list[str]:
        asset = f"hz-{tag}-{target}.tar.gz"
        digest = read_checksum(checksums / f"{asset}.sha256")
        return [f'{indent}url "{base_url}/{asset}"', f'{indent}sha256 "{digest}"']

    return "\n".join(
        [
            "class HzCli < Formula",
            '  desc "Copy-on-write workspaces for parallel humans and agents"',
            f'  homepage "https://github.com/{REPOSITORY}"',
            f'  version "{tag[1:]}"',
            '  license "MIT"',
            "",
            "  on_macos do",
            "    depends_on arch: :arm64",
            "    depends_on macos: :sonoma",
            "",
            "    on_arm do",
            *source("aarch64-apple-darwin", "      "),
            "    end",
            "  end",
            "",
            "  on_linux do",
            "    on_arm do",
            *source("aarch64-unknown-linux-musl", "      "),
            "    end",
            "",
            "    on_intel do",
            *source("x86_64-unknown-linux-musl", "      "),
            "    end",
            "  end",
            "",
            '  conflicts_with "hz", because: "both install an `hz` binary"',
            "",
            "  def install",
            '    bin.install "bin/hz"',
            '    doc.install Dir["share/doc/hz/*"]',
            "  end",
            "",
            "  test do",
            '    assert_match "hz #{version}", shell_output("#{bin}/hz --version")',
            "",
            '    ENV["HZ_DATA_DIR"] = (testpath/"data").to_s',
            '    (testpath/"app/README").write "hello"',
            '    cd testpath/"app" do',
            '      system bin/"hz", "init", "--copy"',
            '      child = shell_output("#{bin}/hz new child --path-only").chomp',
            '      assert_equal "hello", (Pathname(child)/"README").read',
            "    end",
            "  end",
            "end",
            "",
        ]
    )


def extract(archive: Path, directory: Path) -> Path:
    directory.mkdir(parents=True, exist_ok=True)
    subprocess.run(["tar", "-xzf", str(archive), "-C", str(directory)], check=True)
    return directory / archive.name.removesuffix(".tar.gz")


def retag(archive: Path, tag: str, output: Path) -> Path:
    """The same archive published as another release, to exercise updating to it."""
    new = archive.name.removesuffix(".tar.gz").replace(f"-{release_tag()}-", f"-{tag}-")
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as temporary:
        extracted = extract(archive, Path(temporary))
        retagged = output / f"{new}.tar.gz"
        with tarfile.open(retagged, "w:gz") as tar:
            tar.add(extracted, arcname=new)
    write_checksum(retagged)
    return retagged


def check(archive: Path) -> None:
    """Installs the archive as a user would, then updates, reinstalls and fails to install, and
    finally makes a workspace with the installed hz."""
    tag = release_tag()
    if read_checksum(Path(f"{archive}.sha256")) != checksum(archive):
        raise SystemExit(f"{archive.name} does not match its checksum")
    with tempfile.TemporaryDirectory(prefix="hz-install-") as temporary:
        root = Path(temporary).resolve()
        home = root / "home"
        home.mkdir()
        # Nothing from the caller's environment but PATH, so hz and git use the empty HOME.
        environment = {
            "PATH": os.environ["PATH"],
            "HOME": str(home),
            "TERM": "dumb",
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_AUTHOR_NAME": "hz",
            "GIT_AUTHOR_EMAIL": "hz@example.invalid",
            "GIT_COMMITTER_NAME": "hz",
            "GIT_COMMITTER_EMAIL": "hz@example.invalid",
            "HZ_VERSION": tag,
            # Spaces, to catch unquoted paths in the installer.
            "HZ_INSTALL_DIR": str(root / "command dir"),
            "HZ_RELEASES_DIR": str(root / "releases dir"),
            "HZ_RELEASE_BASE_URL": archive.parent.as_uri(),
        }
        command = root / "command dir/hz"
        releases = root / "releases dir"

        def execute(*arguments: str, cwd: Optional[Path] = None, **overrides: Optional[str]):
            """Runs a command; an override of None removes that variable."""
            env = {
                name: value
                for name, value in {**environment, **overrides}.items()
                if value is not None
            }
            return subprocess.run(
                arguments,
                env=env,
                cwd=cwd,
                capture_output=True,
                text=True,
                timeout=120,
                check=False,
            )

        def run(
            *arguments: str,
            ok: bool = True,
            cwd: Optional[Path] = None,
            **overrides: Optional[str],
        ) -> str:
            result = execute(*arguments, cwd=cwd, **overrides)
            if (result.returncode == 0) != ok:
                raise SystemExit(
                    f"{' '.join(arguments)} exited {result.returncode}:\n"
                    f"{result.stdout}{result.stderr}"
                )
            return result.stdout + result.stderr

        def installed() -> Path:
            """The release the command runs, after checking it is the only one kept."""
            target = command.readlink()
            if not run(str(command), "--version").startswith("hz "):
                raise SystemExit(f"{command} does not run")
            if [path.name for path in releases.iterdir()] != [target.parents[1].name]:
                raise SystemExit(f"expected only the installed release in {releases}")
            return target

        installer = str(ROOT / "scripts/install.sh")
        run("sh", installer)
        first = installed()
        if run(str(command), "--version").strip() != f"hz {tag[1:]}":
            raise SystemExit(f"{command} is not hz {tag}")
        if "already installed" not in run(str(command), "update", tag):
            raise SystemExit("hz update reinstalled the running release")
        run("sh", installer)  # a reinstall replaces the release directory
        if installed() == first:
            raise SystemExit("reinstalling kept the old release directory")

        # `hz update` reruns the installer shipped in the release, which drops the old release
        # and nothing else in the directory.
        unrelated = releases / "v1-not-a-release"
        unrelated.mkdir()
        newer = retag(archive, "v99.0.0", root / "newer")
        run(str(command), "update", "99.0.0", HZ_RELEASE_BASE_URL=newer.parent.as_uri())
        if not unrelated.is_dir():
            raise SystemExit("the installer removed a directory it did not make")
        unrelated.rmdir()
        if not installed().parents[1].name.startswith("v99.0.0-"):
            raise SystemExit("hz update did not install the requested release")

        # A download that fails its checksum leaves the installation alone.
        before = command.readlink()
        broken = root / "broken"
        broken.mkdir()
        shutil.copy2(archive, broken / archive.name)
        Path(broken / f"{archive.name}.sha256").write_text(f"{'0' * 64}  {archive.name}\n")
        output = run("sh", installer, ok=False, HZ_RELEASE_BASE_URL=broken.as_uri())
        if "checksum verification failed" not in output or command.readlink() != before:
            raise SystemExit("a corrupt download changed the installation")

        # The installer leaves commands it did not install alone, and `hz update` refuses an hz
        # that nothing it knows installed.
        foreign = root / "foreign"
        foreign.mkdir()
        (foreign / "hz").write_text("#!/bin/sh\n")
        run("sh", installer, ok=False, HZ_INSTALL_DIR=str(foreign))
        extracted = extract(archive, root / "extracted") / "bin/hz"
        if "can only update an hz installed by" not in run(str(extracted), "update", ok=False):
            raise SystemExit("hz update did not refuse an hz that nothing it knows installed")

        # With the default directories, the releases share hz's data directory on Linux; it
        # must stay private, and hz must still keep its registry there.
        defaults = {"HZ_INSTALL_DIR": None, "HZ_RELEASES_DIR": None}
        run("sh", installer, **defaults)
        command = home / ".local/bin/hz"
        share = home / ".local/share/hz"
        if not (share / "releases").is_dir() or stat.S_IMODE(share.stat().st_mode) & 0o077:
            raise SystemExit(f"the installer left {share} missing or readable by others")

        # The installed hz makes a workspace of a Git repository and reads its status.
        project = root / "app"
        (project / "src").mkdir(parents=True)
        (project / "src/main.c").write_text("int main(void) { return 0; }\n")
        run("git", "init", "-q", "-b", "main", cwd=project)
        run("git", "add", ".", cwd=project)
        run("git", "commit", "-q", "-m", "initial", cwd=project)
        # Clone where the filesystem can, as hz init does; copy elsewhere (ext4, tmpfs).
        init = execute(str(command), "--machine", "init", cwd=project)
        if init.returncode == COW_UNAVAILABLE:
            run(str(command), "--machine", "init", "--copy", cwd=project)
        elif init.returncode != 0:
            raise SystemExit(f"hz init exited {init.returncode}:\n{init.stdout}{init.stderr}")
        created = json.loads(run(str(command), "--machine", "new", "child", cwd=project))
        child = Path(created["workspace"]["path"])
        if (child / "src/main.c").read_text() != (project / "src/main.c").read_text():
            raise SystemExit(f"hz new did not copy the project into {child}")
        status = json.loads(run(str(command), "--machine", "git", "status", "child", cwd=project))
        if status["dirty"] or status["workspace"]["id"] != created["workspace"]["id"]:
            raise SystemExit(f"hz git status reported an unexpected state:\n{status}")
        data = share if platform.system() == "Linux" else home / "Library/Application Support/hz"
        if not (data / "hz.sqlite").is_file():
            raise SystemExit(f"hz did not keep its registry in {data}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("version", help="print the tag to release")
    formula_parser = commands.add_parser("formula", help="write the Homebrew formula")
    formula_parser.add_argument("--checksums", type=Path, required=True)
    formula_parser.add_argument(
        "--base-url",
        default=f"https://github.com/{REPOSITORY}/releases/download/{release_tag()}",
    )
    formula_parser.add_argument("--output", type=Path, required=True)
    check_parser = commands.add_parser("check", help="exercise install.sh with an archive")
    check_parser.add_argument("archives", type=Path, help="directory of built archives")
    args = parser.parse_args()

    if args.command == "version":
        print(release_tag())
    elif args.command == "formula":
        args.output.write_text(formula(args.checksums, args.base_url.rstrip("/")))
    elif args.command == "check":
        target = TARGETS[(platform.system(), platform.machine())]
        archive = args.archives.resolve() / f"hz-{release_tag()}-{target}.tar.gz"
        check(archive)
        print(f"{archive.name}: install, update, refusal and workspace checks passed")


if __name__ == "__main__":
    main()
