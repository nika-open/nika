#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""precompact-notes — PreCompact: what must survive the compaction.

A run waiting for the person's answer and a workflow half-written are
state on disk; the conversation that knew about them is about to be
summarized. This prints, as instructions for the summary, the runs that
wait and the workflow files changed since the session started.

It prints names only (workflow, task, trace path), never the text of a
question: a run recorded that text, and here it would arrive as an
instruction. Nothing to keep prints nothing. Compaction is never blocked:
exit 0 whatever is missing, and a missing binary is said in the notes.
"""
import json
import os
import re
import subprocess
import sys

PLAIN = re.compile(r"^[A-Za-z0-9_./:@-]{1,160}$")
SKIP = {"node_modules", ".git", "target", "dist", ".nika"}


def plain(value, fallback):
    return value if isinstance(value, str) and PLAIN.match(value) else fallback


def waiting_runs(cwd):
    """(lines, note) for the runs that wait in cwd."""
    if not os.path.isdir(os.path.join(cwd, ".nika", "traces")):
        return [], None
    helper = os.path.join(os.path.dirname(os.path.abspath(__file__)), "nika-pending.py")
    try:
        proc = subprocess.run(
            [sys.executable, helper, cwd, "--json"], capture_output=True, text=True, check=False, timeout=20
        )
        rows = json.loads(proc.stdout).get("waiting", [])
    except (OSError, subprocess.SubprocessError, json.JSONDecodeError, AttributeError):
        return [], "Runs may be waiting here and could not be listed (no nika binary answered): list them with `nika trace ls` once it does."
    lines = []
    for row in rows[:8]:
        lines.append(
            f"A run of {plain(row.get('workflow'), 'a workflow')} is waiting for the person's answer at task "
            f"{plain(row.get('task'), '(read the trace)')} (trace {plain(row.get('trace'), 'named by nika trace ls')}). "
            "Only the person answers it; read its question again with /nika:pending before asking them."
        )
    return lines, None


def changed_workflows(cwd, session):
    store = os.environ.get("CLAUDE_PLUGIN_DATA") or os.environ.get("TMPDIR") or "/tmp"
    marker = os.path.join(store, f"nika-session-{session}")
    try:
        stat = os.stat(marker)
    except OSError:
        return []
    # session-status.sh writes « started <epoch> » on the marker's first line.
    # The stop gate appends the files it reported below it, which moves the
    # marker's modification time: read the start from the line, and count the
    # files the gate wrote down as work in progress too.
    since = getattr(stat, "st_birthtime", stat.st_mtime)
    found = []
    try:
        with open(marker, encoding="utf-8", errors="replace") as handle:
            for line in handle:
                head, _, tail = line.strip().rpartition(" ")
                if head == "started" and tail.isdigit():
                    since = float(tail)
                    continue
                path = os.path.normpath(head)
                if path.endswith(".nika") and os.path.isfile(os.path.join(cwd, path)):
                    found.append(path)
    except OSError:
        pass
    for folder, dirs, files in os.walk(cwd):
        dirs[:] = [d for d in dirs if d not in SKIP]
        if os.path.relpath(folder, cwd).count(os.sep) >= 3:
            dirs[:] = []
        for name in files:
            path = os.path.join(folder, name)
            try:
                if name.endswith(".nika") and os.path.getmtime(path) > since:
                    found.append(os.path.relpath(path, cwd))
            except OSError:
                continue
    return [p for p in sorted(set(found)) if PLAIN.match(p)][:8]


def main():
    try:
        payload = json.load(sys.stdin)
    except (json.JSONDecodeError, UnicodeDecodeError):
        payload = {}
    payload = payload if isinstance(payload, dict) else {}
    cwd = payload.get("cwd") if os.path.isdir(str(payload.get("cwd") or "")) else os.getcwd()
    session = re.sub(r"[^A-Za-z0-9_-]", "", str(payload.get("session_id") or ""))
    lines, note = waiting_runs(cwd)
    for path in changed_workflows(cwd, session) if session else []:
        lines.append(
            f"Workflow in progress: {path} was changed in this session. Before handing it over, run "
            f"`nika check {path} --json --native-strict` again and quote its verdict."
        )
    if note:
        lines.append(note)
    if lines:
        print("Keep these Nika facts in the summary, as written:")
        print("\n".join(f"- {line}" for line in lines))
    return 0


if __name__ == "__main__":
    sys.exit(main())
