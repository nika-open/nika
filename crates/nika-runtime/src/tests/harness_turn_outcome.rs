// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! An `agent:` task on a seat, end to end on the REAL ACP client: the task record and its
//! terminal frame say how the turn ended, where its counts came from, the context and the
//! adapter's own cost estimate (never `cost_usd`), and keep the whole text, masked, when the
//! value is the turn's last message. A seat that reported no counts leaves `tokens` unknown;
//! a turn that did not end fails with its reason and the text received.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use nika_kernel::ai::harness::{DynAgentBackend, HarnessError, HarnessEventStream, HarnessRequest};
use nika_kernel_mock::{
    MockClock, MockProvider, MockShell, MockToolDefinitionProvider, MockToolExecutor,
};
use nika_verb_agent::AgentVerb;
use nika_verb_exec::ExecVerb;
use nika_verb_infer::InferVerb;
use nika_verb_invoke::InvokeVerb;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use super::*;

/// A key the scripted seat repeats in its narration: no frame may carry it.
const SECRET: &str = "sk-live-7c3a9e1f5b2d";

/// A secret shorter than the bound under which only the payload fields are scrubbed.
const SHORT: &str = "zq7Kx9";

/// `text` with both secrets masked.
fn masked(text: &str) -> String {
    text.replace(SECRET, secret::REDACTED)
        .replace(SHORT, secret::REDACTED)
}

const SESSION: &str = "s-1";

/// One prompt's answer: the updates the seat sends, then the prompt's result.
type Answer = (Vec<Value>, Value);

/// A seat that answers its one prompt with the scripted frames.
struct Seat(Arc<Mutex<Option<Answer>>>);

type Reader = BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>;
type Writer = tokio::io::WriteHalf<tokio::io::DuplexStream>;

async fn send(w: &mut Writer, message: &Value) {
    let mut bytes = serde_json::to_vec(message).expect("JSON");
    bytes.push(b'\n');
    let _ = w.write_all(&bytes).await;
    let _ = w.flush().await;
}

async fn peer(mut r: Reader, mut w: Writer, answer: Arc<Mutex<Option<Answer>>>) {
    let config = json!([{"id":"model","name":"Model","category":"model","type":"select",
        "currentValue":"echo","options":[{"value":"echo","name":"Echo"}]}]);
    let mut line = String::new();
    loop {
        line.clear();
        if !matches!(r.read_line(&mut line).await, Ok(n) if n > 0) {
            return;
        }
        let request: Value = serde_json::from_str(&line).expect("JSON request");
        let result = match request["method"].as_str().unwrap_or_default() {
            "initialize" => json!({"protocolVersion":1,"agentInfo":{"name":"scripted-seat"}}),
            "session/new" => json!({"sessionId": SESSION, "configOptions": config}),
            "session/set_config_option" => json!({"configOptions": config}),
            "session/prompt" => {
                let (frames, result) = answer.lock().expect("answer").take().expect("one prompt");
                for update in frames {
                    let mut frame = json!({"jsonrpc":"2.0","method":"session/update",
                        "params":{"sessionId": SESSION}});
                    frame["params"]["update"] = update;
                    send(&mut w, &frame).await;
                }
                result
            }
            _ => json!({}),
        };
        send(
            &mut w,
            &json!({"jsonrpc":"2.0","id":request["id"],"result":result}),
        )
        .await;
    }
}

impl DynAgentBackend for Seat {
    fn run_agent_boxed(
        &self,
        request: HarnessRequest,
    ) -> Pin<Box<dyn Future<Output = Result<HarnessEventStream, HarnessError>> + Send + '_>> {
        let (ours, theirs) = tokio::io::duplex(1024 * 1024);
        let (client_read, client_write) = tokio::io::split(ours);
        let (peer_read, peer_write) = tokio::io::split(theirs);
        tokio::spawn(peer(
            BufReader::new(peer_read),
            peer_write,
            Arc::clone(&self.0),
        ));
        Box::pin(async move { Ok(nika_harness::drive(client_read, client_write, request)) })
    }
}

/// The run's secrets resolve to [`SECRET`] and [`SHORT`].
struct Resolver;

