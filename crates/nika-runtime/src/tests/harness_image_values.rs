// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! B-D18 · a seat's image as a workflow value, end to end on the REAL ACP client: a scripted
//! agent sends one image on ANOTHER session first, then its own image while reporting a saved
//! path that names a real file holding a third image (the shared generated-images folder of
//! the gauntlet run, where Codex's copy picked another session's PNG). The task's
//! `images: { save: }` must write OUR session's bytes, bind them downstream, and record them in
//! the journal with their digest; the foreign image and the reported file are never taken.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use base64::Engine as _;
use nika_kernel::ai::harness::{DynAgentBackend, HarnessError, HarnessEventStream, HarnessRequest};
use nika_kernel::tool_executor::ToolResult;
use nika_kernel_mock::{
    MockClock, MockProvider, MockShell, MockToolDefinitionProvider, MockToolExecutor,
};
use nika_verb_agent::AgentVerb;
use nika_verb_exec::ExecVerb;
use nika_verb_infer::InferVerb;
use nika_verb_invoke::InvokeVerb;
use serde_json::{Value, json};
use sha2::Digest as _;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use super::*;

/// A PNG header (1×1) and a tag: three distinct images.
fn png(tag: &str) -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01\x08\x02\0\0\0".to_vec();
    bytes.extend_from_slice(tag.as_bytes());
    bytes
}

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", sha2::Sha256::digest(bytes))
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

type Reader = BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>;
type Writer = tokio::io::WriteHalf<tokio::io::DuplexStream>;

async fn send(w: &mut Writer, message: &Value) {
    let mut bytes = serde_json::to_vec(message).expect("JSON");
    bytes.push(b'\n');
    let _ = w.write_all(&bytes).await;
    let _ = w.flush().await;
}

fn update(session: &str, update: &Value) -> Value {
    json!({"jsonrpc":"2.0","method":"session/update",
        "params":{"sessionId":session,"update":update}})
}

/// One image operation as codex-acp reports it: the start, then the completed result with the
/// bytes in `rawOutput.result`, the same bytes as an ACP image block, and its `savedPath`.
async fn image(w: &mut Writer, session: &str, id: &str, bytes: &[u8], saved: &str) {
    let start = json!({"sessionUpdate":"tool_call","toolCallId":id,"title":"Image generation",
        "status":"in_progress"});
    send(w, &update(session, &start)).await;
    let data = b64(bytes);
    let done = json!({"sessionUpdate":"tool_call_update","toolCallId":id,"status":"completed",
        "rawOutput":{"status":"completed","result":data,"savedPath":saved},
        "content":[{"type":"content","content":{"type":"image","mimeType":"image/png","data":data}}]});
    send(w, &update(session, &done)).await;
}

