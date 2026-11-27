// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! The agent path's answer to a permission request: a refusal selects the harness's first
//! one-time refusal, after which the harness decides whether the agent goes on; a refusal the
//! harness offers no way to give is `cancelled`; an ask the engine dropped unanswered is
//! `cancelled` whatever is offered.

use super::*;
use serde_json::json;

/// Every option kind, the remembered ones first, as a harness may list them.
fn every_kind() -> Value {
    json!([
        {"optionId": "yes-always", "name": "Always allow", "kind": "allow_always"},
        {"optionId": "yes-once", "name": "Allow", "kind": "allow_once"},
        {"optionId": "no-always", "name": "Always reject", "kind": "reject_always"},
        {"optionId": "no-once", "name": "Reject", "kind": "reject_once"}
    ])
}

async fn send(writer: &mut tokio::io::WriteHalf<tokio::io::DuplexStream>, frame: &Value) {
    let line = format!("{frame}\n");
    writer
        .write_all(line.as_bytes())
        .await
        .expect("the agent writes");
}

async fn receive(reader: &mut BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>) -> Value {
    let mut line = String::new();
    reader.read_line(&mut line).await.expect("the agent reads");
    serde_json::from_str(line.trim_end()).expect("the client writes JSON")
}

/// The title of the one tool call the scripted agent asks about.
pub(crate) const ASKED: &str = "Remove the build folder";

/// The scripted agent of one turn, on the far end of a duplex pipe: it answers the handshake,
/// asks once about [`ASKED`] offering `options`, then says `then` and ends `end_turn`. It
/// returns the outcome it heard for its ask.
pub(crate) fn one_ask(
    agent: tokio::io::DuplexStream,
    options: Value,
    then: &'static str,
) -> tokio::task::JoinHandle<Value> {
    one_ask_ending(agent, options, then, "end_turn")
}

/// [`one_ask`], the turn ending with the stop reason `stop` instead of `end_turn`.
pub(crate) fn one_ask_ending(
    agent: tokio::io::DuplexStream,
    options: Value,
    then: &'static str,
    stop: &'static str,
) -> tokio::task::JoinHandle<Value> {
    let (agent_read, mut agent_write) = tokio::io::split(agent);
    tokio::spawn(async move {
        let mut reader = BufReader::new(agent_read);
        let init = receive(&mut reader).await;
        let ready = json!({"protocolVersion": 1, "agentCapabilities": {}});
        send(
            &mut agent_write,
            &json!({"jsonrpc": "2.0", "id": init["id"], "result": ready}),
        )
        .await;
        let new = receive(&mut reader).await;
        send(
            &mut agent_write,
            &json!({"jsonrpc": "2.0", "id": new["id"], "result": {"sessionId": "s-refusal"}}),
        )
        .await;
        let prompt = receive(&mut reader).await;
        assert_eq!(prompt["method"], "session/prompt");
        let ask = json!({"sessionId": "s-refusal", "options": options,
            "toolCall": {"title": ASKED, "kind": "delete"}});
        send(
            &mut agent_write,
            &json!({"jsonrpc": "2.0", "id": "ask-1",
                "method": "session/request_permission", "params": ask}),
        )
        .await;
        let answer = receive(&mut reader).await;
        assert_eq!(answer["id"], "ask-1");
        let said = json!({"sessionId": "s-refusal", "update": {
            "sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": then}}});
        send(
            &mut agent_write,
            &json!({"jsonrpc": "2.0", "method": "session/update", "params": said}),
        )
        .await;
        send(
            &mut agent_write,
            &json!({"jsonrpc": "2.0", "id": prompt["id"], "result": {"stopReason": stop}}),
        )
        .await;
        answer["result"]["outcome"].clone()
    })
}

/// One turn whose agent asks once, offering `options`, then says `then` and ends `end_turn`,
/// while the engine answers `decision` (`None`: the reply is dropped unanswered). What the
/// agent heard, and the outcome the turn completed with.
async fn asked(
    options: Value,
    mut decision: Option<PermissionDecision>,
    then: &'static str,
) -> (Value, Option<HarnessOutcome>) {
    let (ours, theirs) = tokio::io::duplex(64 * 1024);
    let (client_read, client_write) = tokio::io::split(ours);
    let agent = one_ask(theirs, options, then);
    let mut stream = drive(
        client_read,
        client_write,
        HarnessRequest::new("clean the build", "/tmp"),
    );
    let mut outcome = None;
    while let Some(event) = std::future::poll_fn(|cx| Pin::new(&mut stream).poll_next(cx)).await {
        match event {
            Ok(
                HarnessEvent::PermissionAsked { reply, .. }
                | HarnessEvent::EscalationAsked { reply, .. },
            ) => match decision.take() {
                Some(decision) => reply.respond(decision),
                None => drop(reply),
            },
            Ok(HarnessEvent::Completed { outcome: done }) => outcome = Some(*done),
            Ok(_) => {}
            Err(error) => panic!("stream error: {error}"),
        }
    }
    (agent.await.expect("the scripted agent ends"), outcome)
}

