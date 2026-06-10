#!/usr/bin/env python3
"""Repo-local AI PR review driver.

This CI driver intentionally keeps the trusted orchestration small:
- gate on a repo-owned allowlist;
- collect only the PR diff plus changed-file surrounding context via git;
- invoke Pi in read-only/no-tool print mode for focused review passes;
- post a one-time PR summary and findings-only review comments.
"""

from __future__ import annotations

import argparse
import copy
import json
import os
import re
import subprocess
import sys
import tempfile
import textwrap
import urllib.error
import urllib.request
from collections import Counter
from pathlib import Path
from typing import Any


MARKER = "<!-- ai-pr-review -->"
SUMMARY_MARKER = "<!-- ai-pr-summary -->"
INLINE_MARKER = "<!-- ai-pr-review:inline -->"
VALID_MODES = {"fast", "balanced", "deep"}
SEVERITIES = ("blocker", "high", "medium", "low")
CATEGORIES = (
    "correctness",
    "security",
    "tests",
    "maintainability",
    "performance",
    "dependency",
)

DEFAULT_CONFIG: dict[str, Any] = {
    "enabled_users": ["phongndo"],
    "provider": {"runtime": "pi", "model_source": "opencode-go"},
    "models": {
        "fast": "opencode-go/deepseek-v4-flash",
        "serious": "opencode-go/deepseek-v4-pro",
        "dedupe": "opencode-go/deepseek-v4-pro",
        "judge": "opencode-go/deepseek-v4-pro",
    },
    "mode": "balanced",
    "agents": {
        "correctness": True,
        "security": True,
        "tests": True,
        "maintainability": True,
        "performance": False,
        "dependency": False,
        "verification": True,
    },
    "thresholds": {
        "min_confidence": 0.65,
        "max_findings": 12,
        "post_low_severity": False,
    },
    "github": {"summary_comment": True, "sticky_comment": False, "inline_comments": True},
}

REVIEWER_FOCUS: dict[str, list[str]] = {
    "correctness": [
        "logic bugs",
        "broken edge cases",
        "bad state transitions",
        "invalid assumptions",
        "concurrency or state bugs",
    ],
    "security": [
        "injection",
        "auth/authz mistakes",
        "path traversal",
        "unsafe shell calls",
        "secrets",
        "unsafe deserialization",
        "sensitive data exposure",
    ],
    "tests": [
        "changed behavior without tests",
        "weak assertions",
        "missing regression tests",
        "test gaps around edge cases",
    ],
    "maintainability": [
        "bad abstractions",
        "duplicated logic",
        "confusing ownership",
        "leaky boundaries",
        "brittle APIs",
        "hard-to-change control flow",
    ],
    "performance": [
        "meaningful regressions only",
        "avoidable O(n²) work",
        "repeated I/O",
        "excessive allocations",
        "unnecessary repeated work",
        "bad caching",
    ],
    "dependency": [
        "new dependencies",
        "risky package changes",
        "unnecessary dependency bloat",
        "supply-chain risk",
        "license concerns if detectable",
    ],
}

FINDING_SCHEMA = """
{
  "title": "Short issue title",
  "severity": "blocker | high | medium | low",
  "category": "correctness | security | tests | maintainability | performance | dependency",
  "file": "path/to/file",
  "line": 123,
  "confidence": 0.0,
  "summary": "What is wrong",
  "why_it_matters": "Concrete risk",
  "minimal_fix": "Smallest reasonable fix",
  "evidence": ["Specific evidence from the diff or repo"]
}
""".strip()

VERIFICATION_SCHEMA = """
{
  "overall_confidence": 0.0,
  "accepted_findings": [
    {
      "title": "Short issue title",
      "severity": "blocker | high | medium | low",
      "category": "correctness | security | tests | maintainability | performance | dependency",
      "file": "path/to/file",
      "line": 123,
      "confidence": 0.0,
      "summary": "What is wrong",
      "why_it_matters": "Concrete risk",
      "minimal_fix": "Smallest reasonable fix",
      "evidence": ["Specific evidence from the diff or repo"],
      "verification_notes": "Why this issue is real and worth posting"
    }
  ],
  "rejected_findings": [
    {
      "title": "Rejected issue title",
      "reason": "duplicate | speculative | style-only | unsupported | already-handled | too-low-severity"
    }
  ]
}
""".strip()

SUMMARY_SCHEMA = """
{
  "pr_summary": "Concise summary of what this pull request changes",
  "notable_changes": ["Short bullet describing an important changed area"],
  "risk_notes": "Concise risk or reviewer note, or an empty string"
}
""".strip()

READ_ONLY_SYSTEM_PROMPT = """
You are a read-only AI pull request reviewer running inside Pi for repository CI.
You must not modify files, push commits, request command execution, or claim that tests ran.
Use only the PR diff and surrounding repository context provided in the user prompt.
Prefer a short, high-signal review over a long noisy one.
Return machine-readable JSON exactly as requested by the prompt.
""".strip()


def log(message: str) -> None:
    print(message, flush=True)


def deep_merge(base: dict[str, Any], overlay: dict[str, Any]) -> dict[str, Any]:
    merged = copy.deepcopy(base)
    for key, value in overlay.items():
        if isinstance(value, dict) and isinstance(merged.get(key), dict):
            merged[key] = deep_merge(merged[key], value)
        else:
            merged[key] = value
    return merged


def strip_yaml_comment(line: str) -> str:
    in_single = False
    in_double = False
    for index, char in enumerate(line):
        if char == "'" and not in_double:
            in_single = not in_single
        elif char == '"' and not in_single:
            in_double = not in_double
        elif char == "#" and not in_single and not in_double:
            if index == 0 or line[index - 1].isspace():
                return line[:index]
    return line


def parse_scalar(value: str) -> Any:
    value = value.strip()
    if value == "":
        return ""
    if value == "[]":
        return []
    if value.startswith("[") and value.endswith("]"):
        inner = value[1:-1].strip()
        if not inner:
            return []
        return [parse_scalar(item.strip()) for item in inner.split(",")]
    if (value.startswith('"') and value.endswith('"')) or (
        value.startswith("'") and value.endswith("'")
    ):
        return value[1:-1]
    lowered = value.lower()
    if lowered == "true":
        return True
    if lowered == "false":
        return False
    if lowered in {"null", "none"}:
        return None
    try:
        if "." in value:
            return float(value)
        return int(value)
    except ValueError:
        return value


