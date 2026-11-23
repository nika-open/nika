# `nika serve` ready for production jobs · report

> Written on 23 November 2026 for the branch `feat/serve-production-jobs`,
> branched from `main` at `8b640a9b7` (0.124.5). Sixteen commits before this report, not pushed.
> The plan is `2026-11-23-serve-production-jobs.md`; this report says what was
> built, what was proved, and what waits for a decision.
>
> Update, same day: the decider accepted ADR-155 and ADR-156 as written.
> Sections 1 and 4 say so. Workstream C starts on a branch stacked on this
> one.
>
> Update, 24 November: workstream C is built on `feat/serve-job-attempts`,
> stacked on this branch. Sections 1 to 7, 9 and 10 now cover it.
> A and B are pull request #1896, C is #1898, stacked on it. Neither is
> merged.

## 1. Where the three workstreams stand

| Workstream | State | Record |
|---|---|---|
| A · attribute, list and retain jobs; manage schedules | built, tested, reviewed | ADR-155 (accepted) |
| B · share the workers fairly; cap the calls per provider | built, tested, reviewed | ADR-155 (accepted) |
| C · re-attempt interrupted jobs | built and tested on the stacked branch `feat/serve-job-attempts`; review in section 5 | ADR-156 (accepted) |

Workstream C touches the run and proof vocabulary, retry ownership and a
closed DTO. No line of it was written before ADR-156 was accepted. One item
of C needed no decision and shipped with B: `--shutdown-grace`. The rest is
eight commits on the stacked branch (section 10).

## 2. The acceptance scenario

| Step | Holds today | Evidence, or what is missing |
|---|---|---|
| A backend submits a job with `owner: "user_123"` and `labels` under the server token | yes | `POST /v1/jobs` (by name), capability `jobAttribution`; both are bound into the idempotency digest and the receipt |
| A user who submits 50 jobs does not delay the others | yes | owner `a` queues 50, `b` and `c` submit one each and start at the next free workers (`a_burst_from_one_owner_does_not_delay_another`) |
| Never more than N calls in flight on a provider | yes, for the model calls of `infer` and `agent` tasks | three fanned-out jobs on a cap of 2: 2 in flight, 10 waiting, peak 2 (`three_fanned_out_jobs_share_the_provider_slots`) |
| 5,000 jobs in an hour | only with a short retention | the store rewrites one snapshot per operation; see section 6 |
| A deploy restarts the resident during a long job, and the job resumes | yes, when the job states `retry: {on_interrupted: resume}` or the resident runs with `--retry-interrupted resume` | a resident process killed by SIGKILL once the first task of a two-task workflow is journaled; the next resident serves that task from the journal and runs the rest, one job over two attempts (`a_resident_killed_in_the_middle_of_a_workflow_is_resumed_by_the_next_one`). The lost attempt's spend is read from its journal and counts against the job's one ceiling. A workflow whose effects can repeat is started again only with the job's own `retry` statement, its consent |
| Support lists `user_123`'s failed jobs of the last seven days | yes, within what retention keeps | `GET /v1/jobs?owner=user_123&status=failed,interrupted&created_after=…`, with `--job-retention-unsuccessful 7d`; each summary now says how many `attempts` the job took and its `retry_state` |
| Support retries one of them | yes | `POST /v1/jobs/{id}/retry` with `{"attempt": N, "mode": "restart"\|"resume"}`: 202 and a new attempt of the same job; the same request again answers 200 and writes nothing (`a_failed_job_is_started_again_by_request_as_a_new_attempt_of_the_same_job`). `GET /v1/jobs/{id}/attempts` shows every attempt |

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

### C · a job that outlives its resident

- **One job, numbered attempts.** A re-attempt keeps the job's id and gets
  its own execution and trace identity. The attempt it replaces stays as
  evidence: its identity, receipt, spend and how it ended.
