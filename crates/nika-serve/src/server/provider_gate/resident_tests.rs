// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! The provider cap through the real door and the real backend: three jobs
//! fan out at once over the `mock` provider, and the resident lets only as
//! many calls in flight as its operator allowed, whichever job sends them.
//!
//! Each call the gate admits is then held by the test, the way a slow
//! provider would hold it, so what is in flight is observed, never timed.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use nika_providers::{ProviderCallEntry, ProviderCallGate, ProviderCallSlot};
use serde_json::{Value, json};

use super::super::production::JournalSeal;
use super::super::tests::{
    TestServer, TestWorld, auth_header, get_request, post_request, wait_for_status,
};
use super::super::{ResidentConfig, ResidentExecutionBackend, ServerLimits};
use super::ProviderGates;

/// Four items at once, one model call each.
const FAN: &str = "nika: fan\nmodel: mock/echo\npermits: {}\ntasks:\n  ask:\n    for_each: { items: [a, b, c, d], max_parallel: 4 }\n    infer: { prompt: 'say ${{ item }}', max_tokens: 64 }\n";

/// A run whose only model call is made by the child workflow it invokes.
const PARENT: &str = "nika: parent\nmodel: mock/echo\npermits: {}\ntasks:\n  call:\n    invoke: { workflow: \"./child.nika\" }\n";
const CHILD: &str = "nika: child\nmodel: mock/echo\npermits: {}\ntasks:\n  ask:\n    infer: { prompt: hi, max_tokens: 64 }\n";

struct NoCustody;

impl JournalSeal for NoCustody {
    fn seal(
        &self,
        _: &mut nika_dap::journal::TraceFileSink,
        _: Option<&str>,
        _: Option<&nika_dap::seal::SealTeardown>,
    ) -> bool {
        false
    }
}

/// What the calls the gate admitted hold while the test keeps them.
#[derive(Debug, Default)]
struct Kept {
    now: AtomicUsize,
    peak: AtomicUsize,
    admitted: AtomicUsize,
}

/// One admitted call, counted until its slot drops.
struct Keeping(Arc<Kept>);

impl Drop for Keeping {
    fn drop(&mut self) {
        self.0.now.fetch_sub(1, Ordering::SeqCst);
    }
}

/// The resident's own gate, with every call it admits kept in flight until
/// the test releases it: a provider that answers on command.
#[derive(Debug)]
struct SlowProvider {
    gates: Arc<ProviderGates>,
    kept: Arc<Kept>,
    answer: tokio::sync::Semaphore,
}

impl SlowProvider {
    fn behind(gates: &Arc<ProviderGates>) -> Arc<Self> {
        Arc::new(Self {
            gates: Arc::clone(gates),
            kept: Arc::new(Kept::default()),
            answer: tokio::sync::Semaphore::new(0),
        })
    }

    fn in_flight(&self) -> usize {
        self.kept.now.load(Ordering::SeqCst)
    }

    fn peak(&self) -> usize {
        self.kept.peak.load(Ordering::SeqCst)
    }

    fn admitted(&self) -> usize {
        self.kept.admitted.load(Ordering::SeqCst)
    }

    /// Let `count` held calls answer.
    fn answer(&self, count: usize) {
        self.answer.add_permits(count);
    }
}

impl ProviderCallGate for SlowProvider {
    fn enter<'a>(&'a self, provider: &'a str) -> ProviderCallEntry<'a> {
        Box::pin(async move {
            let slot = self.gates.enter(provider).await?;
            let now = self.kept.now.fetch_add(1, Ordering::SeqCst) + 1;
            self.kept.peak.fetch_max(now, Ordering::SeqCst);
            self.kept.admitted.fetch_add(1, Ordering::SeqCst);
            let keeping = Keeping(Arc::clone(&self.kept));
            if let Ok(answer) = self.answer.acquire().await {
                answer.forget();
            }
            // The count falls before the real slot returns: the next call
            // is admitted only after this one stopped counting.
            Ok(ProviderCallSlot::holding((keeping, slot)))
        })
    }
}

fn limits() -> ServerLimits {
    ServerLimits::new(
        4096,
        Duration::from_secs(5),
        Duration::from_secs(30),
        Duration::from_secs(2),
        4,
        16,
        64,
        32,
    )
}

/// A resident whose jobs run on the real backend under `cap` calls on the
/// `mock` provider, each admitted call held by the returned provider.
async fn resident(world: &TestWorld, cap: usize) -> (TestServer, Arc<SlowProvider>) {
    for (name, source) in [
        ("fan.nika", FAN),
        ("parent.nika", PARENT),
        ("child.nika", CHILD),
    ] {
        std::fs::write(world.workflows.join(name), source).expect("workflow");
    }
    let gates = ProviderGates::new(&[("mock".to_owned(), cap)].into_iter().collect());
    let provider = SlowProvider::behind(&gates);
    let gate: Arc<dyn ProviderCallGate> = provider.clone();
    let backend = Arc::new(
        ResidentExecutionBackend::new(&world.workflows)
            .with_journal_seal(Arc::new(NoCustody))
            .with_provider_gate(Some(gate)),
    );
    let config = ResidentConfig::new(&world.state)
        .with_limits(limits())
        .with_provider_gates(Some(gates));
    let server = world
        .start_with_capture_action(backend, config, &world.workflows, None)
        .await;
    (server, provider)
}

async fn submit(server: &TestServer, key: &str) -> String {
    submit_named(server, "fan.nika", key).await
}

