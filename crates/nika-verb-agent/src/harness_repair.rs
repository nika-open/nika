// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! The one schema repair of a seat (NIK-402): a final message that misses the task `schema:`
//! gets ONE more prompt on the same session, which still holds the agent's work, instead of
//! failing the task with that work.
//!
//! The spec leaves the repair to the engine: « The engine MAY auto-retry validation failures
//! internally (transparent to the workflow) before surfacing the error (`NIKA-INFER-002`) »
//! (spec 05 §Structured output validation; spec 02 §infer, Conformance), and on a seat « each
//! repair the engine spends is a new request » (spec 02 §Subscription harness access). Spec 02
//! §agent gives `schema:` « the same contract as `infer.schema:` ».
//!
//! - The request asks the backend to keep the session open for one follow-up. A backend
//!   that cannot ends with `Completed` at once: a session closed on its final message gets
//!   no repair, and a miss fails `NIKA-INFER-002` as before.
//! - The repair prompt lists every violation the validator found and the schema, and asks
//!   for the final message only. It is witnessed as `agent_nudge` (reason `schema_repair`).
//! - The schema judges the turn's last message: narration before a tool call is not the
//!   answer. A failure keeps every message of the turn at `partial_output`, and a success of
//!   several messages keeps the whole text beside its value (`seat.transcript`).
//! - The repair is one more request, so it must fit the task's cumulative token budget, like
//!   the native loop's re-ask: a budget the counts the seat reported already meet refuses it
//!   with `NIKA-AGENT-002` and the rejected turn's text. A seat that reported no counts leaves
//!   the ceiling unjudged, and the record says so (`token_ceiling: unjudged`). A seat turn is
//!   unpriced (the subscription's to bound), so the run's USD cap has nothing to charge it.
//! - The repaired message is the terminal one: a second miss fails `NIKA-INFER-002` « after 1
//!   repair attempt ». There is never a second repair.

use nika_kernel::ai::harness::{FollowUpReply, HarnessRequest, TokenUsage};

use crate::observe::NudgeReason;
use crate::shape::Rejection;
use crate::{AgentEvent, AgentInput, AgentObserver, AgentOutput, AgentValue, VerbAgentError};

/// The schema side of one seat session: the task's validator and the repair it spent.
pub(crate) struct SchemaRepair<'a> {
    input: &'a AgentInput,
    validator: Option<&'a jsonschema::Validator>,
    /// The first turn, once the repair prompt was sent: its turns and tokens count with the
    /// repair turn's.
    first: Option<AgentOutput>,
}

impl<'a> SchemaRepair<'a> {
    pub(crate) fn new(input: &'a AgentInput, validator: Option<&'a jsonschema::Validator>) -> Self {
        Self {
            input,
            validator,
            first: None,
        }
    }

    /// The session request, with room for the repair when the task has a schema.
    pub(crate) fn request(&self, request: HarnessRequest) -> HarnessRequest {
        if self.validator.is_some() {
            request.with_follow_up()
        } else {
            request
        }
    }

    /// A turn that ended with its session open. `Some` is the task's output (the lane drops
    /// and the session closes); `None` means the repair prompt went out on the lane and the
    /// session's next turn is the terminal one.
    pub(crate) fn ended(
        &mut self,
        output: AgentOutput,
        lane: FollowUpReply,
        observer: &dyn AgentObserver,
    ) -> Result<Option<AgentOutput>, VerbAgentError> {
        let (Some(validator), Some(schema)) = (self.validator, self.input.schema.as_ref()) else {
            return Ok(Some(output));
        };
        let Some((text, judged)) = texts(&output) else {
            return Ok(Some(output));
        };
        if self.first.is_some() {
            // A backend that ended the repair turn open again: it is the terminal turn.
            return self.completed(output).map(Some);
        }
        let rejection = match crate::shape::validate_text(judged, validator, schema) {
            Ok(value) => return Ok(Some(structured(output, value))),
            Err(rejection) => rejection,
        };
        crate::turn::token_budget_gate(self.input.max_tokens_total, output.total_tokens, text)?;
        observer.on_event(&AgentEvent::Nudged {
            turn: output.turns,
            reason: NudgeReason::SchemaRepair,
            period: 0,
        });
        lane.prompt(repair_prompt(&rejection, schema));
        self.first = Some(output);
        Ok(None)
    }

    /// The repair turn did not come back: the host exited after the first turn, answered the
    /// follow-up with an error, or went silent past its idle deadline. Once the repair prompt
    /// went out, the task fails `NIKA-INFER-002` with the first turn's message at
    /// `partial_output` and its violations, the repair's failure named as the cause, so the
    /// work and an `on_codes: [NIKA-INFER-002]` recovery survive it. `None` before any repair
    /// prompt: the failure is the session's own.
    pub(crate) fn repair_failed(&self, cause: &dyn std::fmt::Display) -> Option<VerbAgentError> {
        let (first, validator) = (self.first.as_ref()?, self.validator?);
        let schema = self.input.schema.as_ref()?;
        let (text, judged) = texts(first)?;
        let rejection = crate::shape::validate_text(judged, validator, schema).err()?;
        let detail = format!("{} (the repair turn failed: {cause})", rejection.detail());
        Some(rejection.into_error(detail, text.to_owned()))
    }

