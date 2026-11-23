// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! Fair admission over HTTP (ADR-155): owners share the workers, one owner
//! is bounded in what it holds, and the resident says what it holds.
//!
//! Every test drives the real door with a backend whose runs end only when
//! the test says so: the order jobs start in is observed, never timed.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use super::tests::{
    TestServer, TestWorld, WORKFLOW, WireResponse, auth_header, get_request, post_request,
    wait_for_status,
};
use super::{ExecutionBackend, ExecutionDisposition, ExecutionOutcome, ServerLimits};

/// Runs wait at a gate. Each records the workflow it runs when it starts, so
/// a test reads which owner's job a worker took.
#[derive(Debug)]
struct Gate {
    started: Mutex<Vec<String>>,
    active: Mutex<BTreeMap<String, (usize, usize)>>,
    open: tokio::sync::Semaphore,
}

impl Gate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            started: Mutex::new(Vec::new()),
            active: Mutex::new(BTreeMap::new()),
            open: tokio::sync::Semaphore::new(0),
        })
    }

    /// The workflows of the runs started so far, in order.
    fn started(&self) -> Vec<String> {
        self.started.lock().expect("started").clone()
    }

    /// Most runs of `workflow` that were ever inside the backend at once.
    fn peak(&self, workflow: &str) -> usize {
        self.active
            .lock()
            .expect("active")
            .get(workflow)
            .map_or(0, |(_, peak)| *peak)
    }

    /// Let `count` runs end.
    fn release(&self, count: usize) {
        self.open.add_permits(count);
    }

    /// Wait until `count` runs have started, and answer their order.
    async fn until_started(&self, count: usize) -> Vec<String> {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let started = self.started();
                if started.len() >= count {
                    break started;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the runs start")
    }

    /// No further run starts while the gate stays shut.
    async fn holds_at(&self, count: usize) {
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(self.started().len(), count, "{:?}", self.started());
    }
}

impl ExecutionBackend for Gate {
    fn execute<'a>(
        &'a self,
        context: nika_execution::ExecutionContext<'a>,
    ) -> Pin<Box<dyn Future<Output = ExecutionOutcome> + Send + 'a>> {
        let workflow = context.snapshot().root().to_owned();
        Box::pin(async move {
            self.started.lock().expect("started").push(workflow.clone());
            {
                let mut active = self.active.lock().expect("active");
                let (now, peak) = active.entry(workflow.clone()).or_default();
                *now += 1;
                *peak = (*peak).max(*now);
            }
            let permit = self.open.acquire().await.expect("the gate stays");
            permit.forget();
            if let Some((now, _)) = self.active.lock().expect("active").get_mut(&workflow) {
                *now -= 1;
            }
            ExecutionDisposition::Succeeded.into()
        })
    }
}

/// `workers` jobs at once, a queue of 64, and a short stop.
fn limits(workers: usize) -> ServerLimits {
    ServerLimits::new(
        1024,
        Duration::from_secs(2),
        Duration::from_secs(30),
        Duration::from_millis(300),
        workers,
        64,
        64,
        32,
    )
}

/// A world serving one workflow per owner, so a run names its owner.
fn world(owners: &[&str]) -> TestWorld {
    let world = TestWorld::new();
    for owner in owners {
        std::fs::write(
            world.workflows.join(format!("{owner}.nika")),
            WORKFLOW.replace("nika: root", &format!("nika: {owner}")),
        )
        .expect("owner workflow");
    }
    world
}

/// Submit one job of `owner`; `anonymous` states no owner.
async fn submit(server: &TestServer, owner: &str, key: &str) -> WireResponse {
    let mut body = json!({"workflow": format!("{owner}.nika")});
    if owner != "anonymous" {
        body["owner"] = json!(owner);
    }
    server
        .request(&post_request(&body.to_string(), key, &auth_header()))
        .await
}

async fn admitted(server: &TestServer, owner: &str, key: &str) -> String {
    let response = submit(server, owner, key).await;
    assert_eq!(response.status, 202, "{key}: {}", response.body);
    response.json()["id"].as_str().expect("job id").to_owned()
}

async fn occupancy(server: &TestServer) -> Value {
    let response = server.request(&get_request("/v1/occupancy")).await;
    assert_eq!(response.status, 200, "{}", response.body);
    response.json()
}

