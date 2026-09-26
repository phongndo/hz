#!/usr/bin/env python3
"""Measure bounded create bursts and check their cleanup in isolated registries."""
import argparse
from concurrent.futures import ThreadPoolExecutor
from contextlib import closing
import hashlib
import json
import os
from pathlib import Path
import platform
import random
import signal
import sqlite3
import subprocess
import sys
import threading
import time

sys.dont_write_bytecode = True
from compare import Tool, calibrate_memory, fixture, git_state, host_details, require, validate


def create_worker(tool, handle, barrier, retry, timeout):
    barrier.wait()
    started = time.perf_counter()
    deadline = started + timeout
    attempts = []
    rng = random.Random(handle)
    while True:
        process = subprocess.Popen(list(map(str, tool.command(*tool.create_args(handle)))),
                                   cwd=tool.base, env=tool.env, text=True,
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                   start_new_session=True)
        expired = False
        try:
            out, err = process.communicate(timeout=max(0.001, deadline - time.perf_counter()))
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            out, err = process.communicate()
            expired = True
        # Retry only the documented operation-lock conflict or SQLite busy.
        conflict = ("another hz operation is in progress" in err or "database is locked" in err)
        attempts.append(dict(code=process.returncode, stdout=out, stderr=err, timeout=expired,
                             conflict=conflict))
        if process.returncode == 0 or expired or not retry or not conflict or time.perf_counter() >= deadline:
            break
        time.sleep(rng.uniform(0.01, 0.05))
    finished = time.perf_counter()
    return dict(handle=handle, attempts=attempts, wall_ms=(finished - started) * 1000,
                finished=finished, success=process.returncode == 0)


def run_burst(tool, workers, round_number, retry, timeout):
    barrier = threading.Barrier(workers + 1)
    mode = "retry" if retry else "burst"
    with ThreadPoolExecutor(max_workers=workers) as pool:
        futures = [pool.submit(create_worker, tool, f"{mode}-{round_number}-{worker}",
                               barrier, retry, timeout) for worker in range(workers)]
        started = time.perf_counter()
        barrier.wait()
        rows = [future.result() for future in futures]
    elapsed = (max(row.pop("finished") for row in rows) - started) * 1000
    return dict(wall_ms=elapsed, completed=sum(row["success"] for row in rows), workers=rows)


def validate_and_clean(tool, batch, expected, restore):
    restored = []
    for worker in batch["workers"]:
        if not worker["success"]:
            continue
        child = Path(worker["attempts"][-1]["stdout"].strip())
        validate(tool, child, expected, False, hashes=True)
        require(tool.remove(child))
        assert not child.exists()
        if restore and tool.name == "hz":
            result = require(tool.invoke("restore", child.name, "--path-only"))
            assert Path(result["stdout"].strip()) == child
            validate(tool, child, expected, False, hashes=True)
            restored.append(result)
            require(tool.remove(child))
    require(tool.gc())
    database = tool.base / ("registry/hz.sqlite" if tool.name == "hz" else "rift.sqlite")
    with closing(sqlite3.connect(database)) as db:
        table = "workspace" if tool.name == "hz" else "rift"
        assert db.execute(f"SELECT count(*) FROM {table}").fetchone()[0] == 1
        if tool.name == "rift":
            assert db.execute("SELECT count(*) FROM trash").fetchone()[0] == 0
    if tool.storage.exists():
        assert all(path.name == ".trash" and not any(path.iterdir()) for path in tool.storage.iterdir())
    return restored


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--hz", required=True)
    parser.add_argument("--rift", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--workers", type=int, default=4)
    parser.add_argument("--rounds", type=int, default=3)
    parser.add_argument("--timeout", type=int, default=120)
    parser.add_argument("--workload", choices=["small", "development"], default="development")
    args = parser.parse_args()
    args.output = args.output.resolve()
    if not __debug__ or not 2 <= args.workers <= 16 or not 1 <= args.rounds <= 20 or args.timeout <= 0:
        parser.error("use assertions, 2–16 workers, 1–20 rounds, and a positive timeout")
    args.output.mkdir(parents=True, exist_ok=False)
    report = dict(workers=args.workers, rounds=args.rounds, workload=args.workload, verified=False,
                  date=time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), platform=platform.platform(),
                  timing="barrier-released concurrent CLI wall latency; no CPU/RSS concurrency claim",
                  retry="10–50 ms deterministic jitter; operation-lock and SQLite-busy errors only",
                  memory_calibration=calibrate_memory(), **host_details(args.output), tools={},
                  binaries={name: dict(path=str(Path(binary).resolve()),
                                       sha256=hashlib.sha256(Path(binary).read_bytes()).hexdigest())
                            for name, binary in (("hz", args.hz), ("rift", args.rift))})
    tools = {name: Tool(name, binary, args.output / name)
             for name, binary in (("hz", args.hz), ("rift", args.rift))}
    expected = {}
    for name, tool in tools.items():
        expected[name] = fixture(tool.source, args.workload)
        tool.expected_git = git_state(tool.source)
        require(tool.init())
        report["tools"][name] = dict(burst=[], retry=[], restore=[] if name == "hz" else None)
    assert expected["hz"] == expected["rift"]
    rng = random.Random(43)
    for retry in (False, True):
        mode = "retry" if retry else "burst"
        for round_number in range(args.rounds):
            order = list(tools)
            rng.shuffle(order)
            for name in order:
                tool = tools[name]
                batch = run_burst(tool, args.workers, round_number, retry, args.timeout)
                report["tools"][name][mode].append(batch)
                # Save even a rejected burst before checking cleanup invariants.
                (args.output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
                for worker in batch["workers"]:
                    final = worker["attempts"][-1]
                    assert worker["success"] or (final["conflict"] and not final["timeout"]), worker
                restores = validate_and_clean(tool, batch, expected[name], restore=retry)
                if name == "hz":
                    report["tools"][name]["restore"].extend(restores)
                if retry:
                    assert batch["completed"] == args.workers, batch
                print(name, mode, round_number, f"{batch['wall_ms']:.2f} ms", batch["completed"], "completed", flush=True)
    report["verified"] = True
    (args.output / "results.json").write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
