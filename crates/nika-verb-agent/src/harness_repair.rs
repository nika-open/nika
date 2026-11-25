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
//! - The repair is one more request, so it must fit the task's cumulative token budget, like
//!   the native loop's re-ask: a budget already met refuses it with `NIKA-AGENT-002` and the
//!   rejected message. A seat turn is unpriced (the subscription's to bound), so the run's
//!   USD cap has nothing to charge it.
//! - The repaired message is the terminal one: a second miss fails `NIKA-INFER-002` « after 1
//!   repair attempt ». There is never a second repair.

use nika_kernel::ai::harness::{FollowUpReply, HarnessRequest};

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
        let AgentValue::Text(text) = &output.output else {
            return Ok(Some(output));
        };
        if self.first.is_some() {
            // A backend that ended the repair turn open again: it is the terminal turn.
            return self.completed(output).map(Some);
        }
        let rejection = match crate::shape::validate_text(text, validator) {
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
        let AgentValue::Text(text) = &first.output else {
            return None;
        };
        let rejection = crate::shape::validate_text(text, validator).err()?;
        let detail = format!("{} (the repair turn failed: {cause})", rejection.detail());
        Some(rejection.into_error(detail, text.clone()))
    }

    /// The session's terminal turn: the first one of a session that closed on it, or the
    /// repair turn. Its final message is validated; a miss is `NIKA-INFER-002` with the
    /// message and every violation it holds.
    pub(crate) fn completed(&self, mut output: AgentOutput) -> Result<AgentOutput, VerbAgentError> {
        let Some(validator) = self.validator else {
            return Ok(output);
        };
        if let Some(first) = &self.first {
            output.turns = output.turns.saturating_add(first.turns);
            output.total_tokens = output.total_tokens.saturating_add(first.total_tokens);
            output.usage.absorb(&first.usage);
        }
        let AgentValue::Text(text) = &output.output else {
            return Ok(output);
        };
        match crate::shape::validate_text(text, validator) {
            Ok(value) => Ok(structured(output, value)),
            Err(rejection) => {
                let why = if self.first.is_some() {
                    "after 1 repair attempt"
                } else {
                    "validated without a repair turn: the seat's session closed with its final \
                     message"
                };
                let detail = format!("{} ({why})", rejection.detail());
                Err(rejection.into_error(detail, text.clone()))
            }
        }
    }
}

/// `output` carrying its validated value.
fn structured(mut output: AgentOutput, value: serde_json::Value) -> AgentOutput {
    output.output = AgentValue::Structured(value);
    output
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
