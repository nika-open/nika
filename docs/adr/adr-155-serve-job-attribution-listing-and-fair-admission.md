---
id: ADR-155
title: "Attribute, list, forget and fairly admit Serve jobs inside one authority domain"
status: accepted
date: 2026-11-23
phase: ""
deciders: ["@ThibautMelen"]
tags: ["architecture", "serve", "jobs", "schedules", "admission"]
affects_crates: ["nika-serve", "nika-cadence", "nika-cli-host", "nika-cli", "nika-providers", "nika-runtime", "nika-service-execution"]
affects_layers: ["L0", "L1.5", "L3", "L4"]
supersedes: []
superseded_by: []
related: []
requires: ["ADR-117", "ADR-119", "ADR-132"]
enables: []
amends: []
fci: []
inv: []
shadow_zones: []
nika_codes: []
timeline: "v0.125"
follow_ups: ["an append-only job history with an index, under its own decision (ADR-119 reserves it for a measured scale signal; the measurement is in Context)", "attempts for interrupted jobs: ADR-156"]
---

# ADR-155: Attribute, list, forget and fairly admit Serve jobs inside one authority domain

## Context

`nika serve` at 0.124.5 admits a job, runs it and answers it by id. An application that backs
its own users with one resident cannot do four things it needs:

1. **Find its jobs again.** A job carries no statement of who it is for, and no route lists jobs.
   Support cannot answer "the failed jobs of user 123 this week".
2. **Keep running.** The job store keeps every record for ever. At its record ceiling (10,000)
   every admission answers `507 job_capacity`, and nothing but deleting the state file frees it.
3. **Manage schedules.** `PUT` and `GET /v1/schedules/{id}` exist; nothing lists the schedules,
   removes one, or runs one by hand.
4. **Share fairly.** One first-in first-out queue serves every caller, so one user who submits
   50 jobs delays everybody behind them, and nothing bounds the calls made to one provider
   across jobs.

Two facts shape what can be decided here.

**One credential is one authority domain** (ADR-117). The resident takes one Bearer token.
Whoever holds it may read and cancel every job. Nothing in this ADR changes that, so nothing in
it may be read as tenancy.

**The store is one snapshot.** Every store operation reloads, revalidates and rewrites the
whole state file. `job::tests::throughput` measures one full job (admit, stamp, start, settle)
and one read on minimal records, on one Apple Silicon laptop (APFS, 23 November 2026):

| Retained jobs | Snapshot | One job (4 writes) | One read |
|---|---|---|---|
| 250 | 0.33 MB | 63 ms | 3.7 ms |
| 500 | 0.67 MB | 87 ms | 7.2 ms |
| 1,000 | 1.34 MB | 140 ms | 14.5 ms |
| 2,000 | 2.68 MB | 244 ms | 29.3 ms |

The cost is linear in the retained jobs: about 0.10 ms per retained job for one job's writes
and 0.015 ms for one read. The store serves one operation at a time. At 5,000 jobs an hour it
keeps up only while it retains fewer than about 3,800 jobs (extrapolated from the table, not
measured), which is under an hour of history at that rate.

## Decision

A job may state who it is for; the resident lists, forgets and admits jobs by that statement,
and the statement is never an authorization boundary.

### 1. Attribution is a statement, bound from admission

- `POST /v1/jobs`, by-name form, accepts `owner` and `labels`. An owner is 1 to 128 bytes of
  `[A-Za-z0-9._:@-]`. Labels are at most 16 pairs: a key of 1 to 64 bytes of `[A-Za-z0-9._/-]`,
  a value of at most 256 bytes without control characters. Outside the law, or null, the request
  is refused `422 invalid_job_owner` or `422 invalid_job_labels` before a job exists. Nothing is
  truncated or repaired.
- Both enter the request digest: the same `Idempotency-Key` under another owner is
  `409 idempotency_conflict` (ADR-132 unchanged: one key, one request).
- Both are hash-bound in the durable record from admission, and nothing changes them later.
- A snapshot body carries none, and a cost review binds none: they are stated on the admission
  that consumes the review.
- The published `Job` object stays closed and does not carry them. They are read on the list.
- The law of their shape has one home, `nika_cadence::attribution`, because a schedule
  declaration is judged by it too (point 4). The predicates are pure.

### 2. Job store format 4

