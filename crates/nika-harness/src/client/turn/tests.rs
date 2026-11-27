// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! An agent turn's facts beyond its text, over a scripted duplex peer: its messages, how it
//! ended, the counts, context, cost and model the adapter reports, and the bound on its text.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::pin::Pin;

use futures_core::Stream;
use nika_kernel::ai::harness::{
    HarnessEvent, HarnessEventStream, HarnessRequest, HarnessStopReason,
};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, ReadHalf, WriteHalf};

use super::*;

type PeerReader = BufReader<ReadHalf<DuplexStream>>;
type PeerWriter = WriteHalf<DuplexStream>;

const SESSION: &str = "s-turn";

async fn read(peer: &mut PeerReader) -> Value {
    let mut line = String::new();
    tokio::time::timeout(std::time::Duration::from_secs(5), peer.read_line(&mut line))
        .await
        .expect("the client speaks within the deadline")
        .expect("read the client");
    serde_json::from_str(&line).expect("the client writes JSON lines")
}

/// Best effort: once the client refused the turn it reads no more, which is not the peer's
/// failure.
async fn write(peer: &mut PeerWriter, message: &Value) {
    let mut bytes = serde_json::to_vec(message).expect("JSON");
    bytes.push(b'\n');
    let _ = peer.write_all(&bytes).await;
    let _ = peer.flush().await;
}

fn update(update: Value) -> Value {
    let mut frame = json!({"jsonrpc":"2.0","method":"session/update",
        "params":{"sessionId":SESSION}});
    frame["params"]["update"] = update;
    frame
}

/// An agent message chunk, with its `messageId` when given.
fn text(text: &str, id: Option<&str>) -> Value {
    let mut chunk = json!({"sessionUpdate":"agent_message_chunk",
        "content":{"type":"text","text":text}});
    if let Some(id) = id {
        chunk["messageId"] = json!(id);
    }
    update(chunk)
}

fn tool_call(id: &str) -> Value {
    update(
        json!({"sessionUpdate":"tool_call","toolCallId":id,"title":"Read config.toml",
        "kind":"read","status":"pending"}),
    )
}

/// The model option a session advertises, `current` its value, `opus` and `sonnet` offered.
fn models(current: &str) -> Value {
    json!([{"id":"model","name":"Model","category":"model","type":"select",
        "currentValue":current,"options":[{"value":"default","name":"Default"},
        {"value":"opus","name":"Opus"},{"value":"sonnet","name":"Sonnet"}]}])
}

/// One scripted turn: the handshake (a model option on `session/new` and the one selection
/// answered when `selected` names the confirmed value), the prompt, then `frames`, then
/// `answer` (a `result` or an `error` member) for the prompt.
async fn turn(
    request: HarnessRequest,
    selected: Option<&'static str>,
    frames: Vec<Value>,
    answer: Value,
) -> Result<HarnessOutcome, HarnessError> {
    played(request, selected, frames, answer).await.1
}

/// [`turn`], with the size of every text chunk the client published before the turn settled.
async fn played(
    request: HarnessRequest,
    selected: Option<&'static str>,
    frames: Vec<Value>,
    answer: Value,
) -> (Vec<usize>, Result<HarnessOutcome, HarnessError>) {
    let (ours, theirs) = tokio::io::duplex(64 * 1024);
    let (client_read, client_write) = tokio::io::split(ours);
    let (peer_read, mut w) = tokio::io::split(theirs);
    let peer = tokio::spawn(async move {
        let mut r = BufReader::new(peer_read);
        let init = read(&mut r).await;
        write(
            &mut w,
            &json!({"jsonrpc":"2.0","id":init["id"],
            "result":{"protocolVersion":1,"agentCapabilities":{}}}),
        )
        .await;
        let new = read(&mut r).await;
        let mut opened = json!({"sessionId": SESSION});
        if selected.is_some() {
            opened["configOptions"] = models("default");
        }
        write(
            &mut w,
            &json!({"jsonrpc":"2.0","id":new["id"],"result":opened}),
        )
        .await;
        if let Some(value) = selected {
            let seat = read(&mut r).await;
            assert_eq!(seat["method"], "session/set_config_option");
            assert_eq!(seat["params"]["value"], value);
            write(
                &mut w,
                &json!({"jsonrpc":"2.0","id":seat["id"],
                "result":{"configOptions": models(value)}}),
            )
            .await;
        }
        let prompt = read(&mut r).await;
        assert_eq!(prompt["method"], "session/prompt");
        for frame in &frames {
            write(&mut w, frame).await;
        }
        let mut last = json!({"jsonrpc":"2.0","id":prompt["id"]});
        for (key, value) in answer.as_object().expect("a result or an error member") {
            last[key] = value.clone();
        }
        write(&mut w, &last).await;
    });
    let settled = settle(crate::client::drive(client_read, client_write, request)).await;
    peer.await.expect("the scripted peer");
    settled
}

