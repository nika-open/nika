// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! `session/cancel` and the `cancelled` stop, judged by the official SDK.
//!
//! The engine's one-turn client (`nika_harness::drive`) never sends `session/cancel` itself:
//! only its conversation driver does, and that driver opens only on a spawned adapter. So the
//! cases here take two routes. One drops the engine's event stream mid-turn and counts what
//! the agent heard. The other relays the engine's own cancel frame, rendered by
//! `nika_harness::wire` exactly as the conversation driver renders it, into the turn, and the
//! SDK agent answers it with a `cancelled` stop. The agent runs the prompt in a task of its own
//! (`cx.spawn`), because a handler blocks the SDK's dispatch loop and the cancel must get in.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol::v1::schema::{
    AGENT_METHOD_NAMES, AgentCapabilities, CancelNotification, ContentBlock, ContentChunk,
    InitializeRequest, InitializeResponse, NewSessionRequest, NewSessionResponse, PromptRequest,
    PromptResponse, SessionId, SessionNotification, SessionUpdate, StopReason, TextContent,
};
use agent_client_protocol::{Agent, ByteStreams};
use futures_core::Stream as _;
use nika_harness::wire;
use nika_kernel::ai::harness::{HarnessEvent, HarnessEventStream, HarnessOutcome, HarnessRequest};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader, DuplexStream};
use tokio::sync::Notify;
use tokio_util::compat::{TokioAsyncReadCompatExt as _, TokioAsyncWriteCompatExt as _};

const SESSION: &str = "sdk-cancel";
/// What the agent says before it is stopped.
const PARTIAL: &str = "half an answer";
/// A safety net only: every case ends in milliseconds when the engine behaves.
const LIMIT: Duration = Duration::from_secs(30);

/// One beat of the engine's event stream, in the order it came.
#[derive(Debug, PartialEq)]
enum Seen {
    Chunk(String),
    Completed(Box<HarnessOutcome>),
    Failed(String),
    Other(String),
}

/// How the agent's prompt task ends its turn.
#[derive(Clone, Copy)]
enum Ending {
    /// Wait for `session/cancel`, then answer `cancelled`.
    OnCancel,
    /// Wait for the test's own signal, then answer `end_turn`.
    OnRelease,
}

/// What the agent heard, and the signals its prompt task waits on.
#[derive(Default)]
struct Ears {
    /// The session of every `session/cancel` the agent parsed, in order.
    cancels: Mutex<Vec<SessionId>>,
    /// How many prompts reached the agent.
    prompts: AtomicUsize,
    /// Raised by the agent's cancel handler.
    cancelled: Notify,
    /// Raised by the test.
    release: Notify,
}

impl Ears {
    fn cancels(&self) -> Vec<SessionId> {
        self.cancels
            .lock()
            .expect("the cancel log is never poisoned")
            .clone()
    }
}

fn chunk(session_id: SessionId, text: &str) -> SessionNotification {
    SessionNotification::new(
        session_id,
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
            text,
        )))),
    )
}

