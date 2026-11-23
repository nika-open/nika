// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! The occupancy part of the contract source (ADR-155): `GET /v1/occupancy`
//! and the refusal an owner meets at its own queue bound.

use serde_json::{Value, json};

/// `/v1/occupancy`.
pub(super) fn path(error: &dyn Fn(&'static str) -> Value) -> Value {
    json!({"get": {
        "summary": "Read what the resident holds against its limits",
        "description": "Health capability `occupancy`. How many jobs run and wait now, the limits they are held to, and the owners that hold the most. A point-in-time reading of this resident's memory, never a durable record: read GET /v1/jobs for the jobs themselves. Authenticated, unlike /health, because it names owners. At most 100 owners are named, busiest first; `owners_omitted` counts the rest.",
        "responses": {
            "200": {"description": "The current load", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Occupancy"}}}},
            "401": error("Error envelope"),
            "408": error("Request deadline")
        }
    }})
}

/// The `429` an admission answers when the stated owner holds every waiting
/// place one owner may hold.
pub(super) fn owner_refusal() -> Value {
    json!({
        "description": "`owner_queue_full`: this owner already has as many jobs waiting as one owner may have. Nothing is written and the idempotency key stays unbound; other owners are still admitted. Retry the same request when one of the owner's jobs starts.",
        "headers": {"Retry-After": {"schema": {"type": "integer", "minimum": 1}, "description": "Seconds to wait before the retry. A hint: the resident cannot know when one of the owner's jobs will start."}},
        "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Error"}}}
    })
}

/// `Occupancy`. Open by contract: a later engine may add a member, and a
/// client ignores what it does not know.
pub(super) fn schemas() -> [(&'static str, Value); 1] {
    let count = json!({"type": "integer", "minimum": 0});
    let bound = json!({"type": ["integer", "null"], "minimum": 1});
    [(
        "Occupancy",
        json!({
            "type": "object",
            "additionalProperties": true,
            "description": "What the resident holds now against its limits. Open by contract: a later engine may add a member, and a client ignores the members it does not know.",
            "required": ["jobs", "owners", "owners_omitted"],
            "properties": {
                "jobs": {
                    "type": "object",
                    "additionalProperties": true,
                    "required": ["running", "queued", "max_concurrent", "queue_capacity", "max_concurrent_per_owner", "max_queued_per_owner"],
                    "properties": {
                        "running": {"allOf": [count.clone()], "description": "Jobs a worker runs now."},
                        "queued": {"allOf": [count.clone()], "description": "Jobs admitted and waiting for a worker."},
                        "max_concurrent": {"type": "integer", "minimum": 1, "description": "Jobs the resident runs at once."},
                        "queue_capacity": {"type": "integer", "minimum": 1, "description": "Jobs that may wait, all owners together. Past it an admission answers 503 `queue_full`."},
                        "max_concurrent_per_owner": {"allOf": [bound.clone()], "description": "Jobs one owner runs at once; null when unbounded. Its other jobs wait."},
                        "max_queued_per_owner": {"allOf": [bound], "description": "Jobs one owner may have waiting; null when unbounded. Past it an admission answers 429 `owner_queue_full`."}
                    }
                },
                "owners": {
                    "type": "array",
                    "maxItems": 100,
                    "description": "The owners that hold a job now, busiest first.",
                    "items": {
                        "type": "object",
                        "additionalProperties": true,
                        "required": ["owner", "running", "queued"],
                        "properties": {
                            "owner": {"type": ["string", "null"], "description": "The stated owner; null for the jobs that state none, which share one lane."},
                            "running": count.clone(),
                            "queued": count.clone()
                        }
                    }
                },
                "owners_omitted": {"allOf": [count], "description": "Owners that hold a job and are not named above."}
            }
        }),
    )]
}