/// The scripted seat: the model option it confirms, then one turn.
async fn peer(mut r: Reader, mut w: Writer, ours: Vec<u8>, foreign: Vec<u8>, shared: String) {
    let config = json!([{"id":"model","name":"Model","category":"model","type":"select",
        "currentValue":"echo","options":[{"value":"echo","name":"Echo"}]}]);
    let mut line = String::new();
    loop {
        line.clear();
        let Ok(n) = r.read_line(&mut line).await else {
            return;
        };
        if n == 0 {
            return;
        }
        let request: Value = serde_json::from_str(&line).expect("JSON request");
        let result = match request["method"].as_str() {
            Some("initialize") => json!({"protocolVersion":1,"agentInfo":{"name":"scripted-seat"}}),
            Some("session/new") => json!({"sessionId":"s-ours","configOptions":config}),
            Some("session/set_config_option") => json!({"configOptions":config}),
            Some("session/prompt") => {
                // Another session's image, interleaved on the same wire: never this task's.
                image(&mut w, "s-theirs", "ig-theirs", &foreign, &shared).await;
                // Ours, whose reported path names the shared folder's newest file.
                image(&mut w, "s-ours", "ig-ours", &ours, &shared).await;
                let chunk = json!({"sessionUpdate":"agent_message_chunk",
                    "content":{"type":"text","text":"a paper boat"}});
                send(&mut w, &update("s-ours", &chunk)).await;
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

struct Seat {
    ours: Vec<u8>,
    foreign: Vec<u8>,
    shared: String,
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
            self.ours.clone(),
            self.foreign.clone(),
            self.shared.clone(),
        ));
        Box::pin(async move { Ok(nika_harness::drive(client_read, client_write, request)) })
    }
}

#[tokio::test]
async fn the_saved_image_is_this_sessions_bytes_bound_downstream_and_journaled() {
    let base = std::env::temp_dir().join(format!("nika-image-values-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let (project, shared_dir) = (base.join("project"), base.join("generated_images"));
    std::fs::create_dir_all(&project).expect("project");
    std::fs::create_dir_all(&shared_dir).expect("shared folder");
    let (ours, foreign, newest) = (png("ours"), png("theirs"), png("shared-newest"));
    let shared = shared_dir.join("newest.png");
    std::fs::write(&shared, &newest).expect("the shared folder's newest image");

    let tools = Arc::new(
        MockToolExecutor::new()
            .enqueue_ok(ToolResult::success("w", "./out/banner.png"))
            .enqueue_ok(ToolResult::success("fx", "{}").with_structured(json!({
                "path": "./out/banner-fx.png", "sha256": sha(b"fx"), "size_bytes": 2,
                "format": "png"}))),
    );
    let invoke = Arc::new(InvokeVerb::new(Arc::clone(&tools)));
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
            Arc::new(MockProvider::new("mock")),
            invoke,
            Arc::new(MockToolDefinitionProvider::new()),
            "mock/echo",
        ),
        MockClock::new(),
        RuntimeConfig::default().with_sandbox_root(project.clone()),
    )
    .with_harness_backend(
        Arc::new(Seat {
            ours: ours.clone(),
            foreign: foreign.clone(),
            shared: shared.to_string_lossy().into_owned(),
        }),
        "scripted-seat".into(),
    )
    .expect("seated");

    let yaml = "nika: image-values\nmodel: mock/echo\npermits:\n  tools: [\"nika:image_fx\"]\n  \
        fs:\n    read: [\"./out/**\"]\n    write: [\"./out/**\"]\ntasks:\n  banner:\n    agent:\n      \
        prompt: \"draw a paper boat\"\n      images:\n        save: \"./out/banner.png\"\n  \
        fx:\n    with:\n      banner: ${{ tasks.banner.output.images[0].path }}\n    invoke:\n      \
        tool: \"nika:image_fx\"\n      args: { input: \"${{ with.banner }}\", out: \
        \"./out/banner-fx.png\", ops: [{ grayscale: {} }] }\n";
    let wf = nika_schema::parse(
        yaml,
        nika_schema::FileId::new(0),
        nika_schema::ParseMode::Strict,
    )
    .expect("fixture parses");
    let report = nika_check::check(&wf);
    assert!(report.is_clean(), "{report:?}");
    let mut sink = VecSink::new();
    let outcome = runtime
        .run(&wf, &report, &mut DeterministicStamper::new(), &mut sink)
        .await
        .expect("the run settles");
    let events = sink.into_events();
    assert!(outcome.ok, "{events:?}");

    // The write: OUR bytes, never the other session's, never the reported file's.
    let calls = tools.captured_calls();
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert_eq!(calls[0].name, "nika:write");
    assert_eq!(calls[0].input["path"], "./out/banner.png");
    let written = base64::engine::general_purpose::STANDARD
        .decode(
            calls[0].input["content"]["bytes_base64"]
                .as_str()
                .expect("bytes"),
        )
        .expect("base64");
    assert_eq!(written, ours, "this session's bytes");
    assert_ne!(written, foreign, "never another session's image");
    assert_ne!(written, newest, "never the file the harness reported");
    // The binding: the next task reads the path the engine wrote.
    assert_eq!(calls[1].name, "nika:image_fx");
    assert_eq!(calls[1].input["input"], "./out/banner.png");

    // The value: one image, ours, with its digest; the message beside it.
    let output = &outcome.records["banner"].output;
    assert_eq!(output["message"], "a paper boat");
    assert_eq!(output["images"].as_array().map(Vec::len), Some(1));
    assert_eq!(output["images"][0]["sha256"], sha(&ours));
    assert_eq!(output["images"][0]["path"], "./out/banner.png");
    assert_eq!(
        (
            output["images"][0]["width"].clone(),
            output["images"][0]["height"].clone()
        ),
        (json!(1), json!(1))
    );

    // The journal: one observation, ours, with the received digest and where it was written.
    let frames: Vec<Value> = events
        .iter()
        .filter(|e| e.kind == EventKind::AgentImageObserved)
        .map(|e| {
            let raw = e
                .fields
                .iter()
                .find(|f| f.key == "harness_image")
                .expect("frame");
            let FieldValue::String(raw) = &raw.value else {
                panic!("image metadata is JSON text");
            };
            serde_json::from_str(raw).expect("observation")
        })
        .collect();
    assert_eq!(
        frames.len(),
        1,
        "the other session's image is never observed: {frames:?}"
    );
    assert_eq!(frames[0]["tool_call_id"], "ig-ours");
    assert_eq!(frames[0]["received_sha256"], sha(&ours));
    assert_eq!(frames[0]["saved_path"], "./out/banner.png");
    assert_eq!(
        frames[0]["reported_saved_path"],
        shared.to_string_lossy().as_ref()
    );
    assert_eq!(frames[0]["file_verified"], false);
    assert_eq!(
        std::fs::read(&shared).expect("shared"),
        newest,
        "the reported file is left as it was"
    );
    let _ = std::fs::remove_dir_all(&base);
}

/// A seat that counts the turns it is asked for, then plays [`Seat`].
struct CountingSeat {
    seat: Seat,
    turns: Arc<std::sync::atomic::AtomicUsize>,
}

impl DynAgentBackend for CountingSeat {
    fn run_agent_boxed(
        &self,
        request: HarnessRequest,
    ) -> Pin<Box<dyn Future<Output = Result<HarnessEventStream, HarnessError>> + Send + '_>> {
        self.turns.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.seat.run_agent_boxed(request)
    }
}