async fn settle(
    mut stream: HarnessEventStream,
) -> (Vec<usize>, Result<HarnessOutcome, HarnessError>) {
    let mut published = Vec::new();
    loop {
        match std::future::poll_fn(|cx| Pin::new(&mut stream).poll_next(cx)).await {
            Some(Ok(HarnessEvent::Completed { outcome })) => return (published, Ok(*outcome)),
            Some(Ok(HarnessEvent::MessageChunk { text })) => published.push(text.len()),
            Some(Ok(_)) => {}
            Some(Err(error)) => return (published, Err(error)),
            None => panic!("the turn completes or fails"),
        }
    }
}

fn end_turn() -> Value {
    json!({"result":{"stopReason":"end_turn"}})
}

/// The narration names a JSON value of its own; the answer after the tool call is the last
/// message, and the whole text stays the record's. Without ids, the tool call is the boundary.
#[tokio::test]
async fn narration_a_tool_call_then_an_answer_is_judged_on_the_answer() {
    let narration = "I will read the config first: {\"path\": \"config.toml\"}. ";
    let answer = "{\"title\": \"Anchoring\", \"tags\": []}";
    let outcome = turn(
        HarnessRequest::new("research", "/tmp"),
        None,
        vec![
            text(narration, None),
            tool_call("t1"),
            text("{\"title\": \"Anchoring\", ", None),
            text("\"tags\": []}", None),
        ],
        end_turn(),
    )
    .await
    .expect("the turn completes");
    assert_eq!(outcome.output, format!("{narration}{answer}"));
    assert_eq!(outcome.last_message.as_deref(), Some(answer));
    assert_eq!(outcome.last_message(), answer);
    assert_eq!(outcome.stop_reason, Some(HarnessStopReason::EndTurn));
}

/// A change of `messageId` starts a message, and a trailing blank message is not the answer; a
/// tool call always splits, even between two chunks of one id.
#[tokio::test]
async fn a_change_of_message_id_starts_a_message() {
    let outcome = turn(
        HarnessRequest::new("research", "/tmp"),
        None,
        vec![
            text("Reading ", Some("msg_01")),
            text("the file.", Some("msg_01")),
            text("{\"a\": 1}", Some("msg_02")),
            text("\n", Some("msg_03")),
        ],
        end_turn(),
    )
    .await
    .expect("the turn completes");
    assert_eq!(outcome.output, "Reading the file.{\"a\": 1}\n");
    assert_eq!(outcome.last_message(), "{\"a\": 1}");

    let split = turn(
        HarnessRequest::new("research", "/tmp"),
        None,
        vec![
            text("Reading ", Some("msg_01")),
            tool_call("t1"),
            text("{\"a\": 1}", Some("msg_01")),
        ],
        end_turn(),
    )
    .await
    .expect("the turn completes");
    assert_eq!(split.output, "Reading {\"a\": 1}");
    assert_eq!(split.last_message.as_deref(), Some("{\"a\": 1}"));
}

/// The ledger and the text after `chunks`, each placed where the driver appends it.
fn ledgered(chunks: &[(&str, Option<&str>)]) -> (TurnLedger, String) {
    let mut ledger = TurnLedger::default();
    let mut text = String::new();
    for (chunk, id) in chunks {
        ledger.chunk(text.len(), chunk, id.map(str::to_owned));
        text.push_str(chunk);
    }
    (ledger, text)
}

