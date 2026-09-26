#!/usr/bin/env python3
"""Isolated, warm-cache CLI comparison. Requires Linux, Python 3 and Git."""
import argparse
from contextlib import closing
import hashlib
import json
import os
from pathlib import Path
import platform
import random
import shutil
import signal
import sqlite3
import statistics
import subprocess
import tempfile
import time


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
        rss = int(memory_text[-1]) if memory_text and memory_text[-1].isdigit() else None
        return dict(argv=list(map(str, argv)), code=proc.returncode,
                    wall_ms=elapsed, user_ms=usage.ru_utime * 1000,
                    system_ms=usage.ru_stime * 1000,
                    cpu_ms=(usage.ru_utime + usage.ru_stime) * 1000, rss_kib=rss,
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


def git(root, *args):
    return require(run(["git", "-C", root, *args], root, git_env()))["stdout"].strip()


def write(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)


def fixture(root, kind):
    root.mkdir(parents=True)
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
        dirs[:] = sorted(d for d in dirs if d != ".git" and
                         not (filtered and d in {"node_modules", "build"}))
        for name in sorted(files):
            if name in {".hz-workspace", ".rift"}:
                continue
            file = Path(directory) / name
            relative = file.relative_to(root).as_posix()
            value = dict(size=file.stat().st_size)
            if hashes:
                with file.open("rb") as stream:
                    value["sha256"] = hashlib.file_digest(stream, "sha256").hexdigest()
            result[relative] = value
    return result


class Tool:
    def __init__(self, name, binary, base, copy=False):
        self.name, self.binary, self.base = name, Path(binary).resolve(), Path(base)
        self.source, self.storage = self.base / "source", self.base / "storage"
        self.base.mkdir(parents=True)
        self.copy = copy
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

    def init(self):
        if self.name == "hz":
            return self.invoke("init", self.source, *(["--copy"] if self.copy else []))
        return self.invoke("init", "--here", self.source)

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
    wanted = {k: v if hashes else {"size": v["size"]} for k, v in expected.items()
              if not (filtered and k.split("/")[0] in {"node_modules", "build"})}
    actual = payload(child, hashes=hashes)
    if actual != wanted:
        raise AssertionError(f"{tool.name}: payload differs: missing={set(wanted)-set(actual)}, "
                             f"extra={set(actual)-set(wanted)}")
    # HEAD, its reflog, marker exclusions and hz's base ref deliberately change.
    mutable = {"HEAD", "logs/HEAD", "info/exclude", "refs/hz/base", "hz-unborn-base"}
    scm_source = {k: v for k, v in payload(tool.source / ".git").items() if k not in mutable}
    scm_child = {k: v for k, v in payload(child / ".git").items() if k not in mutable}
    assert scm_source == scm_child, f"{tool.name}: copied SCM metadata differs"
    assert git(child, "rev-parse", "HEAD") == git(tool.source, "rev-parse", "HEAD")
    assert run(["git", "-C", child, "symbolic-ref", "-q", "HEAD"], child,
               tool.env)["code"] == 1
    assert git(child, "diff", "--binary") == git(tool.source, "diff", "--binary")
    assert git(child, "diff", "--cached", "--binary") == git(tool.source, "diff", "--cached", "--binary")


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
    parser.add_argument("--workloads", nargs="+", default=["small", "development", "large-files"])
    parser.add_argument("--hz-revision", default="unspecified")
    parser.add_argument("--rift-revision", default="unspecified")
    args = parser.parse_args()
    if args.samples < 3:
        parser.error("use at least three measured samples")
    if platform.system() != "Linux":
        parser.error("this benchmark currently measures Linux only")
    timer = os.environ.get("HZ_BENCH_TIME") or shutil.which("time")
    if not timer or "GNU" not in require(run([timer, "--version"], Path.cwd()))["stdout"]:
        parser.error("GNU time must be on PATH (or set HZ_BENCH_TIME)")
    unknown = set(args.workloads) - {"small", "development", "large-files"}
    if unknown:
        parser.error(f"unknown workloads: {sorted(unknown)}")
    args.output.mkdir(parents=True, exist_ok=False)
    report = dict(date=time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
                  platform=platform.platform(), cpu_count=os.cpu_count(),
                  load_start=os.getloadavg(), samples=args.samples, cache="warm; no cache dropping",
                  warmups=1, order_seed=42, workloads={},
                  revisions={"hz": args.hz_revision, "rift": args.rift_revision},
                  memory_method="GNU time child peak RSS; wall and CPU include launcher",
                  cpu_model=next((line.split(":", 1)[1].strip() for line in Path("/proc/cpuinfo").read_text().splitlines()
                                  if line.startswith("model name")), "unknown"),
                  filesystem=require(run(["findmnt", "-T", args.output, "-J", "-o", "TARGET,SOURCE,FSTYPE,OPTIONS"], args.output))["stdout"],
                  initialization="single setup observation per tool/workload; not a sampled benchmark",
                  validation="payload sizes each sample, payload hashes warmup/last; SCM hashes with documented mutable paths excluded; Git state and removal/GC registry+disk assertions each sample",
                  binaries={name: dict(path=str(Path(binary).resolve()),
                                     sha256=hashlib.sha256(Path(binary).read_bytes()).hexdigest())
                            for name, binary in (("hz", args.hz), ("rift", args.rift))})
    rng = random.Random(42)
    for kind in args.workloads:
        tools = {name: Tool(name, binary, args.output / kind / name)
                 for name, binary in (("hz", args.hz), ("rift", args.rift))}
        entries = {}
        expected = {}
        for name, tool in tools.items():
            expected[name] = fixture(tool.source, kind)
            entries[name] = dict(init=require(tool.init()), modes={})
        assert expected["hz"] == expected["rift"]
        for filtered in (False, True):
            mode = "filtered" if filtered else "full"
            for name in tools:
                entries[name]["modes"][mode] = dict(create=[], remove=[], gc=[])
            for sample in range(-1, args.samples):
                order = list(tools)
                rng.shuffle(order)
                for name in order:
                    tool = tools[name]
                    created = require(tool.create(f"sample-{mode}-{sample + 1}", filtered))
                    child = Path(created["stdout"].strip())
                    validate(tool, child, expected[name], filtered, hashes=sample in (-1, args.samples-1))
                    removed = require(tool.remove(child))
                    check_removed(tool, child)
                    collected = require(tool.gc())
                    check_removed(tool, child, collected=True)
                    if sample >= 0:
                        rows = entries[name]["modes"][mode]
                        for operation, row in (("create", created), ("remove", removed), ("gc", collected)):
                            rows[operation].append(row)
            for name in tools:
                rows = entries[name]["modes"][mode]
                rows["summary"] = {op: summarize(rows[op]) for op in ("create", "remove", "gc")}
                print(kind, mode, name, json.dumps(rows["summary"]), flush=True)
        entries["fixture"] = dict(payload_files=len(expected["hz"]),
                                  payload_bytes=sum(row["size"] for row in expected["hz"].values()))
        report["workloads"][kind] = entries
        (args.output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
        # Fixtures remain available for inspection. Cleanup is explicit after review.
    report["load_end"] = os.getloadavg()
    (args.output / "results.json").write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