- **Opt-in.** `--retry-interrupted off|restart|resume` (default `off`, the
  0.124.5 behaviour) and `--retry-max-attempts N` (default 3, the first
  included). A job may state `retry: {on_interrupted, max_attempts}`; it is
  bound into the request digest and can lower the bound, never raise it.
- **Effects need consent.** On its own the resident starts again only a
  workflow it classes as contained, read from the captured workflow and the
  word kept at admission, the stricter of the two. A job's own `retry`
  statement is the consent for anything else.
- **One ceiling for every attempt.** Each lost attempt's spend is read from
  its journal. A spend that cannot be read never counts for zero: the job
  stays `interrupted (unknown_spend)` unless a budget is stated, with a
  ceiling or without one. A model call in flight in a run that rode an agent
  application is such a spend: that lane bills a subscription, not a price.
- **Resume through the judgment `nika run --resume` uses** (ADR-099): the
  completed tasks are served from the journal. A journal whose chain broke,
  or that changed after the decision, fails the attempt with
  `resume_refused`; the resident never falls back to `restart`.
- **No new status, event kind, trace field or closed key.** A job going
  again returns to `queued` and `execution.requeued` says why
  (`attempt 2 of 3 (resume): attempt 1 lost its owner`). A job that will not
  go again ends `interrupted` with its reason as the event's code.
- **By request**: `POST /v1/jobs/{id}/retry` for a job that ended
  `interrupted` or `failed`; `GET /v1/jobs/{id}/attempts`; `GET
  /v1/jobs/{id}/trace/verify?attempt=N`; `attempts` and `retry_state` on
  each list summary. Capability `jobRetry`.
- **A cancellation is durable**: a cancelled job is never started again.

### Where the provider gate sits

The trait is `nika_providers::ProviderCallGate`, beside the cost admission
that already lives there. It is not a kernel trait: it has one caller (the
provider round-trip) and one implementor (a host that already depends on
`nika-providers`), and a kernel trait would add a sealed contract with no
second consumer. The kernel is not touched.

## 4. Decisions

1. **ADR-155** was accepted on 23 November. It is documented and executed.
2. **ADR-156** was accepted on 23 November as written, with the eight
   points its notes list. The three that shape everything else:
   - one job with numbered attempts, rather than a new job per retry;
   - the resident re-attempts for lost ownership only, never for a failure;
   - no status, event kind, trace field or closed key is added: a job that
     will be re-attempted goes back to `queued` and the existing
     `execution.requeued` says so.
3. **Three behaviours this branch changed** and that still deserve a look:
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
6. **Two places where C reads ADR-156 more narrowly than its text.** Both
   are written in the ADR's notes, the spec and the operations guide:
   - section 3 says a job whose journal this engine cannot resume stays
     `interrupted (resume_refused)`. That holds when the resident decides at
     its start. When a stop past its grace has already queued the attempt,
     the refusal comes when a worker claims it, and the job ends
     `failed (resume_refused)` (note 12). Making it `interrupted` there is a
     small change in the claim path, if the decider wants it;
   - section 4 allows a `retry` member on a schedule declared over HTTP.
     Note 5 says the resident never starts again a job a schedule fired:
     the schedule's catch-up owns a lost fire. Such a member would change
     nothing, so it was not added. It waits for the project-schedule
     decision the ADR names as a follow-up.
7. **Unknown spend now refuses with no ceiling as well.** Decision 5 of
   ADR-156 reads "unknown cost refuses unless an explicit budget is stated".
   The first build refused it under a ceiling only, which a review caught;
   the code now follows the decision as written (note 25). It matters only
   to a resident run with no ceiling at all.
8. **`serde_json` gains `float_roundtrip` in `nika-serve`**, so a ceiling or
   a spend in a hashed record reads back to the bits it was written with and
   its digest recomputes. Cargo unifies features: every binary that links
   `nika-serve`, the CLI included, now parses floats with exact round-trip,
   which is slower on float-heavy JSON. No test measured that cost.

## 5. Evidence

