#!/usr/bin/env python3
"""Prepare pinned public workload checkouts; never reuse an existing directory."""
import argparse
import json
import os
from pathlib import Path
import subprocess

REPOSITORIES = {
    "typescript": ("https://github.com/microsoft/TypeScript.git",
                   "c63de15a992d37f0d6cec03ac7631872838602cb", "v5.9.3"),
    "linux": ("https://github.com/torvalds/linux.git",
              "adc218676eef25575469234709c2d87185ca223a", "v6.12"),
}


def prepare(workload, destination):
    url, revision, version = REPOSITORIES[workload]
    destination.mkdir(parents=True, exist_ok=False)
    env = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
    env.update(GIT_CONFIG_GLOBAL="/dev/null", GIT_CONFIG_NOSYSTEM="1",
               npm_config_cache=str(destination.parent / "npm-cache"))
    commands = []

    def execute(*args):
        commands.append(list(args))
        subprocess.run(args, cwd=destination, env=env, check=True)

    def output(*args):
        return subprocess.check_output(args, cwd=destination, env=env, text=True).strip()

    execute("git", "init", "--quiet")
    execute("git", "remote", "add", "origin", url)
    execute("git", "fetch", "--depth", "1", "origin", revision)
    execute("git", "-c", "advice.detachedHead=false", "checkout", "--detach", "FETCH_HEAD")
    assert output("git", "rev-parse", "HEAD") == revision
    versions = {}
    if workload == "typescript":
        versions = {tool: output(tool, "--version") for tool in ("node", "npm")}
        execute("npm", "ci", "--ignore-scripts", "--no-audit", "--no-fund")
        execute("npm", "run", "--ignore-scripts", "build:compiler")
        versions["compiler"] = output("node", "built/local/tsc.js", "--version")
    execute("git", "diff", "--exit-code")
    record = dict(workload=workload, url=url, revision=revision, release=version,
                  versions=versions, preparation=commands,
                  state="source, dependencies and compiler output" if workload == "typescript" else "source checkout only; kernel not built")
    (destination.parent / f"{workload}-fixture.json").write_text(json.dumps(record, indent=2) + "\n")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("workload", choices=REPOSITORIES)
    parser.add_argument("destination", type=Path)
    args = parser.parse_args()
    prepare(args.workload, args.destination.resolve())
