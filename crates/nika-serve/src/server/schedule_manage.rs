// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! Reading and acting on the schedules a resident holds (ADR-155):
//! `GET /v1/schedules`, `DELETE /v1/schedules/{id}` and
//! `POST /v1/schedules/{id}/trigger`.
//!
//! The list shows both origins. Removal and the trigger act on the schedules
//! this door created: a project beat is declared in `nika.yaml`, and the
//! file stays its one owner.

use std::sync::Arc;

use hyper::body::Incoming;
use hyper::header::IF_MATCH;
use hyper::{Request, Response, StatusCode};
use nika_cadence::{ScheduleDefinition, ScheduleOrigin, ScheduleRevision};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use super::AppState;
use super::error::{ApiError, ResponseBody, json_error};
use super::route::{
    admission_error, admission_response, admit_job, collect_body, idempotency_key, json_response,
};
use super::schedule_http::{
    insert_etag, parse_etag, schedule_not_found, schedule_status, single_header,
    store_error_response, validate_workflow,
};
use super::scheduler::lock;
use crate::RequestDigest;
use crate::job::JobBindings;
use crate::schedule::{ScheduleDecisionRecord, ScheduleRemoval};

/// Most schedules one list answers: every API schedule the store admits and
/// every project beat the bounded decision table can serve.
const MAX_LISTED_SCHEDULES: usize = 320;
/// A list projects the next slot of each schedule; one schedule's own status
/// projects more.
const LIST_SLOT_LIMIT: usize = 1;
/// A trigger carries nothing: this only bounds what a caller may send before
/// it is refused.
const MAX_TRIGGER_BODY_BYTES: usize = 1024;
/// The request identity of a trigger. The schedule's id is the whole request:
/// a retry under the same key finds its job even after the schedule changed.
const TRIGGER_DIGEST_DOMAIN: &[u8] = b"nika/schedule-trigger@1\0";

/// `GET /v1/schedules`: API schedules by id, then the project's beats by id.
pub(super) async fn list(state: &AppState) -> Response<ResponseBody> {
    let schedules = Arc::clone(&state.schedules);
    let overview = tokio::task::spawn_blocking(move || schedules.overview()).await;
    let (definitions, decisions) = match overview {
        Ok(Ok(overview)) => overview,
        Ok(Err(error)) => return store_error_response(&error),
        Err(_) => return ApiError::internal().into_response(),
    };
    let now = state.clock.now();
    let last = |origin: ScheduleOrigin, id: &str| -> Option<&ScheduleDecisionRecord> {
        decisions
            .iter()
            .find(|record| record.origin() == origin && record.schedule_id() == id)
    };
    let fire_refusals = lock(&state.fire_refusals).clone();
    let admission = |origin: ScheduleOrigin, id: &str| {
        fire_refusals
            .get(&origin.key(id))
            .map(|detail| json!({"code": "schedule.admission", "detail": detail}))
    };
    let mut listed = Vec::new();
    for definition in &definitions {
        let origin = ScheduleOrigin::Api;
        let record = last(origin, definition.id());
        let mut status = schedule_status(origin, definition, &now, record, LIST_SLOT_LIMIT);
        if status.get("finding").is_none()
            && let Some(finding) = admission(origin, definition.id())
        {
            status["finding"] = finding;
        }
        listed.push(status);
    }
    let invalid = lock(&state.project_load_finding)
        .clone()
        .map(|detail| json!({"code": "project.invalid", "detail": detail}));
    let mut project: Vec<ScheduleDefinition> = lock(&state.last_good_project)
        .iter()
        .map(|schedule| schedule.definition.clone())
        .collect();
    project.extend(
        lock(&state.project_refusals)
            .values()
            .map(|refused| refused.definition().clone()),
    );
    project.sort_by(|left, right| left.id().cmp(right.id()));
    for definition in &project {
        let origin = ScheduleOrigin::Project;
        let record = last(origin, definition.id());
        let mut status = schedule_status(origin, definition, &now, record, LIST_SLOT_LIMIT);
        if status.get("finding").is_none()
            && let Some(finding) = admission(origin, definition.id()).or_else(|| invalid.clone())
        {
            status["finding"] = finding;
        }
        listed.push(status);
    }
    let truncated = listed.len() > MAX_LISTED_SCHEDULES;
    listed.truncate(MAX_LISTED_SCHEDULES);
    json_response(
        StatusCode::OK,
        &json!({"schedules": listed, "truncated": truncated}),
    )
}

