// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! NIK-402 · an `agent:` task with a `schema:` on a seat, end to end on the REAL ACP client: a
//! final message that misses the schema gets one repair prompt in the same session, listing
//! every violation; the trace shows it as `agent_nudge` (reason `schema_repair`); a repaired
//! message that misses again fails `NIKA-INFER-002` with the rejected message (redacted) and
//! every violation on `task_failed`; `on_codes: [NIKA-INFER-002]` retries on a new session.

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
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

/// A key the scripted seat repeats in a rejected message: the trace must never carry it.
const SECRET: &str = "sk-live-4d1e8b2c9a7f";

/// A password that holds the delimiters a cut may land on (a colon, a comma, a space).
const DELIMITED: &str = "ab:cd,ef gh";

/// What the seat read: the method, the session it named, and a prompt's text.
type Heard = Arc<Mutex<Vec<(String, String, String)>>>;

/// A seat that answers each prompt with the next scripted message, whatever the session; each
/// session it opens has a new id.
struct Seat {
    replies: Arc<Mutex<VecDeque<String>>>,
    sessions: Arc<AtomicUsize>,
    heard: Heard,
}

type Reader = BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>;
type Writer = tokio::io::WriteHalf<tokio::io::DuplexStream>;

async fn send(w: &mut Writer, message: &Value) {
    let mut bytes = serde_json::to_vec(message).expect("JSON");
    bytes.push(b'\n');
    let _ = w.write_all(&bytes).await;
    let _ = w.flush().await;
}