    /// A turn refused on its own outcome (an exact selection the agent moved during it): once
    /// the repair prompt went out, the refusal is the repair's failure and the first turn's
    /// message is kept ([`Self::repair_failed`]); before it, the refusal stands as it is.
    pub(crate) fn refused(&self, refusal: VerbAgentError) -> VerbAgentError {
        self.repair_failed(&refusal).unwrap_or(refusal)
    }

    /// The session's terminal turn: the first one of a session that closed on it, or the
    /// repair turn. Its final message is validated; a miss is `NIKA-INFER-002` with the
    /// message and every violation it holds.
    pub(crate) fn completed(&self, mut output: AgentOutput) -> Result<AgentOutput, VerbAgentError> {
        let (Some(validator), Some(schema)) = (self.validator, self.input.schema.as_ref()) else {
            return Ok(output);
        };
        if let Some(first) = &self.first {
            counted_with(&mut output, first, self.input);
        }
        let Some((text, judged)) = texts(&output) else {
            return Ok(output);
        };
        match crate::shape::validate_text(judged, validator, schema) {
            Ok(value) => Ok(structured(output, value)),
            Err(rejection) => {
                let why = if self.first.is_some() {
                    "after 1 repair attempt"
                } else {
                    "validated without a repair turn: the seat's session closed with its final \
                     message"
                };
                let detail = format!("{} ({why})", rejection.detail());
                Err(rejection.into_error(detail, text.to_owned()))
            }
        }
    }
}

/// `output` carrying its validated value. When the value is the turn's last message, the
/// turn's whole text stays the record's (`seat.transcript`).
fn structured(mut output: AgentOutput, value: serde_json::Value) -> AgentOutput {
    let whole = std::mem::replace(&mut output.output, AgentValue::Structured(value));
    if let (Some(seat), AgentValue::Text(text)) = (output.seat.as_mut(), whole)
        && seat.last_message.is_some()
    {
        seat.transcript = Some(text);
    }
    output
}

/// A turn's whole text and the part a schema judges: its last message when the turn held
/// several, else the whole text. `None` for a value that is not text.
fn texts(output: &AgentOutput) -> Option<(&str, &str)> {
    let AgentValue::Text(text) = &output.output else {
        return None;
    };
    let judged = (output.seat.as_ref())
        .and_then(|seat| seat.last_message.as_deref())
        .unwrap_or(text);
    Some((text, judged))
}

/// The repair turn's output counted with the first turn's: turns add up, and tokens by checked
/// arithmetic. When either turn reported no counts, or the two turns' counts overflow a count,
/// the session's tokens are unknown, never one turn's alone nor a capped sum, and a declared
/// ceiling reads unjudged.
fn counted_with(output: &mut AgentOutput, first: &AgentOutput, input: &AgentInput) {
    output.turns = output.turns.saturating_add(first.turns);
    let reported = |out: &AgentOutput| (out.seat.as_ref()).is_none_or(|s| s.usage_source.is_some());
    let known = reported(first) && reported(output) && adds_up(&output.usage, &first.usage);
    if let Some(total) = (output.total_tokens.checked_add(first.total_tokens)).filter(|_| known) {
        output.total_tokens = total;
        output.usage.absorb(&first.usage);
    } else {
        output.total_tokens = 0;
        output.usage = TokenUsage::default();
        if let Some(seat) = output.seat.as_mut() {
            seat.usage_source = None;
            seat.token_ceiling = crate::harness_turn::unjudged(input, None);
        }
    }
}

/// Whether the counts an ACP prompt result can report (input, output, cache reads and writes,
/// thought, total) add up across two turns within a count. A count only one turn reports is
/// that turn's, so it cannot overflow.
fn adds_up(a: &TokenUsage, b: &TokenUsage) -> bool {
    let fits =
        |x: Option<u64>, y: Option<u64>| x.zip(y).is_none_or(|(x, y)| x.checked_add(y).is_some());
    a.input_tokens.checked_add(b.input_tokens).is_some()
        && a.output_tokens.checked_add(b.output_tokens).is_some()
        && fits(a.cache_read_tokens, b.cache_read_tokens)
        && fits(a.cache_write_tokens, b.cache_write_tokens)
        && fits(a.reasoning_tokens, b.reasoning_tokens)
        && fits(a.total_tokens, b.total_tokens)
}

/// The repair prompt: every violation, the schema, and the final message only. The session
/// still holds the work, so it is answered from there.
pub(crate) fn repair_prompt(rejection: &Rejection, schema: &serde_json::Value) -> String {
    format!(
        "Your final message did not satisfy the required output schema. Validation failed \
         with: {}. Do not redo the work: reply with ONLY your final message, a JSON value that \
         satisfies this JSON Schema, no prose, no code fences:\n{}",
        rejection.detail(),
        crate::shape::render_schema(schema)
    )
}