/// A chunk without an id after chunks with one stays in their message: a notice an adapter
/// sends without an id after the answer does not replace the answer as the last message.
#[test]
fn a_chunk_without_an_id_stays_in_the_current_message() {
    let (ledger, text) = ledgered(&[
        ("Reading the file. ", Some("msg_01")),
        ("{\"a\": 1}", Some("msg_02")),
        (" (context compacted)", None),
    ]);
    assert_eq!(
        ledger.last_message(&text).as_deref(),
        Some("{\"a\": 1} (context compacted)")
    );
}

/// A chunk without an id inside a message keeps that message's id: the same id after it
/// continues the message, it does not start another.
#[test]
fn the_same_id_after_a_chunk_without_one_continues_its_message() {
    let (ledger, text) = ledgered(&[
        ("Reading the file. ", Some("msg_01")),
        ("{\"a\": ", Some("msg_02")),
        ("1", None),
        ("}", Some("msg_02")),
    ]);
    assert_eq!(ledger.last_message(&text).as_deref(), Some("{\"a\": 1}"));
}

/// A chunk with an id after chunks without one starts a message.
#[test]
fn a_chunk_with_an_id_after_chunks_without_one_starts_a_message() {
    let (ledger, text) = ledgered(&[
        ("I will check the notes. ", None),
        ("Done. ", None),
        ("{\"a\": 1}", Some("msg_01")),
        ("\n", Some("msg_01")),
    ]);
    assert_eq!(ledger.last_message(&text).as_deref(), Some("{\"a\": 1}\n"));
}

/// Neither an id nor a tool call: the whole text is one message, as before.
#[test]
fn without_a_signal_the_whole_text_is_one_message() {
    let mut ledger = TurnLedger::default();
    ledger.chunk(0, "part one, ", None);
    ledger.chunk(10, "", Some("msg_x".to_owned()));
    ledger.chunk(10, "part two", None);
    assert_eq!(ledger.last_message("part one, part two"), None);
    let mut blank = TurnLedger::default();
    blank.chunk(0, "  ", Some("a".to_owned()));
    blank.chunk(2, "\n", Some("b".to_owned()));
    assert_eq!(
        blank.last_message("  \n"),
        None,
        "all blank: the whole text"
    );
}

/// claude-agent-acp 0.81.1's frames for a completed turn: mid-turn context updates, the
/// end-of-turn update with the session's cost and the model of the last answer, then the
/// recorded prompt result's usage. The counts are the adapter's, exactly; the cost is its
/// estimate; the reported model is the observed one, the configured one kept apart.
#[tokio::test]
async fn the_recorded_claude_frames_give_counts_context_cost_and_model() {
    let outcome = turn(
        HarnessRequest::new("hi", "/tmp"),
        None,
        vec![
            update(
                json!({"sessionUpdate":"usage_update","used":3,"size":200_000,
                "_meta":{"_claude/model":"claude-opus-4-5-20251101"}}),
            ),
            text("ok", Some("msg_01")),
            update(
                json!({"sessionUpdate":"usage_update","used":3,"size":200_000,
                "cost":{"amount":0.0123,"currency":"USD"},
                "_meta":{"_claude/origin":"user","_claude/model":"claude-opus-4-5-20251101"}}),
            ),
        ],
        json!({"result":{"stopReason":"end_turn",
            "usage":{"inputTokens":3,"outputTokens":0,"cachedReadTokens":0,
                     "cachedWriteTokens":0,"totalTokens":3},
            "_meta":{"quota":{"token_count":{"input_tokens":3},"model_usage":[]}}}}),
    )
    .await
    .expect("the turn completes");
    let usage = outcome
        .usage
        .as_ref()
        .expect("the adapter reported its counts");
    assert_eq!((usage.input_tokens, usage.output_tokens), (3, 0));
    assert_eq!(usage.total_tokens, Some(3));
    assert_eq!(outcome.usage_source, Some(AdapterReport::PromptResult));
    assert_eq!(outcome.context, Some(HarnessContext::new(3, 200_000)));
    assert_eq!(
        outcome.cost_estimate,
        HarnessCostEstimate::new(0.0123, "USD")
    );
    assert!(outcome.cost_estimate.is_some());
    assert_eq!(
        outcome.observed_model.as_deref(),
        Some("claude-opus-4-5-20251101")
    );
    assert_eq!(
        outcome.observed_model_source,
        Some(ModelProvenance::AdapterUsageUpdate)
    );
    assert_eq!(
        outcome.selection.configured_model, None,
        "nothing was configured"
    );
    assert!(
        outcome.selection.changed_mid_turn.is_empty(),
        "no model was pinned"
    );
}