/// `DELETE /v1/schedules/{id}` under `If-Match`: the exact current entity
/// tag, or `*` for whichever revision is current.
pub(super) async fn delete(
    headers: &hyper::HeaderMap,
    id: String,
    state: &AppState,
) -> Response<ResponseBody> {
    let Some(expected) = removal_precondition(headers) else {
        return removal_refused(None);
    };
    let schedules = Arc::clone(&state.schedules);
    let lookup = id.clone();
    let removed = tokio::task::spawn_blocking(move || {
        let revision = match &expected {
            Expected::Any => None,
            Expected::Exactly(revision) => Some(revision),
        };
        schedules.remove(&lookup, revision)
    })
    .await;
    match removed {
        Ok(Ok(ScheduleRemoval::Removed(revision))) => {
            lock(&state.fire_refusals).remove(&ScheduleOrigin::Api.key(&id));
            state.schedule_wake.notify_one();
            json_response(
                StatusCode::OK,
                &json!({"removed": true, "id": id, "revision": revision.as_str()}),
            )
        }
        Ok(Ok(ScheduleRemoval::Stale(current))) => removal_refused(Some(&current)),
        Ok(Ok(ScheduleRemoval::Absent)) => absent(&id, state),
        Ok(Err(error)) => store_error_response(&error),
        Err(_) => ApiError::internal().into_response(),
    }
}

/// `POST /v1/schedules/{id}/trigger`: one manual job that runs the schedule's
/// workflow as it admits now, under the schedule's inputs, ceiling, owner and
/// labels. It consumes no slot and touches no ledger: the cadence is unmoved.
pub(super) async fn trigger(
    request: Request<Incoming>,
    id: String,
    state: Arc<AppState>,
) -> Response<ResponseBody> {
    let key = match idempotency_key(request.headers()) {
        Ok(key) => key,
        Err(error) => return error.into_response(),
    };
    let body = match collect_body(request, MAX_TRIGGER_BODY_BYTES).await {
        Ok(body) => body,
        Err(error) => return error.into_response(),
    };
    if !carries_nothing(&body) {
        return ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_schedule_trigger",
            "a trigger takes no body or an empty JSON object: the schedule states what runs",
        )
        .into_response();
    }
    let digest = trigger_digest(&id);
    // ADR-132: a key already bound replays its job before the schedule or the
    // workflow is read again, so a lost-response retry never runs twice.
    match state
        .coordinator
        .replay_manual(key.clone(), digest.clone())
        .await
    {
        Ok(Some(admission)) => return admission_response(admission),
        Ok(None) => {}
        Err(error) => return admission_error(&error),
    }
    let schedules = Arc::clone(&state.schedules);
    let lookup = id.clone();
    let definition = match tokio::task::spawn_blocking(move || schedules.get(&lookup)).await {
        Ok(Ok(Some(definition))) => definition,
        Ok(Ok(None)) => return absent(&id, &state),
        Ok(Err(error)) => return store_error_response(&error),
        Err(_) => return ApiError::internal().into_response(),
    };
    let admitted = match validate_workflow(&definition, &state).await {
        Ok(admitted) => admitted,
        Err(response) => return response,
    };
    let inputs = match super::schedule_inputs::bind(&admitted, &definition) {
        Ok(inputs) => inputs,
        Err(refusal) => {
            return json_error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "schedule.inputs",
                &format!("{} ({})", refusal.message, refusal.code),
            );
        }
    };
    let Ok(attribution) = super::attribution::of_schedule(&definition) else {
        return ApiError::internal().into_response();
    };
    let Ok(world) = admitted.snapshot().encode() else {
        return ApiError::internal().into_response();
    };
    let workflow = admitted.snapshot().root().to_owned();
    let bindings = JobBindings::of(inputs, Some(definition.max_cost_usd())).attributed(attribution);
    admit_job(state, key, digest, workflow, world, None, bindings).await
}

