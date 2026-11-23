# `nika serve` ready for production jobs · report

> Written on 23 November 2026 for the branch `feat/serve-production-jobs`,
> branched from `main` at `8b640a9b7` (0.124.5). Sixteen commits before this report, not pushed.
> The plan is `2026-11-23-serve-production-jobs.md`; this report says what was
> built, what was proved, and what waits for a decision.

## 1. Where the three workstreams stand

| Workstream | State | Record |
|---|---|---|
| A · attribute, list and retain jobs; manage schedules | built, tested, reviewed | ADR-155 (proposed) |
| B · share the workers fairly; cap the calls per provider | built, tested, reviewed | ADR-155 (proposed) |
| C · re-attempt interrupted jobs | **stopped at its ADR, as the protocol requires** | ADR-156 (proposed) |

Workstream C touches the run and proof vocabulary, retry ownership and a
closed DTO. No line of it is written. One item of C needed no decision and
shipped with B: `--shutdown-grace`.

## 2. The acceptance scenario

| Step | Holds today | Evidence, or what is missing |
|---|---|---|
| A backend submits a job with `owner: "user_123"` and `labels` under the server token | yes | `POST /v1/jobs` (by name), capability `jobAttribution`; both are bound into the idempotency digest and the receipt |
| A user who submits 50 jobs does not delay the others | yes | owner `a` queues 50, `b` and `c` submit one each and start at the next free workers (`a_burst_from_one_owner_does_not_delay_another`) |
| Never more than N calls in flight on a provider | yes, for the model calls of `infer` and `agent` tasks | three fanned-out jobs on a cap of 2: 2 in flight, 10 waiting, peak 2 (`three_fanned_out_jobs_share_the_provider_slots`) |
| 5,000 jobs in an hour | only with a short retention | the store rewrites one snapshot per operation; see section 6 |
| A deploy restarts the resident during a long job, and the job resumes | **no** | the job becomes `interrupted` and is not retried; ADR-156 proposes the re-attempt |
| Support lists `user_123`'s failed jobs of the last seven days | yes, within what retention keeps | `GET /v1/jobs?owner=user_123&status=failed&created_after=…`, with `--job-retention-unsuccessful 7d` |
| Support retries one of them | **no** | ADR-156; until then the caller submits the work again under a new idempotency key |

## 3. What was built

### A · jobs an application can find again

- **Store format 4.** A job keeps its owner, its labels, the instant it was
  admitted and its admission rank. A format 3 store is migrated at open,
  under the store's lease, and tested from fixtures.
- **`owner` and `labels`** on the by-name form of `POST /v1/jobs`. They are
  attribution inside one authority domain, never access control: whoever
  holds the token reads every job.
- **`GET /v1/jobs`**: filters on owner, status, workflow, origin, labels and
  admission instants; pages with an opaque cursor, newest first, stable
  while jobs are admitted.
- **Retention**: `--job-retention` and `--job-retention-unsuccessful` forget
  settled jobs after a window. Without them a full store refuses every
  admission, as before.
- **Schedules over HTTP**: owner and labels on a schedule, `GET
  /v1/schedules`, `DELETE /v1/schedules/{id}`, `POST
  /v1/schedules/{id}/trigger`. Capability `scheduleManage`.

### B · a resident that many users share

- **A fair queue.** One lane per owner, admission order inside a lane. A
  free worker goes to the owner with the fewest jobs running, in turn among
  equals. Jobs that state no owner share one lane.
- **Limits as flags**: `--max-concurrent-jobs` (at most 256),
  `--queue-capacity`, `--max-jobs`, `--max-queued-per-owner`,
  `--max-concurrent-per-owner`, `--shutdown-grace`. Without a flag nothing
  changes.
- **`429 owner_queue_full`** with `Retry-After` at an owner's bound. Nothing
  is written and the idempotency key stays free.
- **`--provider-concurrency PROVIDER=N`**: a slot is taken around each
  provider round-trip, across every job, child workflows included. The gate
  owns no retry (ADR-019): a rejected call returns its slot before the verb
  backs off. A `for_each` fans out as before and its calls go as slots free.
  A cancelled job leaves the line.
- **`GET /v1/occupancy`**: jobs running and waiting, the limits, the busiest
  owners, and per capped provider the calls in flight and waiting.
  Capability `occupancy`.

### Where the provider gate sits

The trait is `nika_providers::ProviderCallGate`, beside the cost admission
that already lives there. It is not a kernel trait: it has one caller (the
provider round-trip) and one implementor (a host that already depends on
`nika-providers`), and a kernel trait would add a sealed contract with no
second consumer. The kernel is not touched.

## 4. Decisions that wait

1. **ADR-155** is `proposed`. It is documented and executed; accepting it
   closes A and B.
2. **ADR-156** is `proposed` and blocks all of C. Its notes list eight
   points to rule on. The three that shape everything else:
   - one job with numbered attempts, rather than a new job per retry;
   - the resident re-attempts for lost ownership only, never for a failure;
   - no status, event kind, trace field or closed key is added: a job that
     will be re-attempted goes back to `queued` and the existing
     `execution.requeued` says so.