impl WorkflowSecretResolver for Resolver {
    fn resolve(
        &self,
        name: &str,
        _reference: &nika_schema::types::SecretRef,
    ) -> Result<String, SecretResolveError> {
        match name {
            "tok" => Ok(SECRET.to_owned()),
            "otp" => Ok(SHORT.to_owned()),
            _ => Err(SecretResolveError {
                name: name.to_owned(),
                reason: "absent".to_owned(),
            }),
        }
    }
}

/// One run of an `agent:` task (with `extra` keys) on a seat that answers `answer`: the
/// outcome and the frames.
async fn run_seat(extra: &str, answer: Answer) -> (RunOutcome, Vec<Event>) {
    let yaml = format!(
        "nika: seat-turn-outcome\nmodel: mock/echo\npermits: {{}}\nsecrets:\n  tok: {{ source: \
         env, key: NIKA_TOK }}\n  otp: {{ source: env, key: NIKA_OTP }}\ntasks:\n  ask:\n    agent:\n      prompt: \"research the \
         anchoring bias\"\n{extra}"
    );
    let wf = nika_schema::parse(
        &yaml,
        nika_schema::FileId::new(0),
        nika_schema::ParseMode::Strict,
    )
    .expect("fixture parses");
    let report = nika_check::check(&wf);
    assert!(report.is_clean(), "{report:?}");
    let invoke = Arc::new(InvokeVerb::new(Arc::new(MockToolExecutor::new())));
    let api = Arc::new(MockProvider::new("mock"));
    let runtime = Runtime::new(
        ExecVerb::new(Arc::new(MockShell::new())),
        Arc::clone(&invoke),
        InferVerb::new(
            Arc::new(nika_providers::ProviderRegistry::without_http(
                nika_providers::ProvidersConfig::new(),
            )),
            "mock/echo",
        ),
        AgentVerb::new(
            Arc::clone(&api),
            invoke,
            Arc::new(MockToolDefinitionProvider::new()),
            "mock/echo",
        ),
        MockClock::new(),
        RuntimeConfig::default(),
    )
    .with_secret_resolver(Arc::new(Resolver))
    .with_harness_backend(
        Arc::new(Seat(Arc::new(Mutex::new(Some(answer))))),
        "scripted-seat".into(),
    )
    .expect("seated");
    let mut sink = VecSink::new();
    let outcome = runtime
        .run(&wf, &report, &mut DeterministicStamper::new(), &mut sink)
        .await
        .expect("the run settles");
    assert!(
        api.captured_requests().is_empty(),
        "the seat answers, never the API"
    );
    (outcome, sink.into_events())
}

fn text(text: &str) -> Value {
    json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":text}})
}

fn terminal(events: &[Event], kind: EventKind) -> &Event {
    (events.iter().rev())
        .find(|e| e.kind == kind && e.str_field("task") == Some("ask"))
        .expect("the task's terminal frame")
}

fn no_frame_carries_the_secret(events: &[Event]) {
    for event in events {
        let line = serde_json::to_string(event).expect("event");
        assert!(!line.contains(SECRET), "{:?} carries the key", event.kind);
        assert!(
            !line.contains(SHORT),
            "{:?} carries the short secret",
            event.kind
        );
    }
}

const SCHEMA: &str = "      schema: { type: object, required: [title, tags], properties: { title: \
                      { type: string }, tags: { type: array } } }\n";