/// The revision a removal expects to find.
#[derive(Debug, PartialEq, Eq)]
enum Expected {
    /// `If-Match: *`: whichever revision is current.
    Any,
    /// The exact entity tag the caller read.
    Exactly(ScheduleRevision),
}

/// The precondition of a removal, or `None` when the `If-Match` header is
/// missing, repeated or not an entity tag.
fn removal_precondition(headers: &hyper::HeaderMap) -> Option<Expected> {
    match single_header(headers, IF_MATCH).ok()?? {
        "*" => Some(Expected::Any),
        value => parse_etag(value).map(Expected::Exactly),
    }
}

fn removal_refused(current: Option<&ScheduleRevision>) -> Response<ResponseBody> {
    let body = json!({
        "error": {
            "code": "schedule_precondition_failed",
            "message": "delete requires If-Match: the exact current ETag, or * for whichever revision is current",
            "currentRevision": current.map(ScheduleRevision::as_str),
        }
    });
    let mut response = json_response(StatusCode::PRECONDITION_FAILED, &body);
    if let Some(revision) = current {
        insert_etag(&mut response, revision);
    }
    response
}

/// No API schedule carries this id: a project beat of that name is named as
/// such, anything else is not found.
fn absent(id: &str, state: &AppState) -> Response<ResponseBody> {
    let declared = lock(&state.last_good_project)
        .iter()
        .any(|schedule| schedule.definition.id() == id)
        || lock(&state.project_refusals).contains_key(id);
    if declared {
        json_error(
            StatusCode::CONFLICT,
            "schedule_declared_by_project",
            "this schedule is declared in the project's nika.yaml: edit the file to change or remove it; this door removes and triggers only the schedules it created",
        )
    } else {
        schedule_not_found()
    }
}

fn carries_nothing(body: &[u8]) -> bool {
    body.iter().all(u8::is_ascii_whitespace)
        || serde_json::from_slice::<Value>(body)
            .is_ok_and(|value| value.as_object().is_some_and(serde_json::Map::is_empty))
}

fn trigger_digest(id: &str) -> RequestDigest {
    let mut hasher = Sha256::new();
    hasher.update(TRIGGER_DIGEST_DOMAIN);
    hasher.update(id.as_bytes());
    RequestDigest::from_bytes(hasher.finalize().into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_trigger_body_is_empty_or_an_empty_object() {
        for body in ["", "  \r\n", "{}", " { } \n"] {
            assert!(carries_nothing(body.as_bytes()), "{body:?}");
        }
        for body in ["null", "[]", "{\"inputs\":{}}", "0", "{", "x"] {
            assert!(!carries_nothing(body.as_bytes()), "{body:?}");
        }
    }

    #[test]
    fn a_trigger_is_identified_by_its_schedule_alone() {
        assert_eq!(trigger_digest("daily"), trigger_digest("daily"));
        assert_ne!(trigger_digest("daily"), trigger_digest("weekly"));
        let body = RequestDigest::from_bytes(Sha256::digest(b"daily").into());
        assert_ne!(trigger_digest("daily"), body, "a domain of its own");
    }

    #[test]
    fn a_removal_needs_one_if_match() {
        let headers = |values: &[&str]| {
            let mut headers = hyper::HeaderMap::new();
            for value in values {
                headers.append(IF_MATCH, value.parse().expect("header"));
            }
            headers
        };
        let revision = format!("sha256:{}", "a".repeat(64));
        assert_eq!(removal_precondition(&headers(&["*"])), Some(Expected::Any));
        let expected = ScheduleRevision::from_wire(&revision).expect("revision");
        assert_eq!(
            removal_precondition(&headers(&[&format!("\"{revision}\"")])),
            Some(Expected::Exactly(expected))
        );
        assert_eq!(removal_precondition(&headers(&[])), None);
        assert_eq!(removal_precondition(&headers(&["*", "*"])), None);
        assert_eq!(removal_precondition(&headers(&[&revision])), None);
        assert_eq!(removal_precondition(&headers(&["\"nope\""])), None);
    }
}
