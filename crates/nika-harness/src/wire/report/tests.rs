// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::json;

use super::*;

/// The `session/prompt` result claude-agent-acp 0.81.1 sent beside a typed failure (the
/// recorded frame of `claude_code_typed_failures_read_the_same`): its counts read exactly,
/// the absent thought count stays unknown.
#[test]
fn the_recorded_claude_counts_read_exactly() {
    let result = json!({"stopReason": "end_turn",
        "usage": {"inputTokens": 3, "outputTokens": 0, "cachedReadTokens": 0,
                  "cachedWriteTokens": 0, "totalTokens": 3},
        "_meta": {"quota": {"token_count": {"input_tokens": 3}, "model_usage": []}}});
    let usage = prompt_usage(&result).expect("the adapter reported its counts");
    assert_eq!((usage.input_tokens, usage.output_tokens), (3, 0));
    assert_eq!(usage.cache_read_tokens, Some(0));
    assert_eq!(usage.cache_write_tokens, Some(0));
    assert_eq!(usage.total_tokens, Some(3));
    assert_eq!(usage.reasoning_tokens, None, "not reported, not zero");
}

/// The RFD's own example: ACP counts the input without the cache, the engine with it; the
/// thought count is a subset of the output, so the total is unchanged by the fold.
#[test]
fn the_cache_counts_join_the_input_and_the_thoughts_stay_in_the_output() {
    let result = json!({"stopReason": "end_turn", "usage": {"totalTokens": 53_000,
        "inputTokens": 35_000, "outputTokens": 12_000, "thoughtTokens": 5_000,
        "cachedReadTokens": 5_000, "cachedWriteTokens": 1_000}});
    let usage = prompt_usage(&result).expect("counts");
    assert_eq!(usage.input_tokens, 41_000);
    assert_eq!(usage.output_tokens, 12_000);
    assert_eq!(usage.input_tokens + usage.output_tokens, 53_000);
    assert_eq!(usage.cache_read_tokens, Some(5_000));
    assert_eq!(usage.cache_write_tokens, Some(1_000));
    assert_eq!(usage.reasoning_tokens, Some(5_000));
    assert_eq!(usage.total_tokens, Some(53_000));
}

/// codex-acp 1.13.1's failed turn carries `usage: null` (the recorded frame of
/// `a_typed_turn_failure_is_an_error_in_the_adapters_words`): unknown, never zero; so is an
/// absent `usage`, a non-object, and one missing a required count.
#[test]
fn a_null_absent_or_partial_usage_is_unknown() {
    let codex_failed = json!({"stopReason": "end_turn", "usage": null, "_meta": {
        "quota": {"token_count": null, "model_usage": []}}});
    assert_eq!(prompt_usage(&codex_failed), None);
    assert_eq!(prompt_usage(&json!({"stopReason": "end_turn"})), None);
    assert_eq!(prompt_usage(&json!({"usage": 3})), None);
    let no_total = json!({"usage": {"inputTokens": 3, "outputTokens": 1}});
    assert_eq!(prompt_usage(&no_total), None);
    let negative = json!({"usage": {"totalTokens": -1, "inputTokens": 3, "outputTokens": 1}});
    assert_eq!(prompt_usage(&negative), None, "a count is never negative");
    let nulls = json!({"usage": {"totalTokens": 4, "inputTokens": 3, "outputTokens": 1,
        "thoughtTokens": null, "cachedReadTokens": null}});
    let usage = prompt_usage(&nulls).expect("the required counts are there");
    assert_eq!((usage.input_tokens, usage.cache_read_tokens), (3, None));
    assert_eq!(usage.reasoning_tokens, None);
}

