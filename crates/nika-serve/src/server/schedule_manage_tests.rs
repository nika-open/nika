// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! Schedule management over HTTP (ADR-155): what a schedule states about its
//! jobs, the list, the removal and the manual trigger.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use super::schedule_tests::{ManualClock, NoopBackend, put_request};
use super::tests::{
    TestServer, TestWorld, auth_header, get_request, limits, post_request, wait_for_status,
};
use super::{ExecutionBackend, ExecutionDisposition, ExecutionOutcome};

/// A workflow that declares typed inputs: one required, one defaulted.
const INTAKE_WORKFLOW: &str = concat!(
    "nika: intake\n",
    "inputs:\n",
    "  tenant: { type: string, required: true }\n",
    "  limit: { type: integer, default: 5 }\n",
    "permits:\n",
    "  tools: [\"nika:jq\"]\n",
    "tasks:\n",
    "  value:\n",
    "    invoke:\n",
    "      tool: nika:jq\n",
    "      args: { input: { tenant: \"${{ inputs.tenant }}\", limit: \"${{ inputs.limit }}\" }, expression: \".\" }\n",
);

/// A project beat due at 08:01 UTC daily, named `root` after its workflow.
const DAILY_BEAT: &str = "nika: proj\narm:\n  - workflow: root.nika\n    cadence: \"TZ=UTC 1 8 * * *\"\n    plafond: 0.25\n    manqué: sauter\n";

/// The ceiling and literal inputs each run was given.
type Run = (Option<f64>, BTreeMap<String, Value>);

/// Completes every run and records what it ran under.
#[derive(Debug, Default)]
struct Recording {
    runs: Mutex<Vec<Run>>,
}

impl Recording {
    fn runs(&self) -> Vec<Run> {
        self.runs.lock().expect("recorded runs").clone()
    }
}

impl ExecutionBackend for Recording {
    fn execute<'a>(
        &'a self,
        _context: nika_execution::ExecutionContext<'a>,
    ) -> Pin<Box<dyn Future<Output = ExecutionOutcome> + Send + 'a>> {
        Box::pin(async { ExecutionDisposition::Succeeded.into() })
    }

    fn execute_with_inputs<'a>(
        &'a self,
        context: nika_execution::ExecutionContext<'a>,
        max_cost_usd: Option<f64>,
        _access_pin: Option<&str>,
        inputs: &BTreeMap<String, Value>,
        _cancel: nika_types::cancel::CancelCtx,
    ) -> Pin<Box<dyn Future<Output = ExecutionOutcome> + Send + 'a>> {
        self.runs
            .lock()
            .expect("record run")
            .push((max_cost_usd, inputs.clone()));
        self.execute(context)
    }
}

/// A once schedule at `at`, with the extra members merged in.
fn schedule(workflow: &str, at: &str, extra: &Value) -> String {
    let mut body = json!({
        "workflow": workflow,
        "when": {"kind": "once", "at": at},
        "maxCostUsd": 0.25,
        "missed": "skip"
    });
    for (member, value) in extra.as_object().expect("extra members") {
        body[member] = value.clone();
    }
    body.to_string()
}

async fn create(server: &TestServer, id: &str, body: &str) -> Value {
    let created = server
        .request(&put_request(id, body, "If-None-Match: *\r\n", true))
        .await;
    assert_eq!(created.status, 200, "{}", created.body);
    created.json()
}

fn delete_request(id: &str, precondition: &str) -> String {
    format!(
        "DELETE /v1/schedules/{id} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n{precondition}{}\r\n",
        auth_header()
    )
}

fn trigger_request(id: &str, key: Option<&str>, body: &str) -> String {
    let key = key.map_or_else(String::new, |key| format!("Idempotency-Key: {key}\r\n"));
    format!(
        "POST /v1/schedules/{id}/trigger HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: {}\r\n{key}{}\r\n{body}",
        body.len(),
        auth_header()
    )
}

async fn jobs(server: &TestServer, query: &str) -> Vec<Value> {
    let response = server
        .request(&get_request(&format!("/v1/jobs{query}")))
        .await;
    assert_eq!(response.status, 200, "{}", response.body);
    response.json()["jobs"].as_array().expect("jobs").clone()
}