Every record gains `ordinal` (a rank the store assigns at admission and never gives again),
`created_at`, and the optional `owner`, `labels` and `max_cost_usd`. The change is a versioned
migration 3 to 4 with a frozen format-3 fixture; a format-3 store is read, ranked in its stored
order and rewritten as 4. An unattributed record adds nothing to any hash preimage, so its
hashes are what they were. An engine older than this one refuses a format-4 store by its
version, never reinterprets it.

The rewrite belongs to the first writer, which a resident is only once it holds the server
lease. Opening lifts the format in memory and writes nothing: a second start on a state root
whose live resident is the older engine loses the lease and leaves that resident's file in the
format it reads. Persisting the migration at open would have stopped every later operation of
the live resident.

### 3. `GET /v1/jobs`

- Filters, all optional, combined with AND: `owner`, `status` (several), `workflow`, `origin`,
  `label=key:value` (repeatable), `created_after`, `created_before`; `limit` 1 to 200,
  default 50; `cursor`.
- Newest admission first, by `ordinal`. `next_cursor` names the rank of the last summary served.
  A job admitted while a caller pages is newer than every page still to come, and an evicted
  rank is never reused: no summary repeats and none is skipped.
- A page is light summaries: `id`, `status`, `workflow`, `origin`, `owner`, `labels`,
  `created_at`, `execution_id`, `trace_id` and the redacted `error` of a failed job. Never
  inputs, outputs, a receipt or a settlement.
- An unknown query member, a repeated single member or a value outside its bounds is
  `400 invalid_job_query`; a cursor this list did not write is `400 invalid_job_cursor`. A
  filter is never silently ignored: an ignored `owner` would answer everybody's jobs.
- `JobList` and `JobSummary` are **open** schemas: a later engine may add a member.
- Filtering by `owner` is a query, not a permission.

### 4. Retention is opt-in and evicts only settled jobs

`--job-retention <DURATION>` forgets a `succeeded` job once its settlement is that old;
`--job-retention-unsuccessful` sets the window of `failed`, `cancelled` and `interrupted` jobs,
which otherwise follow the first. The minimum is one minute. Without a flag the resident
behaves as before.

- Eviction runs inside the admission transaction, before the idempotency lookup and the
  capacity check, and once at startup. A read never evicts.
- It removes the record, its events and its captured world. `queued`, `running` and `paused`
  jobs never leave. A settlement that carries no date is never aged.
- The evicted job's idempotency key is forgotten with it. The same key and bytes then create a
  new job: a window must outlive the caller's retries.
- The trace journal under `.nika/traces` does not follow. The store row is an index of a run;
  the journal is its evidence, under its own retention.

### 5. Schedules: attribution, list, removal, trigger

- A schedule may state `owner` and `labels`, under the law of point 1. They live on the
  canonical declaration (`nika_cadence::ScheduleDraft`), enter `ScheduleRevision` only when
  stated, and every job the schedule fires carries them. A schedule that states none keeps its
  revision, its stored row and its integrity, so **the schedule store stays at format 1**: the
  members are written only when stated, the way `inputs` already is. An older engine refuses a
  row that states them (closed row schema), never reads it with the statement dropped.
- A project beat cannot state any: that would be a new `nika.yaml` key, which this ADR does
  not decide.
- `GET /v1/schedules` lists the schedules the door created, by id, then the project's beats, by
  id, each as its status with one next slot. `GET /v1/schedules/{id}` is unchanged.
- `DELETE /v1/schedules/{id}` removes an API schedule under `If-Match` (the exact `ETag`, or
  `*`), with its last-slot decision. A job already fired is untouched. A project beat answers
  `409 schedule_declared_by_project`: its file is its one owner.
- A slot keeps the one job it fired, for as long as the store holds that job. Removal forgets
  the decision, so a schedule declared again can offer a slot that already ran; the slot's key
  then meets that job and the slot is consumed against it, whatever the workflow has become
  since. The same holds when a deploy edits the workflow between a crash and the restart that
  offers the slot again. A slot never fires twice and never stops the resident.
- `POST /v1/schedules/{id}/trigger` admits one **manual** job through the admission of
  `POST /v1/jobs`, under the schedule's workflow, inputs, ceiling, owner and labels. It
  consumes no slot and writes no decision and no firing-ledger entry, so "one slot, one firer"
  holds by construction and the next slot still fires. Its request identity is the schedule id
  alone: one key answers one job, whatever changed since. It takes no body. `JobOrigin` gains
  no variant.

This trigger is the operator's gesture that ADR-136 set aside when it rejected a bearer trigger
as the answer to third-party senders ("it solves the operator gesture"). It takes the
resident's own Bearer and is not an inbound event door.

