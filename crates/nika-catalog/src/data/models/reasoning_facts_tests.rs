// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! The reasoning facts recorded per route (2026-10-10) and the routes left unknown. The two rules
//! that carry facts for one provider stand before a broader rule and must keep its caps.

#![cfg(feature = "capabilities")]

use super::model_capabilities;
use crate::types::model::{ReasoningFacts, RefusedControl, ThinkingOff};

fn facts(provider: &str, model: &str) -> ReasoningFacts {
    model_capabilities(provider, model).reasoning_facts
}

fn thinks(off: ThinkingOff) -> ReasoningFacts {
    ReasoningFacts::new(Some(true), Some(true), Some(off))
}

#[test]
fn recorded_routes_carry_their_facts() {
    for provider in ["deepseek", "deep-seek"] {
        for model in ["deepseek-flash", "deepseek-v4-pro"] {
            assert_eq!(
                facts(provider, model),
                thinks(ThinkingOff::NotReached),
                "{model}"
            );
        }
    }
    assert_eq!(
        facts("gemini", "gemini-2.5-flash"),
        thinks(ThinkingOff::BudgetZero)
    );
    assert_eq!(
        facts("gemini", "gemini-2.5-pro"),
        thinks(ThinkingOff::Never)
    );
    // A model of Anthropic's per-model thinking table carries the one refusal it gives it; a
    // Claude id outside the table carries the same facts and no refusal.
    let claude = ReasoningFacts::new(Some(false), Some(true), Some(ThinkingOff::ByDefault));
    assert_eq!(
        facts("anthropic", "claude-sonnet-4-5-20250929"),
        claude.with_refusals(&[RefusedControl::ThinkingBetweenTools])
    );
    assert_eq!(facts("claude", "claude-opus-4-20250514"), claude);
    assert_eq!(facts("anthropic", "claude-next-9"), claude);
}

#[test]
fn the_new_rules_carry_only_what_their_pages_state() {
    for model in [
        "gemini-4-neon",
        "gemini-4-hydrogen",
        "gemini-4-carbon",
        "gemini-4-argon",
    ] {
        let caps = model_capabilities("gemini", model);
        assert!(caps.reasoning, "{model}");
        assert_eq!(
            caps.reasoning_facts,
            ReasoningFacts::new(Some(true), None, Some(ThinkingOff::Never)),
            "{model}: the output-cap fact is not stated"
        );
        let mut inputs: Vec<&str> = caps.input_modalities.iter().map(|m| m.as_str()).collect();
        inputs.sort_unstable();
        assert_eq!(
            inputs,
            ["audio", "image", "pdf", "text", "video"],
            "{model}"
        );
    }
    for model in [
        "mistral-large-latest",
        "mistral-large-4",
        "mistral-large-4-0",
    ] {
        assert_eq!(
            facts("mistral", model),
            ReasoningFacts::new(Some(true), None, Some(ThinkingOff::Never)),
            "{model} always reasons"
        );
    }
    assert_eq!(
        facts("mistral", "mistral-large-2512"),
        ReasoningFacts::default(),
        "Large 3 keeps its rule"
    );
}

#[test]
fn unrecorded_routes_stay_unknown() {
    for (provider, model) in [
        ("gemini", "gemini-2.5-flash-lite"),
        ("vertex", "gemini-2.5-flash"),
        ("openrouter", "anthropic/claude-sonnet-4-5-20250929"),
        ("bedrock", "anthropic.claude-sonnet-4-20250514-v1:0"),
        ("deepseek", "claude-sonnet-4-5-20250929"),
        ("deepseek", "deepseek-reasoner"),
        ("openai", "o3"),
    ] {
        assert_eq!(
            facts(provider, model),
            ReasoningFacts::default(),
            "{provider}/{model}"
        );
    }
}

#[test]
fn a_rule_that_carries_facts_keeps_the_caps_of_the_rule_it_stands_before() {
    let flash = model_capabilities("gemini", "gemini-2.5-flash");
    let broader = model_capabilities("vertex", "gemini-2.5-flash");
    assert_eq!(flash, broader.with_reasoning_facts(flash.reasoning_facts));
    let model = "claude-sonnet-4-5-20250929";
    let claude = model_capabilities("anthropic", model);
    let broader = model_capabilities("deepseek", model);
    assert_eq!(claude, broader.with_reasoning_facts(claude.reasoning_facts));
}
