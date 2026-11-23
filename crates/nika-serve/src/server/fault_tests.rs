// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! Faults a resident meets in production (ADR-156): a job that takes down
//! every resident that runs it, a job an older engine left in its store,
//! and a lost run whose effects can repeat.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};

use super::retry_tests::{
    StartedThenLost, dropping, kinds_and_statuses, named, running_job, stored_jobs, until_called,
};
use super::tests::{
    PinRecordingBackend, TestBackend, TestServer, TestWorld, auth_header, events_request,
    get_request, parse_sse_data, wait_for_status,
};
use super::{ExecutionDisposition, ServerError};
use crate::JobStore;
use crate::job::OnInterrupted;

const V3: &str = include_str!("../job/store/fixtures/v3.json");

/// A workflow that runs a program: its effect can repeat.
const EFFECTS: &str = "nika: effects\npermits:\n  exec: [\"echo\"]\ntasks:\n  say:\n    exec: { command: [\"echo\", \"x\"] }\n";

fn retry_request(id: &str, body: &str) -> String {
    format!(
        "POST /v1/jobs/{id}/retry HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{}\r\n{body}",
        body.len(),
        auth_header()
    )
}

async fn attempts(server: &TestServer, id: &str) -> Value {
    server
        .request(&get_request(&format!("/v1/jobs/{id}/attempts")))
        .await
        .json()
}