### 6. Fair admission and one cap per provider

**Per owner.**

- `--max-queued-per-owner N`: an owner with N jobs waiting is refused `429 owner_queue_full`
  with `Retry-After`. Nothing is written and the idempotency key stays unbound, so the same
  request is sent again. `Retry-After` is a fixed hint of 5 seconds: the resident cannot know
  when one of the owner's jobs will start. `--max-concurrent-per-owner N`: an owner never has
  more than N jobs running; the rest wait while free workers serve other owners.
- A job waits in the lane of the owner it states, first in first out. Unattributed jobs share
  one anonymous lane under the same two limits.
- A free worker goes to the owner with the fewest jobs running, among the owners that have a
  job waiting and are under their running bound. Among equals it goes in turn: the owner
  served longest ago. Plain round-robin over waiting owners is this rule when no job runs; the
  running count is what makes a second owner start at the next free worker while the first
  already holds workers.
- `--max-concurrent-jobs` and `--queue-capacity` keep their meaning. They, `--max-jobs` and
  `--shutdown-grace` become flags; their defaults are the former constants.
  `--max-concurrent-jobs` stops at 256: a running job holds one thread of the resident's
  blocking pool, which its routes share.
- A place in the queue is reserved before the durable record and given back if the admission
  does not land. A replay of a job that waits, or that a worker has taken and not yet marked
  `running`, takes no second place and is not refused for the one its job has. A queued job
  that is cancelled gives its place back at once.
- A job that carries an approved cost review takes its place before the review is taken. A
  refusal for lack of a place asks the caller to send the same request again, so it must
  leave the review approved. This also changes the older `503 queue_full`, which used to
  spend the review.
- A scheduled fire is durably `running` when it reaches the queue. It is bounded by
  `--queue-capacity` only and served before the lanes: the scheduler watches a fire to its end
  within a window derived from the queue capacity, which a fire parked behind other owners
  could outlive. It counts in its owner's running total.
- Jobs that were `queued` at a stop re-enter their lanes in admission order at the next start,
  past the bounds of that start: a lowered limit never drops an admitted job. They hold their
  places, so a resident that restarts with a full queue refuses new admissions until the
  backlog drains. Before this decision the recovered jobs were held outside the queue's
  count, and a restart could briefly hold twice the capacity.

**Per provider.**

- `--provider-concurrency PROVIDER=N`, repeatable: every model call of every job takes one of
  the provider's N slots before it is sent and returns it when the call returns. A provider
  that is not named is not bounded.
- The seam is `nika_providers::ProviderCallGate`, taken by the provider registry around each
  round-trip (`infer_reported`; a stream keeps its slot until it is dropped). The resident
  implements it with one semaphore per provider and hands it to each run through
  `RuntimeConfig::provider_gate`; child runs compose with the same gate. It is absent under
  `nika run`.
- The trait lives in `nika-providers`, beside `InferenceAdmission`, not in the kernel. Its
  only caller is that crate's round-trip and its only implementor is a host that already
  depends on that crate, so a kernel trait would add a sealed contract (ADR-014) with no
  second consumer.
- Retry ownership (ADR-019) is unchanged. The gate re-sends nothing. A call that ends in a
  transient rejection has returned its slot before its verb backs off, and the next attempt
  waits its turn like any call.
- A `for_each` starts its items as before, up to `max_parallel`. Each item's call waits for a
  slot, so the calls in flight are the smaller of `max_parallel` and the free slots. The
  fan-out does not read the slot count: a snapshot would be stale at once and would slow the
  items that make no model call.
- The wait for a slot counts inside a task's `timeout:`.
- A job cancelled while its calls wait leaves the line: the resident withdraws those calls,
  nothing is sent, and the failure is not one a verb retries. The run then reaches its wave
  boundary and settles `cancelled`. A call already sent is unchanged: it finishes inside the
  cancel grace or is dropped with the run.

**Both.**

- These are operator settings of one resident: flags, like `--run-cost-ceiling`, never
  `nika.yaml` keys. They describe a process, not the project.
- `GET /v1/occupancy`, authenticated, reports running and queued jobs against their limits,
  the busiest owners (at most 100, the rest counted), and for each capped provider its cap,
  the calls in flight and the calls waiting. It is not on the public `/health`: it names
  owners.

### 7. Capabilities

`/health` advertises `jobAttribution`, `jobList`, `scheduleManage` (when the scheduler is live)
and `occupancy`. A client checks the token before it sends a new member: an older resident
may accept an unknown request field without applying it.