/// codex-acp's turn without usage (`usage: null`, its failed-turn shape without a typed
/// failure) is unknown, never zero; its context update carries no cost and no model.
#[tokio::test]
async fn a_codex_turn_with_null_usage_is_unknown() {
    let outcome = turn(
        HarnessRequest::new("hi", "/tmp"),
        None,
        vec![
            text("done", None),
            update(json!({"sessionUpdate":"usage_update","used":53_000,"size":258_400})),
        ],
        json!({"result":{"stopReason":"end_turn","usage":null,
            "_meta":{"quota":{"token_count":null,"model_usage":[]}}}}),
    )
    .await
    .expect("the turn completes");
    assert_eq!(outcome.usage, None);
    assert_eq!(outcome.usage_source, None);
    assert_eq!(outcome.context, Some(HarnessContext::new(53_000, 258_400)));
    assert_eq!(outcome.cost_estimate, None);
    assert_eq!(outcome.observed_model, None);
}

/// The latest update holds: one without a cost clears the earlier estimate, so no old value
/// is shown as current.
#[tokio::test]
async fn an_update_without_a_cost_clears_the_estimate() {
    let outcome = turn(
        HarnessRequest::new("hi", "/tmp"),
        None,
        vec![
            update(json!({"sessionUpdate":"usage_update","used":10,"size":100,
                "cost":{"amount":0.5,"currency":"USD"}})),
            update(json!({"sessionUpdate":"usage_update","used":12,"size":100})),
            text("ok", None),
        ],
        end_turn(),
    )
    .await
    .expect("the turn completes");
    assert_eq!(outcome.cost_estimate, None);
    assert_eq!(outcome.context, Some(HarnessContext::new(12, 100)));
}

/// Every stop reason rides the outcome typed, with the text received: the client reports it,
/// the caller decides. A future word stays unknown.
#[tokio::test]
async fn every_stop_reason_rides_the_outcome() {
    for (word, stop) in [
        ("refusal", HarnessStopReason::Refusal),
        ("max_tokens", HarnessStopReason::MaxTokens),
        ("max_turn_requests", HarnessStopReason::MaxTurnRequests),
        ("cancelled", HarnessStopReason::Cancelled),
        ("paused", HarnessStopReason::Unknown("paused".to_owned())),
    ] {
        let outcome = turn(
            HarnessRequest::new("hi", "/tmp"),
            None,
            vec![text("I can't help with that.", None)],
            json!({"result":{"stopReason":word}}),
        )
        .await
        .expect("the client reports the turn");
        assert_eq!(outcome.stop_reason, Some(stop), "{word}");
        assert_eq!(outcome.output, "I can't help with that.");
    }
}