### Gates on `1d70c3b86`, the last commit that changes code or a lockfile

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

- **Of A.** Three defects, fixed in `b9e73992e`: the migration was written
  outside the store's lease; a cancelled queued job kept its place; a
  scheduled slot whose workflow changed after it fired stopped the resident.
- **Of B.** One contract defect and four smaller ones, fixed in
  `0f2eca070`: an approved cost review was spent by a queue refusal that asks
  for a retry; a replay arriving while a worker held the job could queue it
  twice or be refused; two tests asserted less than they claimed;
  `--max-concurrent-jobs` had no upper bound. The review also asked whether
  counting recovered jobs against the capacity was intended: it is, and it is
  now written down (section 4).

### Workstream C, on `feat/serve-job-attempts`

Gates on `439145eac`, the commit that closes the reviews:

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | pass |
| clippy `-D warnings --all-targets` on `nika-serve` and `nika-dap` | pass |
| `scripts/ci/check-crate-size.sh`, `scripts/ci/check-loc-limits.sh` | pass |
| OpenAPI snapshot and published-SDK contract tests | pass, inside the `nika-serve` tests |
| `scripts/hygiene/check-all.sh`, on `4758d72b8` before the review fixes | 0 red, yellow rows only (exit 1); the run left the tree unchanged |
| the repository's pre-push gate (workspace tests, clippy, sweep, ratchets), on `0fba9e759` | refused: 14,677 tests passed, the sweep had yellow rows only, and the `fn-length` ratchet found `serve_control` at 101 lines |
| the same gate on `3a17f3805`, which makes that function shorter | pass: 14,677 tests passed, 25 skipped; the sweep had yellow rows only; every ratchet held. The gate runs the library tests; CI runs the integration files |

| Target | Result |
|---|---|
| `nika-serve` and `nika-dap`, every test under nextest | 1,044 passed, 5 skipped (577 of `nika-serve`, 467 of `nika-dap`) |

The CI of #1898 caught one defect the local gates had not run: the
`public-api.txt` of `nika-cli-host` lacked the two retry flags. It is fixed
in `572368d19`. The review fixes change no public item.

`verbs::run::tests::answer_without_resume_preseeds_the_gate`, a test of
`nika-cli`, failed once in four runs of `cargo test -p nika-cli --lib`, with
exit 3, and passes alone and under nextest, which runs each test in its own
process. It runs without the lease on the current directory that other tests
of that module take, and reads the current directory as the base does: the
same reads stand on `a9db05bb2`. It was not run on the base, so whether it
fails there too is not known.

Each fix of the reviews was removed in turn, its test seen to fail, and the
file restored byte for byte:

| Removed | Test that failed |
|---|---|
| the record's origin on a queued task | `a_schedule_fired_job_restarted_by_request_settles_under_its_own_origin` |
| a replay puts a queued job back | `a_replayed_retry_puts_back_a_job_its_first_request_left_out_of_the_queue` |
| the bound in the retry state | `a_request_is_held_to_the_bound_and_the_ceiling_of_a_lost_run` |
| a replay after a later attempt | `a_failed_job_is_started_again_by_request_as_a_new_attempt_of_the_same_job` |
| amounts compared as amounts | the same test |
| the journal judged before the cost | `a_journal_that_cannot_be_resumed_refuses_before_its_cost_is_counted`, `a_broken_chain_leaves_the_job_interrupted_rather_than_run_it_from_the_start` |
| unknown spend refused with no ceiling | three tests of the ceiling, the store and the claim |
| a model call in flight on an agent application | `a_call_in_flight_through_an_agent_application_is_unknown_spend` |
| a tool in flight bills no route | the same test and the SIGKILL test |

Before them, C6 removed the resume plan from the backend (the SIGKILL test
failed) and treated every workflow as contained (the effects test failed).
The two journal verifications of a lost attempt now assert their exact
verdict: `incomplete` with a dead writer after a SIGKILL, `ok` with an intact
chain after a stop past the grace, whose run was cancelled and closed its
journal.