## Consequences

### Positive

- An application finds its jobs by owner, label, status, workflow and date with one request,
  and pages without repeats under concurrent admissions and evictions.
- A resident with a retention window no longer stops at its record ceiling.
- A store written by 0.124 opens under 0.125 without an operator step, and an unattributed job
  or schedule keeps every hash, revision and integrity it had.
- One user's burst waits in its own lane; the next owner's job starts at the next free worker.
- An operator sees, removes and runs schedules without editing files or restarting.

### Negative

- **The owner is not a boundary.** A deployment that hands the resident's token to more than
  one trust domain exposes every job to each. The application's backend must hold the token and
  apply its own authorization. This is stated in the route documentation, the operator guide
  and the threat model.
- **One snapshot does not hold a week of consumer traffic.** Seven days at 5,000 jobs an hour
  is 840,000 records. Retention makes a busy resident survive; it does not make the support
  window of the scenario fit unless only the rare unsuccessful jobs are kept for it. The
  durable answer is an append-only job history with an index (follow-up).
- **Retention forgets idempotency.** A caller that retries after the window creates a second
  job. The window is the operator's to size.
- **A slot can fire twice after a crash and a short window.** A scheduled fire creates its job
  under a key derived from the slot, then consumes the slot. If the resident stops between the
  two and the interrupted job is evicted before the slot is consumed again, the slot's key is
  gone and a slot still inside its lateness tolerance fires a new job. It needs an unsuccessful
  window shorter than the schedule's `maxLatenessSeconds`. Keep that window longer than the
  longest tolerance.
- A schedule row that states an owner is refused whole by an older engine. Rolling back after
  attributing a schedule means removing the attribution first.
- Listing reads the whole snapshot for each page (one read, then filter). Its cost is the read
  column of the table.
- **The provider cap does not cover every call.** Calls an agent harness makes through its own
  client (ACP), the image and speech tools, and the authoring door (`POST /v1/compile`) do not
  pass the gate. A deployment that relies on them for volume is not bounded by it.
- **A cap far under the demand becomes task timeouts.** The wait for a slot counts inside a
  task's `timeout:`. The operator sizes the cap from the provider's limit and
  `--max-concurrent-jobs` from the cap; `providers[].waiting` in the occupancy shows the
  pressure.
- A cancelled job's waiting calls notice the stop by reading the run's cancel flag every
  25 ms: the flag has no waker. The cost is one timer per waiting call.
- The queue and the provider slots are one resident's memory. Two residents on one state root
  are still refused, so the limits are never split across processes.

### Neutral

- `nika-cadence` (L0) gains a pure `attribution` module and two schedule members. No I/O, no
  clock.
- The trace journal of an evicted job stays on disk until its own retention removes it.
- A triggered job is manual: `${{ run.scheduled_for }}` reads its own start, and the cadence
  shows no decision for it.
- `nika-providers` (L1.5) gains a `gate` module and `ProviderRegistry::with_call_gate`;
  `RuntimeConfig` (L3) gains one optional member. Without a gate both behave as before.
- The fair queue keeps all its state under one mutex. Loom (ADR-013) covers the lock-free
  cancel token of `nika-types` in this repository; it is not applied to a mutex-held queue,
  whose invariants are held by unit tests over every transition instead.

## Evidence / Affected code

- `crates/nika-cadence/src/attribution.rs` -- the one law of an owner and labels
- `crates/nika-cadence/src/schedule.rs` -- `ScheduleDraft::{owner, labels}`, the revision's
  canonical form, `ScheduleFindingKind::{Owner, Labels}`
- `crates/nika-serve/src/job/attribution.rs` -- `JobAttribution`, `JobBindings`
- `crates/nika-serve/src/job/store.rs` and `crates/nika-serve/src/job/model.rs` -- format 4,
  the migration, the rank
- `crates/nika-serve/src/server/job_list.rs` -- the query, the cursor, the summary
- `crates/nika-serve/src/schedule/store.rs` -- `overview`, `remove`, the row's optional members
- `crates/nika-serve/src/server/schedule_manage.rs` -- list, removal, trigger
- `crates/nika-serve/src/job/tests/throughput.rs` -- the measurement in Context
- `crates/nika-serve/src/server/fair_queue.rs` -- the lanes, the turn, the bounds
- `crates/nika-serve/src/server/occupancy.rs` -- `GET /v1/occupancy`, `429 owner_queue_full`
- `crates/nika-serve/src/server/resident_options.rs` -- the limit flags
- `crates/nika-providers/src/gate.rs` -- `ProviderCallGate`, `ProviderCallSlot`
- `crates/nika-providers/src/registry.rs` -- the slot around a round-trip and a stream
- `crates/nika-runtime/src/config.rs`, `crates/nika-runtime/src/compose.rs` -- the gate carried
  into the composed registry