/// A refusal selects the one-time refusal, never the remembered one listed before it, and a
/// scripted agent that goes on after it ends its turn `end_turn`, completed with its text.
#[tokio::test]
async fn a_refusal_selects_reject_once_and_the_agent_goes_on() {
    let went_on = "I left the build folder in place.";
    let (heard, outcome) = asked(every_kind(), Some(PermissionDecision::Deny), went_on).await;
    assert_eq!(heard, json!({"outcome": "selected", "optionId": "no-once"}));
    let outcome = outcome.expect("the turn completes");
    assert_eq!(outcome.output, went_on);
    assert_eq!(outcome.stop_reason, Some(HarnessStopReason::EndTurn));
}

/// A refusal the harness offers no one-time refusal for is `cancelled`: neither an allow nor a
/// remembered refusal is ever selected in its place.
#[tokio::test]
async fn a_refusal_with_no_reject_once_offered_answers_cancelled() {
    for options in [
        json!([{"optionId": "yes-always", "name": "Always allow", "kind": "allow_always"},
               {"optionId": "yes-once", "name": "Allow", "kind": "allow_once"}]),
        json!([{"optionId": "no-always", "name": "Always reject", "kind": "reject_always"}]),
        json!([]),
    ] {
        let (heard, _) = asked(options.clone(), Some(PermissionDecision::Deny), "ok").await;
        assert_eq!(heard, json!({"outcome": "cancelled"}), "{options}");
    }
}

/// Two one-time refusals in codex-acp 1.13.1's order for a command, as read in its source
/// (`decline`, "continue without running it", then `cancel`, "tell Codex what to do
/// differently"): the first is selected, by its own id.
#[tokio::test]
async fn of_two_one_time_refusals_the_first_is_selected() {
    let options = json!([
        {"optionId": "allow_once", "name": "Yes, proceed", "kind": "allow_once"},
        {"optionId": "allow_for_session",
            "name": "Yes, and don't ask again for this command in this session",
            "kind": "allow_always"},
        {"optionId": "decline", "name": "No, continue without running it", "kind": "reject_once"},
        {"optionId": "cancel", "name": "No, and tell Codex what to do differently",
            "kind": "reject_once"}
    ]);
    let (heard, _) = asked(options, Some(PermissionDecision::Deny), "ok").await;
    assert_eq!(heard, json!({"outcome": "selected", "optionId": "decline"}));
}

/// A kind the protocol does not define is no refusal: beside no `reject_once`, a refusal
/// answers `cancelled`, never the unknown option.
#[tokio::test]
async fn an_unknown_kind_beside_no_reject_once_answers_cancelled() {
    let options = json!([
        {"optionId": "yes-once", "name": "Allow", "kind": "allow_once"},
        {"optionId": "later", "name": "Ask me later", "kind": "reject_later"}
    ]);
    let (heard, _) = asked(options, Some(PermissionDecision::Deny), "ok").await;
    assert_eq!(heard, json!({"outcome": "cancelled"}));
}

/// An ask the engine dropped unanswered is `cancelled` even when a one-time refusal is
/// offered: the session is being torn down, so the agent is never told it may go on.
#[tokio::test]
async fn a_dropped_ask_answers_cancelled_though_a_refusal_is_offered() {
    let (heard, _) = asked(every_kind(), None, "ok").await;
    assert_eq!(heard, json!({"outcome": "cancelled"}));
}

/// The rule on values: each verdict selects its one-time option, `cancelled` when it is not
/// offered, and the remembered options are never selected.
#[test]
fn each_verdict_selects_its_one_time_option_or_cancels() {
    let offered = |options: Value| -> Vec<wire::PermissionOptionIn> {
        serde_json::from_value(options).expect("options")
    };
    let all = offered(every_kind());
    let remembered = offered(json!([
        {"optionId": "yes-always", "name": "Always allow", "kind": "allow_always"},
        {"optionId": "no-always", "name": "Always reject", "kind": "reject_always"}
    ]));
    for (decision, options, expected) in [
        (
            PermissionDecision::AllowOnce,
            &all,
            json!({"outcome": "selected", "optionId": "yes-once"}),
        ),
        (
            PermissionDecision::Deny,
            &all,
            json!({"outcome": "selected", "optionId": "no-once"}),
        ),
        (
            PermissionDecision::AllowOnce,
            &remembered,
            json!({"outcome": "cancelled"}),
        ),
        (
            PermissionDecision::Deny,
            &remembered,
            json!({"outcome": "cancelled"}),
        ),
        (
            PermissionDecision::Deny,
            &Vec::new(),
            json!({"outcome": "cancelled"}),
        ),
    ] {
        assert_eq!(
            permission_outcome(decision, options).outcome,
            expected,
            "{decision:?}"
        );
    }
}
