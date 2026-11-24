// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! `GET /v1/jobs` over HTTP (ADR-155).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::{Value, json};

use super::tests::{
    TestServer, TestWorld, WORKFLOW, auth_header, get_request, limits, post_request,
    wait_for_status,
};
use super::{ExecutionBackend, ExecutionDisposition, ExecutionOutcome};

/// Fails the workflow named `fails.nika` and completes every other one.
struct ByWorkflow;

impl ExecutionBackend for ByWorkflow {
    fn execute<'a>(
        &'a self,
        context: nika_execution::ExecutionContext<'a>,
    ) -> Pin<Box<dyn Future<Output = ExecutionOutcome> + Send + 'a>> {
        Box::pin(async move {
            if context.snapshot().root() == "fails.nika" {
                ExecutionOutcome::failed("NIKA-1704", "the run stopped at its cost ceiling")
            } else {
                ExecutionDisposition::Succeeded.into()
            }
        })
    }
}

fn world() -> TestWorld {
    let world = TestWorld::new();
    std::fs::write(world.workflows.join("fails.nika"), WORKFLOW).expect("workflow");
    world
}

/// Admit one job and wait until it reaches `settles`.
async fn admit(server: &TestServer, key: &str, body: &Value, settles: &str) -> String {
    let created = server
        .request(&post_request(&body.to_string(), key, &auth_header()))
        .await;
    assert_eq!(created.status, 202, "{}", created.body);
    let id = created.json()["id"].as_str().expect("job id").to_owned();
    wait_for_status(server, &id, settles)
        .await
        .expect("the job settles");
    id
}

async fn page(server: &TestServer, query: &str) -> Value {
    let response = server
        .request(&get_request(&format!("/v1/jobs{query}")))
        .await;
    assert_eq!(response.status, 200, "{query}: {}", response.body);
    response.json()
}