/// Answers every secret with one value.
struct OneSecret(&'static str);

impl crate::WorkflowSecretResolver for OneSecret {
    fn resolve(
        &self,
        _name: &str,
        _reference: &nika_schema::types::SecretRef,
    ) -> Result<String, crate::SecretResolveError> {
        Ok(self.0.to_owned())
    }
}

/// The value of `secrets.k`: rendered, it would name the file the run writes.
const SAVE_CANARY: &str = "canary-7Qx2";

/// A secret in `images.save:`, on the report an older check gave (one that passed the field
/// clean), on a seat that would deliver an image: the run refuses the task before it renders the
/// path. The seat never starts a turn, no `nika:write` is asked for, the record holds no image,
/// and nothing the run wrote carries the value.
#[tokio::test]
async fn a_secret_in_the_save_path_is_refused_before_the_seat_starts() {
    let base = std::env::temp_dir().join(format!("nika-image-save-secret-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let project = base.join("project");
    std::fs::create_dir_all(&project).expect("project");

    let tools =
        Arc::new(MockToolExecutor::new().enqueue_ok(ToolResult::success("w", "./out/never.png")));
    let invoke = Arc::new(InvokeVerb::new(Arc::clone(&tools)));
    let turns = Arc::new(std::sync::atomic::AtomicUsize::new(0));
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
            Arc::new(MockProvider::new("mock")),
            invoke,
            Arc::new(MockToolDefinitionProvider::new()),
            "mock/echo",
        ),
        MockClock::new(),
        RuntimeConfig::default().with_sandbox_root(project.clone()),
    )
    .with_harness_backend(
        Arc::new(CountingSeat {
            seat: Seat {
                ours: png("ours"),
                foreign: png("theirs"),
                shared: project.join("none.png").to_string_lossy().into_owned(),
            },
            turns: Arc::clone(&turns),
        }),
        "scripted-seat".into(),
    )
    .expect("seated")
    .with_secret_resolver(Arc::new(OneSecret(SAVE_CANARY)));

    let yaml = "nika: image-save-secret\nmodel: mock/echo\nsecrets:\n  k: { source: env, key: \
        NIKA_IMAGE_SAVE_K }\npermits:\n  fs:\n    write: [\"./out/**\"]\ntasks:\n  draw:\n    \
        agent:\n      prompt: \"draw a paper boat\"\n      images:\n        save: \
        \"./out/${{ secrets.k }}.png\"\n";
    let wf = nika_schema::parse(
        yaml,
        nika_schema::FileId::new(0),
        nika_schema::ParseMode::Strict,
    )
    .expect("fixture parses");
    // Today's check refuses the file on `draw`; the older report passed it clean.
    let mut report = nika_check::check(&wf);
    assert_eq!(
        report
            .secret_leaks
            .iter()
            .map(|l| (l.task.as_str(), l.sink, l.trace.as_str()))
            .collect::<Vec<_>>(),
        [(
            "draw",
            "agent.images.save",
            "secrets.k → images.save @ draw"
        )]
    );
    report.secret_leaks.clear();
    assert!(report.is_clean(), "{report:?}");

    let mut sink = VecSink::new();
    let outcome = runtime
        .run(&wf, &report, &mut DeterministicStamper::new(), &mut sink)
        .await
        .expect("the launch cannot judge a field only the run renders");
    let events = sink.into_events();

    assert!(!outcome.ok, "{events:?}");
    let record = &outcome.records["draw"];
    let error = record.error.as_ref().expect("the refusal is on record");
    assert_eq!(
        (error.code.as_str(), error.message.as_str()),
        (
            "NIKA-SEC-006",
            "refused before the task ran: a secret reaches the `images.save:` of task `draw` \
             (secrets.k → images.save @ draw), and the run renders no secret there · name the \
             path there, or read it from `inputs:` or `const:`"
        )
    );
    assert_eq!(
        turns.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "the seat never starts a turn"
    );
    assert_eq!(
        tools.captured_calls().len(),
        0,
        "no `nika:write` is asked for"
    );
    assert!(record.output["images"].is_null(), "{:?}", record.output);
    assert_eq!(
        events
            .iter()
            .filter(|e| e.str_field("task") == Some("draw"))
            .map(|e| e.kind)
            .collect::<Vec<_>>(),
        [EventKind::TaskScheduled, EventKind::TaskFailed]
    );
    let carrying: Vec<String> = events
        .iter()
        .filter(|e| {
            serde_json::to_string(e)
                .expect("an event serializes")
                .contains(SAVE_CANARY)
        })
        .map(|e| format!("{:?}", e.kind))
        .collect();
    assert_eq!(carrying, Vec::<String>::new(), "no event carries the value");
    assert!(
        !format!("{:?}", outcome.records).contains(SAVE_CANARY),
        "no record carries the value"
    );
    assert!(!project.join("out").exists(), "nothing is written");
    let _ = std::fs::remove_dir_all(&base);
}
