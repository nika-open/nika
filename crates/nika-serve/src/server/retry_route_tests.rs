// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! `POST /v1/jobs/{id}/retry` (ADR-156 section 9): a request starts the
//! latest attempt of a settled job again, under the bound, the ceiling and
//! the resume judgment a lost run is held to.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};

use super::retry_tests::{
    TWO_TASKS, dropping, field, journal_lines, journal_of, kinds_and_statuses, named, names_task,
    pair, running_job, stored_jobs,
};
use super::tests::{
    TestBackend, TestServer, TestWorld, auth_header, events_request, get_request, limits,
    parse_sse_data, post_request, wait_for_status,
};
use super::{ExecutionDisposition, ServerLimits};
use crate::job::{OnInterrupted, RetryPolicy};

fn retry_request(id: &str, body: &str) -> String {
    format!(
        "POST /v1/jobs/{id}/retry HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{}\r\n{body}",
        body.len(),
        auth_header()
    )
}

async fn job(server: &TestServer, id: &str) -> Value {
    server
        .request(&get_request(&format!("/v1/jobs/{id}")))
        .await
        .json()
}

/// A job of the test world that ran once and ended `status`.
async fn settled_job(server: &TestServer, key: &str, status: &str) -> (String, Value) {
    let created = server
        .request(&post_request(&named(&json!({})), key, &auth_header()))
        .await;
    assert_eq!(created.status, 202, "{}", created.body);
    let id = created.json()["id"].as_str().expect("job id").to_owned();
    wait_for_status(server, &id, status).await.expect(status);
    let settled = job(server, &id).await;
    (id, settled)
}

/// Wait until the backend ran `calls` times and the job settled again.
async fn settles_after(
    server: &TestServer,
    backend: &TestBackend,
    id: &str,
    calls: usize,
) -> Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let settled = |seen: &Value| {
        backend.calls() >= calls && seen["status"] != "queued" && seen["status"] != "running"
    };
    let mut seen = job(server, id).await;
    while !settled(&seen) && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(5)).await;
        seen = job(server, id).await;
    }
    assert!(
        settled(&seen),
        "job {id} did not settle after {calls} runs: {seen}"
    );
    seen
}