/// Start a resident over the world whose run never ends, wait until it
/// runs the queued job, and stop it past its grace.
async fn lose_one_run(world: &TestWorld) {
    let lost = StartedThenLost::over(world);
    let server = world
        .start(lost.clone(), dropping(OnInterrupted::Restart))
        .await;
    until_called(&lost).await;
    assert!(matches!(
        server.stop().await,
        Err(ServerError::ShutdownTimeout)
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_job_that_takes_down_every_resident_stops_at_its_bound() {
    let world = TestWorld::new();
    let lost = StartedThenLost::over(&world);
    let server = world
        .start(lost.clone(), dropping(OnInterrupted::Restart))
        .await;
    let (id, _) = running_job(&server, &named(&json!({})), "poisoned-1").await;
    until_called(&lost).await;
    assert!(matches!(
        server.stop().await,
        Err(ServerError::ShutdownTimeout)
    ));
    // Each next resident runs it again and loses it too.
    lose_one_run(&world).await;
    lose_one_run(&world).await;
    let record = &stored_jobs(&world)[0]["record"];
    assert_eq!(record["status"], "interrupted", "{record}");

    // The fourth resident starts nothing: the job took its bound.
    let backend = Arc::new(TestBackend::completes(ExecutionDisposition::Succeeded));
    let server = world
        .start(backend.clone(), dropping(OnInterrupted::Restart))
        .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(backend.calls(), 0, "a job past its bound never runs again");
    let read = attempts(&server, &id).await;
    assert_eq!(read["retry_state"], "exhausted", "{read}");
    assert_eq!(read["retry_reason"], "retry_exhausted", "{read}");
    let views = read["attempts"].as_array().expect("attempts");
    assert_eq!(views.len(), 3, "{read}");
    assert!(
        views.iter().all(|view| view["status"] == "interrupted"),
        "{read}"
    );
    assert_eq!(
        (views[1]["mode"].as_str(), views[2]["mode"].as_str()),
        (Some("restart"), Some("restart"))
    );
    let frames = parse_sse_data(&server.request(&events_request(&id, None)).await.body);
    let ending = frames.last().expect("frames");
    assert_eq!(
        (
            ending["kind"].as_str(),
            ending["status"].as_str(),
            ending["code"].as_str()
        ),
        (
            Some("execution.interrupted"),
            Some("interrupted"),
            Some("retry_exhausted")
        ),
        "{frames:?}"
    );
    assert_eq!(
        kinds_and_statuses(&frames)
            .iter()
            .filter(|(kind, _)| kind == "execution.started")
            .count(),
        3
    );
    server.stop().await.expect("a clean stop");
}

/// A job a format 3 store kept queued carries no captured world, no
/// admission date and no effects class. The store migrates it; the resident
/// refuses to readmit it, as 0.124.5 did, and a request cannot start an
/// attempt that never started.
#[tokio::test(flavor = "multi_thread")]
async fn a_job_an_older_engine_left_queued_is_refused_and_not_started_again() {
    let world = TestWorld::new();
    drop(JobStore::open(&world.state).expect("initialized store"));
    std::fs::write(world.state.join("jobs/state.json"), V3).expect("a format 3 store");
    let backend = Arc::new(TestBackend::completes(ExecutionDisposition::Succeeded));
    let server = world
        .start(backend.clone(), dropping(OnInterrupted::Restart))
        .await;
    let queued = "00000000-0000-4000-8000-000000000002";
    wait_for_status(&server, queued, "failed")
        .await
        .expect("the queued job settles");
    assert_eq!(backend.calls(), 0);
    let read = attempts(&server, queued).await;
    assert_eq!(read["retry_state"], "none", "{read}");
    let views = read["attempts"].as_array().expect("attempts");
    assert_eq!(views.len(), 1, "{read}");
    assert_eq!(views[0]["status"], "failed");
    assert!(views[0].get("execution_id").is_none(), "{read}");
    assert!(views[0].get("started_at").is_none(), "{read}");
    let ask = json!({"attempt": 1, "mode": "restart", "max_cost_usd": 0.5}).to_string();
    let refused = server.request(&retry_request(queued, &ask)).await;
    assert_eq!(refused.status, 409, "{}", refused.body);
    assert_eq!(refused.json()["error"]["code"], "job_not_retryable");

    // The job the older engine settled reads as one attempt, undated.
    let settled = attempts(&server, "00000000-0000-4000-8000-000000000001").await;
    let views = settled["attempts"].as_array().expect("attempts");
    assert_eq!(views.len(), 1, "{settled}");
    assert_eq!(views[0]["status"], "succeeded");
    assert!(views[0]["receipt"].is_object(), "{settled}");
    assert!(
        views[0].get("ended_at").is_none(),
        "an undated event stays undated"
    );
    let listed = server.request(&get_request("/v1/jobs")).await.json();
    assert!(
        listed["jobs"]
            .as_array()
            .expect("jobs")
            .iter()
            .all(|job| job["attempts"] == 1),
        "{listed}"
    );
    server.stop().await.expect("a clean stop");
}

/// A workflow that runs a program can repeat its effect: the resident never
/// starts it again on its own, and says why. A request is the consent.
#[tokio::test(flavor = "multi_thread")]
async fn a_lost_run_with_effects_waits_for_a_request_to_run_again() {
    let world = TestWorld::new();
    std::fs::write(world.workflows.join("effects.nika"), EFFECTS).expect("workflow");
    let lost = StartedThenLost::over(&world);
    let server = world
        .start(lost.clone(), dropping(OnInterrupted::Restart))
        .await;
    let body = json!({"workflow": "effects.nika"}).to_string();
    let (id, first) = running_job(&server, &body, "effects-1").await;
    until_called(&lost).await;
    assert!(matches!(
        server.stop().await,
        Err(ServerError::ShutdownTimeout)
    ));

    let backend = Arc::new(PinRecordingBackend::new());
    let server = world
        .start(backend.clone(), dropping(OnInterrupted::Restart))
        .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(backend.calls(), 0, "nothing runs without consent");
    let read = attempts(&server, &id).await;
    assert_eq!(read["retry_state"], "refused", "{read}");
    assert_eq!(read["retry_reason"], "effects_need_consent", "{read}");
    let listed = server.request(&get_request("/v1/jobs")).await.json();
    assert_eq!(listed["jobs"][0]["retry_state"], "refused", "{listed}");

    // The request consents; its ceiling covers what the backend cannot read.
    let ask = json!({"attempt": 1, "mode": "restart", "max_cost_usd": 0.5}).to_string();
    let retried = server.request(&retry_request(&id, &ask)).await;
    assert_eq!(retried.status, 202, "{}", retried.body);
    wait_for_status(&server, &id, "succeeded")
        .await
        .expect("the second attempt settles");
    assert_eq!(backend.calls(), 1);
    let read = attempts(&server, &id).await;
    assert_eq!(read["retry_state"], "none", "{read}");
    assert_eq!(read["attempts"][0]["status"], "interrupted", "{read}");
    assert_eq!(read["attempts"][0]["execution_id"], first["execution_id"]);
    assert_eq!(read["attempts"][1]["status"], "succeeded", "{read}");
    assert_eq!(read["attempts"][1]["mode"], "restart", "{read}");
    server.stop().await.expect("a clean stop");
}