/// One owner's burst does not delay another: the next free worker goes to
/// the owner that has nothing running, and each lane keeps its order.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_burst_from_one_owner_does_not_delay_another() {
    let world = world(&["a", "b", "c"]);
    let gate = Gate::new();
    let server = world.start(gate.clone(), limits(2)).await;
    let mut burst = Vec::new();
    for index in 0..50 {
        burst.push(admitted(&server, "a", &format!("a-{index}")).await);
    }
    assert_eq!(gate.until_started(2).await, ["a.nika", "a.nika"]);
    let b = admitted(&server, "b", "b-0").await;
    let c = admitted(&server, "c", "c-0").await;
    gate.holds_at(2).await;

    // One worker frees: `b` has nothing running, `a` still has one.
    gate.release(1);
    assert_eq!(gate.until_started(3).await[2], "b.nika");
    // The second of the burst ends: `a` and `c` both run nothing, and among
    // equals the workers go in turn. `a` has waited in the ring longest.
    gate.release(1);
    assert_eq!(gate.until_started(4).await[3], "a.nika");
    // `b` ends: `c` runs nothing while `a` runs one, so `c` goes first.
    gate.release(1);
    assert_eq!(gate.until_started(5).await[4], "c.nika");
    for (id, status) in [
        (&burst[0], "succeeded"),
        (&burst[1], "succeeded"),
        (&b, "succeeded"),
        (&burst[2], "running"),
        (&c, "running"),
        // Inside the burst the admission order holds.
        (&burst[3], "queued"),
    ] {
        wait_for_status(&server, id, status)
            .await
            .expect("the owners share the workers");
    }

    let load = occupancy(&server).await;
    assert_eq!(load["jobs"]["running"], 2);
    assert_eq!(load["jobs"]["queued"], 47);
    assert_eq!(load["jobs"]["max_concurrent"], 2);
    assert_eq!(load["jobs"]["queue_capacity"], 64);
    assert_eq!(load["jobs"]["max_queued_per_owner"], Value::Null);
    assert_eq!(
        load["owners"],
        json!([
            {"owner": "a", "running": 1, "queued": 47},
            {"owner": "c", "running": 1, "queued": 0}
        ])
    );
    assert_eq!(load["owners_omitted"], 0);

    gate.release(52);
    for id in burst.iter().chain([&b, &c]) {
        wait_for_status(&server, id, "succeeded")
            .await
            .expect("every job runs");
    }
    assert_eq!(gate.started().len(), 52);
    server.stop().await.expect("stop");
}

/// An owner that holds its places is refused `429 owner_queue_full` with
/// `Retry-After` and nothing is written; every other owner is admitted.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_owner_at_its_queue_bound_is_refused_alone() {
    let world = world(&["a", "b", "anonymous"]);
    let gate = Gate::new();
    let server = world
        .start(gate.clone(), limits(1).with_max_queued_per_owner(2))
        .await;
    let running = admitted(&server, "a", "a-0").await;
    gate.until_started(1).await;
    let waiting = admitted(&server, "a", "a-1").await;
    admitted(&server, "a", "a-2").await;

    let refused = submit(&server, "a", "a-3").await;
    assert_eq!(refused.status, 429, "{}", refused.body);
    assert_eq!(refused.json()["error"]["code"], "owner_queue_full");
    assert_eq!(refused.header("retry-after"), Some("5"));
    let listed = server.request(&get_request("/v1/jobs?owner=a")).await;
    assert_eq!(
        listed.json()["jobs"].as_array().expect("jobs").len(),
        3,
        "a refusal writes no job"
    );

    // A replay of a job that waits is answered, never refused for its place.
    let replayed = submit(&server, "a", "a-1").await;
    assert_eq!(replayed.status, 200, "{}", replayed.body);
    assert_eq!(replayed.json()["id"], waiting.as_str());

    // Another owner, and the jobs that state none, have their own places.
    admitted(&server, "b", "b-0").await;
    admitted(&server, "anonymous", "n-0").await;
    admitted(&server, "anonymous", "n-1").await;
    let anonymous = submit(&server, "anonymous", "n-2").await;
    assert_eq!(anonymous.status, 429, "{}", anonymous.body);
    let load = occupancy(&server).await;
    assert_eq!(load["jobs"]["max_queued_per_owner"], 2);
    assert_eq!(load["jobs"]["queued"], 5);

    // The refused key was never bound: once a place is free it admits.
    gate.release(1);
    wait_for_status(&server, &running, "succeeded")
        .await
        .expect("the running job ends");
    gate.until_started(2).await;
    let late = submit(&server, "a", "a-3").await;
    assert_eq!(late.status, 202, "{}", late.body);
    gate.release(16);
    wait_for_status(
        &server,
        late.json()["id"].as_str().expect("id"),
        "succeeded",
    )
    .await
    .expect("the late job runs");
    server.stop().await.expect("stop");
}