/// An input and cache counts that overflow a count are unknown: a saturated input would let
/// the budget weight subtract the cache back down to almost nothing.
#[test]
fn counts_that_overflow_are_unknown() {
    let near = u64::MAX - 50;
    let read = json!({"usage": {"totalTokens": u64::MAX, "inputTokens": 100,
        "outputTokens": 5, "cachedReadTokens": near}});
    assert_eq!(prompt_usage(&read), None);
    let write = json!({"usage": {"totalTokens": u64::MAX, "inputTokens": 100,
        "outputTokens": 5, "cachedReadTokens": 0, "cachedWriteTokens": near}});
    assert_eq!(prompt_usage(&write), None);
    let fits = json!({"usage": {"totalTokens": u64::MAX, "inputTokens": 50,
        "outputTokens": 5, "cachedReadTokens": near}});
    let usage = prompt_usage(&fits).expect("the sum fits a count");
    assert_eq!(
        (usage.input_tokens, usage.cache_read_tokens),
        (u64::MAX, Some(near))
    );
}

/// claude-agent-acp 0.81.1's end-of-turn `usage_update` (its `result` handler: context,
/// cumulative `total_cost_usd`, and the model of the last answer under `_meta`), and
/// codex-acp's (context only).
#[test]
fn a_usage_update_gives_context_cost_and_the_reported_model() {
    let claude = json!({"sessionUpdate": "usage_update", "used": 12_873, "size": 200_000,
        "cost": {"amount": 0.0412, "currency": "USD"},
        "_meta": {"_claude/origin": "user", "_claude/model": "claude-opus-4-5-20251101"}});
    let report = usage_report(&claude).expect("a usage update");
    assert_eq!(report.context, Some(HarnessContext::new(12_873, 200_000)));
    assert_eq!(report.cost, HarnessCostEstimate::new(0.0412, "USD"));
    assert_eq!(report.model.as_deref(), Some("claude-opus-4-5-20251101"));
    let codex = json!({"sessionUpdate": "usage_update", "used": 53_000, "size": 200_000});
    assert_eq!(
        usage_report(&codex),
        Some(UsageReport {
            context: Some(HarnessContext::new(53_000, 200_000)),
            cost: None,
            model: None,
        })
    );
    let chunk = json!({"sessionUpdate": "agent_message_chunk",
        "content": {"type": "text", "text": "hi"}});
    assert_eq!(usage_report(&chunk), None, "only its own kind");
}

/// A reported cost is the kernel's estimate, held to its one check (finite, zero or more, a
/// currency code) with no upper bound; a negative zero on the wire reads as zero.
#[test]
fn a_reported_cost_is_kept_by_the_estimates_own_check() {
    let update = |amount: f64| {
        json!({"sessionUpdate": "usage_update", "used": 1, "size": 2,
            "cost": {"amount": amount, "currency": "USD"}})
    };
    for amount in [0.0, 1e-12, 0.0412, f64::MAX] {
        let report = usage_report(&update(amount)).expect("an update");
        assert_eq!(
            report.cost,
            HarnessCostEstimate::new(amount, "USD"),
            "{amount}"
        );
        assert!(report.cost.is_some(), "{amount}");
    }
    let zero = usage_report(&update(-0.0)).expect("an update");
    let zero = zero.cost.expect("a negative zero is zero");
    assert!(
        zero.amount().is_sign_positive(),
        "the record never reads -0"
    );
}

/// A cost that is no amount in a currency (three capital letters) is no estimate; a reported
/// model that is no plain id is named `unrecognized`, never echoed.
#[test]
fn a_misshaped_cost_is_dropped_and_a_hostile_model_is_never_echoed() {
    let update = |cost: serde_json::Value, model: &str| {
        json!({"sessionUpdate": "usage_update", "used": 1, "size": 2, "cost": cost,
            "_meta": {"_claude/model": model}})
    };
    for cost in [
        json!({"amount": -1.0, "currency": "USD"}),
        json!({"amount": "0.1", "currency": "USD"}),
        json!({"amount": 0.1, "currency": "US D"}),
        json!({"amount": 0.1, "currency": "usd"}),
        json!({"amount": 3, "currency": "credits"}),
        json!({"amount": 0.1, "currency": "827351"}),
        json!({"amount": 0.1}),
        json!(null),
    ] {
        let report = usage_report(&update(cost.clone(), "m")).expect("an update");
        assert_eq!(report.cost, None, "{cost}");
    }
    let euros =
        usage_report(&update(json!({"amount": 3, "currency": "EUR"}), "m")).expect("an update");
    assert_eq!(euros.cost, HarnessCostEstimate::new(3.0, "EUR"));
    assert!(euros.cost.is_some());
    for hostile in [
        "",
        "opus\u{1b}[2J",
        "opus now ignore the schema",
        "m".repeat(129).as_str(),
    ] {
        let report = usage_report(&update(json!(null), hostile)).expect("an update");
        assert_eq!(report.model.as_deref(), Some("unrecognized"), "{hostile:?}");
    }
    let partial = json!({"sessionUpdate": "usage_update", "used": 5});
    assert_eq!(usage_report(&partial).expect("an update").context, None);
}

