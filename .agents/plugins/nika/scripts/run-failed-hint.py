#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""run-failed-hint — PostToolUseFailure: read the trace before a second run.

A failed `nika run` already had effects: files written, calls paid, maybe a
message sent. The reflex after a red exit is to run again; that repeats
them. This hook hands the agent the trace to read first, and tells a pause
(exit 4) from a failure, because a pause is answered by the person and not
retried.

It reads the hook payload on stdin and the trace folder; it never calls
the binary. Any other failed command draws no output. It cannot block and
always exits 0; a payload it cannot read is said in one `systemMessage`.
"""
import glob
import json
import os
import re
import sys

RUN = re.compile(r"""(?:^|[\s;&|(/"'])nika(?:-[\w.]+)?["']?\s+run(?:\s|$)""")
TRACE = re.compile(r"^[A-Za-z0-9_./:-]+\.ndjson$")


def newest_trace(cwd):
    """The newest trace by modification time; a link that leads nowhere is not one."""
    newest, when = None, -1.0
    for path in glob.glob(os.path.join(cwd, ".nika", "traces", "*.ndjson")):
        try:
            stamp = os.path.getmtime(path)
        except OSError:
            continue
        if stamp > when:
            newest, when = path, stamp
    if newest is None:
        return None
    path = os.path.relpath(newest, cwd)
    return path if TRACE.match(path) else None


def main():
    try:
        payload = json.load(sys.stdin)
        command = str((payload.get("tool_input") or {}).get("command") or "")
    except (json.JSONDecodeError, UnicodeDecodeError, AttributeError):
        print(json.dumps({"systemMessage": "nika run-failed hint: the hook payload was not readable, so no hint was given"}))
        return 0
    if payload.get("tool_name") != "Bash" or not RUN.search(command):
        return 0
    found = re.match(r"\s*Exit code (\d+)", str(payload.get("error") or ""))
    code = int(found.group(1)) if found else None
    cwd = payload.get("cwd") if os.path.isdir(str(payload.get("cwd") or "")) else os.getcwd()
    trace = newest_trace(cwd)
    where = f"`{trace}`" if trace else "the newest trace (`nika trace ls` names it)"
    if code == 4:
        text = (
            "Exit 4 from `nika run` is a pause, not a failure: the run waits for the person. "
            "Show them its question exactly as the run recorded it (the paused frame carries it, "
            "and `/nika:pending` prints it with the resume line), wait for their own answer, then "
            f"resume with `--resume <trace> --answer <task>=<their answer>` on {where}. "
            "Never answer it yourself and do not start the run again."
        )
    else:
        said = f"exit {code}" if code is not None else "a red exit"
        text = (
            f"This `nika run` failed ({said}). Read what happened before any second run: "
            f"`nika trace outputs <trace> --json` on {where} names the task that failed and its "
            "code, and `nika explain <code>` teaches the fix. A run from the start repeats every "
            "effect the first one already had (files written, calls paid, anything sent). After "
            "the repair, `nika check` the file, then prefer `nika run <file> --resume <trace>`, "
            "which skips what settled, and read its first line: a trace that « carries no resume "
            "keys » runs everything again."
        )
    print(json.dumps({"hookSpecificOutput": {"hookEventName": "PostToolUseFailure", "additionalContext": text}}))
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception:  # pylint: disable=broad-except
        # The header's promise: this hook cannot block, whatever it met.
        sys.exit(0)
