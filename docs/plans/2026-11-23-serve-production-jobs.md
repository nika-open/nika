# `nika serve` ready for production jobs · plan

> Status: A and B are built on `feat/serve-production-jobs`, branched from
> `main` at `8b640a9b7` (0.124.5). C (retry interrupted jobs) stopped at
> ADR-156 until that ADR was accepted, on 23 November; it is built on a
> branch stacked on this one. Three workstreams in order: A (list a user's
> jobs), B (concurrency and fairness), C. The report is
> `2026-11-23-serve-production-jobs-report.md`.

## 1. Goal

Make the resident (`nika serve`) able to back consumer applications: traffic
spikes, thousands of jobs a day, deploys while jobs run, and a support desk
that finds one user's jobs.

Acceptance scenario:

1. A backend posts `POST /v1/jobs` with `owner: "user_123"` and
   `labels: {app: "translation", plan: "pro"}`, using the server-to-server
   token.
2. 5,000 jobs arrive in an hour. A user who submits 50 does not block the
   others, and no provider ever sees more than N concurrent calls.
3. A deploy restarts the server during a four-minute job. When the policy
   allows it, the job resumes after the restart, skips the tasks that
   completed, and stays inside its budget.
4. Support lists the failed jobs of `user_123` over seven days and retries
   one.

## 2. What the code is today

Measured on the branch point, before any change.

### 2.1 Facts that differ from the brief

| Brief | Measured |
|---|---|
| `nika-serve` is about 21,000 lines | 17,520 production lines by the gate's own counter (`scripts/ci/check-crate-size.sh`); the wall is 25,000 and the warning 15,000, so 7,480 lines are left |
| `max_jobs` eviction exists and is to be studied | there is no eviction anywhere: at `max_jobs` (10,000) every new admission answers `507 job_capacity` for ever |
| limits are configured on the server | `nika serve` exposes no limit flag: four workers, a queue of 64 and a 30-second grace are compiled in |
| the trace format changes for attempts | `resumed_from` already rides `workflow_started`; a new attempt needs no new journal field |

Baseline: `cargo test -p nika-serve --lib --locked` passes 434 tests, 3
ignored, on the untouched branch.

### 2.2 Job lifecycle

`POST /v1/jobs` (`server/route.rs`) reads the idempotency key and the bounded
body, hashes the exact body bytes into the request digest, replays a key that
is already bound, then admits one of two forms (ADR-131): a workflow name the
served registry lists, or the snapshot `nika check --json --sdk-snapshot`
prints. `ResidentExecutionCoordinator` (`server/coordinator.rs`) reserves a
slot in a bounded `mpsc` channel (`queue_capacity`, full answers
`503 queue_full`), creates the durable record `queued`, and sends an
`ExecutionTask`. The authority loop (`server/mod.rs`) receives tasks in
arrival order and runs `run_job` while fewer than `max_concurrent_jobs` are
in flight. Dispatch is strictly first in, first out, with no notion of who
asked.

Legal status edges: `queued → running | failed | cancelled`,
`running → paused | succeeded | failed | cancelled`,
`paused → running | failed | cancelled`. `interrupted` has no outgoing edge
(ADR-119, ADR-129).

Each job runs on a blocking worker with its own current-thread runtime
(`server/production.rs`), through `ServiceExecutionDriver`. The provider
registry and its HTTP client are built once per job and once more per child
workflow; nothing provider-related is shared across the process.

### 2.3 Restart

Startup (`prepare_authority`) claims a new server incarnation under
`jobs/server.lock`, then settles what the previous incarnation left: a
`running` job with a complete execution identity becomes `interrupted` with a
receipt; a `running` job without one goes back to `queued`
(`execution.requeued`); `queued` jobs are re-enqueued from their captured
world; `paused` survives unchanged. A graceful stop drains running and queued
jobs for `shutdown_grace`, then aborts what still runs and marks it
`interrupted`. Nothing retries an interrupted job.

### 2.4 The durable job store (format 3)

One file, `jobs/state.json`, at most 16 MiB. Every operation takes an
in-process mutex and a kernel lease, reads the whole snapshot, validates it
completely (every event hash of every job is recomputed), and every mutation
rewrites the whole file atomically. `decode_state` probes the version: 3 is
read directly, 2 goes through an explicit migration, anything else refuses.
A writer stamp (ADR-132) refuses a store a newer engine wrote.

