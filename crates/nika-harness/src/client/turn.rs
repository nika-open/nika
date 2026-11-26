// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! What an agent turn's frames say beyond its text, on the agent path only (a one-shot
//! completion profile keeps its own stricter judgment and reads none of this): where each
//! message of the turn starts, the adapter's usage reports, and the model it reports having
//! answered with, judged against an exact selection.
//!
//! - A message starts at the turn's first chunk, at a chunk whose `messageId` differs from the
//!   previous chunk's, and, between chunks that carry no id, at the first chunk after a
//!   `tool_call`. With neither signal the whole text is one message. The outcome keeps the
//!   whole text and names the last message that is not blank.
//! - The latest `usage_update` holds: its context and its cost replace the earlier ones, so an
//!   update without a cost clears it and no old value is shown as current.
//! - The model a `usage_update` names is the turn's attested model. Under a selection the
//!   client applied, a reported model that is not the confirmed one is recorded as the route
//!   moving the model during the turn, which an exact selection refuses
//!   (`HarnessSelection::moved_refusal`).

use nika_kernel::ai::harness::{
    AdapterReport, HarnessContext, HarnessCostEstimate, HarnessError, HarnessOutcome,
    ModelProvenance, TokenUsage,
};
use serde_json::Value;

use super::Driver;
use crate::wire;

/// The most text one agent turn may carry: past it the turn is refused before more is held,
/// as the one-shot profile refuses past its own bound (`authoring::acp::MAX_ANSWER`).
pub(super) const MAX_TURN_TEXT: usize = 4 * 1024 * 1024;

/// One session's turn facts beyond its text: the message boundaries and the attested model of
/// the turn in flight, and the session's latest context and cost.
#[derive(Debug, Default)]
pub(super) struct TurnLedger {
    /// Where each message of the turn starts in its text, in order.
    starts: Vec<usize>,
    /// The `messageId` of the turn's previous chunk, when it carried one.
    last_id: Option<String>,
    /// A `tool_call` arrived since the turn's previous chunk.
    tool_since: bool,
    /// The model the adapter reported for its last answer during this turn.
    attested: Option<String>,
    /// The session's context occupancy, as the latest `usage_update` gave it.
    context: Option<HarnessContext>,
    /// The session's cumulative cost estimate, as the latest `usage_update` gave it.
    cost: Option<HarnessCostEstimate>,
}

impl TurnLedger {
    /// A chunk of `text` is appended to the turn's text at byte `at`: it starts a message at
    /// the turn's first chunk, on a change of `messageId`, or after a tool call between chunks
    /// that carry no id. An empty chunk starts nothing.
    fn chunk(&mut self, at: usize, text: &str, id: Option<String>) {
        if text.is_empty() {
            return;
        }
        let starts = self.starts.is_empty()
            || match (&self.last_id, &id) {
                (None, None) => self.tool_since,
                (was, now) => was != now,
            };
        if starts {
            self.starts.push(at);
        }
        self.last_id = id;
        self.tool_since = false;
    }

    /// The turn's last message that is not blank, when it is not the whole `text`.
    fn last_message(&self, text: &str) -> Option<String> {
        let mut end = text.len();
        for &start in self.starts.iter().rev() {
            let message = text.get(start..end)?;
            if !message.trim().is_empty() {
                return (message.len() < text.len()).then(|| message.to_owned());
            }
            end = start;
        }
        None
    }

    /// Take in one `usage_update`; the model it names, if any.
    fn report(&mut self, report: wire::UsageReport) -> Option<String> {
        self.context = report.context;
        self.cost = report.cost;
        if report.model.is_some() {
            self.attested.clone_from(&report.model);
        }
        report.model
    }