fn ids(page: &Value) -> Vec<String> {
    page["jobs"]
        .as_array()
        .expect("jobs")
        .iter()
        .map(|job| job["id"].as_str().expect("id").to_owned())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_listing_is_newest_first_and_its_filters_combine() {
    let world = world();
    let server = world.start(Arc::new(ByWorkflow), limits()).await;
    let pro = admit(
        &server,
        "pro",
        &json!({"workflow": "root.nika", "owner": "user_123",
                "labels": {"app": "translation", "plan": "pro"}}),
        "succeeded",
    )
    .await;
    let failed = admit(
        &server,
        "failed",
        &json!({"workflow": "fails.nika", "owner": "user_123",
                "labels": {"app": "translation"}}),
        "failed",
    )
    .await;
    let free = admit(
        &server,
        "free",
        &json!({"workflow": "root.nika", "owner": "user_456",
                "labels": {"app": "translation", "plan": "free"}}),
        "succeeded",
    )
    .await;
    let anonymous = admit(
        &server,
        "anonymous",
        &json!({"workflow": "root.nika"}),
        "succeeded",
    )
    .await;

    let everything = page(&server, "").await;
    assert_eq!(
        ids(&everything),
        [&anonymous, &free, &failed, &pro].map(Clone::clone)
    );
    assert!(
        everything.get("next_cursor").is_none(),
        "one page holds all"
    );

    for (query, expected) in [
        ("?owner=user_123", vec![&failed, &pro]),
        ("?owner=user_123&status=failed", vec![&failed]),
        ("?status=failed,succeeded&label=plan:pro", vec![&pro]),
        ("?status=failed&status=queued", vec![&failed]),
        ("?label=app:translation&label=plan:free", vec![&free]),
        (
            "?label=app:translation&owner=user_456&status=failed",
            vec![],
        ),
        ("?workflow=fails.nika", vec![&failed]),
        ("?workflow=root.nika&owner=user_123", vec![&pro]),
        (
            "?origin=manual&limit=200",
            vec![&anonymous, &free, &failed, &pro],
        ),
        ("?origin=schedule", vec![]),
        ("?owner=nobody", vec![]),
        (
            "?created_after=2020-01-01T00:00:00Z&owner=user_456",
            vec![&free],
        ),
        ("?created_after=2999-01-01T00:00:00Z", vec![]),
        ("?created_before=2020-01-01T00:00:00Z", vec![]),
    ] {
        let expected: Vec<String> = expected.into_iter().cloned().collect();
        assert_eq!(ids(&page(&server, query).await), expected, "{query}");
    }
    server.stop().await.expect("stop");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_summary_is_the_light_projection_with_the_job_s_own_redacted_error() {
    let world = world();
    let server = world.start(Arc::new(ByWorkflow), limits()).await;
    let failed = admit(
        &server,
        "failed",
        &json!({"workflow": "fails.nika", "owner": "user_123",
                "labels": {"app": "translation"}}),
        "failed",
    )
    .await;
    let plain = admit(
        &server,
        "plain",
        &json!({"workflow": "root.nika"}),
        "succeeded",
    )
    .await;

    let listed = page(&server, "").await;
    let jobs = listed["jobs"].as_array().expect("jobs");
    let summary = jobs[1].as_object().expect("summary");
    let job = server
        .request(&get_request(&format!("/v1/jobs/{failed}")))
        .await
        .json();
    assert_eq!(summary["id"], failed.as_str());
    assert_eq!(summary["status"], "failed");
    assert_eq!(summary["workflow"], "fails.nika");
    assert_eq!(summary["origin"], json!({"kind": "manual"}));
    assert_eq!(summary["owner"], "user_123");
    assert_eq!(summary["labels"], json!({"app": "translation"}));
    assert_eq!(summary["execution_id"], job["execution_id"]);
    assert_eq!(summary["trace_id"], job["trace_id"]);
    assert_eq!(summary["error"], job["error"]);
    assert_eq!(
        summary["error"],
        json!({"code": "NIKA-1704", "message": "the run stopped at its cost ceiling"})
    );
    let created = summary["created_at"].as_str().expect("admission date");
    assert!(created.parse::<jiff::Timestamp>().is_ok(), "{created}");
    let mut members: Vec<&str> = summary.keys().map(String::as_str).collect();
    members.sort_unstable();
    assert_eq!(
        members,
        [
            "created_at",
            "error",
            "execution_id",
            "id",
            "labels",
            "origin",
            "owner",
            "status",
            "trace_id",
            "workflow"
        ],
        "a summary never carries inputs, outputs, a receipt or a settlement"
    );

    let unattributed = jobs[0].as_object().expect("summary");
    assert_eq!(unattributed["id"], plain.as_str());
    assert_eq!(unattributed["labels"], json!({}));
    assert!(!unattributed.contains_key("owner"));
    assert!(!unattributed.contains_key("error"));
    server.stop().await.expect("stop");
}

#[tokio::test(flavor = "multi_thread")]
async fn pages_stay_stable_while_jobs_are_admitted() {
    let world = world();
    let server = world.start(Arc::new(ByWorkflow), limits()).await;
    let mut admitted = Vec::new();
    for index in 0..5 {
        let owner = if index % 2 == 0 { "even" } else { "odd" };
        let body = json!({"workflow": "root.nika", "owner": owner});
        admitted.push(admit(&server, &format!("first-{index}"), &body, "succeeded").await);
    }

    let first = page(&server, "?limit=2").await;
    assert_eq!(ids(&first), [&admitted[4], &admitted[3]].map(Clone::clone));
    let cursor = first["next_cursor"].as_str().expect("an older job exists");

    // Jobs admitted while the caller pages are newer than every page to come.
    let mut late = Vec::new();
    for index in 0..2 {
        let body = json!({"workflow": "root.nika", "owner": "even"});
        late.push(admit(&server, &format!("late-{index}"), &body, "succeeded").await);
    }

    let second = page(&server, &format!("?limit=2&cursor={cursor}")).await;
    assert_eq!(ids(&second), [&admitted[2], &admitted[1]].map(Clone::clone));
    let cursor = second["next_cursor"].as_str().expect("an older job exists");
    let third = page(&server, &format!("?limit=2&cursor={cursor}")).await;
    assert_eq!(ids(&third), [admitted[0].clone()]);
    assert!(third.get("next_cursor").is_none(), "the last page ends");

    // A page that ends exactly on the oldest match promises no further page.
    let exact = page(&server, &format!("?limit=1&cursor={cursor}")).await;
    assert_eq!(ids(&exact), [admitted[0].clone()]);
    assert!(exact.get("next_cursor").is_none());

    // The cursor names a rank, so it pages a filtered list the same way.
    let even = page(&server, "?owner=even&limit=2").await;
    assert_eq!(ids(&even), [&late[1], &late[0]].map(Clone::clone));
    let cursor = even["next_cursor"].as_str().expect("older even jobs");
    let older = page(&server, &format!("?owner=even&limit=2&cursor={cursor}")).await;
    assert_eq!(ids(&older), [&admitted[4], &admitted[2]].map(Clone::clone));
    let cursor = older["next_cursor"].as_str().expect("one older even job");
    let oldest = page(&server, &format!("?owner=even&limit=2&cursor={cursor}")).await;
    assert_eq!(ids(&oldest), [admitted[0].clone()]);
    assert!(oldest.get("next_cursor").is_none());

    let fresh = page(&server, "?limit=2").await;
    assert_eq!(ids(&fresh), [&late[1], &late[0]].map(Clone::clone));
    server.stop().await.expect("stop");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_query_outside_its_contract_is_refused_and_the_list_needs_the_token() {
    let world = world();
    let server = world.start(Arc::new(ByWorkflow), limits()).await;
    for (query, code) in [
        ("?page=2", "invalid_job_query"),
        ("?owner=user%20123", "invalid_job_query"),
        ("?status=done", "invalid_job_query"),
        ("?label=app", "invalid_job_query"),
        ("?limit=201", "invalid_job_query"),
        ("?created_after=yesterday", "invalid_job_query"),
        ("?cursor=42", "invalid_job_cursor"),
        ("?cursor=c1.0", "invalid_job_cursor"),
    ] {
        let refused = server
            .request(&get_request(&format!("/v1/jobs{query}")))
            .await;
        assert_eq!(refused.status, 400, "{query}: {}", refused.body);
        assert_eq!(refused.json()["error"]["code"], code, "{query}");
    }
    let anonymous = server
        .request("GET /v1/jobs HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await;
    assert_eq!(anonymous.status, 401, "{}", anonymous.body);
    let empty = page(&server, "").await;
    assert_eq!(empty, json!({"jobs": []}));
    let health = server.request(&get_request("/health")).await.json();
    assert!(
        health["supportedCapabilities"]
            .as_array()
            .expect("capabilities")
            .contains(&json!("jobList"))
    );
    server.stop().await.expect("stop");
}
