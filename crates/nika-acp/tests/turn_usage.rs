// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! Token usage on a turn: a `usage_update` notification during the turn and the draft `usage`
//! of the `session/prompt` result, both sent by the official SDK to the engine's own client.
//!
//! The draft field needs the SDK feature `unstable_end_turn_token_usage` (see the manifest).
//! A tap between the agent and the engine keeps every frame the agent wrote. The first case
//! asserts both reports in their exact wire shape, and that the engine read that same turn to
//! its end: the chunk, then a completed outcome carrying the turn's text. The second case pins
//! what the engine kept of the two reports.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::pin::Pin;
use std::time::Duration;

use agent_client_protocol::v1::schema::{
    AgentCapabilities, ContentBlock, ContentChunk, Cost, InitializeRequest, InitializeResponse,
    NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse, SessionId,
    SessionNotification, SessionUpdate, StopReason, TextContent, Usage, UsageUpdate,
};
use agent_client_protocol::{Agent, ByteStreams};
use futures_core::Stream as _;
use nika_kernel::ai::harness::{HarnessEvent, HarnessEventStream, HarnessOutcome, HarnessRequest};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader, DuplexStream};
use tokio_util::compat::{TokioAsyncReadCompatExt as _, TokioAsyncWriteCompatExt as _};

const SESSION: &str = "sdk-usage";
const CHUNK: &str = "an answer with a meter on it";
/// A safety net only: the case ends in milliseconds when the engine behaves.
const LIMIT: Duration = Duration::from_secs(30);

/// One beat of the engine's event stream, in the order it came.
#[derive(Debug, PartialEq)]
enum Seen {
    Chunk(String),
    Completed(Box<HarnessOutcome>),
    Failed(String),
    Other(String),
}

/// The usage the agent reports while the turn runs: context used and size, and a cost.
fn usage_update() -> UsageUpdate {
    UsageUpdate::new(1_200, 200_000).cost(Cost::new(0.0123, "USD"))
}

/// The usage the agent reports with the prompt result.
fn turn_usage() -> Usage {
    Usage::new(1_500, 1_000, 500)
        .thought_tokens(120)
        .cached_read_tokens(300)
        .cached_write_tokens(40)
}

/// The SDK agent: one `usage_update`, one chunk, then `end_turn` carrying `turn_usage`.
async fn serve(sdk_in: DuplexStream, sdk_out: DuplexStream) {
    let _ = Agent
        .v1()
        .name("sdk-usage-reporter")
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
                    SessionUpdate::UsageUpdate(usage_update()),
                ))?;
                connection.send_notification(SessionNotification::new(
                    prompt.session_id.clone(),
                    SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                        TextContent::new(CHUNK),
                    ))),
                ))?;
                responder.respond(PromptResponse::new(StopReason::EndTurn).usage(turn_usage()))
            },
            agent_client_protocol::on_receive_request!(),
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

/// Drive one turn: engine → agent directly, agent → tap → engine.
async fn metered_turn() -> (Vec<Seen>, Vec<Value>) {
    let (engine_out, sdk_in) = tokio::io::duplex(64 * 1024);
    let (sdk_out, tap_in) = tokio::io::duplex(64 * 1024);
    let (tap_out, engine_in) = tokio::io::duplex(64 * 1024);
    let agent = tokio::spawn(serve(sdk_in, sdk_out));
    let tapped = tokio::spawn(tap(tap_in, tap_out));

    let stream = nika_harness::drive(
        engine_in,
        engine_out,
        HarnessRequest::new("answer, and say what it cost", "/tmp"),
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
    (seen, frames)
}

/// The `session/update` the engine was given for `kind`, as it crossed the wire.
fn update_of_kind<'a>(frames: &'a [Value], kind: &str) -> Vec<&'a Value> {
    frames
        .iter()
        .filter(|frame| frame["method"] == "session/update")
        .map(|frame| &frame["params"]["update"])
        .filter(|update| update["sessionUpdate"] == kind)
        .collect()
}

#[tokio::test]
async fn both_usage_reports_reach_the_engine_and_the_turn_completes() {
    let (seen, frames) = metered_turn().await;

    let reported = json!({
        "sessionUpdate": "usage_update",
        "used": 1_200,
        "size": 200_000,
        "cost": {"amount": 0.0123, "currency": "USD"},
    });
    assert_eq!(update_of_kind(&frames, "usage_update"), vec![&reported]);

    let closed = json!({
        "stopReason": "end_turn",
        "usage": {
            "totalTokens": 1_500,
            "inputTokens": 1_000,
            "outputTokens": 500,
            "thoughtTokens": 120,
            "cachedReadTokens": 300,
            "cachedWriteTokens": 40,
        },
    });
    let results: Vec<&Value> = frames
        .iter()
        .filter_map(|frame| frame.get("result"))
        .filter(|result| result.get("stopReason").is_some())
        .collect();
    assert_eq!(results, vec![&closed]);

    // The engine read the turn through both reports to its end: a refused report would show
    // here as a failure or a missing completion. What it keeps of them is the next case's.
    let [Seen::Chunk(text), Seen::Completed(outcome)] = seen.as_slice() else {
        panic!("the engine reads the turn to its end: {seen:?}");
    };
    assert_eq!(text.as_str(), CHUNK);
    assert_eq!(outcome.output, CHUNK);
}

// documents today's behaviour: the engine keeps neither usage report. The `usage_update`
// yields no event and the outcome's `usage` stays empty. Expected to change: the reader of
// these two shapes must keep them apart. The schema documents the prompt result's `usage` as
// "Token usage for this turn", although the fields of its `Usage` read "Sum of all token types
// across session" and "Total input tokens across all turns". The `usage_update` is a "Context
// window and cost update for a session": `used` is "Tokens currently in context", `size` the
// "Total context window size in tokens", and `cost` a "Cumulative session cost". The change
// has to decide whether the token counts are this turn's or the session's, and keep the
// context window and the cumulative cost apart from this turn's spend.
#[tokio::test]
async fn the_engine_keeps_neither_usage_report() {
    let (seen, _) = metered_turn().await;

    assert_eq!(
        seen,
        vec![
            Seen::Chunk(CHUNK.to_owned()),
            Seen::Completed(Box::new(HarnessOutcome::new(CHUNK))),
        ]
    );
    let Some(Seen::Completed(outcome)) = seen.last() else {
        panic!("the turn completes: {seen:?}");
    };
    assert_eq!(outcome.usage, None, "no usage reaches the outcome today");
}