    /// Close the turn into `outcome`: its last message, the token counts its prompt result
    /// reported, the session's latest context and cost, and the model the adapter reported for
    /// the turn, which replaces the configured one as the observed model with its own
    /// provenance (the configured one stays in the selection).
    fn close_into(&mut self, outcome: &mut HarnessOutcome, usage: Option<TokenUsage>) {
        outcome.last_message = self.last_message(&outcome.output);
        outcome.usage_source = usage.as_ref().map(|_| AdapterReport::PromptResult);
        outcome.usage = usage;
        outcome.context = self.context;
        outcome.cost_estimate.clone_from(&self.cost);
        if let Some(model) = self.attested.take() {
            outcome.observed_model = Some(model);
            outcome.observed_model_source = Some(ModelProvenance::AdapterUsageUpdate);
        }
        self.starts.clear();
        self.last_id = None;
        self.tool_since = false;
    }
}

/// The words of a model id, lowercase: its name after any `provider/` prefix, a bracketed
/// context hint (`[1m]`) set aside, split on every character that is not a letter or a digit.
fn words(id: &str) -> Vec<String> {
    let name = id.rsplit('/').next().unwrap_or(id);
    let name = name.split('[').next().unwrap_or(name);
    name.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

/// Whether the model the adapter reports having answered with is the `confirmed` selection.
/// A session option may be an alias of the id its provider names (claude-agent-acp's `opus`
/// answers as `claude-opus-4-5-20251101`), so the reported id names the confirmed value when
/// it holds every word of it, in order, and adds nothing but numbers (a version, a date) and
/// a leading vendor word. Any other word is another model: `opus` is not `claude-sonnet-…`,
/// `claude-opus-4-6` is not `claude-opus-4-5-…`, and `gpt-5.5` is not `gpt-5.5-codex`.
pub(super) fn names(confirmed: &str, reported: &str) -> bool {
    let (asked, said) = (words(confirmed), words(reported));
    if asked.is_empty() {
        return false;
    }
    let mut wanted = asked.iter().peekable();
    for (at, word) in said.iter().enumerate() {
        if wanted.peek() == Some(&word) {
            wanted.next();
        } else if at > 0 && !word.chars().all(|c| c.is_ascii_digit()) {
            return false;
        }
    }
    wanted.peek().is_none()
}

impl<R, W> Driver<R, W> {
    /// An agent message chunk on the agent path, before it is held: refused past the turn's
    /// bound, else placed among the turn's messages.
    pub(super) fn agent_text(&mut self, update: &Value, text: &str) -> Result<(), HarnessError> {
        if self.output.len().saturating_add(text.len()) > MAX_TURN_TEXT {
            return Err(HarnessError::Refused {
                reason: format!(
                    "the harness's answer exceeded the {} MiB one turn may hold; no answer \
                     accepted",
                    MAX_TURN_TEXT / (1024 * 1024)
                ),
            });
        }
        let at = self.output.len();
        self.turn.chunk(at, text, wire::chunk_message_id(update));
        Ok(())
    }

    /// What an update reports on the agent path: a tool call ends a message without an id,
    /// and a usage report gives the context, the cost and the model of the last answer.
    pub(super) fn agent_report(&mut self, update: &Value) {
        if wire::is_tool_call(update) {
            self.turn.tool_since = true;
        }
        if let Some(model) = wire::usage_report(update).and_then(|r| self.turn.report(r)) {
            self.judge_reported_model(&model);
        }
    }

    /// Under a model the client sent, a reported model that is not the confirmed one is the
    /// route moving the model during the turn: recorded once, in arrival order, for the exact
    /// selection to refuse. Without a sent model nothing was pinned, and the report is only the
    /// outcome's observed model.
    fn judge_reported_model(&mut self, reported: &str) {
        let Some(sent) = self.selection.transmitted_model.as_deref() else {
            return;
        };
        let confirmed = self.observed_model.as_deref().unwrap_or(sent);
        if names(confirmed, reported) {
            return;
        }
        let change = format!("model={reported}");
        if !self.selection.changed_mid_turn.contains(&change) {
            self.selection.changed_mid_turn.push(change);
        }
    }

    /// The turn's outcome from the agent path's ledger (see [`TurnLedger::close_into`]).
    pub(super) fn close_agent_turn(
        &mut self,
        outcome: &mut HarnessOutcome,
        usage: Option<TokenUsage>,
    ) {
        self.turn.close_into(outcome, usage);
    }
}

#[cfg(test)]
mod tests;
