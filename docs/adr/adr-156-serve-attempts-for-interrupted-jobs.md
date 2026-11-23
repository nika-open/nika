---
id: ADR-156
title: "Re-attempt an interrupted Serve job as a new attempt of the same job"
status: accepted
date: 2026-11-23
phase: ""
deciders: ["@ThibautMelen"]
tags: ["architecture", "serve", "jobs", "retry", "resume", "cost"]
affects_crates: ["nika-serve", "nika-cli-host", "nika-cli", "nika-service-execution"]
affects_layers: ["L3", "L4"]
supersedes: []
superseded_by: []
related: ["ADR-019", "ADR-128", "ADR-130", "ADR-132", "ADR-155"]
requires: ["ADR-099", "ADR-117", "ADR-119", "ADR-129"]
enables: []
amends: []
fci: []
inv: []
shadow_zones: []
nika_codes: []
timeline: "v0.125"
follow_ups: ["a retry statement on project schedules, which needs a `nika.yaml` decision", "artifacts of earlier attempts over HTTP", "an authored idempotency key per task (ADR-099 defers it)"]
---

# ADR-156: Re-attempt an interrupted Serve job as a new attempt of the same job

## Context

A resident that stops while a job runs leaves that job `interrupted`. The word is an evidence
state (ADR-129): ownership was lost, the effects are unknown, the journal is incomplete. It is
final. An application whose deploy restarts the resident in the middle of a four-minute job
gets that word back and has to submit the work again, paying again for the tasks that had
already completed.

Three facts of 0.124.5 frame the decision:

- **The journal can already carry a run on.** `nika run --resume <trace>` (ADR-099) skips every
  task whose definition and resolved inputs match a completed record, after it has verified the
  journal's chain. An incomplete journal or a torn tail is accepted: a crash is the use case.
  A resumed run attests the trace it continues as `resumed_from` on its boot manifest.
- **The resident already sends one kind of job back to the queue.** At a restart, a `running`
  job that never received its execution identity goes back to `queued` with the event
  `execution.requeued`. A job that had started becomes `interrupted` with a receipt.
- **Three contracts forbid the obvious shortcuts.** A dead process proves incomplete evidence,
  never a failed run, and no door invents a terminal frame for it (ADR-129). The latest
  published SDK closes the `Job` object, the `JobEvent` object, the `JobReceipt` object and the
  `JobStatus` values (`published_sdk.json`): a new key or status reaches it only with a clock it
  enforces. And the threat model of `nika serve` says to reopen it before "an automatic retry
  of paid work".

Retry ownership is also settled (ADR-019): the transport never re-sends, and a task's `retry:`
belongs to the verb layer. Nothing owns what happens after the process itself is lost.

This ADR decides that. It touches the run and proof vocabulary, retry ownership and a closed
DTO, so it was written and accepted before any of it was implemented.

## Decision

A job keeps its identity. What ran is an **attempt**. An interrupted attempt stays
`interrupted` for ever; the job may receive a new attempt.

### 1. One job, several attempts

- A job has one `JobId` and one idempotency key for its whole life. Its attempts are numbered
  from 1. Each attempt has its own `execution_id`, `trace_id`, journal, status, receipt and
  spend.
- An attempt's terminal state is never rewritten. An attempt that is `interrupted` keeps its
  interrupted receipt, its incomplete journal and its evidence. This is ADR-129 held at the
  level where the evidence lives: the attempt.
- The job's `status`, `execution_id`, `trace_id`, `outputs`, `receipt`, `settlement` and
  `evidence` are those of its **latest attempt**. The closed `Job` object gains no key.
- A replay of the idempotency key answers the same job, whatever its attempt.
- The alternative, a new job per retry linked to the old one, is rejected in Alt A.

### 2. What re-attempts, and what never does

| attempt ended | automatic | by request |
|---|---|---|
| `interrupted` | yes, under the policy below | yes |
| `failed` | never | yes |
| `cancelled`, `succeeded`, `paused` | never | never |

- The resident re-attempts for one reason only: the attempt **lost its owner**. That is a new,
  third owner beside ADR-019, with a trigger the other two cannot see. It never re-attempts
  because a task or a run failed: a failed run already spent the retries its author wrote, and
  a second automatic authority over failures is what ADR-019 exists to prevent.