/// The SDK agent: speaks `PARTIAL`, then ends its turn the way `ending` says.
async fn serve(sdk_in: DuplexStream, sdk_out: DuplexStream, ears: Arc<Ears>, ending: Ending) {
    let ears_on_cancel = Arc::clone(&ears);
    let ears_on_prompt = ears;
    let _ = Agent
        .v1()
        .name("sdk-cancel-judge")
        .on_receive_request(
            async move |initialize: InitializeRequest, responder, _connection| {
                responder.respond(
                    InitializeResponse::new(initialize.protocol_version)
                        .agent_capabilities(AgentCapabilities::new()),
                )
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_new_session: NewSessionRequest, responder, _connection| {
                responder.respond(NewSessionResponse::new(SessionId::new(SESSION)))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |prompt: PromptRequest, responder, connection| {
                ears_on_prompt.prompts.fetch_add(1, Ordering::SeqCst);
                let ears = Arc::clone(&ears_on_prompt);
                let speaker = connection.clone();
                connection.spawn(async move {
                    speaker.send_notification(chunk(prompt.session_id, PARTIAL))?;
                    let stop = match ending {
                        Ending::OnCancel => {
                            ears.cancelled.notified().await;
                            StopReason::Cancelled
                        }
                        Ending::OnRelease => {
                            ears.release.notified().await;
                            StopReason::EndTurn
                        }
                    };
                    responder.respond(PromptResponse::new(stop))
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            async move |cancel: CancelNotification, _connection| {
                ears_on_cancel
                    .cancels
                    .lock()
                    .expect("the cancel log is never poisoned")
                    .push(cancel.session_id);
                ears_on_cancel.cancelled.notify_one();
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .connect_to(ByteStreams::new(sdk_out.compat_write(), sdk_in.compat()))
        .await;
}

/// Read the engine's stream to its end.
async fn collect(mut stream: HarnessEventStream) -> Vec<Seen> {
    let mut seen = Vec::new();
    while let Some(next) = next_beat(&mut stream).await {
        seen.push(next);
    }
    seen
}

async fn next_beat(stream: &mut HarnessEventStream) -> Option<Seen> {
    let next = std::future::poll_fn(|cx| Pin::new(&mut *stream).poll_next(cx)).await?;
    Some(match next {
        Ok(HarnessEvent::MessageChunk { text }) => Seen::Chunk(text),
        Ok(HarnessEvent::Completed { outcome }) => Seen::Completed(outcome),
        Ok(other) => Seen::Other(format!("{other:?}")),
        Err(error) => Seen::Failed(error.to_string()),
    })
}

/// The cancel frame the engine's conversation driver writes for `SESSION`.
fn engine_cancel_frame() -> String {
    wire::notification_line(
        wire::METHOD_SESSION_CANCEL,
        &wire::CancelParams {
            session_id: SESSION.to_owned(),
        },
    )
    .expect("the engine renders its cancel frame")
}

/// Forward every line the engine writes to the agent, and put `cancel` right behind the
/// engine's `session/prompt`, as a client that cancels at once would.
async fn relay(from_engine: DuplexStream, mut to_agent: DuplexStream, cancel: String) {
    let mut lines = BufReader::new(from_engine).lines();
    while let Some(line) = lines.next_line().await.expect("the engine's pipe reads") {
        let frame: Value = serde_json::from_str(&line).expect("the engine writes JSON lines");
        to_agent
            .write_all(format!("{line}\n").as_bytes())
            .await
            .expect("the agent reads while the engine writes");
        if frame["method"] == wire::METHOD_SESSION_PROMPT {
            to_agent
                .write_all(format!("{cancel}\n").as_bytes())
                .await
                .expect("the agent reads the cancel");
        }
    }
}

/// The engine's cancel frame names the method the SDK routes as `session/cancel`.
#[test]
fn the_engine_and_the_sdk_name_cancel_alike() {
    assert_eq!(
        AGENT_METHOD_NAMES.session_cancel,
        wire::METHOD_SESSION_CANCEL
    );
    let frame: Value = serde_json::from_str(&engine_cancel_frame()).expect("one JSON value");
    assert_eq!(
        frame,
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": "session/cancel",
            "params": {"sessionId": SESSION},
        })
    );
}

// documents today's behaviour: after a `session/cancel` the engine reads the `cancelled` stop
// as a completed turn whose answer is the partial text. Expected to change: the outcome carries
// `cancelled` and the agent verb fails the task, keeping the partial text.
#[tokio::test]
async fn a_cancelled_turn_completes_with_the_partial_text() {
    let (engine_out, relay_in) = tokio::io::duplex(64 * 1024);
    let (relay_out, sdk_in) = tokio::io::duplex(64 * 1024);
    let (sdk_out, engine_in) = tokio::io::duplex(64 * 1024);
    let ears = Arc::new(Ears::default());
    let agent = tokio::spawn(serve(sdk_in, sdk_out, Arc::clone(&ears), Ending::OnCancel));
    let relayed = tokio::spawn(relay(relay_in, relay_out, engine_cancel_frame()));

    let stream = nika_harness::drive(
        engine_in,
        engine_out,
        HarnessRequest::new("start, then stop when told", "/tmp"),
    );
    let seen = tokio::time::timeout(LIMIT, collect(stream))
        .await
        .expect("the engine's stream ends");
    tokio::time::timeout(LIMIT, relayed)
        .await
        .expect("the relay ends with the engine")
        .expect("the relay task does not panic");
    tokio::time::timeout(LIMIT, agent)
        .await
        .expect("the agent ends once the relay hangs up")
        .expect("the agent task does not panic");

    // The SDK parsed the engine's frame as a CancelNotification for the engine's session.
    assert_eq!(ears.cancels(), vec![SessionId::new(SESSION)]);
    assert_eq!(ears.prompts.load(Ordering::SeqCst), 1);
    assert_eq!(
        seen,
        vec![
            Seen::Chunk(PARTIAL.to_owned()),
            Seen::Completed(Box::new(HarnessOutcome::new(PARTIAL))),
        ]
    );
}

// documents today's behaviour: dropping the engine's event stream mid-turn sends no
// `session/cancel`; the agent's turn runs on to its own end. Expected to change: dropping a
// turn sends `session/cancel` for its session and waits, bounded, for the `cancelled` stop.
#[tokio::test]
async fn dropping_the_stream_mid_turn_sends_no_cancel() {
    let (engine_out, sdk_in) = tokio::io::duplex(64 * 1024);
    let (sdk_out, engine_in) = tokio::io::duplex(64 * 1024);
    let ears = Arc::new(Ears::default());
    let agent = tokio::spawn(serve(sdk_in, sdk_out, Arc::clone(&ears), Ending::OnRelease));

    let mut stream = nika_harness::drive(
        engine_in,
        engine_out,
        HarnessRequest::new("start, and be left", "/tmp"),
    );
    let first = tokio::time::timeout(LIMIT, next_beat(&mut stream))
        .await
        .expect("the first beat arrives");
    assert_eq!(first, Some(Seen::Chunk(PARTIAL.to_owned())));

    // The caller walks away mid-turn, then the agent is let finish.
    drop(stream);
    ears.release.notify_one();
    tokio::time::timeout(LIMIT, agent)
        .await
        .expect("the agent ends once the engine hangs up")
        .expect("the agent task does not panic");

    assert_eq!(ears.prompts.load(Ordering::SeqCst), 1);
    assert_eq!(ears.cancels(), Vec::<SessionId>::new());
}
