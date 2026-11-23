// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! The retry part of the contract source (ADR-156 section 9):
//! `POST /v1/jobs/{id}/retry` and the request it reads.

use serde_json::{Value, json};

/// `/v1/jobs/{id}/retry`.
pub(super) fn path(error: &dyn Fn(&'static str) -> Value, job: &Value) -> Value {
    json!({"post": {
        "summary": "Start the latest attempt of a settled job again",
        "description": "Health capability `jobRetry`. The latest attempt of a job that ended `interrupted` or `failed`, and that its caller did not cancel, becomes evidence and a new attempt of the same job is queued, with its own execution and trace identity. The request is the caller's consent to run the workflow's effects again. The bound of attempts in force for the job, the one ceiling over every attempt and, for `resume`, the judgment of the attempt's journal hold as for a run that lost its owner (ADR-156). The place in the owner's lane is taken before anything is written. The same request sent again answers 200 with the job as it stands and writes nothing.",
        "parameters": [{"name": "id", "in": "path", "required": true, "schema": {"type": "string", "format": "uuid"}}],
        "requestBody": {"required": true, "content": {"application/json": {"schema": {"$ref": "#/components/schemas/JobRetryRequest"}}}},
        "responses": {
            "200": {"description": "The same request already retried this attempt: the job as it stands", "content": job},
            "202": {"description": "The new attempt is queued", "content": job},
            "401": error("Error envelope"),
            "404": error("`job_not_found`: unknown or forgotten job id"),
            "408": error("Request deadline"),
            "409": error("`job_not_retryable`: the job did not end `interrupted` or `failed`, a cancellation was accepted on it, its latest attempt never started, or `attempt` is not its latest · `retry_exhausted`: the job reached its bound of attempts · `resume_refused`: the attempt's journal cannot be resumed, or a schedule fired the job · `unknown_spend`: what an attempt spent cannot be read under a ceiling, and the request states no `max_cost_usd` · `budget_exhausted`: nothing is left under the job's ceiling"),
            "413": error("`body_too_large`"),
            "422": error("`invalid_job_retry`: the body is not a retry request"),
            "429": super::occupancy::owner_refusal(),
            "503": error("`store_busy`: durable job state unavailable · `queue_full`: the execution queue is at capacity · `stopping`: the resident is stopping")
        }
    }})
}

/// `JobRetryRequest`. Closed: a member it does not know is refused.
pub(super) fn schemas() -> [(&'static str, Value); 1] {
    [(
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
    )]
}
