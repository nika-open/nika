// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! NIK-402 · a seat's final message that misses the task `schema:` gets ONE repair prompt on
//! the same session, which lists every violation; a session that closed on its message, a
//! second miss and a met token budget each fail with the message kept.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use nika_error::traits::NikaErrorCode as _;
use nika_kernel::ai::harness::{
    DynAgentBackend, FollowUpReply, HarnessError, HarnessEvent, HarnessEventStream, HarnessOutcome,
    HarnessRequest,
};
use nika_kernel_mock::{MockProvider, MockToolDefinitionProvider, MockToolExecutor};
use nika_types::TokenUsage;

use super::*;
use crate::harness_path::HarnessSeat;
use crate::observe::NudgeReason;

/// One scripted turn of a seat: its final message, its reported usage, and whether its
/// session stays open after it (a `TurnEnded` lane) or closes on it (`Completed`).
#[derive(Clone)]
struct Turn {
    text: &'static str,
    usage: Option<(u64, u64)>,
    open: bool,
}

fn open(text: &'static str) -> Turn {
    Turn {
        text,
        usage: None,
        open: true,
    }
}

fn closed(text: &'static str) -> Turn {
    Turn {
        text,
        usage: None,
        open: false,
    }
}

/// A seat that plays its turns on ONE session: the prompt sent on a lane runs as the next
/// turn. It records the request and every follow-up prompt.
struct Script {
    turns: Vec<Turn>,
    asked: Mutex<Vec<HarnessRequest>>,
    prompts: Arc<Mutex<Vec<String>>>,
}

struct Playback {
    turns: Vec<Turn>,
    prompts: Arc<Mutex<Vec<String>>>,
    lane: Option<Arc<Mutex<Option<String>>>>,
}

impl futures_core::Stream for Playback {
    type Item = Result<HarnessEvent, HarnessError>;
    fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if let Some(lane) = this.lane.take() {
            let Some(prompt) = lane.lock().unwrap().take() else {
                return Poll::Ready(None);
            };
            this.prompts.lock().unwrap().push(prompt);
        }
        if this.turns.is_empty() {
            return Poll::Ready(None);
        }
        let turn = this.turns.remove(0);
        let mut outcome = HarnessOutcome::new(turn.text);
        outcome.usage = turn
            .usage
            .map(|(input, output)| TokenUsage::new(input, output));
        let outcome = Box::new(outcome);
        if !turn.open {
            return Poll::Ready(Some(Ok(HarnessEvent::Completed { outcome })));
        }
        let slot = Arc::new(Mutex::new(None));
        let sent = Arc::clone(&slot);
        this.lane = Some(slot);
        let follow_up = FollowUpReply::new(Box::new(move |prompt| {
            *sent.lock().unwrap() = Some(prompt);
        }));
        Poll::Ready(Some(Ok(HarnessEvent::TurnEnded { outcome, follow_up })))
    }
}

impl DynAgentBackend for Script {
    fn run_agent_boxed(
        &self,
        request: HarnessRequest,
    ) -> Pin<
        Box<dyn std::future::Future<Output = Result<HarnessEventStream, HarnessError>> + Send + '_>,
    > {
        self.asked.lock().unwrap().push(request);
        let playback = Playback {
            turns: self.turns.clone(),
            prompts: Arc::clone(&self.prompts),
            lane: None,
        };
        Box::pin(async move { Ok(Box::pin(playback) as HarnessEventStream) })
    }
}

/// A recording observer.
#[derive(Default)]
struct Seen(Mutex<Vec<AgentEvent>>);

impl AgentObserver for Seen {
    fn on_event(&self, event: &AgentEvent) {
        AgentEvent::record_into(&mut self.0.lock().unwrap(), event);
    }
}

type Verb = AgentVerb<MockProvider, MockToolExecutor, MockToolDefinitionProvider>;

fn seated(turns: Vec<Turn>) -> (Verb, Arc<Script>) {
    let script = Arc::new(Script {
        turns,
        asked: Mutex::new(Vec::new()),
        prompts: Arc::new(Mutex::new(Vec::new())),
    });
    let tools = Arc::new(MockToolExecutor::new());
    let verb = AgentVerb::new(
        Arc::new(MockProvider::new("mock")),
        Arc::new(nika_verb_invoke::InvokeVerb::new(tools)),
        Arc::new(MockToolDefinitionProvider::new()),
        "openai/gpt-6-luna",
    )
    .with_harness_seat(HarnessSeat::new(script.clone(), "/tmp"));
    (verb, script)
}

fn schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "required": ["title", "summary", "tags"],
        "properties": {
            "title": {"type": "string"},
            "summary": {"type": "string"},
            "tags": {"type": "array"}
        }
    })
}

fn typed() -> AgentInput {
    let mut input = AgentInput::new("research the anchoring bias");
    input.schema = Some(schema());
    input
}

/// The three keys `{"note": ..}` misses, in the schema's order.
const THREE_MISSING: &str = "3 violations: \"title\" is a required property; \"summary\" is a \
                             required property; \"tags\" is a required property";

fn repair_prompt(detail: &str) -> String {
    format!(
        "Your final message did not satisfy the required output schema. Validation failed \
         with: {detail}. Do not redo the work: reply with ONLY your final message, a JSON value \
         that satisfies this JSON Schema, no prose, no code fences:\n{}",
        crate::shape::render_schema(&schema())
    )
}

fn nudges(seen: &Seen) -> Vec<AgentEvent> {
    let log = seen.0.lock().unwrap();
    (log.iter())
        .filter(|event| matches!(event, AgentEvent::Nudged { .. }))
        .cloned()
        .collect()
}

