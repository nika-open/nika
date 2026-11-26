// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! What a seat's turn said about itself, judged before its value is: how it ended, the counts,
//! context and cost its adapter reported, the model it reported having answered with, and the
//! last message a `schema:` judges.
//!
//! - `end_turn` completes, and so does a backend that reports no stop reason. `max_tokens` is a
//!   cut final message (`NIKA-AGENT-006`). `refusal`, `max_turn_requests`, `cancelled` and a
//!   word this engine does not know fail the task (`NIKA-INFER-001`) with the text received:
//!   this path never sends `session/cancel`, so a turn that ends `cancelled` was cut by the
//!   harness, not finished.
//! - The counts are the adapter's (`adapter_prompt_result`); the budget weighs them as it
//!   weighs a native turn. No counts, or counts whose weight overflows a count, is unknown: the
//!   task's tokens stay unknown, and a declared `max_tokens_total` reads `unjudged`.
//! - The configured model, the transmitted one and the requested one are the selection's; the
//!   model the adapter reported for its answer is the responder, with its own evidence.

use nika_kernel::ai::harness::{
    AdapterReport, HarnessOutcome, HarnessStopReason, ModelProvenance, TokenUsage,
};

use crate::{AgentInput, SeatReport, VerbAgentError};

/// The source of counts a backend reported without naming the report they came from.
const UNNAMED_REPORT: &str = "harness_reported";

/// The failure a seat's stop reason is, or `None` when the turn completed.
pub(crate) fn stopped(outcome: &HarnessOutcome) -> Option<VerbAgentError> {
    let stop = outcome.stop_reason.as_ref()?;
    match stop {
        HarnessStopReason::EndTurn => None,
        HarnessStopReason::MaxTokens => Some(VerbAgentError::FinalCut {
            partial_output: outcome.output.clone(),
            stop_reason: stop.as_str().to_owned(),
            spend: Box::default(),
        }),
        _ => Some(VerbAgentError::HarnessStopped {
            stop_reason: stop.as_str().to_owned(),
            partial_output: outcome.output.clone(),
            spend: Box::default(),
        }),
    }
}

/// The seat's report of a completed turn.
pub(crate) fn report(outcome: &HarnessOutcome, input: &AgentInput) -> SeatReport {
    let mut seat = SeatReport::new();
    seat.stop_reason = outcome
        .stop_reason
        .as_ref()
        .map(|stop| stop.as_str().to_owned());
    // A backend that reports counts without naming their shape is still their reporter.
    seat.usage_source = outcome.usage.as_ref().map(|_| {
        outcome
            .usage_source
            .map_or(UNNAMED_REPORT, AdapterReport::as_str)
    });
    seat.token_ceiling = unjudged(input, seat.usage_source);
    seat.context = outcome.context;
    seat.cost_estimate.clone_from(&outcome.cost_estimate);
    seat.last_message.clone_from(&outcome.last_message);
    seat
}

/// `unjudged` when the task declares a token ceiling the seat's missing counts cannot judge.
pub(crate) fn unjudged(input: &AgentInput, usage_source: Option<&str>) -> Option<&'static str> {
    (input.max_tokens_total.is_some() && usage_source.is_none())
        .then_some(SeatReport::CEILING_UNJUDGED)
}

/// The budget weight of the counts the seat reported, fresh input and output as a native
/// turn's ([`crate::turn::budget_weight`]), by checked arithmetic: `None` when the seat reported
/// none, when its cache reads exceed its input, or when the sum overflows a count. Unknown,
/// never a saturated number the ceiling would under-count.
pub(crate) fn weight_of(usage: Option<&TokenUsage>) -> Option<u64> {
    let usage = usage?;
    usage
        .input_tokens
        .checked_sub(usage.cache_read_tokens.unwrap_or(0))?
        .checked_add(usage.output_tokens)
}

/// The model the session confirmed and how, kept apart from the one the adapter reported for
/// its answer. A backend that fills no configured model reports its session model as the
/// observed one, which then stands for the configured model unless the adapter reported it.
pub(crate) fn configured(outcome: &HarnessOutcome) -> (Option<String>, Option<ModelProvenance>) {
    let selection = &outcome.selection;
    if selection.configured_model.is_some() {
        return (
            selection.configured_model.clone(),
            selection.configured_model_source,
        );
    }
    match outcome.observed_model_source {
        Some(ModelProvenance::AdapterUsageUpdate) => (None, None),
        source => (outcome.observed_model.clone(), source),
    }
}

/// The model the adapter reported having answered with, when it did.
pub(crate) fn attested(outcome: &HarnessOutcome) -> Option<String> {
    (outcome.observed_model_source == Some(ModelProvenance::AdapterUsageUpdate))
        .then(|| outcome.observed_model.clone())
        .flatten()
}
