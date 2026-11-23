# TypeScript SDK follow-up to the production jobs work · plan

> Status: not started. The engine work of
> `2026-11-23-serve-production-jobs.md` left the TypeScript SDK out of scope.
> This plan lists what the SDK needs to reach what the resident now serves.
> It is written from the engine's committed contract
> (`crates/nika-serve/openapi.json`), not from the SDK's source.

## 1. Where the SDK stands against this engine

The latest published SDK (`@supernovae-st/nika@0.124.0`) keeps working
unchanged. The engine added no key and no value to the objects that SDK
closes (`Job`, `JobEvent`, `JobReceipt`, `JobStatus`), and no protocol clock
moved. The engine proves it in `server/openapi/published_sdk_tests.rs`.

What that SDK cannot do yet: state an owner, list jobs, read the load,
manage schedules, or read the typed refusal of an owner's full queue.

## 2. Capability detection first

Every new method checks `supportedCapabilities` of `GET /health` before it
sends anything, and refuses locally against an engine that lacks the token.
The refusal is a typed error that names the missing capability and the
engine version. No method may discover a missing route from a 404.

| Capability | What it promises |
|---|---|
| `jobAttribution` | `owner` and `labels` on the by-name form of `POST /v1/jobs` |
| `jobList` | `GET /v1/jobs` |
| `occupancy` | `GET /v1/occupancy`, and `429 owner_queue_full` at an owner's bound |
| `scheduleManage` | `GET /v1/schedules`, `DELETE /v1/schedules/{id}`, `POST /v1/schedules/{id}/trigger`, `owner` and `labels` on a schedule body |

`scheduleManage` appears only beside `schedule`, on a resident whose
scheduler is live.

One trap: the by-name job body is a closed object. An engine without
`jobAttribution` refuses a body that carries `owner`. The SDK must therefore
never send `owner` or `labels` without the capability, even empty.

## 3. Methods

### 3.1 Submitting with an owner

The SDK's job submission accepts `owner` and `labels`.

- `owner`: 1 to 128 bytes of `[A-Za-z0-9._:@-]`.
- `labels`: at most 16 pairs, a key of at most 64 bytes and a value of at
  most 256. Validate locally with these bounds and let the engine's own
  refusal through unchanged when they differ.
- Both are bound into the idempotency digest: the same key with another
  owner is a conflict, which the SDK already reports.
- The owner is attribution, never access control. The SDK documentation
  says so in the same words as the engine: whoever holds the token reads
  every job.

### 3.2 `listJobs`

`GET /v1/jobs` with `owner`, `status` (several), `workflow`, `origin`,
`label` (several `key:value`), `created_after`, `created_before`, `limit`.
The SDK names these as it names its other fields.

- Returns one page: `jobs` and `next_cursor`.
- The cursor is opaque. The SDK sends it back unchanged with the same
  filters and never builds or parses one.
- An async iterator over pages is the convenient form; a single-page call
  stays available.
- `JobSummary` is an open object: unknown keys ride through.
- An instant with a `+` offset must be percent-encoded. Build the query
  with `URLSearchParams`, never by hand.

### 3.3 `occupancy`

`GET /v1/occupancy` returns `jobs`, `owners`, `owners_omitted` and
`providers`. All open objects. `providers` is empty on a resident with no
provider cap.

### 3.4 The refusal of a full owner queue

`429` with the error code `owner_queue_full` and a `Retry-After` header.

- A typed error carrying the seconds of `Retry-After`.
- Nothing was written and the idempotency key is still free: retrying the
  same request with the same key is correct.
- The SDK does not retry on its own. It is distinct from `503 queue_full`,
  which means the whole resident is full.

### 3.5 `listSchedules`, `deleteSchedule`, `triggerSchedule`

- `listSchedules`: `GET /v1/schedules`, returns `schedules` and
  `truncated`.
- `deleteSchedule(id, revision)`: `DELETE /v1/schedules/{id}` with the
  required `If-Match`. `412` carries the current revision; `409
  schedule_declared_by_project` means the schedule belongs to the project
  file and this door cannot remove it.
- `triggerSchedule(id)`: `POST /v1/schedules/{id}/trigger` with the
  required `Idempotency-Key` header, like a submission. It creates a manual
  job (`202`), or answers the same job on a replay (`200`). It can meet the
  same admission refusals as a submission, `owner_queue_full` included.

### 3.6 `retryJob` and attempts: not yet

Re-attempting an interrupted job is proposed in ADR-156 and not decided.
Nothing of it is served. `retryJob`, an attempts reader and a `retry`
member on a submission wait for that decision; this plan only reserves the
names.

One consequence to prepare for if ADR-156 is accepted as written: a job's
`execution_id` and `trace_id` would change when it is re-attempted. A
client that caches them from an early read would hold an earlier attempt's.

## 4. Tests

- Contract: generate or check the SDK's types against the engine's committed
  `openapi.json` for the new objects, and keep reading them open.
- Live: one run against a resident on the `mock` provider with
  `--max-queued-per-owner 1`, covering a submission with an owner, the
  list, the 429 with its header, and the load.
- Old engine: each new method refuses locally against a `/health` that
  lacks its capability.

## 5. Order

1. Capability detection and the typed "capability missing" error.
2. `owner` and `labels` on submission, with `owner_queue_full`.
3. `listJobs`.
4. `listSchedules`, `deleteSchedule`, `triggerSchedule`.
5. `occupancy`.
6. After ADR-156 is decided: `retryJob` and attempts.

## 6. Next engine candidates this plan does not cover

Named so that they are not mistaken for delivered work: events per task
over HTTP, a model or a budget chosen per run over HTTP, and a content
cache. None is designed.