def parse_simple_yaml(path: Path) -> dict[str, Any]:
    """Parse the small YAML subset used by .github/ai-review.yml."""

    parsed: dict[str, Any] = {}
    current_section: str | None = None
    for raw_line in path.read_text(encoding="utf-8").splitlines():
        line = strip_yaml_comment(raw_line).rstrip()
        if not line.strip():
            continue

        indent = len(line) - len(line.lstrip(" "))
        stripped = line.strip()
        if indent == 0:
            if ":" not in stripped:
                continue
            key, value = stripped.split(":", 1)
            key = key.strip()
            value = value.strip()
            current_section = key
            if value:
                parsed[key] = parse_scalar(value)
            else:
                parsed[key] = [] if key == "enabled_users" else {}
            continue

        if current_section is None:
            continue
        if stripped.startswith("- "):
            section = parsed.setdefault(current_section, [])
            if not isinstance(section, list):
                section = []
                parsed[current_section] = section
            section.append(parse_scalar(stripped[2:].strip()))
            continue
        if ":" in stripped:
            section = parsed.setdefault(current_section, {})
            if not isinstance(section, dict):
                section = {}
                parsed[current_section] = section
            key, value = stripped.split(":", 1)
            section[key.strip()] = parse_scalar(value.strip())

    return parsed


def load_config() -> dict[str, Any]:
    config_path = Path(os.environ.get("AI_REVIEW_CONFIG", ".github/ai-review.yml"))
    if not config_path.exists():
        log(f"Config {config_path} not found; using built-in defaults.")
        return copy.deepcopy(DEFAULT_CONFIG)
    loaded = parse_simple_yaml(config_path)
    config = deep_merge(DEFAULT_CONFIG, loaded)
    if str(config.get("mode", "balanced")) not in VALID_MODES:
        config["mode"] = "balanced"
    return config


def as_bool(value: Any) -> bool:
    if isinstance(value, bool):
        return value
    return str(value).strip().lower() in {"1", "true", "yes", "on"}


def set_output(name: str, value: Any) -> None:
    text = str(value)
    output_path = os.environ.get("GITHUB_OUTPUT")
    if output_path:
        with open(output_path, "a", encoding="utf-8") as output:
            output.write(f"{name}={text}\n")
    log(f"{name}={text}")


def github_repo() -> str:
    repo = os.environ.get("GITHUB_REPOSITORY")
    if not repo:
        raise SystemExit("GITHUB_REPOSITORY is not set")
    return repo


def github_token() -> str:
    token = os.environ.get("GITHUB_TOKEN") or os.environ.get("GH_TOKEN")
    if not token:
        raise SystemExit("GITHUB_TOKEN is not set")
    return token


def github_request(method: str, path: str, payload: Any | None = None) -> Any:
    repo = github_repo()
    if path.startswith("https://"):
        url = path
    else:
        url = f"https://api.github.com/repos/{repo}/{path.lstrip('/')}"
    body = None if payload is None else json.dumps(payload).encode("utf-8")
    request = urllib.request.Request(url, data=body, method=method)
    request.add_header("Accept", "application/vnd.github+json")
    request.add_header("Authorization", f"Bearer {github_token()}")
    request.add_header("User-Agent", "hz-ai-pr-review")
    request.add_header("X-GitHub-Api-Version", "2022-11-28")
    if body is not None:
        request.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            data = response.read()
            if not data:
                return None
            return json.loads(data.decode("utf-8"))
    except urllib.error.HTTPError as error:
        detail = error.read().decode("utf-8", errors="replace")
        raise RuntimeError(f"GitHub API {method} {url} failed: {error.code} {detail}") from error


def load_event() -> dict[str, Any]:
    path = os.environ.get("GITHUB_EVENT_PATH")
    if not path or not Path(path).exists():
        return {}
    return json.loads(Path(path).read_text(encoding="utf-8"))


def workflow_input(event: dict[str, Any], name: str, default: str = "") -> str:
    env_name = f"INPUT_{name.upper()}"
    if env_name in os.environ:
        return os.environ[env_name]
    inputs = event.get("inputs") or {}
    value = inputs.get(name, default)
    return "" if value is None else str(value)


def resolve_pr(event: dict[str, Any]) -> dict[str, Any] | None:
    event_name = os.environ.get("GITHUB_EVENT_NAME", "")
    if event_name == "pull_request":
        return event.get("pull_request")
    if event_name == "workflow_dispatch":
        pr_number = workflow_input(event, "pr_number").strip()
        if not pr_number:
            log("workflow_dispatch requires a pr_number input; skipping AI review.")
            return None
        return github_request("GET", f"pulls/{pr_number}")
    log(f"Unsupported event {event_name}; skipping AI review.")
    return None


def pr_metadata(pr: dict[str, Any]) -> dict[str, str]:
    return {
        "pr_number": str(pr.get("number", "")),
        "author": str((pr.get("user") or {}).get("login", "")),
        "base_ref": str((pr.get("base") or {}).get("ref", "")),
        "base_sha": str((pr.get("base") or {}).get("sha", "")),
        "base_repo": str(((pr.get("base") or {}).get("repo") or {}).get("full_name", "")),
        "head_ref": str((pr.get("head") or {}).get("ref", "")),
        "head_sha": str((pr.get("head") or {}).get("sha", "")),
        "head_repo": str(((pr.get("head") or {}).get("repo") or {}).get("full_name", "")),
    }


def is_untrusted_pull_request_event(metadata: dict[str, str]) -> bool:
    if os.environ.get("GITHUB_EVENT_NAME") != "pull_request":
        return False
    base_repo = metadata["base_repo"] or os.environ.get("GITHUB_REPOSITORY", "")
    head_repo = metadata["head_repo"]
    return not base_repo or not head_repo or head_repo != base_repo