The cost of one operation is therefore linear in the number of retained
jobs. The on-demand measurement `job::tests::throughput` prints it; section
7 records the figures and what they mean for the acceptance scenario.

### 2.5 Schedules

`PUT /v1/schedules/{id}` and `GET /v1/schedules/{id}` exist. List, delete and
trigger are pinned absent by a test and by the OpenAPI text. Project beats
(`nika.yaml`) and API schedules ride one planner (`server/scheduler.rs`). A
project slot belongs to whichever firer claims it first in the ARM ledger
(`nika-arm`); the job store adds its own guard, because a fired slot's job is
admitted under the idempotency key of that slot. A scheduled run executes
from the scheduler, holding one queue slot; it does not wait for a worker.

### 2.6 Contract guards that shape the design

- **The published SDK closes objects.** `server/openapi/published_sdk.json`
  lists the keys the published SDK accepts on `Job`, `JobEvent`,
  `JobReceipt`, `JobOrigin`, `Error`, and the values of `JobStatus` and of a
  schedule origin's `decision`. Adding a key or a value there requires
  raising `MACHINE_PROTOCOL_VERSION` in the same commit, and the published
  SDK then refuses this engine. This plan adds none: new data travels on new
  schemas.
- **ADR-117** says the API has no multi-tenant authorization model and that
  tenancy requires a later ADR. ADR-155 states what `owner` is and is not.
- **`ExecutionSnapshot`** is the wire of `nika check --sdk-snapshot`; a
  snapshot body carries no request overlay. It stays untouched.
- **Size walls (ADR-154).** `server/mod.rs`, `job/store.rs` and
  `server/openapi.rs` are near the 1,500-line file warning. New code goes to
  new files.
- **`public-api.txt`.** `nika-serve` is in the public-API coverage floor, so
  every new `pub` item updates its snapshot. New items stay `pub(crate)`
  unless the CLI needs them.

## 3. Workstream A · list a user's jobs

### 3.1 Attribution on admission

`POST /v1/jobs`, by-name form, accepts two optional members:

- `owner`: 1 to 128 bytes of `[A-Za-z0-9._:@-]`. Opaque to the engine.
- `labels`: at most 16 entries. A key is 1 to 64 bytes of `[A-Za-z0-9._/-]`
  (no colon, so `label=key:value` splits on the first colon). A value is 0 to
  256 bytes of UTF-8 without control characters.

Refusals are typed and happen before a job exists: `422 invalid_job_owner`,
`422 invalid_job_labels`. A present `null` is refused like every other
member of that form.

The snapshot form does not accept them: its body is a frozen world with no
overlay, and that envelope is a reserved contract. A snapshot job is
unattributed. This is recorded as a remaining limit.

**Idempotency.** The request digest is the SHA-256 of the exact body bytes,
so `owner` and `labels` are in it by construction. The same key with another
owner is `409 idempotency_conflict`.

**Integrity.** `owner` and `labels` enter the identity and event preimages
only when present, the rule `inputs` already follows. A job without them
hashes exactly as it did in format 3.

**Authorization.** The bearer token is a server-to-server credential.
`owner` is a label the caller states; filtering by it is not an
authorization boundary. Whoever holds the token reads every job. The
operator guide and the threat model say so.

### 3.2 Store format 4

`JobRecord` gains four members, all absent on a record that does not carry
them:

| Member | Meaning |
|---|---|
| `owner` | the stated owner |
| `labels` | the stated labels |
| `created_at` | the admission instant; absent on a migrated record, never invented |
| `ordinal` | the record's admission rank in this store, the listing order |

`PersistedState` gains `next_ordinal`. `STATE_VERSION` becomes 4.

Migration: `decode_state` reads version 4 directly. Version 3 is decoded,
refused if it carries a format-4 member, given ordinals in stored order,
and persisted once as version 4. Version 2 keeps its explicit migration and
ends in the same step. Event hashes, receipts and identity digests are
untouched: a migrated store validates with the same evidence.