async fn listed(server: &TestServer) -> Value {
    let response = server.request(&get_request("/v1/schedules")).await;
    assert_eq!(response.status, 200, "{}", response.body);
    response.json()
}

/// Set the manual clock to `now`, again. A scheduler that was mid-scan at
/// the previous strike sleeps on the generation it then read: striking in
/// the polling loop wakes it whenever it went back to sleep.
fn strike(clock: &ManualClock, now: &str) {
    clock.advance_to(now);
}

/// Poll until `probe` answers, bounded.
async fn eventually<T, F, Fut>(what: &str, mut probe: F) -> T
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Option<T>>,
{
    let found = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(found) = probe().await {
                break found;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    found.expect(what)
}

/// A schedule may state who its jobs are attributed to. The statement is
/// part of its revision, judged by the law of `POST /v1/jobs`, and carried
/// by every job the schedule fires.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_schedule_states_who_its_jobs_are_attributed_to() {
    let world = TestWorld::new();
    let clock = Arc::new(ManualClock::new("2026-09-01T08:00:00Z[UTC]"));
    let server = world
        .start_with_clock(Arc::new(NoopBackend), limits(), clock.clone())
        .await;
    let at = "2026-09-01T09:00:00Z";
    let plain = server
        .request(&put_request(
            "plain",
            &schedule("root.nika", at, &json!({})),
            "If-None-Match: *\r\n",
            true,
        ))
        .await;
    assert_eq!(plain.status, 200, "{}", plain.body);
    let definition = &plain.json()["status"]["definition"];
    assert_eq!(definition["owner"], Value::Null);
    assert_eq!(definition["labels"], json!({}));
    let plain_etag = plain.header("etag").expect("etag").to_owned();

    let stated = json!({"owner": "user_123", "labels": {"app": "translation", "plan": "pro"}});
    let owned = create(&server, "owned", &schedule("root.nika", at, &stated)).await;
    assert_eq!(owned["status"]["definition"]["owner"], "user_123");
    assert_eq!(
        owned["status"]["definition"]["labels"],
        json!({"app": "translation", "plan": "pro"})
    );

    // Stating an owner is a change of the declaration: a new revision.
    let restated = server
        .request(&put_request(
            "plain",
            &schedule("root.nika", at, &json!({"owner": "other_9"})),
            &format!("If-Match: {plain_etag}\r\n"),
            true,
        ))
        .await;
    assert_eq!(restated.status, 200, "{}", restated.body);
    assert_eq!(restated.json()["changed"], true);
    assert_ne!(restated.header("etag"), Some(plain_etag.as_str()));

    // Outside the law: the member is named, and nothing is stored.
    for (extra, code) in [
        (json!({"owner": "user 123"}), "schedule.owner"),
        (json!({"owner": ""}), "schedule.owner"),
        (json!({"labels": {"app:eu": "x"}}), "schedule.labels"),
        (json!({"labels": {"app": "a\nb"}}), "schedule.labels"),
    ] {
        let refused = server
            .request(&put_request(
                "refused",
                &schedule("root.nika", at, &extra),
                "If-None-Match: *\r\n",
                true,
            ))
            .await;
        assert_eq!(refused.status, 422, "{extra}: {}", refused.body);
        assert_eq!(refused.json()["findings"][0]["code"], code, "{extra}");
    }
    let mistyped = server
        .request(&put_request(
            "refused",
            &schedule("root.nika", at, &json!({"labels": {"app": 1}})),
            "If-None-Match: *\r\n",
            true,
        ))
        .await;
    assert_eq!(mistyped.status, 422, "{}", mistyped.body);
    assert_eq!(mistyped.json()["error"]["code"], "schedule.body");
    assert_eq!(
        server
            .request(&get_request("/v1/schedules/refused"))
            .await
            .status,
        404
    );

    // Both schedules fire at 09:00; each job carries what its schedule states.
    let fired = eventually("both scheduled jobs", || async {
        strike(&clock, "2026-09-01T09:00:00Z[UTC]");
        let fired = jobs(&server, "?origin=schedule").await;
        (fired.len() == 2).then_some(fired)
    })
    .await;
    assert!(fired.iter().all(|job| job["origin"]["kind"] == "schedule"));
    let mine = jobs(&server, "?owner=user_123&label=app:translation").await;
    assert_eq!(mine.len(), 1, "{mine:?}");
    assert_eq!(mine[0]["origin"]["schedule_id"], "owned");
    assert_eq!(
        mine[0]["labels"],
        json!({"app": "translation", "plan": "pro"})
    );
    let other = jobs(&server, "?owner=other_9").await;
    assert_eq!(other.len(), 1, "{other:?}");
    assert_eq!(other[0]["origin"]["schedule_id"], "plain");
    assert_eq!(other[0]["labels"], json!({}));
    server.stop().await.expect("stop");
}

