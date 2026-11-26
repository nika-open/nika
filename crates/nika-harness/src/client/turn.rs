// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! What an agent turn's frames say beyond its text, on the agent path only (a one-shot
//! completion profile keeps its own stricter judgment and reads none of this): where each
//! message of the turn starts, the adapter's usage reports, and the model it reports having
//! answered with, judged against an exact selection.
//!
//! - A message starts at the turn's first chunk, at the first chunk after a `tool_call`, and at
//!   a chunk whose `messageId` the current message does not carry: another id, or an id after
//!   chunks that carried none. A chunk without an id stays in the current message, so a notice
//!   an adapter sends without one after the answer never replaces it as the last message. With
//!   no signal the whole text is one message. The outcome keeps the whole text and names the
//!   last message that is not blank.
//! - The latest `usage_update` holds: its context and its cost replace the earlier ones, so an
//!   update without a cost clears it and no old value is shown as current.
//! - The model a `usage_update` names is the turn's attested model. Under a selection the
//!   client applied, a reported model that is demonstrably another model than the confirmed one
//!   is recorded as the route moving the model during the turn, which an exact selection refuses
//!   (`HarnessSelection::moved_refusal`); a spelling the rule cannot place is only recorded.

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
    /// The `messageId` of the current message, when its first chunk carried one.
    current_id: Option<String>,
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
    /// A chunk of `text` is appended to the turn's text at byte `at`. It starts a message at
    /// the turn's first chunk, after a tool call, and when it carries a `messageId` the current
    /// message does not (another id, or an id after chunks that carried none); a chunk without
    /// an id stays in the current message. An empty chunk starts nothing.
    fn chunk(&mut self, at: usize, text: &str, id: Option<String>) {
        if text.is_empty() {
            return;
        }
        let starts = self.starts.is_empty()
            || self.tool_since
            || id
                .as_ref()
                .is_some_and(|now| self.current_id.as_ref() != Some(now));
        if starts {
            self.starts.push(at);
            self.current_id = id;
        }
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
        self.current_id = None;
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

/// The model lines of the ids a reporting adapter names (claude-agent-acp's `_claude/model`).
const LINES: [&str; 3] = ["opus", "sonnet", "haiku"];

/// What a reported model id says about the confirmed selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Verdict {
    /// The reported id is the confirmed model, or the confirmed alias names it.
    Same,
    /// The reported id is demonstrably another model: an exact selection refuses it.
    Different,
    /// The rule cannot place one of the two spellings: the report is recorded, never refused.
    Unplaced,
}

/// A model id read as its parts: its name words and its version numbers, in order, and its
/// release date. A provider revision after the date (Bedrock's `-v1:0`) is set aside.
#[derive(Debug, Default)]
struct Spelling {
    names: Vec<String>,
    version: Vec<u32>,
    date: Option<String>,
}

impl Spelling {
    /// `id` read as its parts, or `None` when it names nothing or holds a number that is
    /// neither a version part (one to three digits) nor a date (eight).
    fn of(id: &str) -> Option<Self> {
        let mut spelled = Self::default();
        for word in words(id) {
            let revision = word.len() > 1
                && word.starts_with('v')
                && word.bytes().skip(1).all(|b| b.is_ascii_digit());
            if spelled.date.is_some() && revision {
                break;
            }
            if word.bytes().all(|b| b.is_ascii_digit()) {
                match word.len() {
                    1..=3 => spelled.version.push(word.parse().ok()?),
                    8 => spelled.date = Some(word),
                    _ => return None,
                }
            } else {
                spelled.names.push(word);
            }
        }
        (!spelled.names.is_empty()).then_some(spelled)
    }

    /// The one model line among the names, when there is exactly one.
    fn line(&self) -> Option<&str> {
        let mut lines = (self.names.iter()).filter(|word| LINES.contains(&word.as_str()));
        let line = lines.next()?;
        lines.next().is_none().then_some(line.as_str())
    }
}

/// What the model the adapter reports having answered with says about the `confirmed`
/// selection. A refusal must be certain, because a false one throws away a finished turn, so
/// only a demonstrated difference is `Different`:
///
/// - both ids name one model line (`opus`, `sonnet`, `haiku`) and the lines differ;
/// - or the reported id names the confirmed one (the confirmed name words end the reported
///   ones, which may add leading vendor words: `opus` is named by
///   `anthropic.claude-opus-4-5-20251101-v1:0`), and both carry a release date that differs, or
///   both carry a version that differs (`claude-opus-4` is not `claude-opus-4-5-20251101`).
///
/// A confirmed spelling without a version is named by any version of its line, and a date only
/// one side carries is no difference (`claude-opus-4` is `claude-opus-4-20250514`). Any other
/// pair is `Unplaced`: an alias the rule cannot read (`opusplan`), an added name word
/// (`gpt-5.5-codex` against `gpt-5.5`), a reported id without the version the confirmed one
/// states, or `unrecognized`.
pub(super) fn verdict(confirmed: &str, reported: &str) -> Verdict {
    let (Some(asked), Some(said)) = (Spelling::of(confirmed), Spelling::of(reported)) else {
        return Verdict::Unplaced;
    };
    if let (Some(wanted), Some(answered)) = (asked.line(), said.line())
        && wanted != answered
    {
        return Verdict::Different;
    }
    if !said.names.ends_with(&asked.names) {
        return Verdict::Unplaced;
    }
    if let (Some(wanted), Some(answered)) = (&asked.date, &said.date)
        && wanted != answered
    {
        return Verdict::Different;
    }
    match (asked.version.is_empty(), said.version.is_empty()) {
        (true, _) => Verdict::Same,
        (false, true) => Verdict::Unplaced,
        (false, false) if asked.version == said.version => Verdict::Same,
        (false, false) => Verdict::Different,
    }
}

/// The prefix of a model move in `HarnessSelection::changed_mid_turn`.
const MODEL_MOVE: &str = "model=";

/// Whether a turn holding `held` bytes of text may take `more`: up to [`MAX_TURN_TEXT`] in all,
/// exactly that much included.
fn admits(held: usize, more: usize) -> bool {
    held.checked_add(more)
        .is_some_and(|total| total <= MAX_TURN_TEXT)
}

impl<R, W> Driver<R, W> {
    /// An agent message chunk on the agent path, before it is held: refused past the turn's
    /// bound, else placed among the turn's messages.
    pub(super) fn agent_text(&mut self, update: &Value, text: &str) -> Result<(), HarnessError> {
        if !admits(self.output.len(), text.len()) {
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

    /// Under a model the client sent, a reported model that is demonstrably another one
    /// ([`verdict`]) is the route moving the model during the turn, recorded for the exact
    /// selection to refuse. Only the first move is kept: one is enough to refuse, and a stream
    /// of new ids grows neither the list nor the refusal. Without a sent model nothing was
    /// pinned, and the report is only the outcome's observed model.
    fn judge_reported_model(&mut self, reported: &str) {
        let Some(sent) = self.selection.transmitted_model.as_deref() else {
            return;
        };
        let confirmed = self.observed_model.as_deref().unwrap_or(sent);
        if verdict(confirmed, reported) != Verdict::Different {
            return;
        }
        let moves = &mut self.selection.changed_mid_turn;
        if !moves.iter().any(|change| change.starts_with(MODEL_MOVE)) {
            moves.push(format!("{MODEL_MOVE}{reported}"));
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
