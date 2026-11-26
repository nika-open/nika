// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! `session/close` against an agent that advertises it, judged by the official SDK.
//!
//! The SDK agent advertises `sessionCapabilities.close` on `initialize` and logs every
//! `session/close` and `session/cancel` it parses. The engine's own client drives one turn
//! against it. The same agent is first proven able to hear a close at all: the SDK's own client
//! closes a session on it, so a log that stays empty under the engine means the engine sent
//! none.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::v1::ConnectionTo;
use agent_client_protocol::v1::schema::{
    AGENT_METHOD_NAMES, AgentCapabilities, CancelNotification, CloseSessionRequest,
    CloseSessionResponse, ContentBlock, ContentChunk, InitializeRequest, InitializeResponse,
    NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse, SessionCapabilities,
    SessionCloseCapabilities, SessionId, SessionNotification, SessionUpdate, StopReason,
    TextContent,
};
use agent_client_protocol::{Agent, ByteStreams, Client};
use futures_core::Stream as _;
use nika_kernel::ai::harness::{HarnessEvent, HarnessEventStream, HarnessOutcome, HarnessRequest};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader, DuplexStream};
use tokio_util::compat::{TokioAsyncReadCompatExt as _, TokioAsyncWriteCompatExt as _};

const SESSION: &str = "sdk-closable";
const CHUNK: &str = "an answer from a closable session";
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

/// What the agent heard: the session of every close and every cancel, in order.
#[derive(Default)]
struct Log {
    closes: Mutex<Vec<SessionId>>,
    cancels: Mutex<Vec<SessionId>>,
}

impl Log {
    fn closes(&self) -> Vec<SessionId> {
        self.closes
            .lock()
            .expect("the close log is never poisoned")
            .clone()
    }

    fn cancels(&self) -> Vec<SessionId> {
        self.cancels
            .lock()
            .expect("the cancel log is never poisoned")
            .clone()
    }
}

/// The capabilities the agent advertises: everything at its default, plus `session/close`.
fn closable() -> AgentCapabilities {
    AgentCapabilities::new()
        .session_capabilities(SessionCapabilities::new().close(SessionCloseCapabilities::new()))
}

