// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! A seat's turn as the task reads it: a turn that did not end is not completed and names its
//! reason, on the first turn as on a repair; the schema judges the last message and the record
//! keeps the whole text; the counts the adapter reported meet the token ceiling, and no counts,
//! or counts that overflow, leave the tokens and the ceiling unknown; the model the adapter
//! reported stands apart from the configured one.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use nika_error::traits::NikaErrorCode as _;
use nika_kernel::ai::harness::{
    AdapterReport, DynAgentBackend, FollowUpReply, HarnessContext, HarnessCostEstimate,
    HarnessError, HarnessEvent, HarnessEventStream, HarnessOutcome, HarnessRequest,
    HarnessSelection, HarnessStopReason, ModelProvenance,
};
use nika_kernel_mock::{MockProvider, MockToolDefinitionProvider, MockToolExecutor};
use nika_types::TokenUsage;
use nika_types::access::{AccessFallback, AccessProtocol, AccessRequirement};
use serde_json::json;

use super::*;
use crate::harness_path::HarnessSeat;

/// A seat that plays its outcomes on ONE session: an open one ends its turn with a lane, and
/// the prompt sent on it runs the next outcome. It records every follow-up prompt.
struct Tape {
    turns: Vec<(HarnessOutcome, bool)>,
    prompts: Arc<Mutex<Vec<String>>>,
}