Fixtures: `fixtures/v3.json` stays frozen; `fixtures/v4.json` is added.
Tests: the version-3 golden migrates and keeps every hash; a second open
reads version 4 without rewriting; the version-4 golden round-trips byte for
byte; a version-3 file with a format-4 member refuses.

`/health` reports `storeFormatVersion.jobs: 4`. An engine older than this
one refuses the store (the writer stamp, then the version).

### 3.3 `GET /v1/jobs`

Query members, all optional, unknown ones refused (`400 invalid_job_query`):

| Member | Meaning |
|---|---|
| `owner` | exact owner |
| `status` | repeatable or comma-separated; any of the seven statuses |
| `workflow` | exact workflow name |
| `origin` | `manual` or `schedule` |
| `label` | `key:value`, repeatable, all must match |
| `created_after`, `created_before` | RFC 3339 instants, exclusive |
| `limit` | 1 to 200, default 50 |
| `cursor` | the `next_cursor` of the previous page |

Order: newest first by `ordinal`. The cursor is opaque to callers and names
the last ordinal served; the next page holds strictly older records. A job
admitted during pagination has a higher ordinal, so it never shifts a later
page; an evicted job simply no longer appears. A bad cursor is
`400 invalid_job_cursor`.

A record without `created_at` (migrated) matches a time filter by its first
dated event, and is excluded from a time-filtered listing when it has none.

Response, on new schemas so `Job` stays closed:

```json
{"jobs": [{"id": "…", "status": "failed", "workflow": "…",
           "origin": {"kind": "manual"}, "owner": "user_123",
           "labels": {"app": "translation"}, "created_at": "…",
           "execution_id": "…", "trace_id": "…",
           "error": {"code": "…", "message": "…"}}],
 "next_cursor": "…"}
```

A summary never carries `inputs`, `outputs`, a receipt or a settlement. The
error follows the redaction of `GET /v1/jobs/{id}`: present only on a failed
job, bounded and path-free.

The store answers a page from one snapshot read under the lease.

### 3.4 Retention of settled jobs

Without eviction the store fills and refuses for ever. Retention is added,
opt-in, so a resident keeps today's behaviour unless the operator asks:

- `--job-retention <DURATION>`: a `succeeded` job is evicted once it has been
  settled that long.
- `--job-retention-unsuccessful <DURATION>`: the same for `failed`,
  `interrupted` and `cancelled`; defaults to `--job-retention`.

Eviction runs inside the admission transaction, before the capacity check,
and once at startup. It removes the record, its events and its captured
world. `queued`, `running` and `paused` jobs are never evicted.

Two consequences are stated in the operator guide:

- An evicted job's idempotency key is forgotten. Retention must exceed the
  caller's retry horizon; the flag refuses less than one minute.
- The trace journal under `.nika/traces` does **not** follow. It is the
  evidence, with its own retention (`nika trace`); the store row is an index
  of it.

### 3.5 Schedules: list, delete, trigger, attribution

- `GET /v1/schedules`: the status projection of every API schedule and every
  live project beat, bounded by the existing ceilings.
- `DELETE /v1/schedules/{id}`: API schedules only, under `If-Match` (the
  current revision, or `*`). It removes the definition and its last-slot
  decision. A project beat is declared in `nika.yaml` and answers
  `409 schedule_declared_by_project`.
- `POST /v1/schedules/{id}/trigger`: requires `Idempotency-Key`. It admits
  one **manual** job that runs the schedule's workflow as it is admitted now,
  with the schedule's inputs, cost ceiling, owner and labels. It consumes no
  slot and writes nothing to the ARM ledger, so "one slot, one firer" holds
  by construction; the next scheduled slot fires as planned. The response is
  the `Job`. Backfill stays absent.
- A schedule may carry `owner` and `labels`. They live on the declaration
  (`nika-cadence`, following `inputs`: part of the revision only when bound,
  so an existing schedule keeps its revision) and every job it fires carries
  them. Project beats cannot set them: that would be a new `nika.yaml` key.

### 3.6 Capabilities

`supportedCapabilities` gains `jobAttribution` (owner and labels on
`POST /v1/jobs`), `jobList` (`GET /v1/jobs`) and, where the scheduler is
live, `scheduleManage` (list, delete, trigger, attribution). A client must
see the token before sending the new members: an engine without it refuses
them as unknown fields.

