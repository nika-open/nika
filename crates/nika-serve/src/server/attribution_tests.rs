// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! `owner` and `labels` on the by-name form of `POST /v1/jobs` (ADR-155).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};

use super::tests::{
    TestBackend, TestWorld, auth_header, get_request, limits, post_request, snapshot_body,
    wait_for_status,
};
use super::{ExecutionDisposition, ServerLimits};

fn stored_jobs(world: &TestWorld) -> Vec<Value> {
    let text = std::fs::read_to_string(world.state.join("jobs/state.json")).expect("job state");
    let state: Value = serde_json::from_str(&text).expect("job state json");
    state["jobs"].as_array().expect("jobs").clone()
}

fn named(extra: &Value) -> String {
    let mut body = json!({"workflow": "root.nika"});
    for (key, value) in extra.as_object().expect("members") {
        body[key] = value.clone();
    }
    body.to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_named_job_keeps_the_owner_and_labels_its_caller_states() {
    let world = TestWorld::new();
    let backend = Arc::new(TestBackend::completes(ExecutionDisposition::Succeeded));
    let server = world.start(backend.clone(), limits()).await;
    let stated = named(&json!({
        "owner": "user_123",
        "labels": {"app": "translation", "plan": "pro"},
    }));

    let created = server
        .request(&post_request(&stated, "owned-1", &auth_header()))
        .await;
    assert_eq!(created.status, 202, "{}", created.body);
    let job = created.json();
    let id = job["id"].as_str().expect("job id").to_owned();
    for member in ["owner", "labels", "created_at"] {
        assert!(
            job.get(member).is_none(),
            "the published Job object stays closed: {member} travels on the job list"
        );
    }
    wait_for_status(&server, &id, "succeeded")
        .await
        .expect("settled");

    let stored = stored_jobs(&world);
    assert_eq!(stored.len(), 1);
    let record = &stored[0]["record"];
    assert_eq!(record["owner"], json!("user_123"));
    assert_eq!(
        record["labels"],
        json!({"app": "translation", "plan": "pro"})
    );
    assert!(record["created_at"].is_string());

    let replayed = server
        .request(&post_request(&stated, "owned-1", &auth_header()))
        .await;
    assert_eq!(replayed.status, 200, "{}", replayed.body);
    assert_eq!(replayed.json()["id"], json!(id));

    // The request digest is the digest of the exact body, so the owner is in it.
    let other_owner = named(&json!({
        "owner": "user_456",
        "labels": {"app": "translation", "plan": "pro"},
    }));
    let conflict = server
        .request(&post_request(&other_owner, "owned-1", &auth_header()))
        .await;
    assert_eq!(conflict.status, 409, "{}", conflict.body);
    assert_eq!(conflict.json()["error"]["code"], "idempotency_conflict");
    assert_eq!(stored_jobs(&world).len(), 1);
    assert_eq!(backend.calls(), 1, "one admission ran, once");
    server.stop().await.expect("stop");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unattributed_job_stores_no_attribution_member() {
    let world = TestWorld::new();
    let backend = Arc::new(TestBackend::completes(ExecutionDisposition::Succeeded));
    let server = world.start(backend, limits()).await;
    let created = server
        .request(&post_request(&named(&json!({})), "plain-1", &auth_header()))
        .await;
    assert_eq!(created.status, 202, "{}", created.body);
    let id = created.json()["id"].as_str().expect("id").to_owned();
    wait_for_status(&server, &id, "succeeded")
        .await
        .expect("settled");
    let record = stored_jobs(&world)[0]["record"].clone();
    assert!(record.get("owner").is_none() && record.get("labels").is_none());
    assert_eq!(record["ordinal"], json!(1));
    server.stop().await.expect("stop");
}

#[tokio::test(flavor = "multi_thread")]
async fn attribution_outside_its_bounds_is_refused_before_a_job_exists() {
    let world = TestWorld::new();
    let backend = Arc::new(TestBackend::completes(ExecutionDisposition::Succeeded));
    // Sixteen labels at their widest are about 4.5 KiB of body.
    let roomy = ServerLimits::new(
        16 * 1024,
        Duration::from_secs(2),
        Duration::from_secs(2),
        Duration::from_secs(10),
        4,
        16,
        64,
        32,
    );
    let server = world.start(backend.clone(), roomy).await;
    let seventeen = (0..17)
        .map(|index| (format!("k{index}"), json!("v")))
        .collect::<serde_json::Map<_, _>>();
    let cases = [
        ("invalid_job_owner", json!({"owner": ""})),
        ("invalid_job_owner", json!({"owner": "user 123"})),
        ("invalid_job_owner", json!({"owner": "user/123"})),
        ("invalid_job_owner", json!({"owner": "a".repeat(129)})),
        ("invalid_job_owner", json!({"owner": 7})),
        ("invalid_job_owner", json!({"owner": null})),
        ("invalid_job_owner", json!({"owner": ["user_123"]})),
        ("invalid_job_labels", json!({"labels": seventeen})),
        ("invalid_job_labels", json!({"labels": {"app:eu": "x"}})),
        ("invalid_job_labels", json!({"labels": {"": "x"}})),
        (
            "invalid_job_labels",
            json!({"labels": {"k".repeat(65): "x"}}),
        ),
        (
            "invalid_job_labels",
            json!({"labels": {"app": "v".repeat(257)}}),
        ),
        (
            "invalid_job_labels",
            json!({"labels": {"app": "line\nbreak"}}),
        ),
        ("invalid_job_labels", json!({"labels": {"app": 7}})),
        ("invalid_job_labels", json!({"labels": {"app": null}})),
        ("invalid_job_labels", json!({"labels": null})),
        ("invalid_job_labels", json!({"labels": ["app"]})),
    ];
    for (index, (code, members)) in cases.iter().enumerate() {
        let refused = server
            .request(&post_request(
                &named(members),
                &format!("refused-{index}"),
                &auth_header(),
            ))
            .await;
        assert_eq!(refused.status, 422, "{members}: {}", refused.body);
        assert_eq!(refused.json()["error"]["code"], *code, "{members}");
    }
    assert!(stored_jobs(&world).is_empty(), "no refusal created a job");
    assert_eq!(backend.calls(), 0, "no refusal executed");

    // The bounds themselves are admitted.
    let sixteen = (0..16)
        .map(|index| (format!("k{index}"), json!("v".repeat(256))))
        .collect::<serde_json::Map<_, _>>();
    let widest = named(&json!({"owner": "a".repeat(128), "labels": sixteen}));
    let admitted = server
        .request(&post_request(&widest, "widest", &auth_header()))
        .await;
    assert_eq!(admitted.status, 202, "{}", admitted.body);
    server.stop().await.expect("stop");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_frozen_world_carries_no_attribution() {
    let world = TestWorld::new();
    let backend = Arc::new(TestBackend::completes(ExecutionDisposition::Succeeded));
    let server = world.start(backend.clone(), limits()).await;
    let mut snapshot: Value =
        serde_json::from_str(&snapshot_body(super::tests::WORKFLOW)).expect("snapshot body");
    snapshot["owner"] = json!("user_123");
    let refused = server
        .request(&post_request(
            &snapshot.to_string(),
            "frozen-owned",
            &auth_header(),
        ))
        .await;
    assert_eq!(refused.status, 422, "{}", refused.body);
    assert_eq!(refused.json()["error"]["code"], "malformed_snapshot");
    assert_eq!(backend.calls(), 0);

    let health = server
        .request("GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await;
    assert!(
        health.json()["supportedCapabilities"]
            .as_array()
            .expect("capabilities")
            .contains(&json!("jobAttribution")),
        "a client reads the token before it states an owner"
    );
    let job = server.request(&get_request("/v1/jobs/unknown")).await;
    assert_eq!(job.status, 404);
    server.stop().await.expect("stop");
}
