// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! A seat that fails for a passing reason is retried by a plain `retry:` (no `on_codes`), and a
//! lost sign-in is not: an `agent:` task on the file's declared codex ACP route, through the
//! runtime's attempt loop, against the real spawn and the real client.
//!
//! - an adapter whose identity probe does not answer inside the probe deadline (a real program
//!   that sleeps; the clock is paused, so the 30 s deadline passes at once) is unanswered,
//!   NIKA-1804 and transient: every declared attempt runs;
//! - a scripted codex-acp that answers `session/new` with `-32603` (« workspace routing
//!   discovery timed out », measured on a loaded machine) is a session that did not open: the
//!   same, and each attempt opens a new session;
//! - the same answer carrying the adapter's `authentication_failed` kind is a lost sign-in:
//!   NIKA-1803, final after its one attempt.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use nika_kernel::ai::harness::{DynAgentBackend, HarnessError, HarnessEventStream, HarnessRequest};
use nika_kernel_mock::{
    MockClock, MockProvider, MockShell, MockToolDefinitionProvider, MockToolExecutor,
};
use nika_providers::probe::{ExecutionLocus, ProviderProbe, ProviderReadiness};
use nika_providers::{ExecutionAccessPlan, ModelNeed, VerbNeeds};
use nika_types::access::AccessClass;
use nika_verb_agent::AgentVerb;
use nika_verb_exec::ExecVerb;
use nika_verb_infer::InferVerb;
use nika_verb_invoke::InvokeVerb;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use super::*;

const MODEL: &str = "openai/gpt-6-astra";

/// The task the measured workflow declares, minus its minute of backoff.
fn workflow() -> RawWorkflow {
    let yaml = format!(
        "nika: harness-retry\nmodel: {MODEL}\npermits: {{}}\nrun:\n  access:\n    via: codex\n    \
         protocol: acp\n    fallback: none\ntasks:\n  ask:\n    retry: {{ max_attempts: 3, \
         backoff_ms: 1 }}\n    agent:\n      prompt: \"summarise the invariants\"\n"
    );
    nika_schema::parse(
        &yaml,
        nika_schema::FileId::new(0),
        nika_schema::ParseMode::Strict,
    )
    .expect("the declared shape parses")
}

/// The plan the CLI resolves for the file alone: a signed-in codex seat, an unused API key.
fn plan(wf: &RawWorkflow) -> ExecutionAccessPlan {
    let readiness =
        |priced, locus, class| ProviderReadiness::new(true, true, None, None, priced, locus, class);
    let api = ProviderProbe::new(
        "openai",
        true,
        true,
        "OPENAI_API_KEY",
        false,
        readiness(true, ExecutionLocus::Cloud, AccessClass::Api),
        "https://api.openai.com",
    );
    let codex = ProviderProbe::new(
        "codex",
        false,
        true,
        "",
        false,
        readiness(false, ExecutionLocus::Loopback, AccessClass::Harness),
        "",
    )
    .with_serves(vec!["openai".to_owned()]);
    let requirement = (wf.run.as_ref())
        .and_then(|run| run.value.access_requirement())
        .expect("declared");
    nika_providers::resolve_execution_plan_declared(
        &[ModelNeed::new(MODEL, false, true)],
        &[api, codex],
        None,
        VerbNeeds::new(false, true),
        Some(&requirement),
    )
}

/// The failed task's record and the attempts its terminal frame reports, after one run of the
/// workflow on `backend`.
async fn run_on(backend: Arc<dyn DynAgentBackend>) -> (TaskErrorRecord, Value) {
    let wf = workflow();
    let report = nika_check::check(&wf);
    assert!(report.is_clean(), "{report:?}");
    let plan = plan(&wf);
    assert!(plan.is_admitted(), "{:?}", plan.pin_refusal);
    let api = Arc::new(MockProvider::new("mock").enqueue_text("never asked"));
    let invoke = Arc::new(InvokeVerb::new(Arc::new(MockToolExecutor::new())));
    let runtime = Runtime::new(
        ExecVerb::new(Arc::new(MockShell::new())),
        Arc::clone(&invoke),
        InferVerb::new(
            Arc::new(nika_providers::ProviderRegistry::without_http(
                nika_providers::ProvidersConfig::new(),
            )),
            MODEL,
        ),
        AgentVerb::new(
            Arc::clone(&api),
            invoke,
            Arc::new(MockToolDefinitionProvider::new()),
            MODEL,
        ),
        MockClock::new(),
        RuntimeConfig::default(),
    )
    .with_access_plan(plan)
    .with_harness_backend(backend, "codex".into())
    .expect("seated");
    let mut sink = VecSink::new();
    let outcome = runtime
        .run(&wf, &report, &mut DeterministicStamper::new(), &mut sink)
        .await
        .expect("the run settles");
    assert!(!outcome.ok, "every attempt fails");
    assert!(api.captured_requests().is_empty(), "no substitute route");
    let error = outcome.records["ask"].error.clone().expect("a failure");
    let terminal = (sink.events().iter().rev())
        .find(|e| e.kind == EventKind::TaskFailed && e.str_field("task") == Some("ask"))
        .and_then(|e| e.str_field("outcome"))
        .expect("the terminal frame's outcome");
    let outcome: Value = serde_json::from_str(terminal).expect("JSON");
    (error, outcome["payload"]["attempts"].clone())
}