/// The list shows the schedules this door created, by id, then the
/// project's beats, each as its own status with one next slot.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_list_shows_api_schedules_then_project_beats() {
    let world = TestWorld::new();
    std::fs::write(world.workflows.join("nika.yaml"), DAILY_BEAT).expect("project nika.yaml");
    let clock = Arc::new(ManualClock::new("2026-09-01T08:00:30Z[UTC]"));
    let server = world
        .start_with_clock(Arc::new(NoopBackend), limits(), clock.clone())
        .await;
    clock.wait_for_sleeps(1).await;
    let only_project = listed(&server).await;
    assert_eq!(only_project["truncated"], false);
    let beats = only_project["schedules"].as_array().expect("schedules");
    assert_eq!(beats.len(), 1, "{only_project}");
    assert_eq!(beats[0]["origin"], "project");
    assert_eq!(beats[0]["definition"]["id"], "root");
    assert_eq!(beats[0]["definition"]["owner"], Value::Null);
    assert_eq!(beats[0]["next"].as_array().expect("next").len(), 1);

    let at = "2026-09-02T09:00:00Z";
    create(&server, "zeta", &schedule("root.nika", at, &json!({}))).await;
    let alpha = create(
        &server,
        "alpha",
        &schedule("root.nika", at, &json!({"owner": "user_123"})),
    )
    .await;
    let all = listed(&server).await;
    let shown: Vec<(&str, &str)> = all["schedules"]
        .as_array()
        .expect("schedules")
        .iter()
        .map(|status| {
            (
                status["origin"].as_str().expect("origin"),
                status["definition"]["id"].as_str().expect("id"),
            )
        })
        .collect();
    assert_eq!(
        shown,
        [("api", "alpha"), ("api", "zeta"), ("project", "root")]
    );
    let first = &all["schedules"][0];
    assert_eq!(first["revision"], alpha["status"]["revision"]);
    assert_eq!(first["definition"], alpha["status"]["definition"]);
    assert_eq!(first["definition"]["owner"], "user_123");
    assert_eq!(first["active"], true);
    assert_eq!(first["lastDecision"], Value::Null);
    assert_eq!(first["next"].as_array().expect("next").len(), 1);
    assert_eq!(first["next"], json!([alpha["status"]["next"][0]]));

    let anonymous = server
        .request("GET /v1/schedules HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await;
    assert_eq!(anonymous.status, 401);
    let health = server
        .request("GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await;
    assert!(
        health.json()["supportedCapabilities"]
            .as_array()
            .expect("capabilities")
            .contains(&json!("scheduleManage"))
    );
    server.stop().await.expect("stop");
}

/// A removal needs `If-Match`, changes nothing under a stale revision, and
/// leaves a project beat to its file.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_schedule_is_removed_under_if_match() {
    let world = TestWorld::new();
    std::fs::write(world.workflows.join("nika.yaml"), DAILY_BEAT).expect("project nika.yaml");
    let clock = Arc::new(ManualClock::new("2026-09-01T08:00:30Z[UTC]"));
    let server = world
        .start_with_clock(Arc::new(NoopBackend), limits(), clock.clone())
        .await;
    clock.wait_for_sleeps(1).await;
    let at = "2026-09-02T09:00:00Z";
    let first = create(&server, "first", &schedule("root.nika", at, &json!({}))).await;
    let second = create(&server, "second", &schedule("root.nika", at, &json!({}))).await;
    let revision = first["status"]["revision"].as_str().expect("revision");
    let other = second["status"]["revision"].as_str().expect("revision");

    let unconditional = server.request(&delete_request("first", "")).await;
    assert_eq!(unconditional.status, 412, "{}", unconditional.body);
    assert_eq!(
        unconditional.json()["error"]["code"],
        "schedule_precondition_failed"
    );
    assert_eq!(
        unconditional.json()["error"]["currentRevision"],
        Value::Null
    );
    let stale = server
        .request(&delete_request(
            "first",
            &format!("If-Match: \"{other}\"\r\n"),
        ))
        .await;
    assert_eq!(stale.status, 412, "{}", stale.body);
    assert_eq!(stale.json()["error"]["currentRevision"], revision);
    assert_eq!(
        stale.header("etag"),
        Some(format!("\"{revision}\"").as_str())
    );
    assert_eq!(
        server
            .request(&get_request("/v1/schedules/first"))
            .await
            .status,
        200,
        "a refused removal changes nothing"
    );

    let removed = server
        .request(&delete_request(
            "first",
            &format!("If-Match: \"{revision}\"\r\n"),
        ))
        .await;
    assert_eq!(removed.status, 200, "{}", removed.body);
    assert_eq!(
        removed.json(),
        json!({"removed": true, "id": "first", "revision": revision})
    );
    assert_eq!(
        server
            .request(&get_request("/v1/schedules/first"))
            .await
            .status,
        404
    );
    let again = server
        .request(&delete_request("first", "If-Match: *\r\n"))
        .await;
    assert_eq!(again.status, 404, "{}", again.body);
    assert_eq!(again.json()["error"]["code"], "schedule_not_found");
    let any = server
        .request(&delete_request("second", "If-Match: *\r\n"))
        .await;
    assert_eq!(any.status, 200, "{}", any.body);

    // The project's beat is declared in its file: not this door's to remove.
    let project = server
        .request(&delete_request("root", "If-Match: *\r\n"))
        .await;
    assert_eq!(project.status, 409, "{}", project.body);
    assert_eq!(
        project.json()["error"]["code"],
        "schedule_declared_by_project"
    );
    let left = listed(&server).await;
    let left = left["schedules"].as_array().expect("schedules");
    assert_eq!(left.len(), 1, "{left:?}");
    assert_eq!(left[0]["origin"], "project");

    let anonymous = server
        .request("DELETE /v1/schedules/second HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nIf-Match: *\r\n\r\n")
        .await;
    assert_eq!(anonymous.status, 401);
    server.stop().await.expect("stop");
}

