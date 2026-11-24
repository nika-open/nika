---
description: Turn the work just done in this conversation into a checked workflow that can run again, showing which step became which task. The workflow is checked, never run
argument-hint: "[which part of the work to keep, in the user's words]"
allowed-tools: Bash(nika check:*), Bash(nika catalog:*), Read, Write, Edit, Glob, Grep
---

Keep what was just done as a workflow the user can run again. Scope:
`$ARGUMENTS` (empty: the most recent piece of work in this conversation).

**1 · List what happened, in order.** Go back through this conversation's
recent work and list:

- the commands that ran, with their arguments;
- the files read;
- the files written or edited, with their paths;
- the model steps: what you judged, summarized, translated or classified
  yourself;
- the hosts touched: the URLs fetched, the APIs called.

Only what happened here: no step you imagine, no host that was not
touched. If « what we just did » could mean two different pieces of work,
ask one question first.

**2 · Map each step to a task, natively.** A URL fetched becomes
`nika:fetch` with the mode that fits; a file read `nika:read`; a search
`nika:grep`; a file listing `nika:glob`; JSON shaping `nika:jq`; a CSV or
YAML conversion `nika:convert`; a file written `nika:write` at the same
path; an edit `nika:edit`. A program with no builtin twin (a test runner,
git) stays an `exec:` with `permits.exec` naming that one program. Your own
judgement becomes an `infer:` with `schema:` when its output is data and
`max_tokens:` always, on the model the user chooses: ask which, and never
pick a paid model or a seat for them. A value you typed becomes an
`inputs:` entry when it changes from run to run, a `const:` when it does
not. A key or token seen in the conversation never enters the file: it is
a `secrets:` entry naming its store, and you say which variable to set.

**3 · Make it the richest honest version.** Read the `nika-authoring`
skill's `references/capabilities.md` and `references/job-anatomy.md`. Add
by default what does not widen the effects the conversation had:
validation of what it reads, state between runs when it will repeat,
dedupe, a bounded fan-out where you did items one by one, recover samples
and retry, typed schemas, a `nika:assert` budget, a run report. The
effects stay exactly the conversation's: the same hosts, paths, programs
and kind of model. Name in one line each the upgrades that would widen
them (a new send, a new host, paid inference, a seat route, a write
outside those paths); none is added without the user's yes.

**Approve once, then let it run.** Asking for this work was the user's
approval of it. Add a gate only where something leaves the machine to a
person or a third party, or cannot be undone, and only when `nika check`
requires it (`NIKA-SEC-009`) or the user asks; prefer `mode: confirm`. A
flow the check does not refuse needs no gate, and cost is bounded by a
`nika:assert` plus the run ceiling, never by a gate. A scheduled workflow
that keeps a gate pauses at every fire (exit 4) on this version: say so.
A step that sent something (a message, a post, a push) is kept only if
the user confirms it belongs in every run.

**4 · Write and check, never run.** Write `workflows/<kebab-name>.nika`, or
the path the user names, then run
`nika check workflows/<kebab-name>.nika --json --native-strict` and repair
every finding until it is clean (`nika explain <code>` teaches one). Do
not run the workflow and do not pass `--answer`: running is the next,
separate step (`/nika:run` where the plugin provides it), with its own yes.

**5 · Show the mapping.** One line per conversation step: what happened,
then the task it became. Then the craft additions, the upgrades only
offered, the check verdict, and every step that could not be captured
(a manual decision, an interactive edit) with the reason.