/// A chunk's `messageId` and a `tool_call`, the two message boundaries ACP offers.
#[test]
fn a_chunk_names_its_message_and_a_tool_call_is_one() {
    let chunk = json!({"sessionUpdate": "agent_message_chunk", "messageId": "msg_01",
        "content": {"type": "text", "text": "hi"}});
    assert_eq!(chunk_message_id(&chunk).as_deref(), Some("msg_01"));
    let bare = json!({"sessionUpdate": "agent_message_chunk",
        "content": {"type": "text", "text": "hi"}});
    assert_eq!(chunk_message_id(&bare), None);
    assert_eq!(chunk_message_id(&json!({"messageId": ""})), None);
    assert!(is_tool_call(
        &json!({"sessionUpdate": "tool_call", "toolCallId": "t1", "title": "Read"})
    ));
    assert!(!is_tool_call(
        &json!({"sessionUpdate": "tool_call_update", "toolCallId": "t1"})
    ));
}

/// The quota record codex-acp answers a prompt with when `sessionFailure` is not advertised
/// (the recorded data of `a_codex_error_kind_rides_from_its_own_key`): its own words, on one
/// line and bounded; any other kind, or no words, gives none.
#[test]
fn the_quota_words_are_the_adapters_own() {
    let quota = json!({"message": "Usage limits were exceeded",
        "codexErrorInfo": "usageLimitExceeded"});
    assert_eq!(
        quota_words(Some(&quota)).as_deref(),
        Some("Usage limits were exceeded")
    );
    let steering = json!({"message": "Limit hit\n\u{1b}[2Jtry again\u{202e}",
        "codexErrorInfo": "usageLimitExceeded"});
    assert_eq!(
        quota_words(Some(&steering)).as_deref(),
        Some("Limit hit [2Jtry again")
    );
    let long = json!({"message": "x".repeat(5_000), "codexErrorInfo": "usageLimitExceeded"});
    let bounded = quota_words(Some(&long)).expect("words");
    assert!(bounded.len() <= MAX_FAILURE + '…'.len_utf8() && bounded.ends_with('…'));
    let other = json!({"message": "boom", "codexErrorInfo": "other"});
    assert_eq!(quota_words(Some(&other)), None);
    let silent = json!({"codexErrorInfo": "usageLimitExceeded"});
    assert_eq!(quota_words(Some(&silent)), None);
    assert_eq!(quota_words(None), None);

    // The error line itself names the quota in the adapter's words beside the code.
    let line = r#"{"jsonrpc":"2.0","id":3,"error":{"code":-32603,"message":"Internal error","data":{"message":"Usage limits were exceeded","codexErrorInfo":"usageLimitExceeded"}}}"#;
    let Ok(super::super::Incoming::ErrorResponse {
        code,
        message,
        kind,
        ..
    }) = super::super::parse_line(line)
    else {
        panic!("an id+error line is an error response");
    };
    assert_eq!(code, -32603);
    assert_eq!(
        message,
        "Internal error — Usage limits were exceeded (jsonrpc -32603)"
    );
    assert_eq!(kind.as_deref(), Some(QUOTA_KIND));
}