def command_gate() -> None:
    config = load_config()
    event = load_event()
    pr = resolve_pr(event)
    if pr is None:
        set_output("should_review", "false")
        return

    metadata = pr_metadata(pr)
    configured_users = config.get("enabled_users", [])
    if isinstance(configured_users, list):
        enabled_users = [str(user) for user in configured_users]
    elif configured_users:
        enabled_users = [str(configured_users)]
    else:
        enabled_users = []
    mode_input = workflow_input(event, "mode", "config").strip()
    mode = mode_input if mode_input in VALID_MODES else str(config.get("mode", "balanced"))
    override = as_bool(workflow_input(event, "override_allowlist", "false"))
    if os.environ.get("GITHUB_EVENT_NAME") != "workflow_dispatch":
        override = False

    author = metadata["author"]
    if not enabled_users:
        log("AI review allowlist is empty; skipping.")
        set_output("should_review", "false")
    elif is_untrusted_pull_request_event(metadata):
        log(
            "AI review skipped: fork pull_request events do not receive review secrets "
            "or a writable token; run workflow_dispatch after maintainer review."
        )
        set_output("should_review", "false")
    elif author not in enabled_users and not override:
        log(f"AI review skipped: PR author @{author} is not allowlisted.")
        set_output("should_review", "false")
    else:
        if author not in enabled_users and override:
            log(f"AI review allowlist override enabled by @{os.environ.get('GITHUB_ACTOR', 'unknown')}.")
        log(f"AI review enabled for PR #{metadata['pr_number']} by @{author} in {mode} mode.")
        set_output("should_review", "true")

    for key, value in metadata.items():
        set_output(key, value)
    set_output("mode", mode)


def run_git(args: list[str], *, text: bool = True) -> str:
    result = subprocess.run(
        ["git", *args],
        check=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=text,
        errors="replace" if text else None,
    )
    return result.stdout if text else result.stdout.decode("utf-8", errors="replace")


def truncate_text(text: str, max_chars: int) -> tuple[str, bool]:
    if len(text) <= max_chars:
        return text, False
    return text[:max_chars] + "\n\n[... truncated by ai-pr-review CI ...]\n", True


def parse_name_status(name_status: str) -> list[dict[str, str]]:
    files: list[dict[str, str]] = []
    for line in name_status.splitlines():
        if not line.strip():
            continue
        parts = line.split("\t")
        status = parts[0]
        if status.startswith("R") or status.startswith("C"):
            if len(parts) >= 3:
                files.append({"status": status, "path": parts[2], "old_path": parts[1]})
        elif len(parts) >= 2:
            files.append({"status": status, "path": parts[1]})
    return files


def parse_changed_ranges(diff_zero: str) -> list[tuple[int, int]]:
    ranges: list[tuple[int, int]] = []
    for match in re.finditer(r"^@@ -\d+(?:,\d+)? \+(\d+)(?:,(\d+))? @@", diff_zero, re.MULTILINE):
        start = int(match.group(1))
        length = int(match.group(2) or "1")
        if length <= 0:
            continue
        ranges.append((start, start + length - 1))
    return ranges


def parse_commentable_lines(diff_text: str) -> set[int]:
    """Return new-file line numbers present in the rendered PR diff."""

    lines: set[int] = set()
    new_line: int | None = None
    for line in diff_text.splitlines():
        hunk = re.match(r"^@@ -\d+(?:,\d+)? \+(\d+)(?:,\d+)? @@", line)
        if hunk:
            new_line = int(hunk.group(1))
            continue
        if new_line is None:
            continue
        if line.startswith("+++") or line.startswith("---"):
            continue
        if line.startswith("+") or line.startswith(" "):
            lines.add(new_line)
            new_line += 1
        elif line.startswith("-"):
            continue
        elif line.startswith("\\"):
            continue
        else:
            new_line = None
    return lines


def merge_ranges(ranges: list[tuple[int, int]], radius: int, line_count: int) -> list[tuple[int, int]]:
    expanded = [(max(1, start - radius), min(line_count, end + radius)) for start, end in ranges]
    expanded.sort()
    merged: list[tuple[int, int]] = []
    for start, end in expanded:
        if not merged or start > merged[-1][1] + 1:
            merged.append((start, end))
        else:
            merged[-1] = (merged[-1][0], max(merged[-1][1], end))
    return merged