/// Under a model the client sent (`opus`, confirmed), the adapter reporting another model
/// for its answer is the route moving the model: recorded, and refused by the exact
/// selection. The confirmed alias answering as its own dated id is no move, and a spelling the
/// rule cannot place is recorded as the observed model, never refused.
#[tokio::test]
async fn a_reported_model_other_than_the_confirmed_one_is_a_move() {
    let reported = |model: &str| {
        update(
            json!({"sessionUpdate":"usage_update","used":3,"size":200_000,
            "cost":{"amount":0.01,"currency":"USD"},"_meta":{"_claude/model":model}}),
        )
    };
    let request = || HarnessRequest::new("hi", "/tmp").with_requested_model("anthropic/opus");
    let moved = turn(
        request(),
        Some("opus"),
        vec![
            text("ok", None),
            reported("claude-sonnet-4-5-20250929"),
            reported("claude-sonnet-4-5-20250929"),
        ],
        end_turn(),
    )
    .await
    .expect("the client reports the turn");
    assert_eq!(moved.selection.transmitted_model.as_deref(), Some("opus"));
    assert_eq!(moved.selection.configured_model.as_deref(), Some("opus"));
    assert_eq!(
        moved.selection.configured_model_source,
        Some(ModelProvenance::ConfirmedSelection)
    );
    assert_eq!(
        moved.observed_model.as_deref(),
        Some("claude-sonnet-4-5-20250929")
    );
    assert_eq!(
        moved.selection.changed_mid_turn,
        ["model=claude-sonnet-4-5-20250929"],
        "recorded once"
    );
    let Some(HarnessError::Selection { reason }) = moved.selection.moved_refusal() else {
        panic!("an exact selection refuses the answer");
    };
    assert!(
        reason.contains("model=claude-sonnet-4-5-20250929"),
        "{reason}"
    );

    let held = turn(
        request(),
        Some("opus"),
        vec![text("ok", None), reported("claude-opus-4-5-20251101")],
        end_turn(),
    )
    .await
    .expect("the client reports the turn");
    assert!(held.selection.changed_mid_turn.is_empty());
    assert!(held.selection.moved_refusal().is_none());
    assert_eq!(
        (held.observed_model.as_deref(), held.observed_model_source),
        (
            Some("claude-opus-4-5-20251101"),
            Some(ModelProvenance::AdapterUsageUpdate)
        )
    );
    assert_eq!(held.selection.configured_model.as_deref(), Some("opus"));

    let unplaced = turn(
        request(),
        Some("opus"),
        vec![text("ok", None), reported("claude-opus-4-5-preview")],
        end_turn(),
    )
    .await
    .expect("the client reports the turn");
    assert!(unplaced.selection.changed_mid_turn.is_empty());
    assert!(unplaced.selection.moved_refusal().is_none());
    assert_eq!(
        unplaced.observed_model.as_deref(),
        Some("claude-opus-4-5-preview")
    );
}

/// One move is enough to refuse: a thousand distinct ids keep the first move only, in the
/// list and in the refusal.
#[tokio::test]
async fn a_stream_of_moved_ids_keeps_the_first_move_only() {
    let mut frames = vec![text("ok", None)];
    frames.extend((0..1000).map(|minor| {
        update(
            json!({"sessionUpdate":"usage_update","used":3,"size":200_000,
            "_meta":{"_claude/model": format!("claude-sonnet-4-{minor}")}}),
        )
    }));
    let moved = turn(
        HarnessRequest::new("hi", "/tmp").with_requested_model("anthropic/opus"),
        Some("opus"),
        frames,
        end_turn(),
    )
    .await
    .expect("the client reports the turn");
    assert_eq!(
        moved.selection.changed_mid_turn,
        ["model=claude-sonnet-4-0"]
    );
    let Some(HarnessError::Selection { reason }) = moved.selection.moved_refusal() else {
        panic!("an exact selection refuses the answer");
    };
    assert!(reason.contains("model=claude-sonnet-4-0"), "{reason}");
    assert!(!reason.contains("claude-sonnet-4-1"), "{reason}");
}