### 3.7 Tests

Pagination stays stable while jobs are inserted and evicted; every filter
alone and combined; migration 3 → 4 on the frozen fixture; idempotency with
an owner (replay, conflict on another owner); typed refusals at each bound;
a summary never leaks inputs or outputs; `/health` advertises the tokens;
retention evicts only settled jobs past their age and frees capacity;
schedule list, delete (precondition, project beat) and trigger (idempotent,
carries attribution, consumes no slot).

## 4. Workstream B · concurrency and fairness

`max_concurrent_jobs` and `queue_capacity` keep their meaning.

### 4.1 Per-owner limits and fair dispatch

- `--max-queued-per-owner N`: an owner with N jobs waiting is refused
  `429 owner_queue_full` with `Retry-After`.
- `--max-concurrent-per-owner N`: an owner never has more than N jobs
  running; the rest wait.
- A free worker goes to the owner with the fewest jobs running, among the
  owners that have a job waiting; in turn among equals; first in, first out
  inside one owner. Unattributed jobs share one anonymous lane, with the
  same two limits. (As built: plain round-robin over waiting owners would
  let an owner that already holds workers take the next one before a
  newcomer. Counting the running jobs is what makes owner B start at the
  next free worker.)

The `mpsc` channel and its permit give way to one bounded fair queue
(`server/fair_queue.rs`): a lane per owner, a ring of owners, the global
capacity and the two per-owner bounds, behind one mutex with a wake signal.
A reservation is taken before the durable record is created and released if
admission fails, exactly as the permit was. Jobs recovered at restart
re-enter their lane without being refused. A scheduled fire is durably
`running` when it reaches the queue: it takes one global place, is served
before the lanes, is never refused or held back by a per-owner bound, and
counts in its owner's running total. The scheduler watches a fire within a
window derived from the queue capacity, which a fire parked behind other
owners could outlive. A queued job that is cancelled gives its place back
at once, and a replay takes no second place. `Retry-After` on
`429 owner_queue_full` is a fixed hint of 5 seconds.

The same commit exposes the limits the resident already had but could not
set: `--max-concurrent-jobs`, `--queue-capacity`, `--max-jobs`,
`--shutdown-grace`.

**Flags, not `nika.yaml`.** These are operator settings of one resident,
like `--run-cost-ceiling` and `--cost-policy`. A `nika.yaml` block would
change the project file's language and need its own decision.

### 4.2 One cap per provider across all jobs

`--provider-concurrency PROVIDER=N` (repeatable). Every model call of every
job waits for one of the provider's N tokens and returns it when the call
returns.

- **Seam.** One trait, `ProviderCallGate`, with an owned slot. As built it
  lives in `nika-providers`, beside `InferenceAdmission`, and not in
  `nika-kernel-ai` as first planned: its only caller is the provider
  round-trip and its only implementor is a host that already depends on
  `nika-providers`, so a kernel trait would add a sealed contract (ADR-014)
  with no second consumer. The gate travels the route the cost admission
  already travels: `RuntimeConfig` → the provider registry → each resolved
  provider, and is taken in `ResolvedProvider::infer_reported`, the one
  point every `infer` and `agent` call crosses, and kept by a stream until
  it is dropped. Child workflows receive it too.
- **Implementation.** `nika-serve` holds one semaphore per provider for the
  process and implements the trait. `nika run` sets none: unlimited, as
  today.
- **Retries (ADR-019).** Unchanged. The transport never re-sends; an authored
  `retry:` belongs to the runtime. The permit is released before the backoff
  sleep and taken again for the next attempt.
- **`for_each`.** Iterations past the available tokens wait at the gate, so
  the effective width is the smaller of `max_parallel` and the free tokens.
- **Cancellation.** A waiter holds nothing while it waits and leaves the
  line when its future is dropped. A job's operator cancel is a flag
  without a waker, so the resident wraps the gate per run and reads the
  flag every 25 ms: a cancelled job's waiting calls are withdrawn, nothing
  is sent, and the failure is not one a verb retries.
- **Timeouts.** The wait counts inside a task's `timeout:`.
- **Not covered:** harness and ACP seats, the image and speech builtins
  (they post directly), and the resident's own authoring and Session seats.
  Recorded as remaining limits.