- A `failed` job is re-attempted only by an explicit request (section 7).

### 3. Two modes

- `resume`: the new attempt runs the job's own captured snapshot through the ADR-099 resume.
  The resident verifies the interrupted journal's chain first, folds it, and gives the plan to
  the service driver. Tasks with a matching completed record are skipped and say so
  (`task.cache_hit`); the others run. The new journal attests `resumed_from`, the interrupted
  attempt's trace. Nothing is added to the trace format.
- `restart`: the new attempt runs the snapshot from its first task. Its journal has no
  `resumed_from`; the link between the two attempts lives in the job record only.
- A journal the chain walk calls `Broken` refuses the resume. The job stays `interrupted` with
  the reason `resume_refused`. The resident never falls back to `restart` on its own, and never
  uses `--resume-unverified`.
- A resume is judged exactly as `nika run --resume` judges one: the chain, the project, the
  workflow source, the engine version, the model seat and the access. Those judgments are
  composed today in the CLI (`verbs/run/resume_setup.rs`) over functions of `nika-dap`, which
  `nika-serve` already depends on. The composition moves to one shared place and both doors
  call it; the resident gets no second copy.
- **An engine upgrade between two attempts refuses the resume.** The version judgment accepts
  another engine only under an exact compatibility declaration, and the resident never makes
  one on its own. The job stays `interrupted` (`resume_refused`); a manual retry in `restart`
  mode stays possible. A deploy of the application around the same engine is not affected.

**Task-level semantics are at least once.** A task that was in flight when the attempt was
interrupted has no completed record, so it runs again. ADR-099 already states that a crash in
the middle of a side effect can fire it twice; this decision makes that reachable without a
human, which is why sections 4 and 5 exist.

Two ADR-099 limits matter more here than under `nika run`, and the documentation of the retry
repeats them:

- A secret that changed between two attempts does not invalidate a skip. A deploy is exactly
  when secrets change.
- A completed task replays its recorded output. A file read by a completed task and changed
  since is replayed as it was read; a task that had not completed reads the new content. The
  two can meet in one run. `restart` is the mode that avoids it. The implementation tests this
  case and, if the resolved-input hash does not catch it, documents the limit rather than
  extending the hash here.

### 4. The policy is opt-in

- Server default: `nika serve --retry-interrupted off|resume|restart`. `off` is the default
  and keeps today's behaviour exactly.
- Server bound: `--retry-max-attempts N` (default 3, counting the first attempt).
- Per job, at submission: `retry: { on_interrupted: off|resume|restart, max_attempts: N }`.
  It may lower the server's bound, never raise it. It is bound into the request digest
  (ADR-132): a replay that states another retry is a conflict.
- A schedule declared over HTTP may state the same `retry` member. A project schedule cannot
  in this version: that is a `nika.yaml` key, out of scope.
- No `.nika` key is added.

### 5. Effects need consent

The resident classes each workflow from what `nika check` already derives: the tightest
boundary the body needs (`EffectivePermits::needed`: exec programs, network hosts, file paths
read and written, tools) and the faces of that derivation it could not complete (`partial`).

- **Without external effect**, all of:
  - no `exec` need;
  - no file path written (the file boundary separates `read` from `write`);
  - no tool need beyond the builtins that read or compute;
  - every network use is a `nika:fetch` whose `method` is absent, `GET` or `HEAD`, written as a
    literal. The network boundary lists hosts and knows no method, so this one reading of the
    task is new;
  - no `agent` task on a harness, and no agent tool outside the rules above;
  - no incomplete face: a computed program, host or path, or a child workflow whose effects
    the derivation did not resolve, classes the workflow with effects.
- **With effects**: everything else.

The class is computed from the job's captured snapshot, once, when the job is admitted, and
kept on the job. It is not recomputed from a workflow file that changed since.

The server default re-attempts only workflows without external effect. A workflow with effects
is re-attempted only when its job stated `retry` at submission: that statement is the consent,
given by the party that knows what the workflow does. Without it the job stays `interrupted`
with the reason `effects_need_consent`.

A model call is not an external effect in this sense. It costs money, and section 6 bounds it.