struct Playback {
    turns: Vec<(HarnessOutcome, bool)>,
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
        let (outcome, open) = this.turns.remove(0);
        let outcome = Box::new(outcome);
        if !open {
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

impl DynAgentBackend for Tape {
    fn run_agent_boxed(
        &self,
        _request: HarnessRequest,
    ) -> Pin<
        Box<dyn std::future::Future<Output = Result<HarnessEventStream, HarnessError>> + Send + '_>,
    > {
        let playback = Playback {
            turns: self.turns.clone(),
            prompts: Arc::clone(&self.prompts),
            lane: None,
        };
        Box::pin(async move { Ok(Box::pin(playback) as HarnessEventStream) })
    }
}

type Verb = AgentVerb<MockProvider, MockToolExecutor, MockToolDefinitionProvider>;

fn seated(turns: Vec<(HarnessOutcome, bool)>) -> (Verb, Arc<Mutex<Vec<String>>>) {
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let tape = Arc::new(Tape {
        turns,
        prompts: Arc::clone(&prompts),
    });
    let verb = AgentVerb::new(
        Arc::new(MockProvider::new("mock")),
        Arc::new(nika_verb_invoke::InvokeVerb::new(Arc::new(
            MockToolExecutor::new(),
        ))),
        Arc::new(MockToolDefinitionProvider::new()),
        "openai/gpt-6-luna",
    )
    .with_harness_seat(HarnessSeat::new(tape as Arc<dyn DynAgentBackend>, "/tmp"));
    (verb, prompts)
}

/// A turn as the ACP client closes it: its text and how it ended.
fn ended(text: &str, stop: &str) -> HarnessOutcome {
    HarnessOutcome::new(text).with_stop_reason(HarnessStopReason::parse(stop))
}

/// `counts` as the adapter reported them in its prompt result.
fn reported(mut outcome: HarnessOutcome, counts: TokenUsage) -> HarnessOutcome {
    outcome.usage = Some(counts);
    outcome.usage_source = Some(AdapterReport::PromptResult);
    outcome
}

fn schema() -> serde_json::Value {
    json!({"type": "object", "required": ["title", "tags"],
        "properties": {"title": {"type": "string"}, "tags": {"type": "array"}}})
}

fn typed() -> AgentInput {
    let mut input = AgentInput::new("research the anchoring bias");
    input.schema = Some(schema());
    input
}

/// A turn stopped by a refusal, by the harness's own request limit, by a cancellation Nika
/// never sent, by the output cap or by a word this engine does not know is not completed: each
/// fails with its reason named and the text received at `partial_output`. `end_turn`
/// completes, and so does a backend that reports no stop reason.
#[tokio::test]
async fn a_turn_that_did_not_end_is_not_completed_and_names_its_reason() {
    let said = "I read the config. I can't help with that.";
    for (stop, code, kept) in [
        ("refusal", "NIKA-INFER-001", "refusal"),
        ("max_turn_requests", "NIKA-INFER-001", "max_turn_requests"),
        ("cancelled", "NIKA-INFER-001", "cancelled"),
        ("max_tokens", "NIKA-AGENT-006", "max_tokens"),
        ("paused", "NIKA-INFER-001", "paused"),
        ("Pause Now!", "NIKA-INFER-001", "unrecognized"),
    ] {
        let (verb, _) = seated(vec![(ended(said, stop), false)]);
        let err = verb
            .run(AgentInput::new("talk"))
            .await
            .expect_err("not completed");
        assert_eq!(err.spec_code(), code, "{stop}");
        assert!(!err.is_transient(), "{stop} is a verdict");
        assert!(
            err.to_string().contains(&format!("stop reason `{kept}`")),
            "{stop}: {err}"
        );
        assert_eq!(
            VerbAgentError::details_of(&err),
            Some(json!({"partial_output": said, "stop_reason": kept})),
            "{stop}"
        );
    }
    let (verb, _) = seated(vec![(ended(said, "end_turn"), false)]);
    let out = verb.run(AgentInput::new("talk")).await.expect("completed");
    assert_eq!(out.output, AgentValue::Text(said.to_owned()));
    assert_eq!(
        out.seat.expect("a seat report").stop_reason.as_deref(),
        Some("end_turn")
    );
    let (verb, _) = seated(vec![(HarnessOutcome::new(said), false)]);
    let out = verb.run(AgentInput::new("talk")).await.expect("completed");
    assert_eq!(out.output, AgentValue::Text(said.to_owned()));
    assert_eq!(out.seat.expect("a seat report").stop_reason, None);
}

/// A repair turn that ends on a refusal or at the output cap fails the repair: the task fails
/// `NIKA-INFER-002` with the first turn's message and its violations, the repair turn's stop
/// named as its failure.
#[tokio::test]
async fn a_repair_turn_that_stops_keeps_the_first_turns_message() {
    let first = r#"{"title": "Anchoring"}"#;
    for stop in ["refusal", "max_tokens"] {
        let (verb, prompts) = seated(vec![
            (ended(first, "end_turn"), true),
            (ended("I can't continue with that.", stop), false),
        ]);
        let err = verb.run(typed()).await.expect_err("the repair stopped");
        assert_eq!(err.spec_code(), "NIKA-INFER-002", "{stop}");
        assert_eq!(
            prompts.lock().unwrap().len(),
            1,
            "{stop}: the repair went out"
        );
        assert_eq!(
            VerbAgentError::details_of(&err),
            Some(json!({"partial_output": first,
                "violations": ["\"tags\" is a required property"]})),
            "{stop}"
        );
        assert!(
            err.to_string().contains(&format!("stop reason `{stop}`")),
            "{stop}: {err}"
        );
    }
}

/// Narration, a tool call, then the answer: the schema judges the answer only, and the record
/// keeps the whole text beside the value. A last message that misses fails with every message
/// of the turn kept.
#[tokio::test]
async fn the_schema_judges_the_last_message_and_the_record_keeps_the_text() {
    let narration = "Let me check the notes: {\"draft\": true}. ";
    let answer = r#"{"title": "Anchoring", "tags": []}"#;
    let mut turn = ended(&format!("{narration}{answer}"), "end_turn");
    turn.last_message = Some(answer.to_owned());
    let (verb, prompts) = seated(vec![(turn, true)]);
    let out = verb.run(typed()).await.expect("the answer validates");
    assert_eq!(
        out.output,
        AgentValue::Structured(json!({"title": "Anchoring", "tags": []}))
    );
    let seat = out.seat.expect("a seat report");
    assert_eq!(
        seat.transcript.as_deref(),
        Some(format!("{narration}{answer}").as_str())
    );
    assert!(prompts.lock().unwrap().is_empty(), "no repair");

    let missed = r#"{"title": "Anchoring"}"#;
    let mut turn = ended(&format!("{narration}{missed}"), "end_turn");
    turn.last_message = Some(missed.to_owned());
    let (verb, _) = seated(vec![(turn, false)]);
    let err = verb.run(typed()).await.expect_err("the answer misses");
    assert_eq!(err.spec_code(), "NIKA-INFER-002");
    assert_eq!(
        VerbAgentError::details_of(&err),
        Some(json!({"partial_output": format!("{narration}{missed}"),
            "violations": ["\"tags\" is a required property"]}))
    );
}

/// The counts the adapter reported meet the ceiling: `max_tokens_total: 15` and a first turn
/// that reports 20 refuses the repair before its prompt, with the counted total.
#[tokio::test]
async fn reported_counts_over_the_ceiling_refuse_the_repair() {
    let first = reported(
        ended(r#"{"note": "long research"}"#, "end_turn"),
        TokenUsage::new(12, 8),
    );
    let (verb, prompts) = seated(vec![(first, true), (ended("{}", "end_turn"), false)]);
    let mut input = typed();
    input.max_tokens_total = Some(15);
    let err = verb.run(input).await.expect_err("the ceiling is met");
    assert_eq!(err.spec_code(), "NIKA-AGENT-002");
    assert!(
        matches!(
            &err,
            VerbAgentError::MaxTokens {
                total_tokens: 20,
                ..
            }
        ),
        "{err:?}"
    );
    assert!(
        prompts.lock().unwrap().is_empty(),
        "no repair prompt went out"
    );
}

/// The recorded claude-agent-acp 0.81.1 counts (3 input, 0 output, no cache) give the exact
/// total with their source, beside the context and the cost estimate of the adapter's last
/// update.
#[tokio::test]
async fn the_recorded_claude_counts_give_the_exact_total_with_provenance() {
    let mut counts = TokenUsage::new(3, 0);
    counts.cache_read_tokens = Some(0);
    counts.cache_write_tokens = Some(0);
    counts.total_tokens = Some(3);
    let mut turn = reported(ended("ok", "end_turn"), counts);
    turn.context = Some(HarnessContext::new(3, 200_000));
    turn.cost_estimate = Some(HarnessCostEstimate::new(0.0123, "USD"));
    let (verb, _) = seated(vec![(turn, false)]);
    let mut input = AgentInput::new("hi");
    input.max_tokens_total = Some(100);
    let out = verb.run(input).await.expect("completed");
    assert_eq!(out.total_tokens, 3);
    assert_eq!((out.usage.input_tokens, out.usage.output_tokens), (3, 0));
    let seat = out.seat.expect("a seat report");
    assert_eq!(seat.usage_source, Some("adapter_prompt_result"));
    assert_eq!(seat.token_ceiling, None, "judged on real counts");
    assert_eq!(seat.context, Some(HarnessContext::new(3, 200_000)));
    assert_eq!(
        seat.cost_estimate,
        Some(HarnessCostEstimate::new(0.0123, "USD"))
    );
    assert_eq!(out.tools_cost_usd, None, "an estimate is never spend");
}

/// A seat that reported no counts (codex-acp's `usage: null`) leaves the tokens unknown, not
/// counted, and a declared ceiling unjudged; across a repair, one unknown turn leaves the
/// session's tokens unknown.
#[tokio::test]
async fn no_counts_leave_the_tokens_and_the_ceiling_unknown() {
    let (verb, prompts) = seated(vec![
        (ended(r#"{"note": "x"}"#, "end_turn"), true),
        (ended(r#"{"title": "t", "tags": []}"#, "end_turn"), false),
    ]);
    let mut input = typed();
    input.max_tokens_total = Some(15);
    let out = verb.run(input).await.expect("repaired");
    assert_eq!(prompts.lock().unwrap().len(), 1, "the repair went out");
    let seat = out.seat.expect("a seat report");
    assert_eq!(seat.usage_source, None);
    assert_eq!(seat.token_ceiling, Some("unjudged"));
    assert_eq!(out.total_tokens, 0);

    let first = reported(ended(r#"{"note": "x"}"#, "end_turn"), TokenUsage::new(4, 2));
    let (verb, _) = seated(vec![
        (first, true),
        (ended(r#"{"title": "t", "tags": []}"#, "end_turn"), false),
    ]);
    let mut input = typed();
    input.max_tokens_total = Some(100);
    let out = verb.run(input).await.expect("repaired");
    let seat = out.seat.expect("a seat report");
    assert_eq!(seat.usage_source, None, "one unknown turn: unknown");
    assert_eq!(seat.token_ceiling, Some("unjudged"));
    assert_eq!(
        (
            out.total_tokens,
            out.usage.input_tokens,
            out.usage.output_tokens
        ),
        (0, 0, 0),
        "never one turn's counts as the session's"
    );
}

/// Counts whose weight cannot be computed are unknown, never a saturated number: an input of
/// 100 beside cache reads near the top of a count (the subtraction would leave almost nothing),
/// and an output near the top (the sum would overflow).
#[tokio::test]
async fn counts_that_cannot_be_weighed_are_unknown() {
    let mut split = TokenUsage::new(100, 5);
    split.cache_read_tokens = Some(u64::MAX - 50);
    for counts in [split, TokenUsage::new(100, u64::MAX - 10)] {
        let (verb, _) = seated(vec![(reported(ended("ok", "end_turn"), counts), false)]);
        let mut input = AgentInput::new("hi");
        input.max_tokens_total = Some(1_000);
        let out = verb.run(input).await.expect("completed");
        let seat = out.seat.expect("a seat report");
        assert_eq!(seat.usage_source, None);
        assert_eq!(seat.token_ceiling, Some("unjudged"));
        assert_eq!(
            (
                out.total_tokens,
                out.usage.input_tokens,
                out.usage.output_tokens
            ),
            (0, 0, 0)
        );
    }
}

/// Across a repair, two turns whose counts add up past a count are unknown, never a capped
/// sum: weights that overflow together (a large input, then a large output), and inputs and
/// cache reads that overflow while the weights fit.
#[tokio::test]
async fn two_turns_whose_counts_overflow_are_unknown() {
    let half = u64::MAX / 2 + 1;
    let mut cached = TokenUsage::new(u64::MAX - 10, 2);
    cached.cache_read_tokens = Some(u64::MAX - 20);
    for (first, second) in [
        (TokenUsage::new(half, 0), TokenUsage::new(0, half)),
        (cached.clone(), cached),
    ] {
        let (verb, prompts) = seated(vec![
            (reported(ended(r#"{"note": "x"}"#, "end_turn"), first), true),
            (
                reported(ended(r#"{"title": "t", "tags": []}"#, "end_turn"), second),
                false,
            ),
        ]);
        let mut input = typed();
        input.max_tokens_total = Some(u64::MAX);
        let out = verb.run(input).await.expect("repaired");
        assert_eq!(prompts.lock().unwrap().len(), 1, "the repair went out");
        let seat = out.seat.expect("a seat report");
        assert_eq!(seat.usage_source, None);
        assert_eq!(seat.token_ceiling, Some("unjudged"));
        assert_eq!(
            (
                out.total_tokens,
                out.usage.input_tokens,
                out.usage.output_tokens
            ),
            (0, 0, 0)
        );
        assert_eq!(out.usage.cache_read_tokens, None);
    }
}

fn declared() -> AccessRequirement {
    AccessRequirement::new()
        .with_via(Some("claude".into()))
        .with_protocol(Some(AccessProtocol::Acp))
        .with_fallback(Some(AccessFallback::None))
}

/// The turn as the ACP client closes it under a confirmed `opus`, with the model the adapter
/// reported for its answer.
fn attested(reported: &str, moved: bool) -> HarnessOutcome {
    let mut outcome = ended("ok", "end_turn");
    let mut selection = HarnessSelection::default();
    selection.model_option = Some("model".into());
    selection.transmitted_model = Some("opus".into());
    selection.configured_model = Some("opus".into());
    selection.configured_model_source = Some(ModelProvenance::ConfirmedSelection);
    if moved {
        selection.changed_mid_turn = vec![format!("model={reported}")];
    }
    outcome.selection = selection;
    outcome.observed_model = Some(reported.to_owned());
    outcome.observed_model_source = Some(ModelProvenance::AdapterUsageUpdate);
    outcome
}

/// Under `fallback: none`, a reported model other than the confirmed one is refused through
/// the moved-selection refusal; the confirmed alias answering as its dated id keeps four facts
/// apart: requested, transmitted, configured, and the responder the adapter reported.
#[tokio::test]
async fn the_reported_model_is_judged_and_kept_apart_from_the_configured_one() {
    let input = || {
        let mut input = AgentInput::new("hi").with_requirement(Some(&declared()));
        input.model = Some("anthropic/opus".into());
        input
    };
    let (verb, _) = seated(vec![(attested("claude-sonnet-4-5-20250929", true), false)]);
    let err = verb.run(input()).await.expect_err("another model answered");
    assert_eq!(err.spec_code(), "NIKA-1805");
    assert!(
        err.to_string().contains("model=claude-sonnet-4-5-20250929"),
        "{err}"
    );

    let (verb, _) = seated(vec![(attested("claude-opus-4-5-20251101", false), false)]);
    let out = verb
        .run(input())
        .await
        .expect("the confirmed model answered");
    assert_eq!(
        out.model_reported.as_deref(),
        Some("claude-opus-4-5-20251101")
    );
    assert_eq!(
        out.model_reported_source,
        Some(ModelProvenance::AdapterUsageUpdate)
    );
    let receipt = out.selection.expect("a receipt").to_json();
    assert_eq!(receipt["model"]["requested"], "anthropic/opus");
    assert_eq!(receipt["model"]["transmitted"], "opus");
    assert_eq!(receipt["model"]["configured"], "opus");
    assert_eq!(receipt["model"]["configured_source"], "confirmed_selection");
    assert_eq!(
        receipt["responder"],
        json!({"model": "claude-opus-4-5-20251101", "evidence": "adapter_usage_update"})
    );
}