/// A refusal must be certain: only a demonstrated difference is another model, and a spelling
/// the rule cannot place is recorded, never refused.
#[test]
fn only_a_demonstrated_difference_is_another_model() {
    use Verdict::{Different, Same, Unplaced};
    for (confirmed, reported, expected) in [
        ("opus", "claude-opus-4-5-20251101", Same),
        ("opus", "anthropic.claude-opus-4-5-20251101-v1:0", Same),
        ("opus", "claude-opus-4-5@20251101", Same),
        ("sonnet", "claude-sonnet-4-5-20250929", Same),
        ("opus[1m]", "claude-opus-4-6", Same),
        ("haiku", "claude-3-5-haiku-20241022", Same),
        ("claude-opus-4-5", "claude-opus-4-5-20251101", Same),
        ("claude-opus-4", "claude-opus-4-20250514", Same),
        ("claude-sonnet-4-0", "claude-sonnet-4-20250514", Same),
        ("claude-opus-4-0", "claude-opus-4-20250514", Same),
        ("anthropic/claude-opus-4-5", "claude-opus-4-5", Same),
        ("gpt-5.5", "gpt-5.5", Same),
        ("opus", "claude-sonnet-4-5-20250929", Different),
        ("claude-opus-4", "claude-opus-4-5-20251101", Different),
        ("claude-opus-4-6", "claude-opus-4-5-20251101", Different),
        (
            "claude-opus-4-5-20251101",
            "claude-opus-4-5-20250929",
            Different,
        ),
        ("gpt-5.5", "gpt-5.4", Different),
        ("claude-opus-4-0", "claude-opus-4-1-20250805", Different),
        ("opus", "claude-opus-4-5-preview", Unplaced),
        ("opusplan", "claude-sonnet-4-5-20250929", Unplaced),
        ("default", "claude-opus-4-5-20251101", Unplaced),
        ("gpt-5.5", "gpt-5.5-codex", Unplaced),
        // A revision before the date (Vertex) is a name word, so the names differ; only a
        // revision after the date (Bedrock's `-v1:0`) is set aside.
        (
            "claude-3-5-sonnet-20241022",
            "claude-3-5-sonnet-v2@20241022",
            Unplaced,
        ),
        ("claude-opus-4-5", "claude-opus", Unplaced),
        ("gpt-4", "gpt-4-0613", Unplaced),
        ("opus", "unrecognized", Unplaced),
        ("", "claude-opus-4-5", Unplaced),
        ("sonnet", "", Unplaced),
    ] {
        assert_eq!(
            verdict(confirmed, reported),
            expected,
            "{confirmed} → {reported}"
        );
    }
}

/// The bound on one turn's text, on values: exactly [`MAX_TURN_TEXT`] bytes are held, one more
/// byte is not, and a sum past the count is past the bound.
#[test]
fn a_turn_holds_exactly_its_bound() {
    assert!(admits(0, MAX_TURN_TEXT));
    assert!(admits(MAX_TURN_TEXT - 1, 1));
    assert!(admits(MAX_TURN_TEXT, 0));
    assert!(!admits(MAX_TURN_TEXT, 1));
    assert!(!admits(1, MAX_TURN_TEXT));
    assert!(!admits(usize::MAX, 1));
}

/// One turn holds exactly its bound of text; one byte more is refused before it is held or
/// published, in Nika's words.
#[tokio::test]
async fn a_turn_past_its_text_bound_is_refused() {
    let big = "x".repeat(MAX_TURN_TEXT);
    let (published, held) = played(
        HarnessRequest::new("hi", "/tmp"),
        None,
        vec![text(&big, None)],
        end_turn(),
    )
    .await;
    let held = held.expect("exactly the bound is held");
    assert_eq!(
        (held.output.len(), published),
        (MAX_TURN_TEXT, vec![MAX_TURN_TEXT])
    );

    let (published, refused) = played(
        HarnessRequest::new("hi", "/tmp"),
        None,
        vec![text(&big, None), text("y", None)],
        end_turn(),
    )
    .await;
    assert_eq!(
        published,
        [MAX_TURN_TEXT],
        "the refused byte is never published"
    );
    let Err(HarnessError::Refused { reason }) = refused else {
        panic!("a turn past its bound is refused, got {refused:?}");
    };
    assert_eq!(
        reason,
        "the harness's answer exceeded the 4 MiB one turn may hold; no answer accepted"
    );
}

/// codex-acp failing the prompt with its quota kind (the `-32603` shape it answers with when
/// `sessionFailure` is not in play): a final refusal that shows the adapter's own words.
#[tokio::test]
async fn a_codex_quota_failure_shows_the_adapters_words() {
    let refused = turn(
        HarnessRequest::new("hi", "/tmp"),
        None,
        Vec::new(),
        json!({"error":{"code":-32603,"message":"Internal error","data":{
            "message":"You've hit your usage limit. Try again in 3 days.",
            "codexErrorInfo":"usageLimitExceeded"}}}),
    )
    .await;
    let Err(HarnessError::Refused { reason }) = refused else {
        panic!("an exhausted quota is a final refusal, got {refused:?}");
    };
    assert_eq!(
        reason,
        "Internal error — You've hit your usage limit. Try again in 3 days. (jsonrpc -32603)"
    );
    assert!(
        !HarnessError::Refused { reason }.is_transient(),
        "no retry heals a quota"
    );
}