### 4.3 Occupancy

`GET /v1/occupancy`, authenticated (never on the public `/health`): running
and queued jobs against their limits, per-owner running and queued counts
(bounded list), and per provider the calls in flight, the waiters and the
cap. Capability `occupancy`. No Prometheus endpoint in this delivery.

### 4.4 Tests

Deterministic, with a backend whose release the test controls:
owner A queues 50 jobs, owner B submits one, and B's job starts at the next
free worker; the per-owner refusal and its `Retry-After`; the running bound
per owner; a queued job cancelled behind an owner bound frees its place and
nothing deadlocks; three concurrent `for_each` jobs never exceed the
provider cap, proven with a gate that counts and releases; restart recovers
lanes. Loom (ADR-013) binds at the crate's admission; the queue is plain
data behind one mutex, so the deterministic tests are the evidence here.

As built, the provider cap is proven at two levels. In `nika-providers`,
real round-trips run against a transport that answers on command and counts
the posts inside it, below the gate: five calls on two slots never show
three in flight. In `nika-serve`, three fanned-out jobs, a child workflow
and a cancelled job run through the real resident backend on the `mock`
provider. Each wiring point was removed in turn (the slot in the registry,
the gate in the composed registry, the child's gate, the withdrawal at a
stop) and the matching test failed each time.

## 5. Workstream C · retry interrupted jobs

**Stopped at ADR-156 until it was accepted, on 23 November.** It decides the
attempt model, the event vocabulary of a retry, the resident's retry
authority beside ADR-019, and the budget rule across attempts. Those four
points touch contracts this work may not change without ratification, so
nothing was implemented before the acceptance.

What the ADR proposes, in short: one job, several attempts; an interrupted
attempt stays `interrupted` for ever with its own journal and receipt; a
retry is a new attempt with a new execution and trace, linked by the
existing `resumed_from`; resume reuses ADR-099 through the service driver;
the policy is opt-in (`--retry-interrupted off|resume|restart`, `off` by
default) with a per-job request bounded by the server; a workflow with
external effects needs explicit consent; the ceiling counts every attempt.

What reading the code added to it: a call in flight at the interruption
counts at the static bound of its task, because the reserved amount died
with the process; an unpriced route keeps the custody it already has; an
engine upgrade between two attempts refuses the resume, since the resident
never declares a version compatibility on its own; the judgments of a resume
are composed in the CLI today and move to one place both doors call. No
event kind, status, trace field or closed key is added: the existing
`execution.requeued` carries the re-attempt.

One C item does not depend on the ADR and ships with B: `--shutdown-grace`.

## 6. Decisions and their records

| Decision | Record | Blocks |
|---|---|---|
| `owner` is attribution inside one authority domain, not tenancy; listing, retention, schedule management, fair admission, provider cap | ADR-155, accepted | nothing: it is documented and executed |
| attempts for interrupted and failed jobs | ADR-156, accepted | all of workstream C |

## 7. Store throughput and the acceptance scenario

Figures from `job::tests::throughput` (one full job: admit, stamp, start,
settle, then one read), with the store and its codec optimized:

| Retained jobs | Snapshot | One job (4 writes) | One read |
|---|---|---|---|
| 250 | 0.33 MB | 63 ms | 3.7 ms |
| 500 | 0.67 MB | 87 ms | 7.2 ms |
| 1,000 | 1.34 MB | 140 ms | 14.5 ms |
| 2,000 | 2.68 MB | 244 ms | 29.3 ms |

Measured on one Apple Silicon laptop, APFS, 23 November 2026, on minimal
records (no inputs, no labels, no settlement payload); a real record is
larger. The cost is linear: about 37 ms of fixed synchronization for the
four writes, plus about 0.10 ms per retained job, and about 0.015 ms per
retained job for one read. Extrapolated, not measured: at 10,000 retained
jobs one job costs about 1.1 s of store time and one read about 0.15 s. The
store serves one operation at a time.

What this means for the scenario:

- 5,000 jobs an hour is one job every 720 ms. With the four writes and a
  few status reads per job, the store keeps up only while it retains fewer
  than about 3,800 jobs. At that rate this is under an hour of history.