async fn submit_named(server: &TestServer, workflow: &str, key: &str) -> String {
    let body = json!({ "workflow": workflow }).to_string();
    let response = server
        .request(&post_request(&body, key, &auth_header()))
        .await;
    assert_eq!(response.status, 202, "{}", response.body);
    response.json()["id"].as_str().expect("job id").to_owned()
}

async fn mock_load(server: &TestServer) -> Value {
    let response = server.request(&get_request("/v1/occupancy")).await;
    assert_eq!(response.status, 200, "{}", response.body);
    let providers = response.json()["providers"].clone();
    assert_eq!(providers.as_array().map(Vec::len), Some(1), "{providers}");
    providers[0].clone()
}

/// The body `GET /v1/occupancy` answers now, checked against the published
/// `Occupancy` schema.
async fn occupancy_under_contract(server: &TestServer) -> Value {
    let response = server.request(&get_request("/v1/occupancy")).await;
    assert_eq!(response.status, 200, "{}", response.body);
    let body = response.json();
    let mut root = super::super::openapi::document();
    root["$ref"] = json!("#/components/schemas/Occupancy");
    let validator = jsonschema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .build(&root)
        .expect("the published schema compiles");
    let errors: Vec<String> = validator
        .iter_errors(&body)
        .map(|error| error.to_string())
        .collect();
    assert!(errors.is_empty(), "{errors:?}\n{body}");
    body
}

/// Wait until the resident reports `in_flight` calls in flight and `waiting`
/// in line on the `mock` provider.
async fn until_load(server: &TestServer, in_flight: usize, waiting: usize) {
    let expected = json!({
        "provider": "mock",
        "max_concurrent": 2,
        "in_flight": in_flight,
        "waiting": waiting,
    });
    let mut last = Value::Null;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while tokio::time::Instant::now() < deadline {
        last = mock_load(server).await;
        if last == expected {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(last, expected, "the provider load never settled");
}

/// Three jobs fan out four calls each. With two slots on the provider, two
/// calls are in flight and ten wait, whichever jobs they belong to; each
/// answer lets exactly one more in; all three jobs succeed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn three_fanned_out_jobs_share_the_provider_slots() {
    let world = TestWorld::new();
    let (server, provider) = resident(&world, 2).await;
    let jobs = [
        submit(&server, "fan-1").await,
        submit(&server, "fan-2").await,
        submit(&server, "fan-3").await,
    ];
    for job in &jobs {
        wait_for_status(&server, job, "running").await.unwrap();
    }

    // Twelve calls want to be in flight; the provider holds two.
    until_load(&server, 2, 10).await;
    assert_eq!(provider.in_flight(), 2);
    assert_eq!(provider.admitted(), 2);
    let occupancy = occupancy_under_contract(&server).await;
    assert_eq!(occupancy["jobs"]["running"], 3, "{occupancy}");

    // One answer, one more call: the line moves by one.
    provider.answer(1);
    until_load(&server, 2, 9).await;
    assert_eq!(provider.admitted(), 3);
    assert_eq!(provider.in_flight(), 2);

    provider.answer(11);
    for job in &jobs {
        wait_for_status(&server, job, "succeeded").await.unwrap();
    }
    assert_eq!(provider.admitted(), 12, "every item made its one call");
    assert_eq!(provider.peak(), 2, "never three calls at once");
    until_load(&server, 0, 0).await;
    server.stop().await.expect("a clean stop");
}

/// A child run is the same job: the call its child workflow makes takes its
/// slot from the same gate as its parent's would.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_child_run_takes_its_slot_from_the_same_gate() {
    let world = TestWorld::new();
    let (server, provider) = resident(&world, 2).await;
    let job = submit_named(&server, "parent.nika", "parent-1").await;
    wait_for_status(&server, &job, "running").await.unwrap();
    until_load(&server, 1, 0).await;
    assert_eq!(
        provider.admitted(),
        1,
        "the child's call went through the gate"
    );

    provider.answer(1);
    wait_for_status(&server, &job, "succeeded").await.unwrap();
    until_load(&server, 0, 0).await;
    server.stop().await.expect("a clean stop");
}

/// A job cancelled while its calls wait for a slot leaves the line, and the
/// jobs behind it still get their turn: nothing stays held.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancelled_job_leaves_the_line_and_the_others_finish() {
    let world = TestWorld::new();
    let (server, provider) = resident(&world, 2).await;
    let first = submit(&server, "fan-1").await;
    wait_for_status(&server, &first, "running").await.unwrap();
    until_load(&server, 2, 2).await;
    let second = submit(&server, "fan-2").await;
    wait_for_status(&server, &second, "running").await.unwrap();
    until_load(&server, 2, 6).await;

    // The first job holds both slots and two places in line: cancel it. The
    // test's hold sits inside the wait a stop ends, so the stop frees the
    // two held calls as well as the two in line.
    let cancel = format!(
        "POST /v1/jobs/{first}/cancel HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: 0\r\n{}\r\n",
        auth_header()
    );
    let response = server.request(&cancel).await;
    assert!(
        matches!(response.status, 200 | 202),
        "{} {}",
        response.status,
        response.body
    );
    wait_for_status(&server, &first, "cancelled").await.unwrap();

    // Its slots came back: the second job's calls took them.
    until_load(&server, 2, 2).await;
    provider.answer(4);
    wait_for_status(&server, &second, "succeeded")
        .await
        .unwrap();
    assert_eq!(provider.peak(), 2);
    until_load(&server, 0, 0).await;
    server.stop().await.expect("a clean stop");
}
