// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! The reasoning facts a capability rule records, `caps.reasoning_facts`: whether the model
//! reasons when the request names no reasoning control, whether that reasoning is spent inside
//! the request's output cap, how a `.nika` file turns it off on this engine, and which request
//! controls its provider refuses because of how it reasons. A fact the rule does not record stays
//! unknown and a refusal it does not list is never claimed; a key or word outside this set fails
//! the build.

use serde::Deserialize;

/// `reasoning_facts = { by_default = …, in_output_cap = …, off = "…", refuses = ["…"] }`.
#[derive(Deserialize, Debug, Default, Clone)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReasoningFactsToml {
    #[serde(default)]
    by_default: Option<bool>,
    #[serde(default)]
    in_output_cap: Option<bool>,
    #[serde(default)]
    off: Option<ThinkingOffToml>,
    #[serde(default)]
    refuses: Vec<RefusedControlToml>,
}

/// The words of `refuses`, one per `RefusedControl` variant of the same name.
#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum RefusedControlToml {
    ForcedToolChoice,
    ThinkingDisabled,
    ThinkingBetweenTools,
}

/// The words of `off`, one per `ThinkingOff` variant of the same name.
#[derive(Deserialize, Debug, Clone, Copy)]
#[serde(rename_all = "kebab-case")]
enum ThinkingOffToml {
    ByDefault,
    BudgetZero,
    NotReached,
    Never,
}

/// The `ReasoningFacts` expression of a rule's facts; an unrecorded one is `None`.
/// A refusal listed twice fails the build.
pub(crate) fn validate_reasoning_facts(
    facts: &ReasoningFactsToml,
    where_: &str,
) -> Result<(), String> {
    for (at, control) in facts.refuses.iter().enumerate() {
        if facts.refuses[..at].contains(control) {
            return Err(format!(
                "{where_}: reasoning_facts.refuses lists {control:?} twice"
            ));
        }
    }
    Ok(())
}

/// The `ReasoningFacts` expression of a rule's facts; an unrecorded one is `None`, and the listed
/// refusals ride `with_refusals`.
pub(crate) fn emit_reasoning_facts(facts: &ReasoningFactsToml) -> String {
    let off = facts.off.map_or_else(
        || "None".to_owned(),
        |off| format!("Some(crate::types::model::ThinkingOff::{off:?})"),
    );
    let facts_expr = format!(
        "crate::types::model::ReasoningFacts::new({:?}, {:?}, {off})",
        facts.by_default, facts.in_output_cap
    );
    if facts.refuses.is_empty() {
        return facts_expr;
    }
    let controls: Vec<String> = facts
        .refuses
        .iter()
        .map(|control| format!("crate::types::model::RefusedControl::{control:?}"))
        .collect();
    format!("{facts_expr}.with_refusals(&[{}])", controls.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(text: &str) -> Result<ReasoningFactsToml, toml::de::Error> {
        toml::from_str(text)
    }

    #[test]
    fn recorded_facts_emit_typed_and_unrecorded_ones_stay_unknown() {
        let facts = parsed("by_default = true\nin_output_cap = false\noff = \"budget-zero\"")
            .expect("valid facts");
        assert_eq!(
            emit_reasoning_facts(&facts),
            "crate::types::model::ReasoningFacts::new(Some(true), Some(false), \
             Some(crate::types::model::ThinkingOff::BudgetZero))"
        );
        assert_eq!(
            emit_reasoning_facts(&ReasoningFactsToml::default()),
            "crate::types::model::ReasoningFacts::new(None, None, None)"
        );
        for (word, variant) in [
            ("by-default", "ByDefault"),
            ("not-reached", "NotReached"),
            ("never", "Never"),
        ] {
            let facts = parsed(&format!("off = \"{word}\"")).expect("a known word");
            let emitted = emit_reasoning_facts(&facts);
            assert!(
                emitted.ends_with(&format!("ThinkingOff::{variant}))")),
                "{emitted}"
            );
        }
    }

    #[test]
    fn listed_refusals_emit_in_order_and_a_repeat_fails_validation() {
        let facts = parsed("off = \"never\"\nrefuses = [\"forced-tool-choice\", \"thinking-disabled\", \"thinking-between-tools\"]")
            .expect("valid facts");
        assert_eq!(
            emit_reasoning_facts(&facts),
            "crate::types::model::ReasoningFacts::new(None, None, \
             Some(crate::types::model::ThinkingOff::Never)).with_refusals(&[\
             crate::types::model::RefusedControl::ForcedToolChoice, \
             crate::types::model::RefusedControl::ThinkingDisabled, \
             crate::types::model::RefusedControl::ThinkingBetweenTools])"
        );
        let twice =
            parsed("refuses = [\"thinking-disabled\", \"thinking-disabled\"]").expect("parses");
        let err = validate_reasoning_facts(&twice, "rule r").expect_err("a repeat fails");
        assert!(
            err.contains("rule r") && err.contains("ThinkingDisabled twice"),
            "{err}"
        );
        assert!(validate_reasoning_facts(&facts, "rule r").is_ok());
    }

    #[test]
    fn an_unknown_key_or_word_fails_the_build() {
        assert!(parsed("reasons_by_default = true").is_err());
        assert!(parsed("off = \"sometimes\"").is_err());
        assert!(parsed("by_default = \"yes\"").is_err());
        assert!(parsed("refuses = [\"temperature\"]").is_err());
    }
}