/// The missed message gets one prompt on the same session naming all three missing keys,
/// and the repaired message is the output; the two turns count together.
#[tokio::test]
async fn a_missed_schema_is_repaired_on_the_same_session() {
    let mut second =
        open(r#"{"title": "Anchoring", "summary": "The first number sticks.", "tags": []}"#);
    second.usage = Some((5, 7));
    let mut first = open(r#"{"note": "seven minutes of research"}"#);
    first.usage = Some((100, 20));
    let (verb, script) = seated(vec![first, second]);
    let seen = Seen::default();
    let out = verb.run_observed(typed(), &seen).await.expect("repaired");
    assert_eq!(
        out.output,
        AgentValue::Structured(serde_json::json!({
            "title": "Anchoring", "summary": "The first number sticks.", "tags": []
        }))
    );
    assert_eq!((out.turns, out.total_tokens), (2, 132));
    assert_eq!((out.usage.input_tokens, out.usage.output_tokens), (105, 27));
    assert!(
        script.asked.lock().unwrap()[0].follow_up,
        "the request kept room"
    );
    assert_eq!(
        *script.prompts.lock().unwrap(),
        [repair_prompt(THREE_MISSING)]
    );
    assert_eq!(
        nudges(&seen),
        [AgentEvent::Nudged {
            turn: 1,
            reason: NudgeReason::SchemaRepair,
            period: 0
        }]
    );
}

/// A repaired message that misses again fails `NIKA-INFER-002` after its one repair, even
/// when the backend keeps the session open again: no second prompt. The record keeps the
/// repaired message and its violations.
#[tokio::test]
async fn a_second_miss_fails_after_one_repair() {
    let (verb, script) = seated(vec![
        open(r#"{"note": "first"}"#),
        open(r#"{"title": "Anchoring"}"#),
        open(r#"{"title": "never asked"}"#),
    ]);
    let err = verb.run(typed()).await.expect_err("still missing");
    assert_eq!(err.spec_code(), "NIKA-INFER-002");
    assert!(!err.is_transient());
    assert_eq!(
        err.to_string(),
        "agent final message failed schema validation: 2 violations: \"summary\" is a required \
         property; \"tags\" is a required property (after 1 repair attempt)"
    );
    assert_eq!(
        VerbAgentError::details_of(&err),
        Some(serde_json::json!({
            "partial_output": r#"{"title": "Anchoring"}"#,
            "violations": [
                "\"summary\" is a required property",
                "\"tags\" is a required property"
            ]
        }))
    );
    assert_eq!(
        script.prompts.lock().unwrap().len(),
        1,
        "one repair, never two"
    );
}

/// A session that closed on its final message gets no repair: the miss fails with the
/// message and every violation, and says why no repair ran.
#[tokio::test]
async fn a_closed_session_gets_no_repair() {
    let (verb, script) = seated(vec![closed(r#"{"note": "done"}"#)]);
    let seen = Seen::default();
    let err = verb
        .run_observed(typed(), &seen)
        .await
        .expect_err("missing");
    assert_eq!(err.spec_code(), "NIKA-INFER-002");
    assert_eq!(
        err.to_string(),
        format!(
            "agent final message failed schema validation: {THREE_MISSING} (validated without \
             a repair turn: the seat's session closed with its final message)"
        )
    );
    assert_eq!(
        VerbAgentError::details_of(&err).expect("details")["partial_output"],
        r#"{"note": "done"}"#
    );
    assert!(script.prompts.lock().unwrap().is_empty());
    assert!(nudges(&seen).is_empty());
}

/// The repair is one more request: a cumulative token budget the first turn already met
/// refuses it with `NIKA-AGENT-002`, the rejected message at `partial_output`.
#[tokio::test]
async fn a_met_token_budget_refuses_the_repair() {
    let mut first = open(r#"{"note": "long research"}"#);
    first.usage = Some((80, 40));
    let (verb, script) = seated(vec![first, open("{}")]);
    let mut input = typed();
    input.max_tokens_total = Some(100);
    let seen = Seen::default();
    let err = verb
        .run_observed(input, &seen)
        .await
        .expect_err("budget met");
    assert_eq!(err.spec_code(), "NIKA-AGENT-002");
    assert!(
        matches!(
            &err,
            VerbAgentError::MaxTokens {
                total_tokens: 120,
                ..
            }
        ),
        "{err:?}"
    );
    assert_eq!(
        VerbAgentError::details_of(&err),
        Some(serde_json::json!({"partial_output": r#"{"note": "long research"}"#}))
    );
    assert!(script.prompts.lock().unwrap().is_empty());
    assert!(nudges(&seen).is_empty());
}

/// A valid first message closes the session at once, and a task without a schema never
/// asks the backend to keep room for a follow-up.
#[tokio::test]
async fn a_valid_message_or_an_untyped_task_spends_no_repair() {
    let valid = r#"{"title": "a", "summary": "b", "tags": ["c"]}"#;
    let (verb, script) = seated(vec![open(valid), open("{}")]);
    let out = verb.run(typed()).await.expect("valid");
    assert_eq!(out.turns, 1);
    assert!(script.prompts.lock().unwrap().is_empty());

    let (verb, script) = seated(vec![closed("plain prose")]);
    let out = verb.run(AgentInput::new("talk")).await.expect("untyped");
    assert_eq!(out.output, AgentValue::Text("plain prose".to_owned()));
    assert!(!script.asked.lock().unwrap()[0].follow_up);
}