- Minimal records are about 1.3 KB each, so the 16 MiB snapshot ceiling is
  reached near 12,000 of them, and sooner with real payloads: it can bind
  before `max_jobs` does.
- The support window (seven days at 5,000 jobs an hour is 840,000 records)
  cannot live in one snapshot, whatever the retention. Keeping only
  unsuccessful jobs for seven days fits when they are rare (a few thousand
  in the window), not otherwise.

So this delivery makes a busy resident survive (retention, typed refusals,
occupancy) and states the bound. It does not make one snapshot hold a week
of consumer traffic. ADR-119 reserves a second persistence form for "a
measured scale signal"; this measurement is that signal. An append-only job
history with an index is proposed as the next step, under its own decision,
and is not built here.

## 8. Line budget

Production lines, `nika-serve` (wall 25,000):

| Step | Estimate | Running total |
|---|---|---|
| today | | 17,520 |
| A · attribution, format 4, listing, retention, schedules | +1,150 | 18,670 |
| B · fair queue, limits, provider gate, occupancy | +800 | 19,470 |
| C · attempts, resume, policy, manual retry (after ADR-156) | +1,400 | 20,870 |

No descent is needed. New files: `job/attribution.rs`, `job/listing.rs`,
`job/retention.rs`, `job/store/list.rs`, `server/job_list.rs`,
`server/schedule_manage.rs`, `server/fair_queue.rs`,
`server/provider_gate.rs`, `server/occupancy.rs`. The OpenAPI builder lives
under `#[cfg(test)]` and does not count. Other crates move by tens of lines:
`nika-cadence` (schedule attribution), `nika-providers`, `nika-runtime`,
`nika-service-execution` (the provider gate), `nika-cli-host` (flags). The
kernel is not touched.

## 9. Commits

Small commits, each green on its own (`cargo fmt`, clippy on the touched
crates, the crate's tests):

1. A1 job attribution and store format 4.
2. A2 `owner` and `labels` on `POST /v1/jobs`.
3. A3 `GET /v1/jobs`.
4. A4 retention.
5. A5 schedule attribution, list, delete, trigger.
6. A6 ADR-155, documentation, OpenAPI text, changelog fragments.
7. B1 fair queue, per-owner limits, limit flags, occupancy (`65b7027a0`).
8. Review fixes on A: the migration written under the lease, a forgotten
   queue entry, a slot keeps its job (`b9e73992e`).
9. B2 provider gate, its flag, `providers` in the occupancy (`30f3b3404`).
10. B3 ADR-155 and this plan brought to what is built (`3318b5f24`).
11. C0 ADR-156, then stop (`a770b2a3c`).
12. The SDK follow-up plan and the roadmap line (`887846ebb`).
13. Review fixes on B: an approved cost review survives a queue refusal, a
    job a worker holds is not queued again, `--max-concurrent-jobs` stops
    at 256 (`0f2eca070`).
14. `url` moved to its patched release (`1d70c3b86`). It is independent of
    the workstreams: an advisory published on 20 November had turned two
    rows of the hygiene sweep red on `main`.
15. The report, `2026-11-23-serve-production-jobs-report.md`.

One pull request per workstream is the natural split for review; they stack
on this branch in that order.

## 10. Gates

Per commit: `cargo fmt --all -- --check`, clippy on the touched crates,
`cargo test -p <crate> --lib --locked`. Before the branch is called ready:
`bash scripts/hygiene/check-all.sh`, `bash scripts/ci/check-layering.sh`,
`bash scripts/ci/check-crate-size.sh`, `bash scripts/ci/check-loc-limits.sh`,
`bash scripts/release/changelog-assemble.sh --check`,
`bash scripts/adr/validate.sh`, the OpenAPI snapshot test, and the
integration tests of `nika-cli` that drive the real binary over HTTP. Tests
use the `mock` provider, no network, no keychain, and the injected clock.

## 11. Out of scope

Several residents on one state root; any new `.nika` or `nika.yaml` key; the
TypeScript SDK (its follow-up is `2026-11-23-sdk-followup.md`, and the
committed OpenAPI is updated here); per-task events over HTTP; a per-run
model or budget choice over HTTP; a content cache.