def git_show_text(revision: str, path: str) -> str | None:
    try:
        result = subprocess.run(
            ["git", "show", f"{revision}:{path}"],
            check=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
    except subprocess.CalledProcessError:
        return None
    data = result.stdout
    if b"\0" in data:
        return None
    return data.decode("utf-8", errors="replace")


def build_context(base_ref: str, head_ref: str) -> dict[str, Any]:
    merge_base = run_git(["merge-base", f"origin/{base_ref}", "HEAD"]).strip()
    head_sha = run_git(["rev-parse", "HEAD"]).strip()
    head_subject = run_git(["log", "-1", "--format=%s", "HEAD"]).strip()
    base_sha = run_git(["rev-parse", f"origin/{base_ref}"]).strip()
    diff_range = f"{merge_base}...HEAD"

    name_status = run_git(["diff", "--name-status", "--find-renames", diff_range, "--"])
    changed_files = parse_name_status(name_status)
    diff_stat = run_git(["diff", "--stat", "--find-renames", diff_range, "--"])
    diff = run_git(
        ["diff", "--no-ext-diff", "--no-color", "--find-renames", "--unified=80", diff_range, "--"]
    )
    diff, diff_truncated = truncate_text(diff, 120_000)

    snippets: list[str] = []
    snippets_truncated = False
    snippet_budget = 80_000
    commentable_lines: dict[str, set[int]] = {}
    for item in changed_files:
        status = item["status"]
        path = item["path"]
        if status.startswith("D"):
            continue
        try:
            diff_context = run_git(
                [
                    "diff",
                    "--no-ext-diff",
                    "--no-color",
                    "--find-renames",
                    "--unified=80",
                    diff_range,
                    "--",
                    path,
                ]
            )
            commentable_lines[path] = parse_commentable_lines(diff_context)
        except subprocess.CalledProcessError:
            commentable_lines[path] = set()
        content = git_show_text("HEAD", path)
        if content is None:
            continue
        lines = content.splitlines()
        if not lines:
            continue
        try:
            diff_zero = run_git(["diff", "--no-ext-diff", "--no-color", "--unified=0", diff_range, "--", path])
        except subprocess.CalledProcessError:
            continue
        ranges = merge_ranges(parse_changed_ranges(diff_zero), radius=60, line_count=len(lines))
        if not ranges:
            ranges = [(1, min(len(lines), 120))]
        file_blocks = [f"### {path}"]
        for start, end in ranges[:6]:
            file_blocks.append(f"```text\n# lines {start}-{end}")
            for line_number in range(start, end + 1):
                file_blocks.append(f"{line_number:>5} | {lines[line_number - 1]}")
            file_blocks.append("```")
        block = "\n".join(file_blocks)
        if sum(len(existing) for existing in snippets) + len(block) > snippet_budget:
            snippets_truncated = True
            break
        snippets.append(block)

    context_text = textwrap.dedent(
        f"""
        PR metadata:
        - author: @{os.environ.get('AI_REVIEW_AUTHOR', '')}
        - base_ref: {base_ref}
        - base_sha: {base_sha}
        - merge_base: {merge_base}
        - head_ref: {head_ref}
        - head_sha: {head_sha}
        - head_commit: {head_subject}

        Changed files:
        ```text
        {name_status.strip() or '(no changed files)'}
        ```

        Diff stat:
        ```text
        {diff_stat.strip() or '(empty diff)'}
        ```

        PR diff with context:
        ```diff
        {diff.strip() or '(empty diff)'}
        ```

        Surrounding changed-file context from HEAD:
        {chr(10).join(snippets) if snippets else '(no text snippets available)'}
        """
    ).strip()
    truncation_notes = []
    if diff_truncated:
        truncation_notes.append("diff truncated")
    if snippets_truncated:
        truncation_notes.append("snippets truncated")
    if truncation_notes:
        context_text += "\n\nContext limits: " + ", ".join(truncation_notes) + "."

    return {
        "text": context_text,
        "merge_base": merge_base,
        "base_sha": base_sha,
        "head_sha": head_sha,
        "head_subject": head_subject,
        "changed_files": changed_files,
        "commentable_lines": commentable_lines,
    }


def reviewer_model(config: dict[str, Any]) -> str:
    models = config.get("models", {})
    return str(models.get("fast") or "opencode-go/deepseek-v4-flash")


def model_for_category(config: dict[str, Any], mode: str, category: str) -> str:
    return reviewer_model(config)


def verification_model(config: dict[str, Any]) -> str:
    models = config.get("models", {})
    return str(models.get("judge") or models.get("dedupe") or "opencode-go/deepseek-v4-pro")


def serious_model(config: dict[str, Any]) -> str:
    models = config.get("models", {})
    return str(models.get("dedupe") or models.get("serious") or "opencode-go/deepseek-v4-pro")


def mode_max_findings(config: dict[str, Any], mode: str) -> int:
    configured = int((config.get("thresholds") or {}).get("max_findings", 12))
    caps = {"fast": 8, "balanced": 12, "deep": 20}
    return max(1, min(configured, caps.get(mode, 12)))


def ensure_credentials(config: dict[str, Any]) -> None:
    models = [str(value) for value in (config.get("models") or {}).values()]
    if any(model.startswith("opencode-go/") for model in models) and not os.environ.get("OPENCODE_API_KEY"):
        raise SystemExit("OPENCODE_API_KEY is required for configured OpenCode Go models")


def pi_env() -> dict[str, str]:
    env = os.environ.copy()
    env.setdefault("PI_SKIP_VERSION_CHECK", "1")
    env.setdefault("PI_TELEMETRY", "0")
    return env


def run_pi(prompt: str, model: str, label: str, *, thinking: str = "low") -> str:
    log(f"Running {label} with {model}")
    with tempfile.NamedTemporaryFile(
        "w", encoding="utf-8", prefix="ai-pr-review-prompt-", suffix=".md"
    ) as prompt_file:
        prompt_file.write(prompt)
        prompt_file.flush()
        command = [
            "pi",
            "--no-session",
            "--no-extensions",
            "--no-skills",
            "--no-prompt-templates",
            "--no-context-files",
            "--no-tools",
            "--no-approve",
            "--system-prompt",
            READ_ONLY_SYSTEM_PROMPT,
            "--model",
            model,
            "--thinking",
            thinking,
            "-p",
            f"@{prompt_file.name}",
        ]
        result = subprocess.run(
            command,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            errors="replace",
            env=pi_env(),
            timeout=900,
        )
    if result.stderr.strip():
        sys.stderr.write(result.stderr)
    if result.returncode != 0:
        raise RuntimeError(f"Pi {label} failed with exit code {result.returncode}")
    return result.stdout.strip()


def reviewer_prompt(category: str, context_text: str, raw_limit: int) -> str:
    focus = "\n".join(f"- {item}" for item in REVIEWER_FOCUS[category])
    return textwrap.dedent(
        f"""
        You are the {category} reviewer for this pull request.

        Focus only on:
        {focus}

        Rules:
        - Review only the supplied PR diff and surrounding context.
        - Report only concrete, evidence-backed issues introduced or exposed by this PR.
        - Do not report style-only, vague, speculative, or generic best-practice concerns.
        - Do not report issues already handled elsewhere in the included context.
        - Prefer no findings over weak findings.
        - Return at most {raw_limit} findings.
        - Use category "{category}" for every finding.

        Return JSON only: an array of findings matching this schema:
        {FINDING_SCHEMA}

        PR context:
        {context_text}
        """
    ).strip()


def serious_recheck_prompt(findings: list[dict[str, Any]], context_text: str, raw_limit: int) -> str:
    return textwrap.dedent(
        f"""
        You are the serious verification pass for medium/high/blocker PR findings.

        Keep only findings that are clearly grounded in the supplied PR diff or nearby context.
        Drop duplicate, speculative, vague, style-only, unsupported, or already-handled findings.
        You may lower severity or improve evidence, but do not invent unrelated new issues.
        Return at most {raw_limit} findings.

        Return JSON only: an array of findings matching this schema:
        {FINDING_SCHEMA}

        Candidate findings:
        ```json
        {json.dumps(findings, indent=2, sort_keys=True)}
        ```

        PR context:
        {context_text}
        """
    ).strip()


def verification_prompt(findings: list[dict[str, Any]], context_text: str, max_findings: int, min_confidence: float) -> str:
    return textwrap.dedent(
        f"""
        You are the final verification reviewer for this pull request.

        Your job is to reduce noise before CI posts GitHub review comments.
        Verify the candidate findings against the supplied PR diff and nearby context.

        Requirements:
        - Deduplicate findings that describe the same root cause.
        - Reject speculative, vague, style-only, unsupported, or already-handled findings.
        - Drop findings below confidence {min_confidence:.2f}.
        - Verify each accepted finding is grounded in the PR diff or nearby code.
        - Check whether the issue is already handled elsewhere in the included context.
        - Merge duplicates into one stronger finding.
        - Lower severity when appropriate.
        - Do not invent new unrelated issues unless you discover a clear blocker while verifying an existing finding.
        - Prefer a short high-signal review.
        - Set overall_confidence from 0.00 to 1.00 for the final review result, including a no-findings result.
        - Return at most {max_findings} accepted findings.

        Return JSON only: an object matching this schema:
        {VERIFICATION_SCHEMA}

        Candidate findings:
        ```json
        {json.dumps(findings, indent=2, sort_keys=True)}
        ```

        PR context:
        {context_text}
        """
    ).strip()


def summary_prompt(context_text: str) -> str:
    return textwrap.dedent(
        f"""
        You are writing the one-time PR summary comment for this pull request.

        Requirements:
        - Summarize only what changed in the supplied PR diff and nearby context.
        - Do not report review findings, issues, suggestions, or required fixes.
        - Be concise and useful to a human reviewer.
        - Return at most 5 notable_changes bullets.

        Return JSON only: an object matching this schema:
        {SUMMARY_SCHEMA}

        PR context:
        {context_text}
        """
    ).strip()


def extract_json(text: str) -> Any:
    stripped = text.strip()
    if not stripped:
        raise ValueError("empty model response")
    try:
        return json.loads(stripped)
    except json.JSONDecodeError:
        pass

    fence = re.search(r"```(?:json)?\s*(.*?)```", stripped, re.DOTALL | re.IGNORECASE)
    if fence:
        try:
            return json.loads(fence.group(1).strip())
        except json.JSONDecodeError:
            pass

    decoder = json.JSONDecoder()
    for index, char in enumerate(stripped):
        if char not in "[{":
            continue
        try:
            value, _ = decoder.raw_decode(stripped[index:])
            return value
        except json.JSONDecodeError:
            continue
    raise ValueError("could not parse JSON from model response")


def clean_string(value: Any, max_len: int = 800) -> str:
    text = str(value or "").strip()
    text = re.sub(r"\s+", " ", text)
    if len(text) > max_len:
        return text[: max_len - 1].rstrip() + "…"
    return text


def parse_confidence(value: Any, default: float = 0.0) -> float:
    try:
        if isinstance(value, str) and value.strip().endswith("%"):
            parsed = float(value.strip()[:-1]) / 100.0
        else:
            parsed = float(value)
    except (TypeError, ValueError):
        parsed = default
    if parsed > 1.0:
        parsed = parsed / 100.0
    return min(1.0, max(0.0, parsed))


def normalize_finding(data: Any, default_category: str | None = None) -> dict[str, Any] | None:
    if not isinstance(data, dict):
        return None
    title = clean_string(data.get("title"), 160)
    file_path = clean_string(data.get("file"), 300).lstrip("./")
    if not title or not file_path or file_path.startswith("/") or ".." in Path(file_path).parts:
        return None

    severity = clean_string(data.get("severity"), 32).lower()
    if severity not in SEVERITIES:
        return None
    category = clean_string(data.get("category") or default_category, 32).lower()
    if category not in CATEGORIES:
        return None
    try:
        line = int(data.get("line"))
    except (TypeError, ValueError):
        return None
    try:
        confidence = float(data.get("confidence"))
    except (TypeError, ValueError):
        confidence = 0.0
    confidence = min(1.0, max(0.0, confidence))

    evidence_value = data.get("evidence", [])
    if isinstance(evidence_value, list):
        evidence = [clean_string(item, 500) for item in evidence_value if clean_string(item, 500)]
    else:
        evidence = [clean_string(evidence_value, 500)] if clean_string(evidence_value, 500) else []

    finding = {
        "title": title,
        "severity": severity,
        "category": category,
        "file": file_path,
        "line": line,
        "confidence": confidence,
        "summary": clean_string(data.get("summary"), 1000),
        "why_it_matters": clean_string(data.get("why_it_matters"), 1000),
        "minimal_fix": clean_string(data.get("minimal_fix"), 1000),
        "evidence": evidence,
    }
    if data.get("verification_notes"):
        finding["verification_notes"] = clean_string(data.get("verification_notes"), 1000)
    return finding


def parse_findings_response(text: str, default_category: str | None = None) -> list[dict[str, Any]]:
    value = extract_json(text)
    if isinstance(value, dict):
        value = value.get("findings", [])
    if not isinstance(value, list):
        raise ValueError("reviewer response must be a JSON array")
    findings = []
    for item in value:
        finding = normalize_finding(item, default_category)
        if finding is not None:
            findings.append(finding)
    return findings


def parse_verification_response(text: str) -> dict[str, Any]:
    value = extract_json(text)
    if not isinstance(value, dict):
        raise ValueError("verification response must be a JSON object")
    pr_summary = clean_string(value.get("pr_summary") or value.get("summary"), 1200)
    overall_confidence = parse_confidence(value.get("overall_confidence"), 0.0)
    accepted = []
    for item in value.get("accepted_findings", []):
        finding = normalize_finding(item)
        if finding is not None and finding.get("evidence"):
            if item.get("verification_notes"):
                finding["verification_notes"] = clean_string(item.get("verification_notes"), 1000)
            accepted.append(finding)
    rejected = []
    for item in value.get("rejected_findings", []):
        if not isinstance(item, dict):
            continue
        rejected.append(
            {
                "title": clean_string(item.get("title"), 160) or "Rejected finding",
                "reason": clean_string(item.get("reason"), 80) or "unsupported",
            }
        )
    return {
        "pr_summary": pr_summary,
        "overall_confidence": overall_confidence,
        "accepted_findings": accepted,
        "rejected_findings": rejected,
    }


def parse_summary_response(text: str) -> dict[str, Any]:
    value = extract_json(text)
    if not isinstance(value, dict):
        raise ValueError("summary response must be a JSON object")
    notable_value = value.get("notable_changes", [])
    if isinstance(notable_value, list):
        notable_changes = [clean_string(item, 240) for item in notable_value if clean_string(item, 240)]
    else:
        notable_changes = [clean_string(notable_value, 240)] if clean_string(notable_value, 240) else []
    return {
        "pr_summary": clean_string(value.get("pr_summary") or value.get("summary"), 1200),
        "notable_changes": notable_changes[:5],
        "risk_notes": clean_string(value.get("risk_notes"), 800),
    }


def dedupe_key(finding: dict[str, Any]) -> str:
    title = re.sub(r"[^a-z0-9]+", " ", finding["title"].lower()).strip()
    return f"{finding['category']}|{finding['file']}|{title}"


def filter_verified_findings(
    verified: dict[str, Any],
    changed_paths: set[str],
    min_confidence: float,
    max_findings: int,
    post_low: bool,
) -> tuple[list[dict[str, Any]], list[dict[str, Any]], list[dict[str, str]]]:
    rejected = list(verified.get("rejected_findings", []))
    accepted: list[dict[str, Any]] = []
    hidden_low: list[dict[str, Any]] = []
    seen: set[str] = set()

    for finding in verified.get("accepted_findings", []):
        if finding["file"] not in changed_paths:
            rejected.append({"title": finding["title"], "reason": "unsupported"})
            continue
        if finding["confidence"] < min_confidence:
            rejected.append({"title": finding["title"], "reason": "too-low-severity"})
            continue
        key = dedupe_key(finding)
        if key in seen:
            rejected.append({"title": finding["title"], "reason": "duplicate"})
            continue
        seen.add(key)
        if finding["severity"] == "low" and not post_low:
            hidden_low.append(finding)
            continue
        accepted.append(finding)

    rank = {severity: index for index, severity in enumerate(SEVERITIES)}
    accepted.sort(key=lambda item: (rank[item["severity"]], item["file"], item["line"], -item["confidence"]))
    if len(accepted) > max_findings:
        for finding in accepted[max_findings:]:
            rejected.append({"title": finding["title"], "reason": "too-low-severity"})
        accepted = accepted[:max_findings]
    return accepted, hidden_low, rejected


def markdown_escape(text: str) -> str:
    return text.replace("\r", " ").strip()


def text_block_escape(text: str) -> str:
    return str(text).replace("\r", " ").replace("```", "'''")


def text_block(lines: list[str]) -> str:
    body = "\n".join(text_block_escape(line) for line in lines).rstrip()
    return f"```text\n{body}\n```"


def copyable_findings_block(findings: list[dict[str, Any]]) -> list[str]:
    if not findings:
        return []

    lines: list[str] = []
    heading = {"blocker": "Blocker", "high": "High", "medium": "Medium", "low": "Low"}
    for severity in SEVERITIES:
        severity_findings = [finding for finding in findings if finding["severity"] == severity]
        if not severity_findings:
            continue
        if lines:
            lines.append("")
        lines.append(heading[severity])
        for index, finding in enumerate(severity_findings, start=1):
            evidence = finding["evidence"][0] if finding.get("evidence") else "See PR diff/context."
            lines.extend(
                [
                    f"{index}. {finding['file']}:{finding['line']} - {finding['title']}",
                    f"   Category: {finding['category']}",
                    f"   Confidence: {finding['confidence']:.2f}",
                    f"   Risk: {finding['why_it_matters'] or finding['summary']}",
                    f"   Evidence: {evidence}",
                    f"   Minimal fix: {finding['minimal_fix']}",
                    f"   Verification: {finding.get('verification_notes', 'Accepted by final verification reviewer.')}",
                ]
            )

    return [
        "<details>",
        "<summary>Copy findings</summary>",
        "",
        text_block(lines),
        "",
        "</details>",
        "",
    ]


def fallback_pr_summary(changed_files: list[dict[str, str]]) -> str:
    if not changed_files:
        return "No changed files were present in the reviewed diff."
    paths = [item["path"] for item in changed_files]
    sample = ", ".join(paths[:5])
    if len(paths) > 5:
        sample += f", and {len(paths) - 5} more"
    status_counts = Counter(item["status"][0] for item in changed_files)
    parts = []
    labels = {"A": "added", "M": "modified", "D": "deleted", "R": "renamed", "C": "copied"}
    for status, label in labels.items():
        count = status_counts.get(status, 0)
        if count:
            parts.append(f"{count} {label}")
    status_summary = ", ".join(parts) if parts else f"{len(paths)} changed"
    return f"Reviewed {len(paths)} changed file(s) ({status_summary}): {sample}."


def render_summary_comment(
    *,
    author: str,
    reviewed_diff: str,
    summary: dict[str, Any],
) -> str:
    lines = [
        SUMMARY_MARKER,
        "",
        "## AI PR Summary",
        "",
        f"Author: @{author}",
        f"Reviewed diff: {reviewed_diff}",
        "",
        markdown_escape(str(summary.get("pr_summary") or "")),
        "",
    ]
    notable_changes = summary.get("notable_changes") or []
    if notable_changes:
        lines.extend(["### Notable changes", ""])
        lines.extend(f"- {markdown_escape(str(item))}" for item in notable_changes)
        lines.append("")
    risk_notes = markdown_escape(str(summary.get("risk_notes") or ""))
    if risk_notes:
        lines.extend(["### Reviewer note", "", risk_notes, ""])
    lines.extend(
        [
            "---",
            "Generated once when AI review first saw this PR. Later pushes run findings-only review comments.",
        ]
    )
    return "\n".join(lines).rstrip() + "\n"


def inline_commentable(finding: dict[str, Any], commentable_lines: dict[str, set[int]]) -> bool:
    return finding["line"] in commentable_lines.get(finding["file"], set())


def inline_comment_body(finding: dict[str, Any]) -> str:
    evidence = finding["evidence"][0] if finding.get("evidence") else "See PR diff/context."
    return "\n".join(
        [
            INLINE_MARKER,
            f"**{markdown_escape(finding['title'])}**",
            "",
            f"**Severity:** {finding['severity']} · **Category:** {finding['category']} · **Confidence:** {finding['confidence']:.2f}",
            "",
            f"**Why it matters:** {markdown_escape(finding['why_it_matters'] or finding['summary'])}",
            "",
            f"**Evidence:** {markdown_escape(evidence)}",
            "",
            f"**Minimal fix:** {markdown_escape(finding['minimal_fix'])}",
            "",
            f"**Verification:** {markdown_escape(finding.get('verification_notes', 'Accepted by final verification reviewer.'))}",
        ]
    )


def render_comment(
    *,
    author: str,
    reviewed_diff: str,
    diff_summary: str,
    findings: list[dict[str, Any]],
) -> str:
    lines = [
        MARKER,
        "",
        "## AI PR Review Findings",
        "",
        f"Author: @{author}",
        f"Reviewed diff: {reviewed_diff}",
        "",
    ]
    if diff_summary:
        lines.extend(["### Diff Summary", "", markdown_escape(diff_summary), ""])

    heading = {"blocker": "Blocker", "high": "High", "medium": "Medium", "low": "Low"}
    for severity in SEVERITIES:
        severity_findings = [finding for finding in findings if finding["severity"] == severity]
        if not severity_findings:
            continue
        lines.append(f"### {heading[severity]}")
        lines.append("")
        for index, finding in enumerate(severity_findings, start=1):
            evidence = finding["evidence"][0] if finding.get("evidence") else "See PR diff/context."
            lines.extend(
                [
                    f"{index}. `{finding['file']}:{finding['line']}` — {markdown_escape(finding['title'])}",
                    f"   Category: {finding['category']}",
                    f"   Confidence: {finding['confidence']:.2f}",
                    f"   Risk: {markdown_escape(finding['why_it_matters'] or finding['summary'])}",
                    f"   Evidence: {markdown_escape(evidence)}",
                    f"   Minimal fix: {markdown_escape(finding['minimal_fix'])}",
                    f"   Verification: {markdown_escape(finding.get('verification_notes', 'Accepted by final verification reviewer.'))}",
                    "",
                ]
            )

    lines.extend(copyable_findings_block(findings))

    lines.extend(
        [
            "---",
            "Scope: reviewed the PR diff and surrounding changed-file context only. No project scripts, tests, commits, or pushes were run by this reviewer.",
        ]
    )
    return "\n".join(lines).rstrip() + "\n"


def render_inline_review_summary(
    *,
    reviewed_diff: str,
    diff_summary: str,
    inline_count: int,
    fallback_count: int,
) -> str:
    lines = [
        "## AI PR Review — Diff Summary",
        "",
        f"Reviewed diff: {markdown_escape(reviewed_diff)}",
        "",
        markdown_escape(diff_summary),
        "",
        f"Inline findings: {inline_count}",
    ]
    if fallback_count:
        lines.append(
            f"Issue-comment fallback findings: {fallback_count} (not anchorable on the diff)."
        )
    lines.extend(
        [
            "",
            "Detailed findings are posted inline on the changed lines below.",
        ]
    )
    return "\n".join(lines).rstrip() + "\n"


def delete_existing_inline_comments(pr_number: str) -> int:
    deleted = 0
    page = 1
    while True:
        comments = github_request("GET", f"pulls/{pr_number}/comments?per_page=100&page={page}")
        if not comments:
            break
        for comment in comments:
            if INLINE_MARKER not in str(comment.get("body", "")):
                continue
            github_request("DELETE", f"pulls/comments/{comment['id']}")
            deleted += 1
        page += 1
    return deleted


def post_inline_review(
    pr_number: str,
    head_sha: str,
    reviewed_diff: str,
    diff_summary: str,
    findings: list[dict[str, Any]],
    commentable_lines: dict[str, set[int]],
) -> int:
    comments = []
    for finding in findings:
        if not inline_commentable(finding, commentable_lines):
            continue
        comments.append(
            {
                "path": finding["file"],
                "line": finding["line"],
                "side": "RIGHT",
                "body": inline_comment_body(finding),
            }
        )

    deleted = delete_existing_inline_comments(pr_number)
    if deleted:
        log(f"Deleted {deleted} existing AI inline review comment(s).")
    if not comments:
        log("No accepted findings could be anchored to diff lines for inline review comments.")
        return 0
    fallback_count = len(findings) - len(comments)
    if fallback_count:
        log(f"{fallback_count} accepted finding(s) could not be anchored inline.")
    review_body = render_inline_review_summary(
        reviewed_diff=reviewed_diff,
        diff_summary=diff_summary,
        inline_count=len(comments),
        fallback_count=fallback_count,
    )

    try:
        github_request(
            "POST",
            f"pulls/{pr_number}/reviews",
            {
                "commit_id": head_sha,
                "body": review_body,
                "event": "COMMENT",
                "comments": comments,
            },
        )
        log(f"Posted {len(comments)} AI inline GitHub review comment(s).")
        return len(comments)
    except Exception as batch_error:
        log(f"Batch inline review failed, retrying comments individually: {batch_error}")

    posted = 0
    failures = 0
    for comment in comments:
        try:
            payload = {"commit_id": head_sha, **comment}
            github_request(
                "POST",
                f"pulls/{pr_number}/comments",
                payload,
            )
            posted += 1
        except Exception as error:
            failures += 1
            log(
                f"Could not anchor inline comment at {comment.get('path')}:{comment.get('line')}: {error}"
            )
    if posted:
        log(f"Posted {posted} AI inline GitHub review comment(s).")
    if failures:
        raise RuntimeError(f"{failures} inline GitHub review comment(s) could not be anchored")
    return posted


def find_issue_comment_id(pr_number: str, marker: str) -> int | None:
    page = 1
    while True:
        comments = github_request("GET", f"issues/{pr_number}/comments?per_page=100&page={page}")
        if not comments:
            break
        for comment in comments:
            if marker in str(comment.get("body", "")):
                return int(comment["id"])
        page += 1
    return None


def delete_issue_comment(pr_number: str, marker: str) -> bool:
    existing_id = find_issue_comment_id(pr_number, marker)
    if existing_id is None:
        return False
    github_request("DELETE", f"issues/comments/{existing_id}")
    return True


def post_sticky_comment(pr_number: str, body: str, *, marker: str = MARKER) -> None:
    existing_id = find_issue_comment_id(pr_number, marker)

    if existing_id is None:
        github_request("POST", f"issues/{pr_number}/comments", {"body": body})
        log("Created AI PR comment.")
    else:
        github_request("PATCH", f"issues/comments/{existing_id}", {"body": body})
        log("Updated AI PR comment.")


def post_no_findings_comment(
    pr_number: str,
    head_sha: str,
    reviewed_diff: str,
    diff_summary: str,
) -> None:
    marker = f"<!-- ai-pr-review:no-findings:{head_sha} -->"
    body = "\n".join(
        [
            marker,
            "",
            "## AI PR Review — Diff Summary",
            "",
            f"Reviewed diff: {markdown_escape(reviewed_diff)}",
            "",
            markdown_escape(diff_summary),
            "",
            "No findings.",
        ]
    )
    post_sticky_comment(pr_number, body, marker=marker)


def post_findings_comment(pr_number: str, head_sha: str, body: str) -> None:
    marker = f"<!-- ai-pr-review:findings:{head_sha} -->"
    body = body.replace(MARKER, marker, 1)
    post_sticky_comment(pr_number, body, marker=marker)


def command_summary() -> None:
    config = load_config()
    github_config = config.get("github") or {}
    if not as_bool(github_config.get("summary_comment", True)):
        log("One-time PR summary comment disabled by configuration.")
        return
    ensure_credentials(config)

    pr_number = os.environ.get("AI_REVIEW_PR_NUMBER", "")
    author = os.environ.get("AI_REVIEW_AUTHOR", "")
    base_ref = os.environ.get("AI_REVIEW_BASE_REF", "")
    head_ref = os.environ.get("AI_REVIEW_HEAD_REF", "")
    if not pr_number or not base_ref:
        raise SystemExit("AI_REVIEW_PR_NUMBER and AI_REVIEW_BASE_REF are required")

    if find_issue_comment_id(pr_number, SUMMARY_MARKER) is not None:
        log("One-time AI PR summary already exists; skipping.")
        return

    context = build_context(base_ref, head_ref)
    response = run_pi(
        summary_prompt(context["text"]),
        reviewer_model(config),
        "one-time PR summary",
        thinking="low",
    )
    summary = parse_summary_response(response)
    if not summary.get("pr_summary"):
        summary["pr_summary"] = fallback_pr_summary(context["changed_files"])
    reviewed_diff = clean_string(context.get("head_subject"), 240) or clean_string(
        head_ref or base_ref or "HEAD", 240
    )
    post_sticky_comment(
        pr_number,
        render_summary_comment(author=author, reviewed_diff=reviewed_diff, summary=summary),
        marker=SUMMARY_MARKER,
    )


def command_review() -> None:
    config = load_config()
    ensure_credentials(config)

    pr_number = os.environ.get("AI_REVIEW_PR_NUMBER", "")
    author = os.environ.get("AI_REVIEW_AUTHOR", "")
    base_ref = os.environ.get("AI_REVIEW_BASE_REF", "")
    head_ref = os.environ.get("AI_REVIEW_HEAD_REF", "")
    mode = os.environ.get("AI_REVIEW_MODE") or str(config.get("mode", "balanced"))
    if mode not in VALID_MODES:
        mode = "balanced"
    if not pr_number or not base_ref:
        raise SystemExit("AI_REVIEW_PR_NUMBER and AI_REVIEW_BASE_REF are required")

    context = build_context(base_ref, head_ref)
    context_text = context["text"]
    changed_paths = {item["path"] for item in context["changed_files"]}
    max_findings = mode_max_findings(config, mode)
    raw_limit = max(4, min(max_findings, 10))
    min_confidence = float((config.get("thresholds") or {}).get("min_confidence", 0.65))
    post_low = as_bool((config.get("thresholds") or {}).get("post_low_severity", False))

    raw_findings: list[dict[str, Any]] = []
    agents = config.get("agents") or {}
    for category in CATEGORIES:
        if not as_bool(agents.get(category, False)):
            continue
        model = model_for_category(config, mode, category)
        prompt = reviewer_prompt(category, context_text, raw_limit)
        try:
            response = run_pi(prompt, model, f"{category} reviewer")
            raw_findings.extend(parse_findings_response(response, category))
        except Exception as error:  # Keep other focused reviewers useful.
            log(f"{category} reviewer produced no usable findings: {error}")

    if mode == "balanced":
        serious_candidates = [
            finding for finding in raw_findings if finding["severity"] in {"blocker", "high", "medium"}
        ]
        low_candidates = [finding for finding in raw_findings if finding["severity"] == "low"]
        if serious_candidates:
            response = run_pi(
                serious_recheck_prompt(serious_candidates, context_text, max_findings),
                serious_model(config),
                "balanced serious recheck",
                thinking="medium",
            )
            raw_findings = low_candidates + parse_findings_response(response)

    verification_response = run_pi(
        verification_prompt(raw_findings, context_text, max_findings, min_confidence),
        verification_model(config),
        "final verification reviewer",
        thinking="medium" if mode != "deep" else "high",
    )
    verified = parse_verification_response(verification_response)
    visible, _hidden_low, _rejected = filter_verified_findings(
        verified,
        changed_paths=changed_paths,
        min_confidence=min_confidence,
        max_findings=max_findings,
        post_low=post_low,
    )
    reviewed_diff = clean_string(context.get("head_subject"), 240) or clean_string(
        head_ref or base_ref or "HEAD", 240
    )
    diff_summary = clean_string(verified.get("pr_summary"), 1200) or fallback_pr_summary(
        context["changed_files"]
    )

    github_config = config.get("github") or {}
    inline_failed = False
    if as_bool(github_config.get("inline_comments", False)):
        try:
            post_inline_review(
                pr_number,
                context["head_sha"],
                reviewed_diff,
                diff_summary,
                visible,
                context["commentable_lines"],
            )
        except Exception as error:
            inline_failed = True
            log(f"Could not post inline GitHub review comments: {error}")

    sticky_enabled = as_bool(github_config.get("sticky_comment", False))
    if sticky_enabled or inline_failed or not as_bool(github_config.get("inline_comments", False)):
        issue_findings = visible
    else:
        issue_findings = [
            finding
            for finding in visible
            if not inline_commentable(finding, context["commentable_lines"])
        ]

    if not visible:
        post_no_findings_comment(pr_number, context["head_sha"], reviewed_diff, diff_summary)
    elif issue_findings:
        body = render_comment(
            author=author,
            reviewed_diff=reviewed_diff,
            diff_summary=diff_summary,
            findings=issue_findings,
        )
        post_findings_comment(pr_number, context["head_sha"], body)
    elif delete_issue_comment(pr_number, MARKER):
        log("Deleted stale AI PR review findings comment.")
    else:
        log("No AI review findings to comment.")


def main() -> None:
    parser = argparse.ArgumentParser(description="Repo-local AI PR review CI driver")
    subparsers = parser.add_subparsers(dest="command", required=True)
    subparsers.add_parser("gate", help="Check allowlist and emit GitHub Actions outputs")
    subparsers.add_parser("summary", help="Post the one-time AI PR summary comment")
    subparsers.add_parser("review", help="Run Pi review agents and post findings-only comments")
    args = parser.parse_args()
    if args.command == "gate":
        command_gate()
    elif args.command == "summary":
        command_summary()
    elif args.command == "review":
        command_review()


if __name__ == "__main__":
    main()