async fn peer(mut r: Reader, mut w: Writer, seat: (Arc<Mutex<VecDeque<String>>>, String, Heard)) {
    let (replies, session, heard) = seat;
    let config = json!([{"id":"model","name":"Model","category":"model","type":"select",
        "currentValue":"echo","options":[{"value":"echo","name":"Echo"}]}]);
    let mut line = String::new();
    loop {
        line.clear();
        if !matches!(r.read_line(&mut line).await, Ok(n) if n > 0) {
            return;
        }
        let request: Value = serde_json::from_str(&line).expect("JSON request");
        let method = request["method"].as_str().unwrap_or_default().to_owned();
        let text = request["params"]["prompt"][0]["text"]
            .as_str()
            .unwrap_or_default();
        let named = request["params"]["sessionId"].as_str().unwrap_or_default();
        (heard.lock().expect("heard")).push((method.clone(), named.to_owned(), text.to_owned()));
        let result = match method.as_str() {
            "initialize" => json!({"protocolVersion":1,"agentInfo":{"name":"scripted-seat"}}),
            "session/new" => json!({"sessionId":session,"configOptions":config}),
            "session/set_config_option" => json!({"configOptions":config}),
            "session/prompt" => {
                let reply = replies
                    .lock()
                    .expect("replies")
                    .pop_front()
                    .expect("a reply");
                let chunk = json!({"sessionUpdate":"agent_message_chunk",
                    "content":{"type":"text","text":reply}});
                send(
                    &mut w,
                    &json!({"jsonrpc":"2.0","method":"session/update",
                    "params":{"sessionId":session,"update":chunk}}),
                )
                .await;
                json!({"stopReason":"end_turn"})
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
        let session = format!("s-{}", self.sessions.fetch_add(1, Ordering::SeqCst) + 1);
        let seat = (Arc::clone(&self.replies), session, Arc::clone(&self.heard));
        tokio::spawn(peer(BufReader::new(peer_read), peer_write, seat));
        Box::pin(async move { Ok(nika_harness::drive(client_read, client_write, request)) })
    }
}

/// The composer's resolver, scripted: `tok` is [`SECRET`], `pass` is [`DELIMITED`].
struct Resolver;

impl WorkflowSecretResolver for Resolver {
    fn resolve(
        &self,
        name: &str,
        _reference: &nika_schema::types::SecretRef,
    ) -> Result<String, SecretResolveError> {
        match name {
            "tok" => Ok(SECRET.to_owned()),
            "pass" => Ok(DELIMITED.to_owned()),
            _ => Err(SecretResolveError {
                name: name.to_owned(),
                reason: "absent".to_owned(),
            }),
        }
    }
}

/// One run of the typed task (with `retry` when given) on a seat that answers `replies` in
/// order: the outcome, the frames, and what the seat read.
async fn run_seat(
    retry: &str,
    replies: &[&str],
) -> (RunOutcome, Vec<Event>, Vec<(String, String, String)>) {
    let yaml = format!(
        "nika: seat-schema-repair\nmodel: mock/echo\npermits: {{}}\nsecrets:\n  tok: {{ source: \
         env, key: NIKA_TOK }}\n  pass: {{ source: env, key: NIKA_PASS }}\ntasks:\n  ask:\n{retry}    \
         agent:\n      prompt: \"research the \
         anchoring bias\"\n      schema: {{ type: object, required: [title, summary, tags], \
         properties: {{ title: {{ type: string }}, summary: {{ type: string }}, tags: {{ type: \
         array }} }} }}\n"
    );
    let wf = nika_schema::parse(
        &yaml,
        nika_schema::FileId::new(0),
        nika_schema::ParseMode::Strict,
    )
    .expect("fixture parses");
    let report = nika_check::check(&wf);
    assert!(report.is_clean(), "{report:?}");
    let heard: Heard = Arc::new(Mutex::new(Vec::new()));
    let seat = Seat {
        replies: Arc::new(Mutex::new(
            replies.iter().map(|r| (*r).to_owned()).collect(),
        )),
        sessions: Arc::new(AtomicUsize::new(0)),
        heard: Arc::clone(&heard),
    };
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
    .with_harness_backend(Arc::new(seat), "scripted-seat".into())
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
    let heard = heard.lock().expect("heard").clone();
    (outcome, sink.into_events(), heard)
}

/// The violations of `{"note": ..}`, in the schema's order, as the repair prompt names them.
const THREE_MISSING: &str = "Validation failed with: 3 violations: \"title\" is a required \
                             property; \"summary\" is a required property; \"tags\" is a \
                             required property. Do not redo the work: reply with ONLY your final \
                             message";

const VALID: &str = r#"{"title": "Anchoring", "summary": "The first number sticks.", "tags": []}"#;

/// The methods the seat read, and the session and text of each prompt.
fn prompts(heard: &[(String, String, String)]) -> Vec<(String, String)> {
    (heard.iter())
        .filter(|(method, ..)| method == "session/prompt")
        .map(|(_, session, text)| (session.clone(), text.clone()))
        .collect()
}

fn methods(heard: &[(String, String, String)]) -> Vec<&str> {
    heard.iter().map(|(method, ..)| method.as_str()).collect()
}

const ONE_SESSION: [&str; 5] = [
    "initialize",
    "session/new",
    "session/set_config_option",
    "session/prompt",
    "session/prompt",
];

/// `agent_nudge` frames as (turn, reason, period).
fn nudges(events: &[Event]) -> Vec<(Option<i64>, Option<&str>, Option<i64>)> {
    (events.iter())
        .filter(|e| e.kind == EventKind::AgentNudge)
        .map(|e| {
            (
                e.int_field("turn"),
                e.str_field("reason"),
                e.int_field("period"),
            )
        })
        .collect()
}

fn terminal(events: &[Event], kind: EventKind) -> Value {
    let raw = (events.iter().rev())
        .find(|e| e.kind == kind && e.str_field("task") == Some("ask"))
        .and_then(|e| e.str_field("outcome"))
        .expect("the terminal frame's outcome");
    serde_json::from_str(raw).expect("JSON")
}

/// The missed message is repaired by the SAME session's next prompt, which names all three
/// missing keys; the repaired value is the output and the trace shows one `schema_repair`.
#[tokio::test]
async fn a_seat_message_that_misses_the_schema_is_repaired_in_its_session() {
    let (outcome, events, heard) =
        run_seat("", &[r#"{"note": "seven minutes of research"}"#, VALID]).await;
    assert!(outcome.ok, "{events:?}");
    assert_eq!(
        outcome.records["ask"].output,
        json!({"title": "Anchoring", "summary": "The first number sticks.", "tags": []})
    );
    assert_eq!(methods(&heard), ONE_SESSION);
    let asked = prompts(&heard);
    assert_eq!(
        (asked[0].0.as_str(), asked[1].0.as_str()),
        ("s-1", "s-1"),
        "one session"
    );
    assert!(
        asked[1].1.starts_with(&format!(
            "Your final message did not satisfy the required output schema. {THREE_MISSING}"
        )),
        "{}",
        asked[1].1
    );
    assert_eq!(nudges(&events), [(Some(1), Some("schema_repair"), Some(0))]);
}

/// A repaired message that misses again fails `NIKA-INFER-002` after its one repair: the
/// terminal frame keeps the rejected message, redacted, and all three violations.
#[tokio::test]
async fn a_second_miss_fails_with_the_redacted_message_and_every_violation() {
    let rejected = format!(r#"{{"note": "used {SECRET}"}}"#);
    let (outcome, events, heard) = run_seat("", &[r#"{"note": "first"}"#, rejected.as_str()]).await;
    assert!(!outcome.ok);
    let error = outcome.records["ask"].error.clone().expect("a failure");
    assert_eq!(
        (error.code.as_str(), error.transient),
        ("NIKA-INFER-002", false)
    );
    assert_eq!(methods(&heard), ONE_SESSION, "one repair, never two");
    assert!(prompts(&heard)[1].1.contains(THREE_MISSING));
    let failed = terminal(&events, EventKind::TaskFailed);
    assert_eq!(
        failed["payload"]["error"]["details"],
        json!({
            "partial_output": format!(r#"{{"note": "used {}"}}"#, secret::REDACTED),
            "violations": [
                "\"title\" is a required property",
                "\"summary\" is a required property",
                "\"tags\" is a required property"
            ]
        })
    );
    let journal = (events.iter())
        .map(|e| serde_json::to_string(e).expect("event"))
        .collect::<String>();
    assert!(!journal.contains(SECRET), "no frame carries the key");
}

/// The trace masks a secret before it cuts the rejected message: a password that holds a
/// colon, a comma and a space, across the message's 64 KiB cap, rides no frame, not even the
/// head a cut on its comma would keep (`ab:cd`). The frame keeps the masked message's first
/// 64 KiB; the record keeps the masked message whole, since a failed dispatch's record never
/// carries a secret (`dispatch::masked`).
#[tokio::test]
async fn a_secret_across_the_cut_is_masked_before_the_cut() {
    let lead = format!("{{\"note\": \"{}", "word ".repeat(13_104));
    let cap = 64 * 1024;
    assert!(
        lead.len() < cap && cap < lead.len() + DELIMITED.len(),
        "the cap falls inside the password"
    );
    let rejected = format!("{lead}{DELIMITED}{}\"}}", " word".repeat(200));
    let (outcome, events, _) = run_seat("", &[r#"{"note": "first"}"#, rejected.as_str()]).await;
    let error = outcome.records["ask"].error.clone().expect("a failure");
    assert_eq!(error.code, "NIKA-INFER-002");
    let masked = rejected.replace(DELIMITED, secret::REDACTED);
    assert_eq!(
        error.details.as_ref().and_then(|d| d.get("partial_output")),
        Some(&Value::String(masked.clone())),
        "on_error reads the whole message, masked like every failed record"
    );
    for event in &events {
        let line = serde_json::to_string(event).expect("event");
        assert!(
            !line.contains("ab:cd"),
            "{:?} keeps the password's head",
            event.kind
        );
    }
    assert_eq!(
        terminal(&events, EventKind::TaskFailed)["payload"]["error"]["details"]["partial_output"],
        format!(
            "{lead}{}…[truncated · kept {} of {} bytes]",
            secret::REDACTED,
            lead.len() + secret::REDACTED.len(),
            masked.len()
        )
    );
}

/// `on_codes: [NIKA-INFER-002]` replays a task whose repair missed: a NEW session, which gets
/// its own one repair.
#[tokio::test]
async fn on_codes_infer_002_retries_on_a_new_session() {
    let retry = "    retry: { max_attempts: 2, backoff_ms: 1, on_codes: [NIKA-INFER-002] }\n";
    let (outcome, events, heard) = run_seat(
        retry,
        &[
            r#"{"note": "a"}"#,
            r#"{"note": "b"}"#,
            r#"{"note": "c"}"#,
            VALID,
        ],
    )
    .await;
    assert!(outcome.ok, "{events:?}");
    assert_eq!(methods(&heard), ONE_SESSION.repeat(2));
    let sessions: Vec<String> = prompts(&heard).into_iter().map(|(s, _)| s).collect();
    assert_eq!(sessions, ["s-1", "s-1", "s-2", "s-2"]);
    assert_eq!(
        terminal(&events, EventKind::TaskCompleted)["payload"]["attempts"],
        json!(2)
    );
    assert_eq!(nudges(&events).len(), 2, "one repair per session");
}