3. **Three behaviours this branch changed** and that deserve a look:
   - jobs recovered at a restart now count against `--queue-capacity`.
     A resident that restarts with a full queue refuses new admissions
     until the backlog drains. Before, a restart could hold twice the
     capacity for a while;
   - a full queue no longer spends an approved cost review;
   - `--max-concurrent-jobs` refuses a value above 256.
4. **The roadmap line.** `ROADMAP.md` now names this work for 0.125 beside
   native video. The other public pages that repeat "native video is planned
   for 0.125" were left as they are.
5. **The dependency fix** (section 8) rides this branch as one commit. It
   touches nothing of the serve work and can go to `main` on its own.

## 5. Evidence

### Gates on `9eaa908fb`, the last commit that changes code or a lockfile

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | pass |
| clippy `-D warnings --all-targets` on the eight crates the branch changes | pass |
| `scripts/ci/check-layering.sh` | pass |
| `scripts/ci/check-crate-size.sh`, `scripts/ci/check-loc-limits.sh` | pass |
| `scripts/release/changelog-assemble.sh --check` | pass, 10 fragments |
| `scripts/adr/validate.sh` | 116 ADRs, 0 errors |
| OpenAPI snapshot and published-SDK contract tests | pass, inside the `nika-serve` tests |
| `cargo audit` and `cargo deny check` on the three lockfiles | pass |
| `scripts/hygiene/check-all.sh` | 41 green, 7 yellow, 0 red; the run left the tree unchanged |

### Tests

| Target | Result |
|---|---|
| `nika-serve` lib | 519 passed, 4 ignored (434 before the branch) |
| `nika-providers` lib | 569 passed |
| `nika-runtime` lib | 511 passed |
| `nika-service-execution` lib | 152 passed, 1 ignored |
| `nika-cli-host` lib | 494 passed |
| `nika-cli` lib, `serve` | 35 passed |
| `nika-cli` integration `serve`, the real binary over HTTP | 19 passed |
| `nika-cadence` lib | 250 passed |
| `nika-error` lib | 81 passed |

All of them use the `mock` provider or a test transport, no network, no
keychain, and controlled release instead of sleeps where order matters.

The seven yellow rows of the sweep are warnings this branch did not add:
a stale proposed ADR (ADR-036), the Gate 5 and mutation-proof ratchets, one
guard owed at a future admission, a memory-file check, the crates between
15,000 and 25,000 lines and the files between 1,500 and 2,500. No crate and
no file entered either window through this work: `server/mod.rs` was
already in it.

### Each wiring point removed in turn

A test that passes with the code removed proves nothing, so each of these
was removed, the named test was seen to fail, and the code was restored:

| Removed | Test that failed |
|---|---|
| the child run inherits the gate | `a_child_run_takes_its_slot_from_the_same_gate` |
| the composed registry receives the gate | the three resident gate tests |
| a stop withdraws a waiting call | `a_stopped_run_leaves_the_line`, `a_cancelled_job_leaves_the_line_and_the_others_finish` |
| the reported call takes a slot | three tests of `nika-providers` |
| the queue place is taken before the cost review | `an_owner_at_its_queue_bound_keeps_its_approved_review` |
| the queue remembers the jobs a worker holds | `a_job_a_worker_holds_is_not_queued_again` |

### Two independent reviews

Each was a read-only review by an agent that had not written the code.

- **Of A.** Three defects, fixed in `85c005729`: the migration was written
  outside the store's lease; a cancelled queued job kept its place; a
  scheduled slot whose workflow changed after it fired stopped the resident.
- **Of B.** One contract defect and four smaller ones, fixed in
  `95e11b1f0`: an approved cost review was spent by a queue refusal that asks
  for a retry; a replay arriving while a worker held the job could queue it
  twice or be refused; two tests asserted less than they claimed;
  `--max-concurrent-jobs` had no upper bound. The review also asked whether
  counting recovered jobs against the capacity was intended: it is, and it is
  now written down (section 4).

## 6. Known limits of what was built

- **The store holds one snapshot.** Every job operation rewrites it. Measured
  on one laptop: one job costs about 37 ms of fixed synchronization plus
  about 0.10 ms per retained job. At 5,000 jobs an hour the store keeps up
  only while it retains fewer than about 3,800 jobs, which is under an hour
  of history at that rate. Seven days of that traffic cannot live in one
  snapshot. Keeping only the unsuccessful jobs for seven days fits when they
  are rare. ADR-119 reserves a second persistence form for a measured scale
  signal; this measurement is that signal. An append-only job history is the
  proposed next step, under its own decision. It is not built.
- **An interrupted job is not retried.** Unchanged from 0.124.5.
- **The provider cap covers the model calls of `infer` and `agent` tasks.**
  Calls an agent harness makes through its own client, the image and speech
  tools, and the authoring door are not counted.