/// A removed schedule never fires, and a restart does not bring it back.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_removed_schedule_never_fires() {
    let world = TestWorld::new();
    let backend = Arc::new(Recording::default());
    let clock = Arc::new(ManualClock::new("2026-09-01T08:00:00Z[UTC]"));
    let server = world
        .start_with_clock(backend.clone(), limits(), clock.clone())
        .await;
    let at = "2026-09-01T09:00:00Z";
    create(&server, "gone", &schedule("root.nika", at, &json!({}))).await;
    create(&server, "stays", &schedule("root.nika", at, &json!({}))).await;
    let removed = server
        .request(&delete_request("gone", "If-Match: *\r\n"))
        .await;
    assert_eq!(removed.status, 200, "{}", removed.body);
    let fired = eventually("the remaining schedule fires", || async {
        strike(&clock, "2026-09-01T09:00:00Z[UTC]");
        let fired = jobs(&server, "?origin=schedule").await;
        (!fired.is_empty()).then_some(fired)
    })
    .await;
    assert_eq!(fired.len(), 1, "{fired:?}");
    assert_eq!(fired[0]["origin"]["schedule_id"], "stays");
    server.stop().await.expect("stop");

    let restarted = world
        .start_with_clock(backend.clone(), limits(), clock.clone())
        .await;
    assert_eq!(
        restarted
            .request(&get_request("/v1/schedules/gone"))
            .await
            .status,
        404
    );
    let held = listed(&restarted).await;
    let held: Vec<&str> = held["schedules"]
        .as_array()
        .expect("schedules")
        .iter()
        .map(|status| status["definition"]["id"].as_str().expect("id"))
        .collect();
    assert_eq!(held, ["stays"]);
    assert_eq!(jobs(&restarted, "?origin=schedule").await.len(), 1);
    restarted.stop().await.expect("restart stop");
}

