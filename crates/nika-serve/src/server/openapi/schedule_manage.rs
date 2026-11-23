// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! The schedule management part of the contract source (ADR-155):
//! `GET /v1/schedules`, `DELETE /v1/schedules/{id}` and
//! `POST /v1/schedules/{id}/trigger`.

use serde_json::{Value, json};

/// `/v1/schedules`.
pub(super) fn list_path(error: &dyn Fn(&'static str) -> Value) -> Value {
    json!({"get": {
        "summary": "List the schedules this resident holds",
        "description": "Health capability `scheduleManage`. The schedules this door created, by id, then the beats the project's `nika.yaml` declares, by id. Each entry is the status GET /v1/schedules/{id} answers, planned now, with one next slot. A project beat is read here: GET /v1/schedules/{id} answers an API schedule, or a project beat that carries a finding. `truncated` is true when the resident holds more schedules than one list answers.",
        "responses": {
            "200": {"description": "Every schedule, planned now", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/ScheduleList"}}}},
            "401": error("Error envelope"),
            "408": error("Request deadline"),
            "500": error("`schedule_store_corrupt`: durable schedule state failed integrity validation"),
            "503": error("`schedule_store_unavailable`: durable schedule state unavailable")
        }
    }})
}

/// The `delete` operation of `/v1/schedules/{id}`.
pub(super) fn delete_operation(error: &dyn Fn(&'static str) -> Value) -> Value {
    json!({
        "summary": "Remove one schedule this door created",
        "description": "Health capability `scheduleManage`. Requires If-Match: the exact current ETag, or `*` for whichever revision is current. Removes the definition and its last-slot decision in one durable replacement; a schedule declared again under the same id starts with no history. A job the schedule already fired is untouched and settles on its own. A beat the project's `nika.yaml` declares is not this door's to remove and answers 409.",
        "parameters": [{"name": "If-Match", "in": "header", "required": true, "schema": {"type": "string", "maxLength": 96}}],
        "responses": {
            "200": {"description": "Removed", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/ScheduleRemoved"}}}},
            "401": error("Error envelope"),
            "404": error("`schedule_not_found`"),
            "409": error("`schedule_declared_by_project`: declared in the project file"),
            "412": {"description": "Precondition refused; carries the current revision", "headers": {"ETag": {"schema": {"type": "string"}, "description": "The current revision, when one exists"}}, "content": {"application/json": {"schema": {"$ref": "#/components/schemas/SchedulePreconditionFailed"}}}},
            "503": error("`schedule_store_unavailable`: durable schedule state unavailable")
        }
    })
}

/// `/v1/schedules/{id}/trigger`.
pub(super) fn trigger_path(error: &dyn Fn(&'static str) -> Value, job: &Value) -> Value {
    json!({
        "parameters": [{"name": "id", "in": "path", "required": true, "schema": {"type": "string", "minLength": 1, "maxLength": 255}}],
        "post": {
            "summary": "Run one schedule now, as a manual job",
            "description": "Health capability `scheduleManage`. Admits ONE manual job that runs the schedule's workflow as it admits now, under the inputs, cost ceiling, owner and labels the schedule declares (the declared ceiling restricts the resident's, never widens it). The request carries no body, or an empty JSON object. The trigger consumes no slot and records no schedule decision: the cadence is unmoved and the next slot fires as planned. A paused schedule can be triggered: a pause holds the cadence, not the operator. The job's origin is `manual`; find it by the schedule's owner and labels. The `Idempotency-Key` binds to the schedule id: a retry under the same key answers the same job, even after the schedule changed, and the same key on another schedule or on POST /v1/jobs answers 409. This is an operator gesture behind the one bearer credential, never an inbound event door for a third party. A beat the project's `nika.yaml` declares is not this door's to trigger and answers 409.",
            "parameters": [{"$ref": "#/components/parameters/IdempotencyKey"}],
            "responses": {
                "202": {"description": "Created", "content": job},
                "200": {"description": "Idempotent replay", "content": job},
                "400": error("Invalid idempotency key"),
                "401": error("Error envelope"),
                "404": error("`schedule_not_found`"),
                "408": error("Request deadline"),
                "409": error("`idempotency_conflict`: the key is bound to another request · `schedule_declared_by_project`: declared in the project file"),
                "413": error("A trigger carries no body"),
                "422": error("`invalid_schedule_trigger`: the body is not empty · `schedule.workflow`, `schedule.workflow_outside_registry` or `schedule.inputs`: the schedule's workflow or inputs no longer admit"),
                "503": error("Execution queue or durable store unavailable"),
                "507": error("Durable job capacity exhausted")
            }
        }
    })
}

/// The attribution members of `SchedulePut`.
pub(super) fn put_members() -> [(&'static str, Value); 2] {
    [
        (
            "owner",
            json!({
                "type": "string", "minLength": 1, "maxLength": 128, "pattern": "^[A-Za-z0-9._:@-]+$",
                "description": "Health capability `scheduleManage`. Who the jobs this schedule starts are attributed to: every job it fires or a trigger admits carries it (GET /v1/jobs?owner=). The same law as `owner` on POST /v1/jobs: attribution inside one authority domain, never an authorization boundary. Part of the schedule revision only when stated, so a schedule that states none keeps its revision."
            }),
        ),
        (
            "labels",
            json!({
                "type": "object", "maxProperties": 16,
                "propertyNames": {"pattern": "^[A-Za-z0-9._/-]+$", "minLength": 1, "maxLength": 64},
                "additionalProperties": {"type": "string", "maxLength": 256},
                "description": "The labels every job this schedule starts carries, under the law of `labels` on POST /v1/jobs. Part of the schedule revision only when stated."
            }),
        ),
    ]
}

/// `ScheduleList` and `ScheduleRemoved`, open by contract like `ScheduleStatus`.
pub(super) fn schemas() -> [(&'static str, Value); 2] {
    [
        (
            "ScheduleList",
            json!({
                "type": "object",
                "additionalProperties": true,
                "description": "Every schedule the resident holds: API schedules by id, then project beats by id. Open by contract: a later engine may add a member.",
                "required": ["schedules", "truncated"],
                "properties": {
                    "schedules": {"type": "array", "maxItems": 320, "items": {"$ref": "#/components/schemas/ScheduleStatus"}},
                    "truncated": {"type": "boolean", "description": "True when the resident holds more schedules than one list answers."}
                }
            }),
        ),
        (
            "ScheduleRemoved",
            json!({
                "type": "object",
                "additionalProperties": true,
                "required": ["removed", "id", "revision"],
                "properties": {
                    "removed": {"type": "boolean", "const": true},
                    "id": {"type": "string", "minLength": 1, "maxLength": 255},
                    "revision": {"type": "string", "pattern": "^sha256:[0-9a-f]{64}$", "description": "The revision that was removed."}
                }
            }),
        ),
    ]
}

/// The committed document describes the three management routes, and a
/// schedule body that may state who its jobs are attributed to.
#[test]
fn the_served_contract_describes_schedule_management() {
    let spec = super::document();
    let paths = &spec["paths"];
    let list = &paths["/v1/schedules"]["get"];
    for status in ["200", "401", "408", "500", "503"] {
        assert!(list["responses"][status].is_object(), "list {status}");
    }
    let delete = &paths["/v1/schedules/{id}"]["delete"];
    assert_eq!(delete["parameters"][0]["name"], "If-Match");
    assert_eq!(delete["parameters"][0]["required"], true);
    for status in ["200", "401", "404", "409", "412", "503"] {
        assert!(delete["responses"][status].is_object(), "delete {status}");
    }
    let trigger = &paths["/v1/schedules/{id}/trigger"]["post"];
    assert!(
        trigger.get("requestBody").is_none(),
        "a trigger has no body"
    );
    for status in [
        "200", "202", "400", "401", "404", "409", "413", "422", "503", "507",
    ] {
        assert!(trigger["responses"][status].is_object(), "trigger {status}");
    }
    assert_eq!(
        trigger["responses"]["202"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/Job"
    );
    let schemas = &spec["components"]["schemas"];
    assert_eq!(schemas["ScheduleList"]["additionalProperties"], true);
    assert_eq!(schemas["ScheduleRemoved"]["additionalProperties"], true);
    let put = &schemas["SchedulePut"];
    assert_eq!(put["additionalProperties"], false);
    assert_eq!(put["properties"]["owner"]["maxLength"], 128);
    assert_eq!(put["properties"]["labels"]["maxProperties"], 16);
    let by_name = &schemas["JobByName"]["properties"];
    for member in ["pattern", "minLength", "maxLength"] {
        assert_eq!(
            put["properties"]["owner"][member], by_name["owner"][member],
            "one owner law on both doors: {member}"
        );
    }
    for member in ["maxProperties", "propertyNames", "additionalProperties"] {
        assert_eq!(
            put["properties"]["labels"][member], by_name["labels"][member],
            "one label law on both doors: {member}"
        );
    }
}