- **The wait for a slot counts inside a task's `timeout:`.** A cap far under
  the demand shows as task timeouts, with nothing sent.
- **A stopped job leaves the provider line within 25 ms**, by polling its
  stop flag. Operator cancellation is cooperative in the runtime and has no
  waker; the reserved cancel member of the inference request was left alone.
- **`Retry-After` is a fixed 5 seconds.** The resident cannot know when one
  of the owner's jobs will start.
- **The limits are one process's memory.** Several residents on one state
  root stay out of scope.
- **Loom was not applied to the fair queue.** The repository uses it for the
  lock-free cancel token. The queue is plain data behind one mutex with one
  reader; its deterministic tests are the evidence.
- **No Prometheus exporter.** The occupancy route was chosen; an exporter
  was optional and is not built.

## 7. What lives outside this repository

- **The known-limits page** is published from the documentation
  repository. It needs four entries: the single-snapshot store and its
  measured bound; interrupted jobs are not retried; what the provider cap
  does not count; the owner is attribution, not access control.
- **The TypeScript SDK** was out of scope. `2026-11-23-sdk-followup.md`
  lists what it needs. The published SDK keeps working against this engine:
  no closed object gained a key and no protocol clock moved.

## 8. The advisory that had turned the sweep red

RUSTSEC-2026-0592 was published on 20 November against `url` 2.5.8:
reading the origin of a deeply nested `blob:` URL overflows the stack. It
turned two rows of the sweep red (`cargo-audit-rustsec` and
`cargo-deny-policy`) on `main` as on this branch, which had changed no
manifest and no lockfile.

`9eaa908fb` moves the three lockfiles (the root, `fuzz/` and
`crates/nika-acp/`) to `url` 2.5.9 and the `form_urlencoded` 1.2.3 it
requires, the way the repository's earlier dependency fix did: lockfiles
only, the new releases read before the bump, one `security` fragment.

- The engine was not found exposed. It reads the origin of `http` and
  `https` URLs only: the crawl of `nika:fetch` compares links its extractor
  has already narrowed to those schemes, and the other readers take a vetted
  or operator-set endpoint.
- 2.5.9 brings one change a workflow can observe: a `^` in the path of a
  fetched URL is now sent as `%5E`.
- The lib tests of the crates that parse URLs pass on the new lockfile
  (`nika-http`, `nika-check`, `nika-check-analyzer`, `nika-extract`,
  `nika-builtin`, `nika-compile`, `nika-providers-access`). Those of
  `nika-dap`, `nika-browser` and `nika-cli-host-typesafe` were not run; they
  parse a URL and read its host.

## 9. Sizes

| Crate | Production lines | Wall |
|---|---|---|
| `nika-serve` | 20,303 (17,520 before) | 25,000 |
| `nika-providers` | 15,991 | 25,000 |
| `nika-runtime` | 16,846 | 25,000 |
| `nika-cli-host` | 16,928 | 25,000 |
| `nika-cli` | 17,225 | 25,000 |
| `nika-service-execution` | 5,866 | 25,000 |
| `nika-cadence` | 8,260 | 25,000 |

`check-crate-size.sh` and `check-loc-limits.sh` pass. `server/mod.rs` is at
1,579 lines; it was at 1,555 before this work, above the 1,500 warning and
under the 2,500 wall. The plan estimated `nika-serve` at 19,470 after B; it
is 833 lines above that. C was estimated at 1,400 more, which still fits.

## 10. Commits

| Commit | What |
|---|---|
| `b41df2799` | measure what one job costs the store as it grows |
| `11fa47cf7` | the plan |
| `0584fb445` | A1 · attribution, admission instant and rank in store format 4 |
| `5ed6aece2` | A2 · `owner` and `labels` on the by-name job form |
| `1f300cdad` | A3 · `GET /v1/jobs` |
| `c41982f4b` | A4 · retention |
| `cc846d3ae` | A5 · list, remove and trigger schedules |
| `93d2d445e` | A6 · ADR-155 |
| `31285cca9` | B1 · fair queue, per-owner limits, limit flags, occupancy |
| `85c005729` | review fixes on A |
| `46d711ea1` | B2 · the provider gate |
| `a1963ab1c` | B3 · ADR-155 and the plan brought to what is built |
| `64ca2b1d6` | C0 · ADR-156, proposed |
| `a59e8d25e` | the SDK follow-up plan and the roadmap line |
| `95e11b1f0` | review fixes on B |
| `9eaa908fb` | `url` moved to its patched release |

One pull request per workstream is the natural split: A (`0584fb445` to
`93d2d445e`, with `85c005729`), then B (`31285cca9`, `46d711ea1`,
`a1963ab1c`, `95e11b1f0`), the documents last. The dependency fix is
independent of all of them.

## 11. Next candidates, not designed

An append-only job history with an index; events per task over HTTP; a model
or a budget chosen per run over HTTP; a content cache; an exporter for the
occupancy figures.
