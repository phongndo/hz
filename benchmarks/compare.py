#!/usr/bin/env python3
"""Isolated, warm-cache CLI comparison on Linux and macOS."""
import argparse
from contextlib import closing
import fcntl
import hashlib
import json
import os
from pathlib import Path
import platform
import plistlib
import random
import shutil
import signal
import sqlite3
import statistics
import stat
import subprocess
import sys
import tempfile
import time

RSS_DIVISOR = 1
ARTIFACTS = {
    "node_modules", ".pnpm-store", "target", ".venv", "venv", ".tox", ".nox",
    "__pycache__", ".pytest_cache", ".mypy_cache", ".ruff_cache", ".next",
    ".nuxt", ".svelte-kit", ".turbo", ".vite", ".parcel-cache", ".cache",
    "dist", "build", "coverage",
}


def run(argv, cwd, env=None, timeout=120):
    """Measure one process tree, including CLI startup, without output pipe stalls."""
    timer = os.environ.get("HZ_BENCH_TIME") or shutil.which("time")
    with tempfile.TemporaryFile() as out, tempfile.TemporaryFile() as err, tempfile.NamedTemporaryFile() as memory:
        measured = list(map(str, argv))
        if timer:
            measured = [timer, "-f", "%M", "-o", memory.name, "--", *measured]
        started = time.perf_counter_ns()
        proc = subprocess.Popen(measured, cwd=cwd, env=env,
                                stdout=out, stderr=err, start_new_session=True)
        expired = False

        def expire(_sig, _frame):
            nonlocal expired
            expired = True
            os.killpg(proc.pid, signal.SIGKILL)

        previous = signal.signal(signal.SIGALRM, expire)
        signal.alarm(timeout)
        try:
            _, status, usage = os.wait4(proc.pid, 0)
            elapsed = (time.perf_counter_ns() - started) / 1e6
            proc.returncode = os.waitstatus_to_exitcode(status)
        finally:
            signal.alarm(0)
            signal.signal(signal.SIGALRM, previous)
        out.seek(0)
        err.seek(0)
        memory.seek(0)
        memory_text = memory.read().decode().strip().splitlines()
        raw_rss = int(memory_text[-1]) if memory_text and memory_text[-1].isdigit() else None
        rss = raw_rss / RSS_DIVISOR if raw_rss is not None else None
        return dict(argv=list(map(str, argv)), code=proc.returncode,
                    wall_ms=elapsed, user_ms=usage.ru_utime * 1000,
                    system_ms=usage.ru_stime * 1000,
                    cpu_ms=(usage.ru_utime + usage.ru_stime) * 1000, rss_kib=rss, rss_raw=raw_rss,
                    stdout=out.read().decode(errors="replace"),
                    stderr=err.read().decode(errors="replace"), timeout=expired)


def require(result):
    if result["code"] != 0:
        raise RuntimeError(json.dumps(result, indent=2))
    return result


def git_env():
    env = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
    return dict(env, GIT_CONFIG_GLOBAL="/dev/null", GIT_CONFIG_NOSYSTEM="1", GIT_OPTIONAL_LOCKS="0",
                GIT_AUTHOR_NAME="benchmark", GIT_AUTHOR_EMAIL="bench@example.invalid",
                GIT_COMMITTER_NAME="benchmark", GIT_COMMITTER_EMAIL="bench@example.invalid",
                GIT_AUTHOR_DATE="2026-01-01T00:00:00Z", GIT_COMMITTER_DATE="2026-01-01T00:00:00Z")


def git(root, *args, timeout=600):
    # `diff` can refresh the index even with GIT_OPTIONAL_LOCKS=0. Observers
    # must not change its cached stat data between concurrent-child checks.
    return require(run(["git", "-c", "diff.autoRefreshIndex=false", "-C", root, *args],
                       root, git_env(), timeout=timeout))["stdout"].strip()


def git_state(root):
    return {"head": git(root, "rev-parse", "HEAD"),
            "diff": git(root, "diff", "--binary", "--no-ext-diff", "--no-textconv"),
            "cached": git(root, "diff", "--cached", "--binary", "--no-ext-diff", "--no-textconv")}


def write(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)


