// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! Every wire v1 stop reason, read by the engine's own client.
//!
//! The official SDK plays the agent: one text chunk, then a `session/prompt` result that ends
//! the turn with one stop reason. The engine's `nika_harness::drive` plays the client over
//! in-process pipes, as in `conformance.rs`. Each case pins what the engine yields for that
//! stop reason today, whole outcome included, so a change to what the engine keeps from a stop
//! reason shows here first.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::pin::Pin;
use std::time::Duration;

use agent_client_protocol::v1::schema::{
    AgentCapabilities, ContentBlock, ContentChunk, InitializeRequest, InitializeResponse,
    NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse, SessionId,
    SessionNotification, SessionUpdate, StopReason, TextContent,
};
use agent_client_protocol::{Agent, ByteStreams};
use futures_core::Stream as _;
use nika_kernel::ai::harness::{HarnessEvent, HarnessEventStream, HarnessOutcome, HarnessRequest};
use serde_json::json;
use tokio::io::{DuplexStream, ReadHalf, WriteHalf};
use tokio_util::compat::{TokioAsyncReadCompatExt as _, TokioAsyncWriteCompatExt as _};

/// The session id the agent hands out.
const SESSION: &str = "sdk-stop";
/// The one chunk the agent speaks before it stops.
const CHUNK: &str = "the words before the stop";
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

/// The SDK agent: answers `initialize` and `session/new`, then speaks `CHUNK` and ends the
/// turn with `stop`.
fn spawn_agent(
    reader: ReadHalf<DuplexStream>,
    writer: WriteHalf<DuplexStream>,
    stop: StopReason,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let _ = Agent
            .v1()
            .name("sdk-stop-reasons")
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
                    connection.send_notification(SessionNotification::new(
                        prompt.session_id.clone(),
                        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                            TextContent::new(CHUNK),
                        ))),
                    ))?;
                    responder.respond(PromptResponse::new(stop))
                },
                agent_client_protocol::on_receive_request!(),
            )
            .connect_to(ByteStreams::new(writer.compat_write(), reader.compat()))
            .await;
    })
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

/// Drive one engine turn against an agent that stops with `stop`.
async fn turn_ending_with(stop: StopReason) -> Vec<Seen> {
    let (engine_side, sdk_side) = tokio::io::duplex(64 * 1024);
    let (engine_read, engine_write) = tokio::io::split(engine_side);
    let (sdk_read, sdk_write) = tokio::io::split(sdk_side);
    let agent = spawn_agent(sdk_read, sdk_write, stop);

    let stream = nika_harness::drive(
        engine_read,
        engine_write,
        HarnessRequest::new("speak, then stop", "/tmp"),
    );
    let seen = tokio::time::timeout(LIMIT, collect(stream))
        .await
        .expect("the engine's stream ends");
    tokio::time::timeout(LIMIT, agent)
        .await
        .expect("the agent ends once the engine hangs up")
        .expect("the agent task does not panic");
    seen
}

/// What the engine yields today for every stop reason: the chunk, then a completed outcome
/// whose text is the chunk and whose every other field is empty.
fn completed_with_the_chunk() -> Vec<Seen> {
    vec![
        Seen::Chunk(CHUNK.to_owned()),
        Seen::Completed(Box::new(HarnessOutcome::new(CHUNK))),
    ]
}

// documents today's behaviour: the outcome of an `end_turn` names no stop reason. Expected to
// change: the outcome carries `end_turn`, and the turn still completes with its text.
#[tokio::test]
async fn end_turn_completes_with_the_turns_text() {
    assert_eq!(
        turn_ending_with(StopReason::EndTurn).await,
        completed_with_the_chunk()
    );
}

// documents today's behaviour: a `refusal` completes exactly like `end_turn`, the refused text
// standing as the answer. Expected to change: the outcome carries `refusal` and the agent verb
// fails the task instead of accepting the text.
#[tokio::test]
async fn refusal_completes_like_end_turn() {
    assert_eq!(
        turn_ending_with(StopReason::Refusal).await,
        completed_with_the_chunk()
    );
}

// documents today's behaviour: a `max_tokens` cut completes exactly like `end_turn`, nothing
// marking the text as cut. Expected to change: the outcome carries `max_tokens` and the agent
// verb reports the text as cut short.
#[tokio::test]
async fn max_tokens_completes_like_end_turn() {
    assert_eq!(
        turn_ending_with(StopReason::MaxTokens).await,
        completed_with_the_chunk()
    );
}

// documents today's behaviour: a `max_turn_requests` stop completes exactly like `end_turn`.
// Expected to change: the outcome carries `max_turn_requests` and the agent verb fails the
// task, keeping the partial text.
#[tokio::test]
async fn max_turn_requests_completes_like_end_turn() {
    assert_eq!(
        turn_ending_with(StopReason::MaxTurnRequests).await,
        completed_with_the_chunk()
    );
}

// documents today's behaviour: a `cancelled` stop completes exactly like `end_turn`, the cut
// text standing as the answer. Expected to change: the outcome carries `cancelled` and the
// agent verb fails the task, keeping the partial text.
#[tokio::test]
async fn cancelled_completes_like_end_turn() {
    assert_eq!(
        turn_ending_with(StopReason::Cancelled).await,
        completed_with_the_chunk()
    );
}

/// The five cases above send these exact words: the SDK's names for the stop reasons, which
/// the engine reads as plain strings.
#[test]
fn the_sdk_names_the_five_stop_reasons_on_the_wire() {
    let names: Vec<serde_json::Value> = [
        StopReason::EndTurn,
        StopReason::Refusal,
        StopReason::MaxTokens,
        StopReason::MaxTurnRequests,
        StopReason::Cancelled,
    ]
    .iter()
    .map(|stop| serde_json::to_value(stop).expect("a stop reason serializes"))
    .collect();
    assert_eq!(
        names,
        vec![
            json!("end_turn"),
            json!("refusal"),
            json!("max_tokens"),
            json!("max_turn_requests"),
            json!("cancelled"),
        ]
    );
}