/// A real program whose identity probe never answers: the probe deadline passes, the task is
/// retried to its declared bound, and the record names the unanswered probe.
#[tokio::test(start_paused = true)]
async fn an_unanswered_identity_probe_is_retried_by_a_plain_retry() {
    let adapter = nika_harness::HarnessAdapter::new("codex", "/bin/sh")
        .expect("id")
        .with_version_args(vec!["-c".to_owned(), "exec sleep 600".to_owned()])
        .with_version_pin(nika_harness::VersionPin::new((0, 16), 1));
    let backend = Arc::new(nika_harness::SpawnedHarness::new(adapter));
    let (error, attempts) = run_on(backend).await;
    assert_eq!(error.code, "NIKA-1804");
    assert!(error.transient);
    assert!(
        error
            .message
            .ends_with("version probe `/bin/sh -c exec sleep 600` did not answer in 30s"),
        "{}",
        error.message
    );
    assert_eq!(attempts, json!(3), "every declared attempt ran");
}

type Peer = (
    BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>,
    tokio::io::WriteHalf<tokio::io::DuplexStream>,
);

async fn read(peer: &mut Peer) -> Option<Value> {
    let mut line = String::new();
    let n = peer.0.read_line(&mut line).await.expect("read");
    (n != 0).then(|| serde_json::from_str(&line).expect("json"))
}

async fn write(peer: &mut Peer, message: &Value) {
    let mut bytes = serde_json::to_vec(message).expect("json");
    bytes.push(b'\n');
    peer.1.write_all(&bytes).await.expect("write");
}

/// A codex-acp that admits `initialize` and answers each `session/new` with `error`, recording
/// every request it reads.
struct FailingSessions {
    error: Value,
    seen: Arc<Mutex<Vec<String>>>,
}

impl DynAgentBackend for FailingSessions {
    fn run_agent_boxed(
        &self,
        request: HarnessRequest,
    ) -> Pin<Box<dyn Future<Output = Result<HarnessEventStream, HarnessError>> + Send + '_>> {
        let (ours, theirs) = tokio::io::duplex(64 * 1024);
        let (client_read, client_write) = tokio::io::split(ours);
        let (peer_read, peer_write) = tokio::io::split(theirs);
        let (error, seen) = (self.error.clone(), Arc::clone(&self.seen));
        tokio::spawn(async move {
            let mut peer: Peer = (BufReader::new(peer_read), peer_write);
            while let Some(request) = read(&mut peer).await {
                let method = request["method"].as_str().unwrap_or_default().to_owned();
                seen.lock().expect("seen").push(method.clone());
                let answer = match method.as_str() {
                    "initialize" => {
                        json!({"jsonrpc":"2.0","id":request["id"],"result":{"protocolVersion":1}})
                    }
                    _ => json!({"jsonrpc":"2.0","id":request["id"],"error":error}),
                };
                write(&mut peer, &answer).await;
            }
        });
        Box::pin(async move { Ok(nika_harness::drive(client_read, client_write, request)) })
    }
}

/// `-32603` at `session/new` is a session that did not open: retried, a new session each time.
/// The same answer typed `authentication_failed` is a lost sign-in: one attempt, final.
#[tokio::test]
async fn an_internal_error_at_session_new_is_retried_and_a_lost_sign_in_is_not() {
    let busy = json!({"code":-32603,"message":"Internal error",
        "data":{"details":"workspace routing discovery timed out"}});
    let expired = json!({"code":-32603,"message":"Internal error",
        "data":{"errorKind":"authentication_failed"}});
    for (error, code, transient, attempts) in [
        (busy, "NIKA-1804", true, 3),
        (expired, "NIKA-1803", false, 1),
    ] {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let backend = Arc::new(FailingSessions {
            error,
            seen: Arc::clone(&seen),
        });
        let (record, reported) = run_on(backend).await;
        assert_eq!(record.code, code);
        assert_eq!(record.transient, transient, "{}", record.message);
        assert_eq!(reported, json!(attempts), "{code}");
        let methods = seen.lock().expect("seen").clone();
        assert_eq!(
            methods,
            ["initialize", "session/new"].repeat(attempts),
            "one new session per attempt, never a prompt"
        );
    }
}