def fixture(root, kind):
    root.mkdir(parents=True, exist_ok=True)  # it may already be an empty subvolume
    source_count, dependencies, artifacts, artifact_size = {
        "small": (100, 0, 0, 0),
        "development": (2000, 20000, 64, 1024 * 1024),
        "large-files": (100, 0, 8, 32 * 1024 * 1024),
    }[kind]
    for i in range(source_count):
        write(root / "src" / f"package-{i // 100}" / f"file-{i}.txt",
              (f"source {i:08d}\n".encode() * 128))
    write(root / ".gitignore", b"node_modules/\nbuild/\n")
    git(root, "init", "-q", "-b", "main")
    git(root, "add", ".")
    git(root, "commit", "-q", "-m", "fixture")
    # Pack SCM metadata identically; many loose objects are a separate workload.
    git(root, "gc", "--quiet")
    for i in range(dependencies):
        write(root / "node_modules" / f"package-{i // 100}" / f"file-{i}.js",
              (f"dependency {i:08d}\n".encode() * 64))
    rng = random.Random(123456)
    block = rng.randbytes(1024 * 1024)
    for i in range(artifacts):
        path = root / "build" / f"artifact-{i}.bin"
        path.parent.mkdir(exist_ok=True)
        with path.open("wb") as output:
            for _ in range(artifact_size // len(block)):
                output.write(block)
    write(root / "src" / "staged.txt", b"staged changes\n")
    git(root, "add", "src/staged.txt")
    write(root / "src" / "package-0" / "file-0.txt", b"unstaged changes\n")
    write(root / "untracked.txt", b"untracked content\n")
    return payload(root)


def payload(root, filtered=False, hashes=True):
    result = {}
    for directory, dirs, files in os.walk(root):
        for name in sorted(dirs + files):
            file = Path(directory) / name
            relative = file.relative_to(root)
            if (name in {".hz-workspace", ".rift", ".hz-workspaces"} or
                    (directory == str(root) and name == ".git") or
                    (filtered and excluded(relative))):
                if name in dirs:
                    dirs.remove(name)
                continue
            info = file.lstat()
            if stat.S_ISLNK(info.st_mode):
                value = dict(kind="symlink", target=os.readlink(file))
            elif stat.S_ISDIR(info.st_mode):
                value = dict(kind="directory", mode=stat.S_IMODE(info.st_mode))
            elif stat.S_ISREG(info.st_mode):
                value = dict(kind="file", mode=stat.S_IMODE(info.st_mode), size=info.st_size)
                if hashes:
                    with file.open("rb") as stream:
                        value["sha256"] = hashlib.file_digest(stream, "sha256").hexdigest()
            else:
                raise ValueError(f"benchmark source contains a special file: {file}")
            result[relative.as_posix()] = value
        dirs.sort()
    return result


def excluded(relative):
    parts = Path(relative).parts
    if any(part in {".git", ".hg", ".jj"} for part in parts):
        return False
    return any(part in ARTIFACTS for part in parts) or any(
        parent == ".yarn" and child in {"cache", "unplugged", "install-state.gz", "build-state.yml"}
        for parent, child in zip(parts, parts[1:]))


# _IOW(0x94, 14, struct btrfs_ioctl_vol_args), a 4096-byte argument.
BTRFS_IOC_SUBVOL_CREATE = 0x5000940E


def create_subvolume(path):
    """Create `path` as a btrfs subvolume, as hz needs to snapshot a source."""
    arguments = bytearray(4096)
    name = path.name.encode()
    arguments[8:8 + len(name)] = name
    parent = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
    try:
        fcntl.ioctl(parent, BTRFS_IOC_SUBVOL_CREATE, arguments)
    finally:
        os.close(parent)


def copy_fixture(source, destination):
    """Copy a supplied checkout without changing it or losing hardlink groups."""
    links = {}

    def copy_file(src, dst):
        info = os.stat(src, follow_symlinks=False)
        key = (info.st_dev, info.st_ino)
        if info.st_nlink > 1 and key in links:
            os.link(links[key], dst)
        else:
            shutil.copy2(src, dst)
            if info.st_nlink > 1:
                links[key] = dst
        return dst

    shutil.copytree(source, destination, symlinks=True, copy_function=copy_file,
                    dirs_exist_ok=True)
    return payload(destination)


def calibrate_memory():
    # GNU time and the kernel may expose different RSS units on Darwin.
    # Compare with the platform's native rusage, rather than assuming a unit.
    global RSS_DIVISOR
    probe = require(run([sys.executable, "-c",
                         "import resource; a=bytearray(16*1024*1024); "
                         "print(resource.getrusage(resource.RUSAGE_SELF).ru_maxrss)"], Path.cwd()))
    native = int(probe["stdout"])
    expected_kib = native / (1024 if platform.system() == "Darwin" else 1)
    ratio = probe["rss_raw"] / expected_kib
    if 0.8 < ratio < 1.2:
        RSS_DIVISOR = 1
    elif 819 < ratio < 1229:
        RSS_DIVISOR = 1024
    else:
        raise RuntimeError(f"cannot establish GNU time RSS units: {probe}")
    return dict(native_rss=native, gnu_time_rss=probe["rss_raw"], divisor=RSS_DIVISOR)


def host_details(directory):
    if platform.system() == "Darwin":
        # diskutil expects a device or mount point, not an arbitrary directory.
        mount = subprocess.check_output(["df", "-P", str(directory)], text=True).splitlines()[-1].split(None, 5)[5]
        filesystem = plistlib.loads(subprocess.check_output(
            ["/usr/sbin/diskutil", "info", "-plist", mount]))
        return dict(cpu_model=require(run(["sysctl", "-n", "machdep.cpu.brand_string"], directory))["stdout"].strip(),
                    filesystem={key: filesystem.get(key) for key in (
                        "FilesystemType", "FilesystemName", "MountPoint", "DeviceIdentifier", "TotalSize")})
    return dict(cpu_model=next((line.split(":", 1)[1].strip() for line in Path("/proc/cpuinfo").read_text().splitlines()
                               if line.startswith("model name")), "unknown"),
                filesystem=json.loads(require(run(["findmnt", "-T", directory, "-J", "-o", "TARGET,SOURCE,FSTYPE,OPTIONS"], directory))["stdout"]))


class Tool:
    def __init__(self, name, binary, base, copy=False):
        self.name, self.binary, self.base = name, Path(binary).resolve(), Path(base)
        self.source, self.storage = self.base / "source", self.base / "storage"
        self.base.mkdir(parents=True)
        self.copy = copy
        self.expected_git = None
        self.env = git_env()
        self.env["HZ_DATA_DIR"] = str(self.base / "registry")

    def command(self, *args):
        prefix = [self.binary]
        if self.name == "rift":
            prefix += ["--database", self.base / "rift.sqlite"]
        return prefix + list(args)

    def invoke(self, *args, **options):
        # The cwd must be outside the source: Rift init can replace its inode.
        return run(self.command(*args), self.base, self.env, **options)

    def init(self, timeout=120):
        if self.name == "hz":
            return self.invoke("init", self.source, *(["--copy"] if self.copy else []), timeout=timeout)
        return self.invoke("init", "--here", self.source, timeout=timeout)

    def create_args(self, name, filtered=False):
        if self.name == "hz":
            return ["new", name, "--from", self.source, "--into", self.storage,
                    "--filtered" if filtered else "--full", "--no-hooks", "--path-only"]
        return ["create", self.source, "--into", self.storage, "--name", name,
                "--no-hooks", *([] if filtered else ["--copy-all"])]

    def create(self, name, filtered=False):
        return self.invoke(*self.create_args(name, filtered))

    def remove(self, child):
        return self.invoke("remove", child, "--no-hooks")

    def gc(self):
        return self.invoke("gc")


def validate(tool, child, expected, filtered, hashes):
    wanted = {k: v if hashes else {key: value for key, value in v.items() if key != "sha256"}
              for k, v in expected.items() if not (filtered and excluded(k))}
    actual = payload(child, hashes=hashes)
    if actual != wanted:
        changed = [key for key in wanted.keys() & actual.keys() if wanted[key] != actual[key]]
        raise AssertionError(f"{tool.name}: payload differs: missing={list(set(wanted)-set(actual))[:10]}, "
                             f"extra={list(set(actual)-set(wanted))[:10]}, changed={changed[:10]}")
    # HEAD, its reflog, marker exclusions and hz's base ref deliberately change,
    # as does the index's cached stat data; its staged content is checked below.
    mutable = {"HEAD", "logs/HEAD", "info/exclude", "refs/hz", "refs/hz/base", "hz-unborn-base",
               "index"}
    scm_source = {k: v for k, v in payload(tool.source / ".git").items() if k not in mutable}
    scm_child = {k: v for k, v in payload(child / ".git").items() if k not in mutable}
    assert scm_source == scm_child, f"{tool.name}: copied SCM metadata differs"
    if tool.expected_git is None:
        tool.expected_git = git_state(tool.source)
    assert git(child, "rev-parse", "HEAD") == tool.expected_git["head"]
    assert run(["git", "-C", child, "symbolic-ref", "-q", "HEAD"], child,
               tool.env)["code"] == 1
    assert git(child, "diff", "--binary", "--no-ext-diff", "--no-textconv") == tool.expected_git["diff"]
    assert git(child, "diff", "--cached", "--binary", "--no-ext-diff", "--no-textconv") == tool.expected_git["cached"]


def summarize(rows):
    result = {}
    for key in ("wall_ms", "cpu_ms", "rss_kib"):
        values = [row[key] for row in rows]
        result[key] = dict(median=statistics.median(values), min=min(values), max=max(values))
    return result


def check_removed(tool, child, collected=False):
    assert not child.exists()
    if tool.name == "hz":
        with closing(sqlite3.connect(tool.base / "registry/hz.sqlite")) as db:
            rows = db.execute("SELECT state, trash_path FROM workspace WHERE parent_id IS NOT NULL").fetchall()
        if collected:
            assert not rows
        else:
            assert len(rows) == 1 and rows[0][0] == "trashed" and Path(rows[0][1]).is_dir()
    else:
        with closing(sqlite3.connect(tool.base / "rift.sqlite")) as db:
            assert db.execute("SELECT count(*) FROM rift WHERE parent_id IS NOT NULL").fetchone()[0] == 0
            rows = db.execute("SELECT path FROM trash").fetchall()
        if collected:
            assert not rows
        else:
            assert len(rows) == 1 and Path(rows[0][0]).is_dir()
    if collected and tool.storage.exists():
        assert all(path.name == ".trash" and not any(path.iterdir()) for path in tool.storage.iterdir())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--hz", required=True)
    parser.add_argument("--rift", required=True)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--samples", type=int, default=7)
    parser.add_argument("--setup-timeout", type=int, default=600,
                        help="Seconds allowed for each one-time initialization (default: 600)")
    parser.add_argument("--workloads", nargs="+", choices=["small", "development", "large-files"])
    parser.add_argument("--source", type=Path, help="Read-only input checkout; copied into disposable roots")
    parser.add_argument("--modes", nargs="+", choices=["full", "filtered"], default=["full", "filtered"])
    parser.add_argument("--hz-subvolume", action="store_true",
                        help="On btrfs, make hz's disposable source a subvolume so full creates are snapshots")
    parser.add_argument("--hz-revision", default="unspecified")
    parser.add_argument("--rift-revision", default="unspecified")
    args = parser.parse_args()
    args.output = args.output.resolve()
    if args.samples < 3:
        parser.error("use at least three measured samples")
    if args.setup_timeout <= 0:
        parser.error("--setup-timeout must be positive")
    if not __debug__:
        parser.error("validation requires Python assertions; do not use -O")
    if platform.system() not in {"Linux", "Darwin"}:
        parser.error("this benchmark supports Linux and macOS")
    timer = os.environ.get("HZ_BENCH_TIME") or shutil.which("time")
    if not timer or "GNU" not in require(run([timer, "--version"], Path.cwd()))["stdout"]:
        parser.error("GNU time must be on PATH (or set HZ_BENCH_TIME)")
    if args.source:
        args.source = args.source.resolve(strict=True)
        if args.workloads:
            parser.error("--source and --workloads are alternatives")
        if not (args.source / ".git").is_dir() or (args.source / ".git").is_symlink():
            parser.error("--source requires a standalone Git checkout")
        if any((args.source / marker).exists() for marker in (".rift", ".hz-workspace")):
            parser.error("use an unregistered checkout as the benchmark source")
        if args.output.resolve().is_relative_to(args.source):
            parser.error("output must be outside the source checkout")
        if "filtered" in args.modes and any(excluded(path) for path in git(args.source, "ls-files", "-z").split("\0")):
            parser.error("source contains tracked paths matched by filtering; benchmark it with --modes full")
        args.workloads = ["repository"]
    elif not args.workloads:
        args.workloads = ["small", "development", "large-files"]
    args.output.mkdir(parents=True, exist_ok=False)
    report = dict(date=time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
                  platform=platform.platform(), cpu_count=os.cpu_count(),
                  load_start=os.getloadavg(), samples=args.samples, cache="warm; no cache dropping",
                  warmups=1, order_seed=42, workloads={},
                  verified=False,
                  revisions={"hz": args.hz_revision, "rift": args.rift_revision},
                  memory_method="GNU time child peak RSS; wall and CPU include launcher",
                  memory_calibration=calibrate_memory(),
                  **host_details(args.output),
                  initialization="single setup observation per tool/workload; not a sampled benchmark",
                  setup_timeout_seconds=args.setup_timeout,
                  hz_source="btrfs subvolume" if args.hz_subvolume else "directory",
                  git_observer_timeout_seconds=600,
                  validation="payload names, kinds, modes, symlink targets and sizes each sample; payload hashes warmup/last; SCM hashes with documented mutable paths excluded; Git state and removal/GC registry+disk assertions each sample",
                  binaries={name: dict(path=str(Path(binary).resolve()),
                                     sha256=hashlib.sha256(Path(binary).read_bytes()).hexdigest())
                            for name, binary in (("hz", args.hz), ("rift", args.rift))})
    if args.source:
        report["source"] = dict(path=str(args.source), commit=git(args.source, "rev-parse", "HEAD"))
    # Capture semantics from the original checkout, whose index stat cache is
    # still valid. Disposable copies carry different inodes; repeatedly diffing
    # their source indexes would reread every tracked file on large workspaces.
    reference_git = git_state(args.source) if args.source else None
    rng = random.Random(42)
    for kind in args.workloads:
        tools = {name: Tool(name, binary, args.output / kind / name)
                 for name, binary in (("hz", args.hz), ("rift", args.rift))}
        entries = {}
        report["workloads"][kind] = entries
        expected = {}
        for name, tool in tools.items():
            print(kind, name, "preparing disposable source", flush=True)
            if name == "hz" and args.hz_subvolume:
                create_subvolume(tool.source)
            expected[name] = copy_fixture(args.source, tool.source) if args.source else fixture(tool.source, kind)
            tool.expected_git = reference_git if args.source else git_state(tool.source)
            entries[name] = dict(init=tool.init(timeout=args.setup_timeout), modes={})
            (args.output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
            require(entries[name]["init"])
            # A working checkout's index caches its files' stat data. Copying
            # the fixture, or a tool converting it, leaves that stale, so
            # refresh it as Git in the checkout would.
            run(["git", "-C", tool.source, "update-index", "-q", "--refresh"], tool.source, tool.env,
                timeout=args.setup_timeout)
        assert expected["hz"] == expected["rift"]
        for mode in args.modes:
            filtered = mode == "filtered"
            for name in tools:
                entries[name]["modes"][mode] = dict(create=[], first_status=[], remove=[], gc=[])
            for sample in range(-1, args.samples):
                order = list(tools)
                rng.shuffle(order)
                for name in order:
                    tool = tools[name]
                    # Filesystems defer work, such as committing the previous
                    # sample's deletions; flush it so no tool pays another's.
                    os.sync()
                    created = require(tool.create(f"sample-{mode}-{sample + 1}", filtered))
                    child = Path(created["stdout"].strip())
                    if sample >= 0:
                        entries[name]["modes"][mode]["create"].append(created)
                    report["checking"] = dict(workload=kind, mode=mode, sample=sample, tool=name, child=str(child))
                    (args.output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
                    # Until Git's cached stat data matches, the first Git
                    # command in a copy rereads every tracked file.
                    status = require(run(["git", "-C", child, "status", "--porcelain"], child,
                                         tool.env, timeout=600))
                    if sample >= 0:
                        entries[name]["modes"][mode]["first_status"].append(status)
                    print(kind, mode, name, "sample", sample, f"created in {created['wall_ms']:.2f} ms, "
                          f"first git status {status['wall_ms']:.2f} ms; validating", flush=True)
                    validate(tool, child, expected[name], filtered, hashes=sample in (-1, args.samples-1))
                    removed = require(tool.remove(child))
                    check_removed(tool, child)
                    collected = require(tool.gc())
                    check_removed(tool, child, collected=True)
                    print(kind, mode, name, "sample", sample, "verified and collected", flush=True)
                    if sample >= 0:
                        rows = entries[name]["modes"][mode]
                        for operation, row in (("remove", removed), ("gc", collected)):
                            rows[operation].append(row)
            for name in tools:
                rows = entries[name]["modes"][mode]
                rows["summary"] = {op: summarize(rows[op])
                                   for op in ("create", "first_status", "remove", "gc")}
                print(kind, mode, name, json.dumps(rows["summary"]), flush=True)
        entries["fixture"] = dict(payload_files=sum(row["kind"] == "file" for row in expected["hz"].values()),
                                  payload_entries=len(expected["hz"]),
                                  payload_bytes=sum(row.get("size", 0) for row in expected["hz"].values()))
        report["workloads"][kind] = entries
        (args.output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
        # Fixtures remain available for inspection. Cleanup is explicit after review.
    report["load_end"] = os.getloadavg()
    report.pop("checking", None)
    report["verified"] = True
    (args.output / "results.json").write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