### 6. One ceiling for all attempts

- The job's ceiling covers the sum of every attempt's spend.
- A new attempt runs under what is left: the ceiling less what the earlier attempts committed.
  A settled attempt states its spend in its settlement. For an interrupted attempt the
  resident reads its journal, task by task:

  | task in the interrupted journal | counts as |
  |---|---|
  | completed | its journaled `cost_usd` |
  | started, not completed, priced route | its worst-case bound, the one the check certificate computes for one body run, times its retries and its fan-out |
  | started, not completed, no bound (no token limit, no catalog price, a fan-out sized at run time) | unknown |
  | any call on an unpriced route | unknown |

  A call in flight is never zero. The amount the engine reserved for it lived in the process
  that died and is not in the journal, so the static bound stands in for it. It can only
  overstate.
- **Unknown cost refuses the automatic re-attempt**: the job stays `interrupted` with the
  reason `unknown_spend`. The existing rule holds: an unpriced call never counts for zero.
- **An explicit budget lifts the refusal on a priced route.** `retry.max_cost_usd`, stated at
  submission or on a manual retry, is the ceiling of the re-attempts alone. Whoever states it
  accepts that the unknown spend before it is not counted. The job's cumulative spend then
  reports that part as unknown, never as zero.
- **An unpriced route keeps its own custody, unchanged.** A run of unknown cost already goes
  through the cost review door and the cost observation journal: a run killed between its
  `prepared` and `settled` rows leaves an unknown exposure that blocks the next such run until
  it is reconciled. A re-attempt is a new run in front of that same door. A one-time review is
  not reused, a standing rule is judged again, and no retry statement or budget passes the
  block.
- When nothing is left, there is no new attempt: reason `budget_exhausted`.

### 7. Stop, restart and the durable mark

- `--shutdown-grace` (delivered with ADR-155) is how long a stop waits. At its expiry a
  running attempt becomes `interrupted`, as today.
- **The durable mark is the queue itself.** In the same store write that records the
  interruption, the resident creates the next attempt as `queued` when the policy, the consent,
  the budget and the attempt bound all allow it. At a clean stop that write happens before the
  process ends. After a kill it happens at the next start, in the pass that today marks
  orphaned jobs `interrupted`. The existing recovery of queued jobs then starts the attempt in
  its owner's lane, with no priority over other owners.
- A job that will be re-attempted therefore never shows `interrupted` as its status: it goes
  from `running` to `queued`. A job whose status is `interrupted` is final unless someone asks
  for a retry. A published SDK that waits for a terminal status keeps working.
- The event is the existing `execution.requeued`: a job of a previous incarnation went back to
  the queue. No event kind is added. Its `message` names the attempt and the mode. The closed
  `JobEvent` object gains no key.
- The interrupted attempt keeps its own `execution.interrupted` event and receipt in the
  attempt history.

### 8. Crash loop

- The attempt counter is written when an attempt is created, before it runs. A job that takes
  the resident down each time it runs is counted each time.
- At `max_attempts` the job stays `interrupted`. Its last `execution.interrupted` event carries
  the code `retry_exhausted`, and the attempts route says the same.
- A re-attempt waits its turn in the fair queue (ADR-155). It is never started ahead of other
  owners' jobs.

### 9. Manual retry

`POST /v1/jobs/{id}/retry`, authenticated, health capability `jobRetry`.

- Body: `{ "attempt": N, "mode": "resume" | "restart", "max_cost_usd": X }`. `attempt` is the
  number of the attempt being retried and makes the request idempotent per attempt: the same
  request sent twice creates one new attempt and answers it both times.
- Allowed when the latest attempt is `interrupted` or `failed`. The request is the consent of
  section 5. Sections 6 and 8 apply unchanged, and so do the queue bounds of ADR-155.
- Refusals: `409 job_not_retryable` (another status, or `attempt` is not the latest),
  `409 retry_exhausted`, `409 unknown_spend`, `409 resume_refused`, `404` for a job retention
  has forgotten, and the admission refusals of the queue.

### 10. Reading attempts

- `GET /v1/jobs/{id}/attempts`: an open object with the policy in force, each attempt
  (`attempt`, `status`, `execution_id`, `trace_id`, `mode`, `resumed_from`, `receipt`, `spend`,
  instants) and the retry state (`none`, `queued`, `exhausted`, `refused` with its reason).