/// An owner never has more jobs running than its share; its other jobs
/// wait while free workers serve other owners.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_owner_runs_at_most_its_share_of_the_workers() {
    let world = world(&["a", "b"]);
    let gate = Gate::new();
    let server = world
        .start(gate.clone(), limits(4).with_max_concurrent_per_owner(1))
        .await;
    let mut jobs = Vec::new();
    for index in 0..3 {
        jobs.push(admitted(&server, "a", &format!("a-{index}")).await);
    }
    gate.until_started(1).await;
    gate.holds_at(1).await;
    jobs.push(admitted(&server, "b", "b-0").await);
    assert_eq!(gate.until_started(2).await, ["a.nika", "b.nika"]);
    gate.holds_at(2).await;
    let load = occupancy(&server).await;
    assert_eq!(load["jobs"]["max_concurrent_per_owner"], 1);
    assert_eq!(
        load["owners"],
        json!([
            {"owner": "a", "running": 1, "queued": 2},
            {"owner": "b", "running": 1, "queued": 0}
        ])
    );

    // Each time one of its jobs ends, the owner's next starts.
    gate.release(4);
    for id in &jobs {
        wait_for_status(&server, id, "succeeded")
            .await
            .expect("every job runs");
    }
    assert_eq!(gate.peak("a.nika"), 1, "never two of one owner at once");
    assert_eq!(gate.started().len(), 4);
    server.stop().await.expect("stop");
}

/// A job cancelled while it waits behind its owner's bound gives its place
/// back at once, and nothing is left stuck behind it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancelled_waiting_job_frees_its_place() {
    let world = world(&["a"]);
    let gate = Gate::new();
    let server = world
        .start(
            gate.clone(),
            limits(2)
                .with_max_queued_per_owner(1)
                .with_max_concurrent_per_owner(1),
        )
        .await;
    let running = admitted(&server, "a", "a-0").await;
    gate.until_started(1).await;
    let waiting = admitted(&server, "a", "a-1").await;
    assert_eq!(submit(&server, "a", "a-2").await.status, 429);

    let cancel = format!(
        "POST /v1/jobs/{waiting}/cancel HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: 0\r\n{}\r\n",
        auth_header()
    );
    let cancelled = server.request(&cancel).await;
    assert_eq!(cancelled.status, 200, "{}", cancelled.body);
    assert_eq!(cancelled.json()["status"], "cancelled");
    assert_eq!(occupancy(&server).await["jobs"]["queued"], 0);

    let next = admitted(&server, "a", "a-2").await;
    gate.holds_at(1).await;
    gate.release(2);
    wait_for_status(&server, &running, "succeeded")
        .await
        .expect("the running job ends");
    wait_for_status(&server, &next, "succeeded")
        .await
        .expect("the job admitted in the freed place runs");
    assert_eq!(gate.started().len(), 2, "the cancelled job never ran");
    wait_for_status(&server, &waiting, "cancelled")
        .await
        .expect("it stays cancelled");
    server.stop().await.expect("stop");
}

/// Jobs that waited when the resident stopped re-enter their owner's lane:
/// the restarted resident serves the owners in turn again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn waiting_jobs_re_enter_their_lanes_after_a_restart() {
    let world = world(&["a", "b"]);
    let gate = Gate::new();
    let server = world.start(gate.clone(), limits(1)).await;
    let interrupted = admitted(&server, "a", "a-0").await;
    gate.until_started(1).await;
    let mut waiting = Vec::new();
    for index in 1..4 {
        waiting.push(admitted(&server, "a", &format!("a-{index}")).await);
    }
    waiting.push(admitted(&server, "b", "b-0").await);
    // The run at the gate outlives the grace: the stop interrupts it and
    // leaves the four others queued.
    assert!(server.stop().await.is_err(), "the stop times out its run");

    let reopened = Gate::new();
    let restarted = world.start(reopened.clone(), limits(1)).await;
    wait_for_status(&restarted, &interrupted, "interrupted")
        .await
        .expect("the run that was cut is interrupted");
    reopened.release(4);
    for id in &waiting {
        wait_for_status(&restarted, id, "succeeded")
            .await
            .expect("every waiting job runs after the restart");
    }
    assert_eq!(
        reopened.started(),
        ["a.nika", "b.nika", "a.nika", "a.nika"],
        "the owners are served in turn, each lane in its order"
    );
    restarted.stop().await.expect("stop");
}

/// Occupancy needs the token, and the capability is advertised.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn occupancy_is_authenticated_and_advertised() {
    let world = world(&[]);
    let server = world.start(Gate::new(), limits(1)).await;
    let anonymous = server
        .request("GET /v1/occupancy HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await;
    assert_eq!(anonymous.status, 401);
    assert_eq!(
        occupancy(&server).await,
        json!({
            "jobs": {
                "running": 0,
                "queued": 0,
                "max_concurrent": 1,
                "queue_capacity": 64,
                "max_concurrent_per_owner": null,
                "max_queued_per_owner": null
            },
            "owners": [],
            "owners_omitted": 0
        })
    );
    let health = server
        .request("GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await;
    assert!(
        health.json()["supportedCapabilities"]
            .as_array()
            .expect("capabilities")
            .contains(&json!("occupancy"))
    );
    server.stop().await.expect("stop");
}
