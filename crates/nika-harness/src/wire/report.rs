// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! What an adapter reports about a turn beyond its text, read leniently: the token counts of
//! the `session/prompt` result (`usage`, a draft ACP field), the `usage_update` notification
//! (stable ACP: context occupancy, cumulative cost, and claude-agent-acp's model of the last
//! answer), the `messageId` of a message chunk, and codex-acp's own words for an exhausted
//! quota. A missing or mis-shaped field reads as absent, never as zero.

use nika_kernel::ai::harness::{HarnessContext, HarnessCostEstimate, TokenUsage};
use serde_json::Value;

use super::{MAX_FAILURE, bounded_to, one_line};

/// codex-acp's kind of an exhausted usage quota (`error.data.codexErrorInfo`).
pub(crate) const QUOTA_KIND: &str = "usageLimitExceeded";

/// The longest model id an adapter report may name; a longer one is unrecognized.
const MAX_MODEL_WORD: usize = 128;

/// The token counts of a `session/prompt` result, in the engine's convention: the input counts
/// its cache subsets, the output its reasoning subset. ACP's draft `usage` counts the input
/// without the cache (`totalTokens` = input + output + cache reads + cache writes, the RFD's
/// example and both installed adapters), so the cache counts are added back. `None` when
/// `usage` is absent, `null`, or lacks one of its three required counts (`totalTokens`,
/// `inputTokens`, `outputTokens`): unknown, never zero. An optional count that is absent or
/// `null` stays `None`.
pub(crate) fn prompt_usage(result: &Value) -> Option<TokenUsage> {
    let usage = result.get("usage")?.as_object()?;
    let count = |key: &str| usage.get(key).and_then(Value::as_u64);
    let (total, input, output) = (
        count("totalTokens")?,
        count("inputTokens")?,
        count("outputTokens")?,
    );
    let (read, write) = (count("cachedReadTokens"), count("cachedWriteTokens"));
    let prompt = input
        .saturating_add(read.unwrap_or(0))
        .saturating_add(write.unwrap_or(0));
    let mut tokens = TokenUsage::new(prompt, output);
    tokens.cache_read_tokens = read;
    tokens.cache_write_tokens = write;
    tokens.reasoning_tokens = count("thoughtTokens");
    tokens.total_tokens = Some(total);
    Some(tokens)
}

/// One `usage_update`, as far as it could be read.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct UsageReport {
    /// `used` and `size`, when both are counts.
    pub(crate) context: Option<HarnessContext>,
    /// `cost`, when it holds a finite, non-negative `amount` and an ISO 4217 `currency` code.
    pub(crate) cost: Option<HarnessCostEstimate>,
    /// claude-agent-acp's `_meta["_claude/model"]`: an id, or `unrecognized` when the report
    /// names one that is no plain model id.
    pub(crate) model: Option<String>,
}

/// The update when it is a `usage_update`, else `None` (every other kind is observed elsewhere).
pub(crate) fn usage_report(update: &Value) -> Option<UsageReport> {
    if update.get("sessionUpdate").and_then(Value::as_str) != Some("usage_update") {
        return None;
    }
    let count = |key: &str| update.get(key).and_then(Value::as_u64);
    let context = count("used")
        .zip(count("size"))
        .map(|(used, size)| HarnessContext::new(used, size));
    // The currency rides the record, so only an ISO 4217 code shape is kept: three ASCII
    // capitals. Any other unit is no estimate the record can name.
    let cost = update.get("cost").and_then(|cost| {
        let amount = cost.get("amount")?.as_f64()?;
        let currency = cost.get("currency")?.as_str()?;
        let code = currency.len() == 3 && currency.bytes().all(|b| b.is_ascii_uppercase());
        (amount.is_finite() && amount >= 0.0 && code)
            .then(|| HarnessCostEstimate::new(amount, currency))
    });
    let model = update
        .pointer("/_meta/_claude~1model")
        .and_then(Value::as_str)
        .map(model_word);
    Some(UsageReport {
        context,
        cost,
        model,
    })
}

/// A model id an adapter reported, kept only as a plain id (ASCII letters, digits and
/// `. _ - : / @ [ ] +`, at most [`MAX_MODEL_WORD`] bytes), else `unrecognized`: it reaches a
/// refusal and the receipt, so no adapter text rides it.
fn model_word(raw: &str) -> String {
    let plain = !raw.is_empty()
        && raw.len() <= MAX_MODEL_WORD
        && raw
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-:/@[]+".contains(c));
    if plain { raw } else { "unrecognized" }.to_owned()
}

/// The `messageId` of an agent message chunk, when it carries one: a change of it starts a
/// new message (ACP `ContentChunk.messageId`).
pub(crate) fn chunk_message_id(update: &Value) -> Option<String> {
    update
        .get("messageId")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
}

/// Whether the update reports a new tool call (`tool_call`): between chunks that carry no
/// `messageId`, it ends the message before it.
pub(crate) fn is_tool_call(update: &Value) -> bool {
    update.get("sessionUpdate").and_then(Value::as_str) == Some("tool_call")
}

/// codex-acp's own words for an exhausted quota: `error.data.message` beside its
/// `usageLimitExceeded` kind, on one line and bounded like a turn failure's title. `None` for
/// any other error, or when the adapter gave no words: the JSON-RPC code alone names no quota.
pub(crate) fn quota_words(data: Option<&Value>) -> Option<String> {
    let data = data?;
    if data.get("codexErrorInfo").and_then(Value::as_str) != Some(QUOTA_KIND) {
        return None;
    }
    let words = one_line(data.get("message")?.as_str()?);
    (!words.is_empty()).then(|| bounded_to(&words, MAX_FAILURE))
}

#[cfg(test)]
mod tests;