/// The SDK agent: a one-chunk `end_turn` turn, and a `session/close` it answers and logs.
async fn serve(sdk_in: DuplexStream, sdk_out: DuplexStream, log: Arc<Log>) {
    let log_on_cancel = Arc::clone(&log);
    let log_on_close = log;
    let _ = Agent
        .v1()
        .name("sdk-closable-agent")
        .on_receive_request(
            async move |initialize: InitializeRequest, responder, _connection| {
                responder.respond(
                    InitializeResponse::new(initialize.protocol_version)
                        .agent_capabilities(closable()),
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
                connection.send_notification(SessionNotification::new(
                    prompt.session_id.clone(),
                    SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                        TextContent::new(CHUNK),
                    ))),
                ))?;
                responder.respond(PromptResponse::new(StopReason::EndTurn))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |close: CloseSessionRequest, responder, _connection| {
                log_on_close
                    .closes
                    .lock()
                    .expect("the close log is never poisoned")
                    .push(close.session_id);
                responder.respond(CloseSessionResponse::new())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            async move |cancel: CancelNotification, _connection| {
                log_on_cancel
                    .cancels
                    .lock()
                    .expect("the cancel log is never poisoned")
                    .push(cancel.session_id);
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .connect_to(ByteStreams::new(sdk_out.compat_write(), sdk_in.compat()))
        .await;
}

/// Forward every line the agent writes to the engine, keeping a parsed copy of each. Once the
/// engine has hung up, the rest is still kept, just no longer delivered.
async fn tap(from_agent: DuplexStream, mut to_engine: DuplexStream) -> Vec<Value> {
    let mut lines = BufReader::new(from_agent).lines();
    let mut frames = Vec::new();
    let mut engine_listens = true;
    while let Some(line) = lines.next_line().await.expect("the agent's pipe reads") {
        frames.push(serde_json::from_str(&line).expect("the agent writes one JSON value a line"));
        if engine_listens {
            let delivered = to_engine.write_all(format!("{line}\n").as_bytes()).await;
            engine_listens = delivered.is_ok();
        }
    }
    frames
}

/// Read the engine's stream to its end.
async fn collect(mut stream: HarnessEventStream) -> Vec<Seen> {
    let mut seen = Vec::new();
    while let Some(next) = std::future::poll_fn(|cx| Pin::new(&mut stream).poll_next(cx)).await {
        seen.push(match next {
            Ok(HarnessEvent::MessageChunk { text }) => Seen::Chunk(text),
            Ok(HarnessEvent::Completed { outcome }) => Seen::Completed(outcome),
            Ok(other) => Seen::Other(format!("{other:?}")),
            Err(error) => Seen::Failed(error.to_string()),
        });
    }
    seen
}

/// The SDK's own client closes a session on the agent: the instrument hears a close.
#[tokio::test]
async fn the_agent_hears_a_close_from_the_sdks_own_client() {
    let (client_out, sdk_in) = tokio::io::duplex(64 * 1024);
    let (sdk_out, client_in) = tokio::io::duplex(64 * 1024);
    let log = Arc::new(Log::default());
    let agent = tokio::spawn(serve(sdk_in, sdk_out, Arc::clone(&log)));

    let answered = tokio::time::timeout(
        LIMIT,
        Client.v1().name("sdk-closing-client").connect_with(
            ByteStreams::new(client_out.compat_write(), client_in.compat()),
            |connection: ConnectionTo<Agent>| async move {
                let init = connection
                    .send_request(InitializeRequest::new(ProtocolVersion::V1))
                    .block_task()
                    .await?;
                let session = connection
                    .send_request(NewSessionRequest::new(PathBuf::from("/")))
                    .block_task()
                    .await?;
                let closed = connection
                    .send_request(CloseSessionRequest::new(session.session_id))
                    .block_task()
                    .await?;
                Ok::<_, agent_client_protocol::Error>((init.agent_capabilities, closed))
            },
        ),
    )
    .await
    .expect("the client's dialogue ends")
    .expect("the agent answers initialize, session/new and session/close");
    tokio::time::timeout(LIMIT, agent)
        .await
        .expect("the agent ends once the client hangs up")
        .expect("the agent task does not panic");

    assert_eq!(answered, (closable(), CloseSessionResponse::new()));
    assert_eq!(log.closes(), vec![SessionId::new(SESSION)]);
    assert_eq!(log.cancels(), Vec::<SessionId>::new());
}

// documents today's behaviour: the engine never sends `session/close`, even to an agent that
// advertises it; the session is left to end with the pipe. Expected to change: once the turn
// is read, the engine closes a session the agent advertised as closable, and sends no close to
// an agent that did not advertise it.
#[tokio::test]
async fn the_engine_sends_no_close_to_an_agent_that_advertises_it() {
    let (engine_out, sdk_in) = tokio::io::duplex(64 * 1024);
    let (sdk_out, tap_in) = tokio::io::duplex(64 * 1024);
    let (tap_out, engine_in) = tokio::io::duplex(64 * 1024);
    let log = Arc::new(Log::default());
    let agent = tokio::spawn(serve(sdk_in, sdk_out, Arc::clone(&log)));
    let tapped = tokio::spawn(tap(tap_in, tap_out));

    let stream = nika_harness::drive(
        engine_in,
        engine_out,
        HarnessRequest::new("answer once", "/tmp"),
    );
    let seen = tokio::time::timeout(LIMIT, collect(stream))
        .await
        .expect("the engine's stream ends");
    tokio::time::timeout(LIMIT, agent)
        .await
        .expect("the agent ends once the engine hangs up")
        .expect("the agent task does not panic");
    let frames = tokio::time::timeout(LIMIT, tapped)
        .await
        .expect("the tap ends with the agent")
        .expect("the tap task does not panic");

    // The engine was told the session is closable.
    let advertised: Vec<&Value> = frames
        .iter()
        .filter_map(|frame| frame.pointer("/result/agentCapabilities/sessionCapabilities"))
        .collect();
    assert_eq!(advertised, vec![&json!({"close": {}})]);
    // The turn completed, and the agent heard neither a close nor a cancel.
    assert_eq!(
        seen,
        vec![
            Seen::Chunk(CHUNK.to_owned()),
            Seen::Completed(Box::new(HarnessOutcome::new(CHUNK))),
        ]
    );
    assert_eq!(log.closes(), Vec::<SessionId>::new());
    assert_eq!(log.cancels(), Vec::<SessionId>::new());
}

/// The wire shapes of `session/close`, as the SDK writes them.
#[test]
fn the_sdk_writes_close_as_v1_names_it() {
    assert_eq!(AGENT_METHOD_NAMES.session_close, "session/close");
    assert_eq!(
        serde_json::to_value(CloseSessionRequest::new(SessionId::new(SESSION)))
            .expect("the request serializes"),
        json!({"sessionId": SESSION})
    );
    assert_eq!(
        serde_json::to_value(CloseSessionResponse::new()).expect("the response serializes"),
        json!({})
    );
    assert_eq!(
        serde_json::to_value(closable().session_capabilities).expect("the capability serializes"),
        json!({"close": {}})
    );
}