Two reviews of C, read-only, by agents that had not written the code: one
of C1 to C3, one of C4 to C6. Fixed in `439145eac`:

- a request restarting a job a schedule fired queued a task of manual
  origin, so its receipt never matched and its result could not be
  recorded;
- a request ending between the store's write and its queue entry left the
  job queued with no place, and its replay did not put it back;
- a replay was no longer recognised after a later attempt, and `1` and
  `1.0` were two budgets;
- `retry_state` ignored the bound;
- the automatic decision counted the cost before it judged the journal, so
  a broken journal said `unknown_spend`;
- unknown spend was refused only under a ceiling (section 4, item 7);
- a model call in flight through an agent application was priced from the
  catalog;
- a busy store read as a job with no captured world;
- a cancellation that met a stop between its read and its durable mark was
  lost.

The last two have no test that injects the race. What the reviews found and
this work does not close is in section 6 and in ADR-156 notes 33 to 36.

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
- **The resident starts a job again only when its run lost its owner.** A
  run that failed is started again only by request.
- **A journal read after its first record reads as a run that sent
  nothing.** A run killed between sending a provider call and journaling it
  shows less spend than it made. The ADR names this limit.
- **A zero ceiling** makes every re-attempt `budget_exhausted`.
- **A job admitted through a cost review states no `retry`** and is not
  classed: a lost one stays `interrupted (effects_need_consent)`. A request
  can still start it again.
- **`nika run --resume` of a killed CLI run regains its whole ceiling.** The
  CLI keeps no attempt ledger; the resident counts every attempt of a job.
- **A resumed task is served from the journal** even when a file it read has
  changed since. That is ADR-099's rule, unchanged here.
- **An attempt that never started** (refused at readmission) cannot be
  started again by request.
- **Of two different requests racing for one attempt**, the first written
  wins and the other is `409 job_not_retryable`.
- **Artifacts are served for the latest attempt only.**
- **A job a format 3 store left running** is judged by the same rules (read
  as having effects, since that format kept no class). No frozen format 3
  store holds a running job, so this case is not proved from a fixture.
- **The `429 owner_queue_full` of the retry route** is the admission's own
  refusal, tested on `POST /v1/jobs`; no test drives it through the retry
  route.
- **A worker can outlive its attempt.** A run interrupted while its resident
  lives (the execution timeout, a stop) is told to stop and dropped; its
  blocking work may still run when a request starts the next attempt. Not
  reproduced. The two attempts never share an identity or a journal.
- **The retry request writes through the store's control lane**, as a
  cancellation does: that lane is bounded and its send blocks a runtime
  thread while it is full.
- **The journal names its routes for the whole run.** When any model of a
  run rode an agent application, every model call it had in flight counts
  as unknown spend, a plain one included. It can only overstate.
- **The warnings of the resume judgment are not shown on the job**, only
  its verdict.
- **The bound of a crash loop is proved for stops past their grace**,
  where the stopping resident decides. A resident killed outright leaves the
  decision to the next start: one such kill is proved (the SIGKILL test), and
  no test kills three resident processes in a row to reach the bound that
  way.
- **The two races fixed without a test that injects them**: a cancellation
  that meets a stop between its read and its durable mark, and a store too
  busy to read the captured world. Section 5 says which fixes have a
  witness.
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
  measured bound; what the resident starts again on its own and what needs
  a request (section 6); what the provider cap does not count; the owner is
  attribution, not access control.
- **The TypeScript SDK** was out of scope. `2026-11-23-sdk-followup.md`
  lists what it needs, `retryJob` and the attempts reader included. The
  published SDK keeps working against this engine: no closed object gained a
  key and no protocol clock moved. One change it will meet: a job's
  `execution_id` and `trace_id` change when it is started again.

## 8. The advisory that had turned the sweep red

