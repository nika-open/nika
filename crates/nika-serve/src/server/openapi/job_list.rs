// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! The job list's part of the contract source (`GET /v1/jobs` · ADR-155).

use serde_json::{Value, json};

/// The `get` operation of `/v1/jobs`.
pub(super) fn operation(error: &dyn Fn(&'static str) -> Value) -> Value {
    json!({
        "summary": "List jobs, newest first",
        "description": "Health capability `jobList`. One page of the jobs this resident keeps, newest admission first, as light summaries: a summary never carries inputs, outputs, a receipt or a settlement (read them on GET /v1/jobs/{id}). Every query member is optional and they combine with AND. The list is a view inside one authority domain: filtering by `owner` is never an authorization boundary, and whoever holds the token reads every job. Pages are stable: a job admitted while a caller pages is newer than every page still to come, so no summary repeats or is skipped; a job the resident stopped keeping simply no longer appears. An unknown member, a repeated single member or a value outside its bounds is refused, never ignored.",
        "parameters": [
            {"name": "owner", "in": "query", "required": false,
             "description": "The exact owner the job's caller stated.",
             "schema": {"type": "string", "minLength": 1, "maxLength": 128, "pattern": "^[A-Za-z0-9._:@-]+$"}},
            {"name": "status", "in": "query", "required": false, "style": "form", "explode": true,
             "description": "Any of these statuses. Repeat the member or separate values with commas.",
             "schema": {"type": "array", "items": {"$ref": "#/components/schemas/JobStatus"}}},
            {"name": "workflow", "in": "query", "required": false,
             "description": "The exact served workflow name captured at admission.",
             "schema": {"type": "string", "minLength": 1, "maxLength": 4096}},
            {"name": "origin", "in": "query", "required": false,
             "description": "`manual`: submitted through POST /v1/jobs. `schedule`: fired by a resident schedule.",
             "schema": {"type": "string", "enum": ["manual", "schedule"]}},
            {"name": "label", "in": "query", "required": false, "style": "form", "explode": true,
             "description": "`key:value`, split on the first colon. Repeatable, at most 16; every pair must be a label of the job.",
             "schema": {"type": "array", "maxItems": 16, "items": {"type": "string", "pattern": "^[A-Za-z0-9._/-]{1,64}:"}}},
            {"name": "created_after", "in": "query", "required": false,
             "description": "Admitted strictly after this RFC 3339 instant. Write a `+` offset as `%2B`. A job admitted before the resident dated admissions is placed by its first dated event, and left out of a dated listing when it has none.",
             "schema": {"type": "string", "format": "date-time"}},
            {"name": "created_before", "in": "query", "required": false,
             "description": "Admitted strictly before this RFC 3339 instant.",
             "schema": {"type": "string", "format": "date-time"}},
            {"name": "limit", "in": "query", "required": false,
             "schema": {"type": "integer", "minimum": 1, "maximum": 200, "default": 50}},
            {"name": "cursor", "in": "query", "required": false,
             "description": "The `next_cursor` of the previous page, sent back unchanged with the same filters. Opaque: never build or parse one.",
             "schema": {"type": "string", "maxLength": 32}}
        ],
        "responses": {
            "200": {"description": "One page", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/JobList"}}}},
            "400": error("`invalid_job_query`: an unknown member, a repeated single member or a value outside its bounds · `invalid_job_cursor`: not a cursor this list wrote"),
            "401": error("Error envelope"),
            "408": error("Request deadline"),
            "503": error("`store_busy`: durable job state unavailable")
        }
    })
}

/// `JobList` and `JobSummary`. Both are open by contract, unlike `Job`: a
/// later engine may add a member, and a client ignores what it does not know.
pub(super) fn schemas() -> [(&'static str, Value); 2] {
    [
        (
            "JobList",
            json!({
                "type": "object",
                "additionalProperties": true,
                "description": "One page of the job list, newest admission first. Open by contract: a later engine may add a member, and a client ignores the members it does not know.",
                "required": ["jobs"],
                "properties": {
                    "jobs": {"type": "array", "maxItems": 200, "items": {"$ref": "#/components/schemas/JobSummary"}},
                    "next_cursor": {
                        "type": "string",
                        "description": "Present while an older match exists: send it back as `cursor` with the same filters. Absent on the last page."
                    }
                }
            }),
        ),
        (
            "JobSummary",
            json!({
                "type": "object",
                "additionalProperties": true,
                "description": "What the list shows of one job. Open by contract, like `JobList`. It never carries inputs, outputs, a receipt or a settlement.",
                "required": ["id", "status", "workflow", "origin", "labels"],
                "properties": {
                    "id": {"type": "string", "format": "uuid"},
                    "status": {"$ref": "#/components/schemas/JobStatus"},
                    "workflow": {"type": "string", "description": "The served workflow name captured at admission."},
                    "origin": {"$ref": "#/components/schemas/JobOrigin"},
                    "owner": {"type": "string", "description": "What the caller stated; absent on an unattributed job."},
                    "labels": {"type": "object", "additionalProperties": {"type": "string"}, "description": "What the caller stated; empty on an unlabelled job."},
                    "created_at": {"type": "string", "format": "date-time", "description": "When the resident admitted the job (UTC). For a job admitted before the resident dated admissions: the date of its first dated event, or absent."},
                    "execution_id": {"type": "string"},
                    "trace_id": {"type": "string"},
                    "error": {
                        "type": "object",
                        "additionalProperties": false,
                        "description": "On a failed job only: the redacted diagnosis GET /v1/jobs/{id} carries.",
                        "required": ["code", "message"],
                        "properties": {"code": {"type": "string"}, "message": {"type": "string"}}
                    }
                }
            }),
        ),
    ]
}

/// The committed document describes the list the route answers: its nine
/// query members, two open schemas, and a summary without the heavy members.
#[test]
fn the_served_contract_describes_the_job_list() {
    let spec = super::document();
    let get = &spec["paths"]["/v1/jobs"]["get"];
    let members: Vec<&str> = get["parameters"]
        .as_array()
        .expect("parameters")
        .iter()
        .map(|parameter| parameter["name"].as_str().expect("name"))
        .collect();
    assert_eq!(
        members,
        [
            "owner",
            "status",
            "workflow",
            "origin",
            "label",
            "created_after",
            "created_before",
            "limit",
            "cursor"
        ]
    );
    for status in ["200", "400", "401", "408", "503"] {
        assert!(get["responses"][status].is_object(), "{status}");
    }
    let schemas = &spec["components"]["schemas"];
    for open in ["JobList", "JobSummary"] {
        assert_eq!(schemas[open]["additionalProperties"], true, "{open}");
    }
    assert_eq!(
        schemas["Job"]["additionalProperties"], false,
        "the published Job object stays closed"
    );
    let summary = schemas["JobSummary"]["properties"]
        .as_object()
        .expect("summary members");
    for heavy in ["inputs", "outputs", "receipt", "settlement", "evidence"] {
        assert!(!summary.contains_key(heavy), "a summary carries no {heavy}");
    }
    assert!(
        spec["paths"]["/v1/jobs"]["post"].is_object(),
        "the admission door keeps its path"
    );
}
