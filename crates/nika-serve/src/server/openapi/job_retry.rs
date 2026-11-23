// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! The attempts part of the contract source (ADR-156 sections 9 and 10):
//! `POST /v1/jobs/{id}/retry` and the request it reads, and
//! `GET /v1/jobs/{id}/attempts` and what it answers.

use serde_json::{Value, json};

/// `/v1/jobs/{id}/retry`.
pub(super) fn path(error: &dyn Fn(&'static str) -> Value, job: &Value) -> Value {
    json!({"post": {
        "summary": "Start the latest attempt of a settled job again",
        "description": "Health capability `jobRetry`. The latest attempt of a job that ended `interrupted` or `failed`, and that its caller did not cancel, becomes evidence and a new attempt of the same job is queued, with its own execution and trace identity. The request is the caller's consent to run the workflow's effects again. The bound of attempts in force for the job, the one ceiling over every attempt and, for `resume`, the judgment of the attempt's journal hold as for a run that lost its owner (ADR-156). The place in the owner's lane is taken before anything is written. The same request sent again answers 200 with the job as it stands and writes nothing.",
        "parameters": [{"name": "id", "in": "path", "required": true, "schema": {"type": "string", "format": "uuid"}}],
        "requestBody": {"required": true, "content": {"application/json": {"schema": {"$ref": "#/components/schemas/JobRetryRequest"}}}},
        "responses": {
            "200": {"description": "The same request already retried this attempt: the job as it stands. A job still `queued` is put back in its owner's lane unless it waits already", "content": job},
            "202": {"description": "The new attempt is queued", "content": job},
            "401": error("Error envelope"),
            "404": error("`job_not_found`: unknown or forgotten job id"),
            "408": error("Request deadline"),
            "409": error("`job_not_retryable`: the job did not end `interrupted` or `failed`, a cancellation was accepted on it, its latest attempt never started, or `attempt` is not its latest · `retry_exhausted`: the job reached its bound of attempts · `resume_refused`: the attempt's journal cannot be resumed, or a schedule fired the job · `unknown_spend`: what an attempt spent cannot be read, and no `max_cost_usd` is stated · `budget_exhausted`: nothing is left under the job's ceiling"),
            "413": error("`body_too_large`"),
            "422": error("`invalid_job_retry`: the body is not a retry request"),
            "429": super::occupancy::owner_refusal(),
            "503": error("`store_busy`: durable job state unavailable · `queue_full`: the execution queue is at capacity · `stopping`: the resident is stopping")
        }
    }})
}

/// `/v1/jobs/{id}/attempts`.
pub(super) fn attempts_path(error: &dyn Fn(&'static str) -> Value) -> Value {
    json!({"get": {
        "summary": "Read every attempt of a job",
        "description": "Health capability `jobRetry`. The policy in force for the job (the resident's `--retry-interrupted` and `--retry-max-attempts`, as the job's own `retry` narrows them), where it stands about being started again, and each attempt oldest first. An attempt that was started again keeps its identity, its receipt, how it ended and what the decision that closed it counted; the attempt in hand carries its spend in its own settlement on `GET /v1/jobs/{id}`. Each attempt has its own journal: `GET /v1/jobs/{id}/trace/verify?attempt=N` verifies one.",
        "parameters": [{"name": "id", "in": "path", "required": true, "schema": {"type": "string", "format": "uuid"}}],
        "responses": {
            "200": {"description": "The job's attempts", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/JobAttempts"}}}},
            "401": error("Error envelope"),
            "404": error("`job_not_found`: unknown or forgotten job id"),
            "408": error("Request deadline"),
            "503": error("`store_busy`: durable job state unavailable")
        }
    }})
}

/// The retry states a job shows on the list and on its attempts.
pub(super) fn retry_state_schema() -> Value {
    json!({
        "type": "string",
        "enum": ["none", "queued", "exhausted", "refused"],
        "description": "`queued`: a new attempt waits in its owner's lane. `exhausted`: the job took its bound of attempts, so a request would be refused. `refused`: the resident decided not to start it again, and says why (`retry_reason`, and the code of the job's last event or error). `none`: nothing waits and nothing was refused, including a job whose latest attempt failed for a reason of its own while its bound leaves room. Open: a later engine may add a word."
    })
}

/// `JobRetryRequest`, closed: a member it does not know is refused.
/// `JobAttempts` and `JobAttempt`, open.
pub(super) fn schemas() -> [(&'static str, Value); 3] {
    [
        retry_request_schema(),
        ("JobAttempts", attempts_schema()),
        ("JobAttempt", attempt_schema()),
    ]
}

fn attempts_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": true,
        "required": ["job_id", "policy", "retry_state", "attempts"],
        "properties": {
            "job_id": {"type": "string", "format": "uuid"},
            "policy": {
                "type": "object",
                "additionalProperties": true,
                "required": ["on_interrupted", "max_attempts"],
                "properties": {
                    "on_interrupted": {"type": "string", "enum": ["off", "restart", "resume"]},
                    "max_attempts": {"type": "integer", "minimum": 1, "maximum": 16}
                }
            },
            "retry_state": retry_state_schema(),
            "retry_reason": {"type": "string", "enum": ["retry_exhausted", "effects_need_consent", "unknown_spend", "budget_exhausted", "resume_refused"], "description": "Present when `retry_state` is `exhausted` or `refused`."},
            "attempts": {"type": "array", "minItems": 1, "items": {"$ref": "#/components/schemas/JobAttempt"}}
        }
    })
}

fn attempt_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": true,
        "required": ["attempt", "status"],
        "properties": {
            "attempt": {"type": "integer", "minimum": 1},
            "status": {"$ref": "#/components/schemas/JobStatus"},
            "execution_id": {"type": "string", "description": "Absent on an attempt that waits and has not started."},
            "trace_id": {"type": "string"},
            "mode": {"type": "string", "enum": ["restart", "resume"], "description": "How this attempt was started again; absent on the first."},
            "resumed_from": {"type": "string", "description": "The trace this attempt continues, when it resumed the one before it."},
            "receipt": {"$ref": "#/components/schemas/JobReceipt"},
            "spend": {
                "type": "object",
                "additionalProperties": false,
                "required": ["usd"],
                "description": "On an attempt that was started again: what the decision counted against the job's ceiling. `unknown` is present when some of it could not be read; it never counts for zero.",
                "properties": {"usd": {"type": "number", "minimum": 0}, "unknown": {"type": "boolean", "const": true}}
            },
            "started_at": {"type": "string", "format": "date-time"},
            "ended_at": {"type": "string", "format": "date-time", "description": "For an attempt queued again after losing its resident, when it was queued again."}
        }
    })
}

fn retry_request_schema() -> (&'static str, Value) {
    (
        "JobRetryRequest",
        json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["attempt", "mode"],
            "properties": {
                "attempt": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 4_294_967_295_u64,
                    "description": "The attempt this request retries, the first being 1: the job's latest. Naming it makes the request safe to send again."
                },
                "mode": {
                    "type": "string",
                    "enum": ["restart", "resume"],
                    "description": "`restart` runs the captured workflow from its first task. `resume` serves the tasks the attempt's journal completed and runs the rest, once the journal is judged as `nika run --resume` judges one; the resident never falls back to `restart` on its own."
                },
                "max_cost_usd": {
                    "type": "number",
                    "minimum": 0,
                    "description": "A ceiling for the re-attempts alone (USD), in force for the attempts after this one until another request states one. Stating it accepts that an attempt whose spend could not be read is followed by another. It never raises the ceiling that covers every attempt of the job."
                }
            }
        }),
    )
}