- The open `JobSummary` of `GET /v1/jobs` gains `attempts` (a count) and `retry_state`.
- `GET /v1/jobs/{id}/trace/verify` gains an optional `attempt` query member. Each attempt's
  journal has its own chain and seal; `nika trace verify` works on each unchanged.
- Artifacts are served for the latest attempt only in this version.

### 11. Store

The attempt history is a set of optional members of a job in store format 4, which has not
shipped. A job without them has one attempt. The migration from format 3 is the one ADR-155
describes; there is no format 5.

## Consequences

### Positive

- A deploy no longer loses a long job: under `resume` the completed tasks are not paid twice.
- The evidence of an interruption is never overwritten, and each attempt verifies on its own.
- A client keeps one identifier. A published SDK reads every response it could read before.
- No trace format change, no new event kind, no new status, no `.nika` or `nika.yaml` key.
- A poisoned job stops after a bounded number of attempts.

### Negative

- **A task can run twice.** In-flight tasks are replayed. A workflow with effects must be
  written for it, or left without a retry statement.
- **A job's `execution_id` and `trace_id` change** when it is re-attempted. A client that
  stored them from an earlier read holds an earlier attempt's. This is the visible price of one
  job identity, and it is documented on the route.
- **A call in flight is charged at its worst case.** An interruption usually catches a model
  call in flight. It counts at its static bound, which overstates, so a job close to its
  ceiling may have nothing left for a second attempt. A task with no token limit has no
  bound: its re-attempt is refused unless the job stated `retry.max_cost_usd`.
- **An engine upgrade loses the resume** of the jobs it interrupts (section 3).
- The resident becomes a retry owner. The rule that keeps it disjoint from ADR-019 is narrow
  (lost ownership only) and must stay narrow.
- A re-attempt needs the interrupted journal on disk. Trace retention or job retention that
  removes it first makes the retry impossible.
- A changed secret or a changed file can meet a replayed output (section 3).

### Neutral

- `execution.requeued` gains a second sender. Its meaning is unchanged.
- The retry reasons are codes of the resident (`effects_need_consent`, `unknown_spend`,
  `budget_exhausted`, `resume_refused`, `retry_exhausted`), served on the attempts route and as
  the `code` of an event.

## Threat model

`docs/security/nika-serve-threat-model.md` is reopened with this ADR, as it requires for an
automatic retry of paid work. The delta:

- **Repeated paid work.** Bounded by `max_attempts`, by one ceiling over all attempts, and by
  the refusal on unknown cost. Off unless the operator or the submitter turns it on.
- **Repeated effects.** Never automatic without the submitter's statement.
- **A crash loop as denial of service.** Bounded by the counter written before each attempt;
  re-attempts hold no priority in the queue.
- **A forged journal as an authority claim.** The resume verifies the chain and refuses a
  broken one; the unverified opt-out is not reachable over HTTP.
- **The retry route.** One more authenticated write. It is inside the one authority domain of
  ADR-117: whoever holds the token can already submit the same work.

## What was read on the code, and what was not

Read at 0.124.5, on the branch this ADR is written on:

1. **Spend in a journal.** Completed tasks carry `cost_usd` (the cost-replay leg of
   `nika trace verify` sums it). A reservation lives in the process's admission account and
   is not journaled. Hence the table of section 6.
2. **The resume seam.** `AuthorizedRuntime::with_resume_plan` exists; `nika-dap` holds the chain
   walk, the plan fold, the version judgment and the continuation link, and `nika-serve`
   depends on it. The composition of the judgments is in the CLI only.
3. **The permits derivation.** File paths are derived per direction. Network needs are hosts.
   `PartialFaces` names an incomplete `exec`, `net`, `fs` or `composed` face.
4. **The resume key.** It hashes the task as written and the values its references rendered
   to. File modification and file content at a path the task reads are outside it; skill
   contents and child workflow sources are inside. A completed read of a file that changed
   since is therefore skipped. Section 3 documents the limit and does not extend the key.
5. **Unknown-cost custody.** `nika_dap::cost_journal` and the resident's cost review door
   already hold the rule section 6 leaves unchanged.

