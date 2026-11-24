#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""author-stop-gate — SubagentStop: nika-author does not return an unchecked file.

The author's last step is `nika check`. This gate holds it to that: when
the subagent stops, every workflow it wrote or named is checked with the
resolved binary, and it is sent back when findings remain, or when it wrote
a workflow and its return message names no path (the coordinator could not
find the file, and neither could this gate).

Laws: only `nika-author` is judged · at most two send-backs per subagent,
then it returns and the person is told what is left · a missing binary, a
check that cannot run or a payload this script cannot read is a warning,
never a block: the agent stops normally.

Reads the hook payload on stdin. Exit 0 lets the agent stop (a warning
goes out as `systemMessage`); exit 2 sends it back, the reason on stderr.
"""
import json
import os
import re
import subprocess
import sys

MAX_SEND_BACKS = 2
WRITERS = {"Write", "Edit", "MultiEdit"}
NAME = re.compile(r"[A-Za-z0-9_./~@+-]+\.nika\b")


def warn(text):
    print(json.dumps({"systemMessage": f"nika author gate: {text}"}))
    sys.exit(0)


def nika_bin(location):
    kit = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "bin", "nika-kit")
    if os.access(kit, os.X_OK):
        try:
            out = subprocess.run(
                [kit, "--path", location], capture_output=True, text=True, check=False, timeout=15
            )
            if out.returncode == 0 and out.stdout.strip():
                return out.stdout.strip()
        except (OSError, subprocess.SubprocessError):
            pass
    return os.environ.get("NIKA_BIN") or "nika"


def walk(node):
    if isinstance(node, dict):
        yield node
        for value in node.values():
            yield from walk(value)
    elif isinstance(node, list):
        for value in node:
            yield from walk(value)


def read_transcript(path):
    """(workflow paths written through the edit tools, text of the last turn)."""
    written, last = [], ""
    try:
        with open(path, encoding="utf-8", errors="replace") as handle:
            for line in handle:
                try:
                    entry = json.loads(line)
                except json.JSONDecodeError:
                    continue
                blocks = list(walk(entry))
                for block in blocks:
                    if block.get("type") == "tool_use" and block.get("name") in WRITERS:
                        target = (block.get("input") or {}).get("file_path")
                        if isinstance(target, str) and target.endswith(".nika"):
                            written.append(target)
                if any(b.get("role") == "assistant" or b.get("type") == "assistant" for b in blocks):
                    last = line
    except OSError:
        pass
    return written, last


def findings(nika, path, cwd):
    """None when clean, a short reason when refused; raises OSError with no binary."""
    proc = subprocess.run(
        [nika, "check", path, "--json", "--native-strict"],
        capture_output=True, text=True, check=False, cwd=cwd, timeout=120,
    )
    if proc.returncode == 0:
        return None
    if proc.returncode != 2:
        raise RuntimeError(f"nika check exited {proc.returncode} on {path}")
    try:
        report = json.loads(proc.stdout)
    except json.JSONDecodeError:
        return "nika check refuses it (run it for the reason)"
    rows = [f"{f.get('code')}: {str(f.get('message') or '')[:160]}" for f in report.get("findings") or []]
    rows += [str(b)[:200] for b in (report.get("verdicts") or {}).get("blockers") or []]
    if not report.get("native_strict_clean", True):
        rows += [f"hint {h.get('kind')} on {h.get('task')}" for h in report.get("hints") or []]
    return "; ".join(rows[:3]) or "nika check refuses it (run it for the reason)"


def main():
    try:
        payload = json.load(sys.stdin)
    except (json.JSONDecodeError, UnicodeDecodeError):
        warn("the hook payload was not JSON, so nothing was checked")
    if not isinstance(payload, dict):
        warn("the hook payload had an unknown shape, so nothing was checked")
    if not str(payload.get("agent_type") or "").endswith("nika-author"):
        return 0
    cwd = payload.get("cwd") if os.path.isdir(str(payload.get("cwd") or "")) else os.getcwd()
    message = str(payload.get("last_assistant_message") or "")
    written, last_turn = read_transcript(str(payload.get("agent_transcript_path") or ""))
    # The return text is the message; a host that hands the report back through
    # a tool leaves it empty, and the last turn of the transcript stands in.
    said = message or last_turn
    named = NAME.findall(said)
    unnamed = sorted({w for w in written if os.path.basename(w) not in said})

    paths = []
    for candidate in written + named:
        full = os.path.expanduser(candidate)
        full = full if os.path.isabs(full) else os.path.join(cwd, full)
        if os.path.isfile(full) and full not in paths:
            paths.append(full)

    problems = []
    if unnamed and (message or last_turn):
        problems.append(
            "you wrote " + ", ".join(unnamed) + " and your return names no such path: "
            "name the workflow's path so it can be found and checked"
        )
    nika = nika_bin(cwd)
    try:
        for path in paths[:5]:
            reason = findings(nika, path, cwd)
            if reason:
                problems.append(f"{os.path.relpath(path, cwd)} does not pass nika check: {reason}")
    except FileNotFoundError:
        warn("no nika binary was found, so the returned workflow was not checked")
    except (OSError, RuntimeError, subprocess.SubprocessError) as err:
        warn(f"the check could not run ({err}), so the returned workflow was not checked")
    if not problems:
        return 0

    agent = re.sub(r"[^A-Za-z0-9_-]", "", str(payload.get("agent_id") or "")) or "unknown"
    store = os.environ.get("CLAUDE_PLUGIN_DATA") or os.environ.get("TMPDIR") or "/tmp"
    counter = os.path.join(store, f"nika-author-gate-{agent}")
    try:
        with open(counter, encoding="utf-8") as handle:
            seen = int(handle.read().strip() or 0)
    except (OSError, ValueError):
        seen = 0
    if seen >= MAX_SEND_BACKS:
        warn(f"nika-author returned after {seen} repair rounds with this left: " + " · ".join(problems))
    try:
        os.makedirs(store, exist_ok=True)
        with open(counter, "w", encoding="utf-8") as handle:
            handle.write(str(seen + 1))
    except OSError:
        warn("could not record the repair round, so the agent was not sent back: " + " · ".join(problems))
    left = MAX_SEND_BACKS - seen - 1
    print(
        "Before you return: " + " · ".join(problems) + ". Repair the cause, run "
        "`nika check <file> --json --native-strict` until it is clean, and name the path in your "
        f"return. If a finding cannot clear, say which and why instead. ({left} more round"
        f"{'' if left == 1 else 's'} before you are let through with it reported.)",
        file=sys.stderr,
    )
    return 2


if __name__ == "__main__":
    sys.exit(main())