fn events_of(world: &TestWorld) -> usize {
    stored_jobs(world)[0]["events"]
        .as_array()
        .expect("events")
        .len()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_job_is_started_again_by_request_as_a_new_attempt_of_the_same_job() {
    let world = TestWorld::new();
    let backend = Arc::new(TestBackend::completes(ExecutionDisposition::Failed));
    let server = world.start(backend.clone(), limits()).await;
    let (id, first) = settled_job(&server, "failed-1", "failed").await;

    // The attempt left no journal: under the resident's ceiling, the request
    // states what the re-attempts alone may spend.
    let ask = json!({"attempt": 1, "mode": "restart", "max_cost_usd": 0.5}).to_string();
    let retried = server.request(&retry_request(&id, &ask)).await;
    assert_eq!(retried.status, 202, "{}", retried.body);
    assert_eq!(retried.json()["id"], id.as_str());
    assert_eq!(retried.json()["status"], "queued", "{}", retried.body);
    let second = settles_after(&server, &backend, &id, 2).await;
    assert_eq!(second["status"], "failed", "{second}");
    assert_ne!(second["execution_id"], first["execution_id"]);
    assert_eq!(second["receipt"]["execution_id"], second["execution_id"]);

    // The same request again finds the attempt it created: nothing is
    // written and nothing runs.
    let before = events_of(&world);
    let replay = server.request(&retry_request(&id, &ask)).await;
    assert_eq!(replay.status, 200, "{}", replay.body);
    assert_eq!(replay.json()["execution_id"], second["execution_id"]);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(backend.calls(), 2, "a replay runs nothing");
    assert_eq!(events_of(&world), before, "a replay writes nothing");
    // Another request for the attempt already retried is not a replay.
    let other = json!({"attempt": 1, "mode": "restart", "max_cost_usd": 1}).to_string();
    let refused = server.request(&retry_request(&id, &other)).await;
    assert_eq!(refused.status, 409, "{}", refused.body);
    assert_eq!(refused.json()["error"]["code"], "job_not_retryable");

    // The stream shows both attempts and why the second one exists.
    let frames = parse_sse_data(&server.request(&events_request(&id, None)).await.body);
    assert_eq!(
        kinds_and_statuses(&frames),
        [
            pair("execution.started", "running"),
            pair("execution.settled", "failed"),
            pair("execution.requeued", "queued"),
            pair("execution.started", "running"),
            pair("execution.settled", "failed"),
        ],
        "{frames:?}"
    );
    assert_eq!(
        frames[2]["message"],
        "attempt 2 of 3 (restart): attempt 1 retried by request"
    );

    // The first attempt is evidence: its identity, its receipt, its end and
    // the request that closed it.
    let stored = stored_jobs(&world);
    let attempt = &stored[0]["record"]["attempts"][0];
    assert_eq!(attempt["status"], "failed", "{attempt}");
    assert_eq!(attempt["execution_id"], first["execution_id"]);
    assert_eq!(attempt["receipt"]["execution_id"], first["execution_id"]);
    assert_eq!(attempt["terminal_sequence"], 2, "{attempt}");
    assert_eq!(attempt["requeued_sequence"], 3, "{attempt}");
    assert_eq!(attempt["next"], "restart");
    assert_eq!(attempt["requested"], json!({"max_cost_usd": 0.5}));

    // A store holding a retried attempt reads back whole: its hash chain
    // binds the first attempt's end through the attempt it closed.
    server.stop().await.expect("a clean stop");
    let server = world.start(backend.clone(), limits()).await;
    let reread = job(&server, &id).await;
    assert_eq!(reread["status"], "failed", "{reread}");
    assert_eq!(reread["execution_id"], second["execution_id"]);
    server.stop().await.expect("a clean stop");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_retry_request_names_the_latest_settled_attempt_and_nothing_else() {
    let world = TestWorld::new();
    let failing = Arc::new(TestBackend::completes(ExecutionDisposition::Failed));
    let server = world.start(failing.clone(), limits()).await;
    let (id, _) = settled_job(&server, "failed-2", "failed").await;
    let before = events_of(&world);
    for body in [
        "",
        "not json",
        "[]",
        "{}",
        r#"{"attempt": 0, "mode": "restart"}"#,
        r#"{"attempt": 1, "mode": "Restart"}"#,
        r#"{"attempt": 1, "mode": "off"}"#,
        r#"{"attempt": 1, "mode": "restart", "max_cost_usd": -1}"#,
        r#"{"attempt": 1, "mode": "restart", "max_cost_usd": null}"#,
        r#"{"attempt": 1, "mode": "restart", "max_attempts": 2}"#,
    ] {
        let refused = server.request(&retry_request(&id, body)).await;
        assert_eq!(refused.status, 422, "{body:?}: {}", refused.body);
        assert_eq!(refused.json()["error"]["code"], "invalid_job_retry");
    }
    let ask = json!({"attempt": 1, "mode": "restart"}).to_string();
    for unknown in ["nope", "1b4e28ba-2fa1-41d2-883f-0016d3cca427"] {
        let missing = server.request(&retry_request(unknown, &ask)).await;
        assert_eq!(missing.status, 404, "{unknown}: {}", missing.body);
        assert_eq!(missing.json()["error"]["code"], "job_not_found");
    }
    // Only the latest attempt is named.
    let later = json!({"attempt": 2, "mode": "restart"}).to_string();
    let refused = server.request(&retry_request(&id, &later)).await;
    assert_eq!(refused.status, 409, "{}", refused.body);
    assert_eq!(refused.json()["error"]["code"], "job_not_retryable");
    // An attempt that left no journal cannot be resumed; the resident never
    // runs it from its first task instead.
    let resume = json!({"attempt": 1, "mode": "resume"}).to_string();
    let refused = server.request(&retry_request(&id, &resume)).await;
    assert_eq!(refused.status, 409, "{}", refused.body);
    assert_eq!(refused.json()["error"]["code"], "resume_refused");
    assert_eq!(events_of(&world), before, "a refusal writes nothing");
    assert_eq!(failing.calls(), 1, "a refusal runs nothing");
    server.stop().await.expect("a clean stop");

    // A job that succeeded is not retried.
    let world = TestWorld::new();
    let succeeding = Arc::new(TestBackend::completes(ExecutionDisposition::Succeeded));
    let server = world.start(succeeding.clone(), limits()).await;
    let (id, _) = settled_job(&server, "done-1", "succeeded").await;
    let refused = server.request(&retry_request(&id, &ask)).await;
    assert_eq!(refused.status, 409, "{}", refused.body);
    assert_eq!(refused.json()["error"]["code"], "job_not_retryable");
    server.stop().await.expect("a clean stop");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_request_is_held_to_the_bound_and_the_ceiling_of_a_lost_run() {
    let world = TestWorld::new();
    let backend = Arc::new(TestBackend::completes(ExecutionDisposition::Failed));
    let bounded: ServerLimits = limits()
        .with_retry_policy(RetryPolicy::new(OnInterrupted::Off, 2))
        .with_default_max_cost_usd(Some(1.0));
    let server = world.start(backend.clone(), bounded).await;
    let (id, _) = settled_job(&server, "ceiling-1", "failed").await;

    // The attempt left no journal: what it spent is unknown, and under a
    // ceiling an unknown spend never counts for zero.
    let bare = json!({"attempt": 1, "mode": "restart"}).to_string();
    let refused = server.request(&retry_request(&id, &bare)).await;
    assert_eq!(refused.status, 409, "{}", refused.body);
    assert_eq!(refused.json()["error"]["code"], "unknown_spend");
    // A budget of the re-attempts alone bounds what the new one may spend.
    let budgeted = json!({"attempt": 1, "mode": "restart", "max_cost_usd": 0.5}).to_string();
    let retried = server.request(&retry_request(&id, &budgeted)).await;
    assert_eq!(retried.status, 202, "{}", retried.body);
    let second = settles_after(&server, &backend, &id, 2).await;
    assert_eq!(second["status"], "failed", "{second}");

    // Two attempts is the bound in force.
    let again = json!({"attempt": 2, "mode": "restart", "max_cost_usd": 0.5}).to_string();
    let refused = server.request(&retry_request(&id, &again)).await;
    assert_eq!(refused.status, 409, "{}", refused.body);
    assert_eq!(refused.json()["error"]["code"], "retry_exhausted");
    assert_eq!(backend.calls(), 2);
    server.stop().await.expect("a clean stop");
}

/// A job of [`TWO_TASKS`] that states no retry, lost by a resident stopped
/// past its grace once `first` completed: it stays `interrupted`.
async fn interrupted_after_its_first_task(world: &TestWorld, journals: &Path) -> (String, Value) {
    use crate::server::production::ResidentExecutionBackend;

    std::fs::write(world.workflows.join("two.nika"), TWO_TASKS).expect("workflow");
    let backend = Arc::new(ResidentExecutionBackend::new(&world.workflows));
    let server = world.start(backend, dropping(OnInterrupted::Off)).await;
    let body = json!({"workflow": "two.nika"}).to_string();
    let (id, first) = running_job(&server, &body, "interrupted-1").await;
    let mut seen = Vec::new();
    for _ in 0..500 {
        seen = journal_of(journals, &first).map_or_else(Vec::new, |path| journal_lines(&path));
        if names_task(&seen, "task_completed", "first") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        names_task(&seen, "task_completed", "first"),
        "the first task completed before the stop: {first} {seen:?}"
    );
    let _stopped = server.stop().await;
    let stored = stored_jobs(world);
    assert_eq!(stored[0]["record"]["status"], "interrupted", "{stored:?}");
    (id, first)
}

#[tokio::test(flavor = "multi_thread")]
async fn an_interrupted_job_resumes_by_request_from_its_journal() {
    use crate::server::production::ResidentExecutionBackend;

    let world = TestWorld::new();
    let journals = world.workflows.join(nika_dap::store::TRACE_DIR);
    let (id, first) = interrupted_after_its_first_task(&world, &journals).await;
    let backend = Arc::new(ResidentExecutionBackend::new(&world.workflows));
    let server = world.start(backend, dropping(OnInterrupted::Off)).await;
    let ask = json!({"attempt": 1, "mode": "resume"}).to_string();
    let retried = server.request(&retry_request(&id, &ask)).await;
    assert_eq!(retried.status, 202, "{}", retried.body);

    // The resumed attempt still waits out `second`: longer than the shared
    // five-second wait allows under a loaded test run.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let mut settled = Value::Null;
    while tokio::time::Instant::now() < deadline {
        settled = job(&server, &id).await;
        if settled["status"] != "running" && settled["status"] != "queued" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let lines = journal_of(&journals, &settled).map_or_else(Vec::new, |path| journal_lines(&path));
    assert_eq!(settled["status"], "succeeded", "{settled} {lines:?}");
    assert_ne!(settled["execution_id"], first["execution_id"]);
    let boot = lines
        .iter()
        .find(|line| line["kind"] == "workflow_started")
        .expect("the boot record");
    assert_eq!(
        field(boot, "resumed_from"),
        Some(&first["trace_id"]),
        "{boot}"
    );
    assert!(names_task(&lines, "task_cache_hit", "first"), "{lines:?}");
    assert!(
        !names_task(&lines, "task_completed", "first"),
        "served from the journal, never run again"
    );
    assert!(names_task(&lines, "task_completed", "second"), "{lines:?}");
    let stored = stored_jobs(&world);
    let attempt = &stored[0]["record"]["attempts"][0];
    assert_eq!(attempt["status"], "interrupted", "{attempt}");
    assert_eq!(attempt["next"], "resume", "{attempt}");
    server.stop().await.expect("a clean stop");
}