Not established, to settle while implementing:

1. Whether a task that was started and not completed is always identifiable in a journal
   with a torn tail, fan-out items included.
2. Whether the check certificate's bound is reachable per task from the resident without
   running the whole check again.
3. Whether a resumed attempt restores the artifacts of the tasks it skipped, as the CLI does
   from the earlier trace.

## Evidence / Affected code

Written for the `restart` mode of the automatic re-attempt (the first delivery):

- `crates/nika-dap/src/attempt.rs` -- the effects class from the check report, the route a run
  rode, the spend of a journal with no terminal record
- `crates/nika-serve/src/job/attempt.rs` -- the policy, the statement, the attempt history, the
  decision and the fold of what is left under the ceiling
- `crates/nika-serve/src/job/store/lost.rs` -- the one write that records a loss and queues the
  next attempt or leaves the job `interrupted` with its reason; the durable cancellation mark
- `crates/nika-serve/src/server/retry.rs` -- the resident's judge, the ceiling of the attempt
  about to run
- `crates/nika-serve/src/server/mod.rs`, `crates/nika-serve/src/server/store.rs` -- the start
  pass and the stop path ask the same judge
- `crates/nika-cli-host/src/serve_args.rs`, `crates/nika-cli/src/verbs/serve.rs` --
  `--retry-interrupted off|restart`, `--retry-max-attempts`
- `crates/nika-serve/src/job/tests/attempts.rs`, `crates/nika-serve/src/server/retry_tests.rs`,
  `crates/nika-dap/src/attempt_tests.rs`

Planned:

- `crates/nika-serve/src/job/` -- the attempt history, the counter, the reasons
- `crates/nika-serve/src/job/store.rs` -- `settle_interrupted_jobs` creates the next attempt
- `crates/nika-serve/src/server/` -- the policy, the classification, the budget fold, the
  retry route, the attempts route
- `crates/nika-service-execution` -- a resume plan given by the resident
- `crates/nika-cli-host`, `crates/nika-cli` -- `--retry-interrupted`, `--retry-max-attempts`
- `crates/nika-serve/openapi.json`, `docs/security/nika-serve-threat-model.md`
- Fault-injection tests beside `server/tests/shutdown/process.rs`: a kill in the middle of a
  mock workflow then a restart (two attempts, completed tasks skipped in the journal, total
  spend under the ceiling); a stop with a grace shorter than the job; a poisoned job; a
  workflow with effects and no consent; an idempotency replay across attempts; a format 3
  store migrated and then re-attempted

## Alternatives considered

### Alt A -- a new job for each retry

A retry creates a job with a new id that names the job it retries. Every job stays immutable
after its terminal status, which is the simplest reading of ADR-129. Rejected: the client has
to follow a chain of identifiers, the idempotency key no longer names one piece of work, and
the list of a user's jobs shows one request several times. The evidence is just as safe on an
attempt as on a job.

### Alt B -- put the job back to `running`

Reuse the interrupted execution and journal. Rejected: it rewrites an evidence state and
appends to a journal whose writer died, which ADR-129 forbids.

### Alt C -- a new event kind `execution.retrying` and an `attempt` key

Clearer on the event stream. Rejected for this version: a published SDK closes the `JobEvent`
object, so the key would need a protocol clock, and `execution.requeued` already means what
happens. It can be revisited with the next SDK.

### Alt D -- re-attempt failed jobs automatically

Rejected: it is a second retry authority over failures (ADR-019).

### Alt E -- count unknown spend as zero, or at the whole ceiling

Zero breaks the rule on unpriced calls. The whole ceiling makes every interrupted metered job
unresumable. The refusal with an explicit budget says what is known and lets the submitter
decide.

### Alt F -- resume by default

Rejected: the effects of an interrupted attempt are unknown by definition.

## Related

- ADR-099 -- the resume this reuses, with its trust rule and its limits
- ADR-129 -- `interrupted` is evidence: held per attempt
- ADR-130 -- one vocabulary, one owner: no status or event kind is added
- ADR-019 -- retry ownership: a third, disjoint owner
- ADR-128 -- one settlement per run: one per attempt
- ADR-132 -- identity and idempotency: the retry statement is bound into the request digest
- ADR-119 -- the durable job store
- ADR-155 -- attribution, fair admission, `--shutdown-grace`, store format 4
- ADR-117 -- one credential, one authority domain