- `crates/nika-service-execution/src/lib.rs` -- a child run composes with its parent's gate
- `crates/nika-serve/src/server/provider_gate.rs` -- the resident's semaphores, the per-run
  withdrawal, the load
- `crates/nika-serve/src/server/fair_admission_tests.rs`,
  `crates/nika-serve/src/server/provider_gate/resident_tests.rs`,
  `crates/nika-providers/src/gate_tests.rs` -- fairness, bounds, the cap across three
  fanned-out jobs, a child run, cancellation
- `crates/nika-serve/openapi.json` -- the served contract
- Commits `17c9ddebf` (format 4), `699fa2a91` (attribution), `4d8edcd68` (list), `4d96f57f9`
  (retention), `783a61c09` (schedules), `65b7027a0` (fair admission, limit flags, occupancy),
  `b9e73992e` (the migration written under the lease, a forgotten queue entry, a slot keeps its
  job), `30f3b3404` (the provider cap)

## Alternatives considered

### Alt A -- one token per owner

Real tenancy: the resident would hold a credential per owner and authorize each read. It
changes ADR-117's single authority domain, the token file, the threat model and every route.
Rejected for this delivery: the scenario's caller is one backend that already authorizes its
users. Nothing here prevents it later, and the `owner` member would then be derived from the
credential instead of stated.

### Alt B -- a second persistence form now

An append-only history with an index answers the scale limit in Context. ADR-119 reserves it
for a measured signal, which now exists. Rejected for this delivery: it replaces the store's
transaction model and deserves its own decision and migration. Retention is what keeps a
resident alive meanwhile.

### Alt C -- offset pagination, or a cursor over `created_at`

An offset repeats or skips a row whenever a job is admitted or evicted between two pages. A
date is not unique and a clock can step back. The rank is unique, monotone and never reused.

### Alt D -- bump the schedule store to format 2

It would make every older engine refuse every schedule store after an upgrade, including the
ones that state no owner. Writing the members only when stated gives the same refusal exactly
where it is needed.

### Alt E -- a `trigger` origin, or a trigger that consumes the next slot

A new `JobOrigin` variant changes a published object for no reader that needs it: the job's
labels can already say why it ran. Consuming a slot would make a manual gesture move the
cadence and compete with the scheduler for "one slot, one firer".

### Alt F -- weighted or priority queues

Fewest-running-first across owners, in turn among equals, with two bounds answers the
scenario. Weights need a notion of who deserves more, which only real tenancy (Alt A) can
state.

### Alt G -- the provider gate as a kernel trait

Rejected. A kernel trait is a sealed contract every layer can see (ADR-014). The gate has one
caller, the provider round-trip, and one implementor, a host above it; both already meet in
`nika-providers`, where `InferenceAdmission` set the precedent for a host-supplied object the
registry consults. Moving it to the kernel later stays possible if a second transport needs it.

### Alt H -- the fan-out reads the free slots

Rejected. Making `for_each` start only as many items as slots are free needs a count that is
stale as soon as it is read, shared across runs, and it would hold back items that call no
model. Taking the slot around each call gives the same bound on calls in flight with no
change to the runtime's fan-out.

### Alt I -- provider caps in `nika.yaml`

Rejected. The project file describes the project and travels with it; a cap describes one
process and the account behind it. A block there would also be a new project-file contract.

## Related

- ADR-117 -- one credential, one authority domain: unchanged
- ADR-119 -- the durable job store and its reserved second form
- ADR-132 -- identity and idempotency: the request digest now binds attribution
- ADR-136 -- inbound event bindings: the trigger here is the operator gesture it set aside
- ADR-019 -- retry ownership: unchanged by the provider gate
- ADR-014 -- sealed kernel traits: untouched, the gate is not one (Alt G)
- ADR-013 -- loom: where it applies in this repository (Neutral)
- ADR-156 -- attempts for interrupted jobs
- `docs/crate-specs/nika-serve.md`, `docs/ops/README.md`

## Notes

Revisit when a deployment needs more than one trust domain behind one resident (Alt A), or
when the retained history a deployment needs no longer fits one snapshot (Alt B).