/// claude-agent-acp 0.81.1's frames: narration that repeats a key, a tool call, the JSON
/// answer, then its end-of-turn update and the recorded prompt-result counts. The value is the
/// answer; the frame keeps the whole text with the key masked, the exact token total with its
/// source, the context, and the cost as the adapter's estimate beside no `cost_usd`.
#[tokio::test]
async fn a_seat_turn_records_its_text_counts_context_and_estimate() {
    let narration = format!("I read the notes (token {SECRET}, code {SHORT}) first. ");
    let answer = r#"{"title": "Anchoring", "tags": []}"#;
    let frames = vec![
        text(&narration),
        json!({"sessionUpdate":"tool_call","toolCallId":"t1","title":"Read notes.md",
            "kind":"read","status":"pending"}),
        text(answer),
        json!({"sessionUpdate":"usage_update","used":3,"size":200_000,
            "cost":{"amount":0.0123,"currency":"USD"},
            "_meta":{"_claude/origin":"user","_claude/model":"claude-opus-4-5-20251101"}}),
    ];
    let result = json!({"stopReason":"end_turn",
        "usage":{"inputTokens":3,"outputTokens":0,"cachedReadTokens":0,
                 "cachedWriteTokens":0,"totalTokens":3}});
    let (outcome, events) = run_seat(
        &format!("{SCHEMA}      max_tokens_total: 100\n"),
        (frames, result),
    )
    .await;
    assert!(outcome.ok, "{events:?}");
    assert_eq!(
        outcome.records["ask"].output,
        json!({"title": "Anchoring", "tags": []})
    );
    let done = terminal(&events, EventKind::TaskCompleted);
    assert_eq!(done.int_field("tokens"), Some(3));
    assert_eq!(
        done.str_field("usage_source"),
        Some("adapter_prompt_result")
    );
    assert_eq!(done.str_field("seat_stop_reason"), Some("end_turn"));
    assert_eq!(
        done.str_field("token_ceiling"),
        None,
        "judged on real counts"
    );
    assert_eq!(done.int_field("context_used"), Some(3));
    assert_eq!(done.int_field("context_size"), Some(200_000));
    let estimate: Value =
        serde_json::from_str(done.str_field("cost_estimate").expect("an estimate")).expect("JSON");
    assert_eq!(
        estimate,
        json!({"amount": 0.0123, "currency": "USD", "source": "adapter_usage_update",
            "scope": "session", "billed": false})
    );
    assert_eq!(done.field("cost_usd"), None, "an estimate is never spend");
    assert_eq!(
        done.str_field("model_reported"),
        Some("claude-opus-4-5-20251101")
    );
    assert_eq!(
        done.str_field("model_reported_source"),
        Some("adapter_usage_update")
    );
    assert_eq!(
        done.str_field("seat_transcript"),
        Some(format!("{}{answer}", masked(&narration)).as_str())
    );
    no_frame_carries_the_secret(&events);
}

/// codex-acp's turn without counts (`usage: null`): `tokens` is unknown, not 0, the frame
/// says the counts were not reported, and a declared ceiling reads unjudged.
#[tokio::test]
async fn a_seat_turn_without_counts_leaves_the_tokens_unknown() {
    let frames = vec![
        text("done"),
        json!({"sessionUpdate":"usage_update","used":53_000,"size":258_400}),
    ];
    let result = json!({"stopReason":"end_turn","usage":null,
        "_meta":{"quota":{"token_count":null,"model_usage":[]}}});
    let (outcome, events) = run_seat("      max_tokens_total: 15\n", (frames, result)).await;
    assert!(outcome.ok, "{events:?}");
    let done = terminal(&events, EventKind::TaskCompleted);
    assert_eq!(done.field("tokens"), None, "unknown, never 0");
    assert_eq!(done.str_field("usage_source"), Some("unreported"));
    assert_eq!(done.str_field("token_ceiling"), Some("unjudged"));
    assert_eq!(done.field("tokens_in"), None);
    assert_eq!(done.field("cost_estimate"), None);
    assert_eq!(done.int_field("context_used"), Some(53_000));
}

/// A refused turn fails `NIKA-INFER-001` naming its reason, the text received kept in the
/// record with the key masked.
#[tokio::test]
async fn a_refused_seat_turn_fails_with_its_reason_and_the_masked_text() {
    let said = format!("I looked at {SECRET} and {SHORT}, and I can't help with that.");
    let (outcome, events) =
        run_seat("", (vec![text(&said)], json!({"stopReason":"refusal"}))).await;
    assert!(!outcome.ok);
    let error = outcome.records["ask"].error.clone().expect("a failure");
    assert_eq!(
        (error.code.as_str(), error.transient),
        ("NIKA-INFER-001", false)
    );
    assert!(
        error.message.contains("stop reason `refusal`"),
        "{}",
        error.message
    );
    assert_eq!(
        error.details.as_ref().map(|d| Value::Object(d.clone())),
        Some(json!({"partial_output": masked(&said),
            "stop_reason": "refusal"}))
    );
    let _ = terminal(&events, EventKind::TaskFailed);
    no_frame_carries_the_secret(&events);
}