## Notes

Found while writing the `restart` mode, 2026-11-23. None of it changes a decision above; each
point narrows or corrects a sentence of it.

1. **No journal that can be read is unknown spend.** A run's journal opens at its first event,
   and a journal that fails to open or to write disables its own lane while the run goes on. A
   missing journal therefore cannot be told apart from a run that sent nothing, and counts as
   unknown, never as zero.
2. **The static bound prices the declared output only.** The check's per-task bound multiplies
   the output token limit by the output price, the `for_each` fan-out and the `retry` attempts.
   The input of a call in flight is not in it, so section 6's "it can only overstate" holds for
   the output only: the spend read from a lost attempt can be short by the input of the calls
   it had in flight. The next attempt's own calls are metered as any run's are. A route that
   bills reasoning beyond the declared output has no bound in section 6's sense: a call in
   flight on it makes the spend unknown.
3. **Which tasks were in flight** (first open point of "What was read"). Every task's
   `task.scheduled` record is written before the first task runs, and a wave's records are
   written before the next wave exists. The tasks with no terminal record in the first wave that
   has one are the ones that may have been sent; a fan-out task stays unsettled until all its
   items settle and counts whole. A workflow with an `on_error: { recover: ... }` task defers
   records past that wave, and then every unsettled task counts. A journal whose writes failed
   during the run stops early: what it shows is short, and one that stops after its first record
   reads as a run that sent nothing.
4. **The per-task bound** (second open point). The resident checks the job's captured workflow
   again, once per lost job, when it decides; it does not read a stored certificate.
5. **Scope of the automatic re-attempt.** A job a schedule fired on its beat is never re-attempted
   by the resident: its schedule's catch-up owns what follows a lost fire. A job admitted through
   a cost review states no `retry` (`422 invalid_job_retry`) and is not classed, so it stays
   `interrupted` with `effects_need_consent`. An accepted cancellation is made durable on the job
   before the run is told, so no later incarnation starts it again; that mark is not part of the
   event chain.
6. **Not yet delivered**: the `resume` mode, the manual retry route, the attempts route and the
   members of section 10, a `retry` member on a schedule declared over HTTP.
7. **The class is read twice.** The word kept on the job at admission enters the job's event
   chain only when the job starts. The resident reads the class again from the captured
   workflow, whose digest the job's identity binds, and the stricter of the two holds. A world
   whose digest is not the job's own is not read, and the job is then not contained.
8. **A journal is evidence only while its chain holds.** A torn last line is what a kill
   leaves and reads. A journal whose chain is broken, or that has a line that does not parse
   before its last, counts as unknown spend, like a missing one.
9. **A child workflow in flight is unknown spend.** The per-task bound of section 6 comes
   from the parent's own check, which does not bound the calls of a child workflow. A task that
   calls a child and did not settle makes the spend unknown; a settled one counts what it
   reported.
10. **Every job admitted from this version on keeps its class.** Each door that admits a
    job classes it, whatever the policy, so a later resident started with a policy can decide.
    Under `off` no decision changes; the record gains that one optional member.
11. **A cancellation racing a worker.** A cancellation accepted while a worker claims the
    job is made durable as for a running job before the run is told.

Decisions asked of the decider before any code, and accepted as written on 2026-11-23:

1. One job with attempts (section 1) rather than a job per retry (Alt A).
2. The resident as a retry owner for lost ownership only (section 2).
3. `execution.requeued` reused, no trace format change (sections 3 and 7).
4. Consent by the retry statement for workflows with effects (section 5).
5. A call in flight counts at its static bound; unknown cost refuses unless an explicit
   budget is stated; an unpriced route keeps its existing custody (section 6).
6. The attempt history inside store format 4 (section 11).
7. The durable mark is a queued attempt: a job waiting for its re-attempt reads `queued`,
   and only a job nobody will re-attempt reads `interrupted` (section 7).
8. An engine upgrade between two attempts refuses the resume instead of declaring a version
   compatibility for the operator (section 3).
