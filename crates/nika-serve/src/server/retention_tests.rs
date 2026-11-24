// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! Job retention as a resident applies it (ADR-155): at an admission, at
//! startup, and never under a window shorter than a minute.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};

use super::schedule_tests::ManualClock;
use super::tests::{
    TestBackend, TestServer, TestWorld, auth_header, get_request, limits, post_request,
    wait_for_status,
};
use super::{ExecutionDisposition, ResidentAuthority, ResidentConfig, ServerError};

const BODY: &str = r#"{"workflow":"root.nika"}"#;

async fn admit(server: &TestServer, key: &str) -> (u16, String) {
    let created = server
        .request(&post_request(BODY, key, &auth_header()))
        .await;
    let id = created.json()["id"].as_str().expect("job id").to_owned();
    wait_for_status(server, &id, "succeeded")
        .await
        .expect("the job settles");
    (created.status, id)
}

async fn listed(server: &TestServer) -> Vec<Value> {
    let page = server.request(&get_request("/v1/jobs")).await.json();
    page["jobs"].as_array().expect("jobs").clone()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_resident_forgets_the_jobs_its_retention_no_longer_keeps() {
    let world = TestWorld::new();
    let backend = Arc::new(TestBackend::completes(ExecutionDisposition::Succeeded));
    let clock = Arc::new(ManualClock::new("2026-11-16T10:00:00Z[UTC]"));
    let limits = limits().with_job_retention(Duration::from_secs(3_600));
    let server = world
        .start_with_clock(backend.clone(), limits, clock.clone())
        .await;
    let (status, old) = admit(&server, "old").await;
    assert_eq!(status, 202);
    let replay = server
        .request(&post_request(BODY, "old", &auth_header()))
        .await;
    assert_eq!(replay.status, 200, "inside its window a key replays");

    clock.advance_to("2026-11-16T11:00:01Z[UTC]");
    let read = server
        .request(&get_request(&format!("/v1/jobs/{old}")))
        .await;
    assert_eq!(read.status, 200, "a read evicts nothing");

    let (status, new) = admit(&server, "new").await;
    assert_eq!(status, 202);
    let gone = server
        .request(&get_request(&format!("/v1/jobs/{old}")))
        .await;
    assert_eq!(gone.status, 404, "{}", gone.body);
    assert_eq!(gone.json()["error"]["code"], "job_not_found");
    let jobs = listed(&server).await;
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0]["id"], new.as_str());

    // The evicted job's key is forgotten: the same request is a new job.
    let (status, again) = admit(&server, "old").await;
    assert_eq!(status, 202, "an evicted key creates a job");
    assert_ne!(again, old);
    server.stop().await.expect("stop");

    // A start evicts too, before any admission.
    clock.advance_to("2026-11-16T13:00:00Z[UTC]");
    let restarted = world.start_with_clock(backend, limits, clock).await;
    assert_eq!(listed(&restarted).await, Vec::<Value>::new());
    for id in [&new, &again] {
        let read = restarted
            .request(&get_request(&format!("/v1/jobs/{id}")))
            .await;
        assert_eq!(read.status, 404);
    }
    restarted.stop().await.expect("stop");
    let state: Value = serde_json::from_str(
        &std::fs::read_to_string(world.state.join("jobs/state.json")).expect("job state"),
    )
    .expect("job state json");
    assert_eq!(state["jobs"], json!([]));
    assert_eq!(state["next_ordinal"], 4, "ranks are never given twice");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_retention_window_under_a_minute_refuses_the_start() {
    let world = TestWorld::new();
    for short in [
        limits().with_job_retention(Duration::from_secs(59)),
        limits().with_unsuccessful_job_retention(Duration::ZERO),
    ] {
        let backend = Arc::new(TestBackend::completes(ExecutionDisposition::Succeeded));
        let config = ResidentConfig::new(&world.state).with_limits(short);
        assert!(matches!(
            ResidentAuthority::open(config, backend).await,
            Err(ServerError::InvalidConfig(message)) if message.contains("one minute")
        ));
    }
    assert!(
        !world.state.join("jobs").exists(),
        "a refused start writes no state"
    );
}