/// What the `intake` schedule declares for its jobs.
fn declared() -> Value {
    json!({
        "inputs": {"tenant": "acme", "limit": 7},
        "owner": "user_123",
        "labels": {"app": "translation"}
    })
}

const TRIGGER_AT: &str = "2026-09-02T09:00:00Z";

/// A resident holding the API schedules `intake` (inputs, owner, labels) and
/// `other`, and the project beat `root`. Nothing is due.
async fn intake_server() -> (TestWorld, Arc<Recording>, TestServer) {
    let world = TestWorld::new();
    std::fs::write(world.workflows.join("intake.nika"), INTAKE_WORKFLOW).expect("intake workflow");
    std::fs::write(world.workflows.join("nika.yaml"), DAILY_BEAT).expect("project nika.yaml");
    let backend = Arc::new(Recording::default());
    let clock = Arc::new(ManualClock::new("2026-09-01T08:00:30Z[UTC]"));
    let server = world
        .start_with_clock(backend.clone(), limits(), clock.clone())
        .await;
    clock.wait_for_sleeps(1).await;
    let intake = schedule("intake.nika", TRIGGER_AT, &declared());
    create(&server, "intake", &intake).await;
    let other = schedule("root.nika", TRIGGER_AT, &json!({}));
    create(&server, "other", &other).await;
    (world, backend, server)
}

async fn trigger(server: &TestServer, id: &str, key: &str) -> super::tests::WireResponse {
    server.request(&trigger_request(id, Some(key), "")).await
}

/// A trigger admits one manual job under everything the schedule declares,
/// and leaves the cadence where it was.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_trigger_admits_one_manual_job_under_the_schedule() {
    let (_world, backend, server) = intake_server().await;
    let before = server.request(&get_request("/v1/schedules/intake")).await;
    let etag = before.header("etag").expect("etag").to_owned();

    // Refusals admit nothing.
    let keyless = server.request(&trigger_request("intake", None, "")).await;
    assert_eq!(keyless.status, 400, "{}", keyless.body);
    assert_eq!(keyless.json()["error"]["code"], "invalid_idempotency_key");
    let overridden = server
        .request(&trigger_request(
            "intake",
            Some("t-0"),
            "{\"inputs\":{\"tenant\":\"evil\"}}",
        ))
        .await;
    assert_eq!(overridden.status, 422, "{}", overridden.body);
    assert_eq!(
        overridden.json()["error"]["code"],
        "invalid_schedule_trigger"
    );
    let unknown = trigger(&server, "nowhere", "t-0").await;
    assert_eq!(unknown.status, 404, "{}", unknown.body);
    assert_eq!(unknown.json()["error"]["code"], "schedule_not_found");
    let project = trigger(&server, "root", "t-0").await;
    assert_eq!(project.status, 409, "{}", project.body);
    assert_eq!(
        project.json()["error"]["code"],
        "schedule_declared_by_project"
    );
    assert!(jobs(&server, "").await.is_empty());

    // One manual job, under the schedule's inputs, ceiling, owner and labels.
    let created = server
        .request(&trigger_request("intake", Some("t-1"), "{}"))
        .await;
    assert_eq!(created.status, 202, "{}", created.body);
    let job = created.json();
    let id = job["id"].as_str().expect("job id").to_owned();
    assert_eq!(job["status"], "queued");
    wait_for_status(&server, &id, "succeeded")
        .await
        .expect("the triggered job settles");
    assert_eq!(
        backend.runs(),
        [(
            Some(0.25),
            BTreeMap::from([
                ("limit".to_owned(), json!(7)),
                ("tenant".to_owned(), json!("acme")),
            ])
        )]
    );
    let mine = jobs(&server, "?owner=user_123&origin=manual").await;
    assert_eq!(mine.len(), 1, "{mine:?}");
    assert_eq!(mine[0]["id"], id.as_str());
    assert_eq!(mine[0]["origin"], json!({"kind": "manual"}));
    assert_eq!(mine[0]["workflow"], "intake.nika");
    assert_eq!(mine[0]["labels"], json!({"app": "translation"}));

    // The cadence is unmoved: same revision, no decision, same next slot.
    let after = server.request(&get_request("/v1/schedules/intake")).await;
    assert_eq!(after.header("etag"), Some(etag.as_str()));
    assert_eq!(after.json(), before.json());
    assert_eq!(after.json()["lastDecision"], Value::Null);
    server.stop().await.expect("stop");
}