RUSTSEC-2026-0592 was published on 20 November against `url` 2.5.8:
reading the origin of a deeply nested `blob:` URL overflows the stack. It
turned two rows of the sweep red (`cargo-audit-rustsec` and
`cargo-deny-policy`) on `main` as on this branch, which had changed no
manifest and no lockfile.

`1d70c3b86` moves the three lockfiles (the root, `fuzz/` and
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
| `nika-serve` | 22,648 after C (20,303 after B, 17,520 before) | 25,000 |
| `nika-dap` | 18,367 after C | 25,000 |
| `nika-providers` | 15,991 | 25,000 |
| `nika-runtime` | 16,846 | 25,000 |
| `nika-cli-host` | 16,928 | 25,000 |
| `nika-cli` | 17,225 | 25,000 |
| `nika-service-execution` | 5,866 | 25,000 |
| `nika-cadence` | 8,260 | 25,000 |

`check-crate-size.sh` and `check-loc-limits.sh` pass. The figures after C
were measured before the review fixes, which add 416 lines net under
`crates/`, 246 of them in tests. `server/mod.rs` is at 1,681 lines after
them; it was at 1,555 before this work, above the 1,500 warning and under
the 2,500 wall. The plan estimated `nika-serve` at 19,470 after B and C at
1,400 more; before the review fixes `nika-serve` stood 2,345 lines above its
size after B, most of it the retry route, the attempts read and the judgment of a lost run. The
other crates' figures before the branch were not measured.

## 10. Commits

| Commit | What |
|---|---|
| `b41df2799` | measure what one job costs the store as it grows |
| `a4f699901` | the plan |
| `17c9ddebf` | A1 · attribution, admission instant and rank in store format 4 |
| `699fa2a91` | A2 · `owner` and `labels` on the by-name job form |
| `4d8edcd68` | A3 · `GET /v1/jobs` |
| `4d96f57f9` | A4 · retention |
| `783a61c09` | A5 · list, remove and trigger schedules |
| `6e9859c45` | A6 · ADR-155 |
| `65b7027a0` | B1 · fair queue, per-owner limits, limit flags, occupancy |
| `b9e73992e` | review fixes on A |
| `30f3b3404` | B2 · the provider gate |
| `3318b5f24` | B3 · ADR-155 and the plan brought to what is built |
| `a770b2a3c` | C0 · ADR-156, proposed |
| `887846ebb` | the SDK follow-up plan and the roadmap line |
| `0f2eca070` | review fixes on B |
| `1d70c3b86` | `url` moved to its patched release |

One pull request per workstream is the natural split: A (`17c9ddebf` to
`6e9859c45`, with `b9e73992e`), then B (`65b7027a0`, `30f3b3404`,
`3318b5f24`, `0f2eca070`), the documents last. The dependency fix is
independent of all of them.

Workstream C, on `feat/serve-job-attempts`:

| Commit | What |
|---|---|
| `5b443e7e9` | C1 · a job whose run lost its owner is started again: attempts, the policy, consent, one ceiling |
| `693010a5b` | C2 · one resume judgment for `nika run --resume` and the resident |
| `bdf064c38` | C3 · resume an interrupted job from its journal |
| `3f58c3fca` | C4 · `POST /v1/jobs/{id}/retry` |
| `1a4c12db2` | C5 · `GET /v1/jobs/{id}/attempts`, the list members, `trace/verify?attempt=N` |
| `4758d72b8` | C6 · the fault tests |
| `572368d19` | the retry flags in the public API of `nika-cli-host` |
| `439145eac` | what the two reviews of C found |
| `0fba9e759` | this report on C, and the SDK plan for `jobRetry` |
| `3a17f3805` | the control lane answers a persisted write in one place (`fn-length`) |

## 11. Next candidates, not designed

An append-only job history with an index; events per task over HTTP; a model
or a budget chosen per run over HTTP; a content cache; an exporter for the
occupancy figures; a `retry` statement on project schedules, which needs a
`nika.yaml` decision; the artifacts of earlier attempts over HTTP.
