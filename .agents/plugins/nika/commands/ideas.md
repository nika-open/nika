---
description: Read this project and propose five ambitious workflows that fit it, each mapped to the Nika capabilities it uses, with its design and its one gate if any. Nothing is written until the user picks one
argument-hint: "[a focus, in the user's words]"
allowed-tools: Bash(nika list:*), Bash(nika catalog:*), Read, Glob, Grep
---

Propose the workflows this project would actually use. Focus:
`$ARGUMENTS` (empty: the whole project). This command reads and proposes.
It writes no file, runs nothing, and spends no model call on the project's
data.

**1 · Read the project, read-only.** Look for what already repeats, or
should:

- scripts (`scripts/`, `bin/`, shell and Python entry points), the
  `Makefile` targets, the `scripts` of `package.json`, a `justfile` or
  `Taskfile`;
- CI files (`.github/workflows/`, `.gitlab-ci.yml`, other CI configs),
  above all their scheduled jobs;
- cron and launchd files (a crontab kept in the repository, `*.plist`
  agents), the beats of an existing `nika.yaml`, the workflows `nika list`
  finds;
- data folders (CSV, JSON, exports, fixtures), docs (`docs/`, the README,
  the changelog), content (posts, locales, images).

Read names and headers first; open a file only when it decides an idea.
Never open `.env` or credential files, never quote a secret, and never run
the project's scripts.

**2 · Scan what Nika can do.** Read the `nika-authoring` skill's
`references/capabilities.md` and `references/job-anatomy.md`. Prefer the
jobs where a workflow is the right tool: a fan-out over many items (pages,
locales, files, images, tickets), a media pipeline, the same input
compared across several models, parallel agent seats over many items,
crawling and research, a watcher that acts on change, an exact
reconciliation (jq, not model arithmetic), anything that must run again,
be audited or be capped. Skip one-shot questions and code edits.

**3 · Propose five.** For each idea, in the user's language:

- a name, and one sentence on what it does for them;
- the evidence: the file in this project it builds on (the script it
  replaces, the CI job it watches, the folder it reads);
- a one-paragraph design: the phases (gather, normalize, judge, act,
  report), the builtins and fields each phase uses, the branch points,
  the state kept between runs, the run report;
- the capabilities it uses, by exact name (`nika:fetch` `mode: feed`,
  `for_each` with `max_parallel`, `nika:json_diff`, `nika:chart` …);
- its effects: what it reads, the hosts it reaches, what it writes, the
  model it needs (local, paid, or a seat), and its one gate if any;
- its cost: how many model calls a run makes and with which ceiling; the
  estimate itself comes from `nika check` once the file exists.

Make the five different jobs, not five versions of one. At least one runs
with no model at all; at least one reaches past the obvious (media, a
crawl, seats in parallel, a model comparison) when the project gives it
something to work on. Rank them by usefulness.

**Approve once, then let it run.** A gate stands only where something
leaves the machine to a person or a third party, or cannot be undone, and
only when `nika check` requires it (`NIKA-SEC-009`) or the user asks for
one; prefer `mode: confirm` then. A flow the check does not refuse needs
no gate. Cost is bounded by a `nika:assert` on a computed budget plus the
run ceiling, never by a gate. A scheduled workflow that keeps a gate
pauses at every fire (exit 4) on this version: say so, and design the gate
out where the risk allows.

**4 · Write nothing until they pick.** End with one question: which one to
build, if any. On a pick, pass the idea's one-sentence description to
`/nika:make` where the plugin provides it, and say that this sentence is
yours, so they can reword it; elsewhere follow the `nika-authoring` skill's
design steps with the design above. The effects the idea needs beyond what
the user already said (a send, a new host, paid inference, a seat route)
are named again before any file is written.