/// A trigger key answers one job: under a retry and after the schedule
/// changed. It is bound to its schedule, and a pause does not hold it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_trigger_key_answers_one_job_whatever_changed() {
    let (world, backend, server) = intake_server().await;
    let read = server.request(&get_request("/v1/schedules/intake")).await;
    let etag = read.header("etag").expect("etag").to_owned();
    let created = trigger(&server, "intake", "t-1").await;
    assert_eq!(created.status, 202, "{}", created.body);
    let id = created.json()["id"].as_str().expect("job id").to_owned();
    wait_for_status(&server, &id, "succeeded")
        .await
        .expect("the triggered job settles");

    let replayed = trigger(&server, "intake", "t-1").await;
    assert_eq!(replayed.status, 200, "{}", replayed.body);
    assert_eq!(replayed.json()["id"], id.as_str());
    let mut paused = declared();
    paused["active"] = json!(false);
    paused["pauseReason"] = json!("held for the test");
    paused["pauseUntil"] = json!("2026-12-01");
    let update = server
        .request(&put_request(
            "intake",
            &schedule("intake.nika", TRIGGER_AT, &paused),
            &format!("If-Match: {etag}\r\n"),
            true,
        ))
        .await;
    assert_eq!(update.status, 200, "{}", update.body);
    assert_eq!(update.json()["changed"], true);
    let still = trigger(&server, "intake", "t-1").await;
    assert_eq!(still.status, 200, "{}", still.body);
    assert_eq!(still.json()["id"], id.as_str());
    assert_eq!(backend.runs().len(), 1, "a replay runs nothing");

    // A pause holds the cadence, not the operator.
    let while_paused = trigger(&server, "intake", "t-2").await;
    assert_eq!(while_paused.status, 202, "{}", while_paused.body);
    let second = while_paused.json()["id"].as_str().expect("id").to_owned();
    assert_ne!(second, id);
    wait_for_status(&server, &second, "succeeded")
        .await
        .expect("the second triggered job settles");

    // One key, one request: another schedule or the job door conflicts.
    let elsewhere = trigger(&server, "other", "t-1").await;
    assert_eq!(elsewhere.status, 409, "{}", elsewhere.body);
    assert_eq!(elsewhere.json()["error"]["code"], "idempotency_conflict");
    let by_name = server
        .request(&post_request(
            &json!({"workflow": "root.nika"}).to_string(),
            "t-1",
            &auth_header(),
        ))
        .await;
    assert_eq!(by_name.status, 409, "{}", by_name.body);

    // A schedule whose inputs no longer admit refuses, and admits nothing.
    std::fs::write(
        world.workflows.join("intake.nika"),
        INTAKE_WORKFLOW.replace("tenant: { type: string", "tenant: { type: integer"),
    )
    .expect("retype the input");
    let misfit = trigger(&server, "intake", "t-3").await;
    assert_eq!(misfit.status, 422, "{}", misfit.body);
    assert_eq!(misfit.json()["error"]["code"], "schedule.inputs");
    assert_eq!(jobs(&server, "?owner=user_123").await.len(), 2);
    server.stop().await.expect("stop");
}
