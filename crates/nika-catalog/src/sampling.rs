// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! The request controls a provider's API refuses on a model, as the catalog records them: a
//! sampling parameter other than its default, and a thinking budget. Anthropic's Messages API
//! answers both with HTTP 400 on a model whose capability rule takes no temperature or lists no
//! thinking-budget parameter while recording its reasoning (the Claude 5 models fix their
//! sampling and think adaptively). The `openai` provider's own API answers a temperature other
//! than the default the same way on a model whose rule takes none (the o-series, GPT-5, GPT-5
//! mini and GPT-5 nano). Another OpenAI-compatible route, `openrouter` included, may drop such
//! a control instead, so no refusal is claimed there.
//!
//! Two more Messages API refusals are recorded per model, from Anthropic's tool-use and
//! thinking pages (read 2026-11-24): a forced tool choice, and a `thinking` type that turns the
//! model's reasoning down. Each rides its capability rule (`reasoning_facts.refuses`), so a model
//! added to a rule takes the rule's refusals in plain sight and a model no rule lists is never
//! refused: the provider answers.

use crate::data::generated::find_provider;
use crate::data::models::model_capabilities;
use crate::types::ParamFlag;
use crate::types::model::{ReasoningFacts, RefusedControl};

/// The wire model a catalog nickname names (`anthropic`/`sonnet` names `claude-sonnet-5-5`);
/// `None` for any other name, which is its own wire id.
#[must_use]
pub fn nickname_wire(provider: &str, name: &str) -> Option<&'static str> {
    find_provider(provider)
        .and_then(|row| row.models.iter().find(|m| m.id == name))
        .map(|m| m.model)
}

/// The wire model `provider`/`name` names: a nickname's model, any other name as given.
fn wire_model<'a>(provider: &str, name: &'a str) -> &'a str {
    nickname_wire(provider, name).unwrap_or(name)
}

/// Whether `provider` speaks Anthropic's Messages API, where a refused control is an error.
fn anthropic_route(provider: &str) -> bool {
    find_provider(provider).and_then(|row| row.api_dialect) == Some("anthropic")
}

/// Whether `provider` is the `openai` provider (its id or an alias), whose own API answers a
/// refused sampling control with an error rather than dropping it.
fn openai_route(provider: &str) -> bool {
    find_provider(provider).is_some_and(|row| row.id == "openai")
}

/// Whether the provider's API refuses a sampling parameter (`temperature`, `top_p`, `top_k`)
/// other than its default on `provider`/`name` with an error: an Anthropic route or the
/// `openai` provider whose model takes no temperature. `false` wherever the catalog records no
/// such refusal.
#[must_use]
pub fn refuses_sampling(provider: &str, name: &str) -> bool {
    (anthropic_route(provider) || openai_route(provider))
        && !model_capabilities(provider, wire_model(provider, name)).supports_temperature
}

/// Whether the provider's API refuses a thinking budget (`thinking: { budget_tokens }`) on
/// `provider`/`name` with an error: an Anthropic route whose model's rule records its reasoning
/// and lists no thinking-budget parameter (it thinks adaptively, steered by effort). `false`
/// wherever the catalog records no such refusal, an unrecorded model included.
#[must_use]
pub fn refuses_thinking_budget(provider: &str, name: &str) -> bool {
    if !anthropic_route(provider) {
        return false;
    }
    let caps = model_capabilities(provider, wire_model(provider, name));
    caps.reasoning
        && caps.reasoning_facts != ReasoningFacts::default()
        && !caps
            .supported_parameters
            .contains(&ParamFlag::ThinkingBudget)
}

/// Whether the provider's API refuses a forced tool choice (`tool_choice` `any` or `tool`) on
/// `provider`/`name` with an error on every request: an Anthropic route whose model's rule lists
/// `forced-tool-choice` in `reasoning_facts.refuses`. Such a request must let the model choose
/// (`auto`), with structured output where it needs a fixed shape. `false` wherever no refusal is
/// recorded.
#[must_use]
pub fn refuses_forced_tool_choice(provider: &str, name: &str) -> bool {
    refuses(provider, name, RefusedControl::ForcedToolChoice)
}

/// Whether the provider's API refuses `thinking: { type: <kind> }` on `provider`/`name` with an
/// error on every request, for the two types that turn a model's reasoning down: `disabled` and
/// `between_tools`, each where the model's rule lists it in `reasoning_facts.refuses`. `false`
/// for any other type and wherever the catalog records no such refusal, a model no rule lists
/// included. A type the model takes may still be refused at some effort levels (Sonnet 5.5's
/// `between_tools` above `high`, Haiku 5.5's and Opus 5's `disabled` at `xhigh` or `max`); that
/// is not claimed here.
#[must_use]
pub fn refuses_thinking_type(provider: &str, name: &str, kind: &str) -> bool {
    let control = match kind {
        "disabled" => RefusedControl::ThinkingDisabled,
        "between_tools" => RefusedControl::ThinkingBetweenTools,
        _ => return false,
    };
    refuses(provider, name, control)
}

/// Whether an Anthropic route's model rule lists `control` among the controls it refuses.
fn refuses(provider: &str, name: &str, control: RefusedControl) -> bool {
    anthropic_route(provider)
        && model_capabilities(provider, wire_model(provider, name))
            .reasoning_facts
            .refuses(control)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::model::ThinkingOff;

    /// Anthropic's thinking-tool-workflows page (2026-11-24): Opus 5.5, Sonnet 5.5, Fable 5.2,
    /// Mythos 5.2, Fable 5.1 and Mythos 5.1 refuse a forced tool choice on every request; « Other
    /// models: Forced tool use is accepted », so Haiku 5.5, Opus 5, Fable 5, the earlier models
    /// and an id the data does not know accept it, and no other route claims the refusal.
    #[test]
    fn the_current_claude_models_refuse_a_forced_tool_choice() {
        for (provider, name) in [
            ("anthropic", "claude-fable-5-2"),
            ("anthropic", "claude-mythos-5-2"),
            ("anthropic", "claude-fable-5-1"),
            ("anthropic", "claude-mythos-5-1"),
            ("anthropic", "claude-opus-5-5"),
            ("anthropic", "claude-sonnet-5-5"),
            ("anthropic", "sonnet"),
            ("anthropic", "opus"),
            ("claude", "claude-fable-5-2"),
        ] {
            assert!(
                refuses_forced_tool_choice(provider, name),
                "{provider}/{name}"
            );
        }
        for (provider, name) in [
            ("anthropic", "claude-haiku-5-5"),
            ("anthropic", "haiku"),
            ("anthropic", "claude-opus-5"),
            ("anthropic", "claude-fable-5"),
            ("anthropic", "claude-opus-4-8"),
            ("anthropic", "claude-sonnet-4-6"),
            ("anthropic", "claude-next-9"),
            ("openrouter", "anthropic/claude-fable-5.2"),
            ("openai", "claude-fable-5-2"),
        ] {
            assert!(
                !refuses_forced_tool_choice(provider, name),
                "{provider}/{name}"
            );
        }
    }

    /// Anthropic's per-model thinking table (2026-11-24): the always-thinking models refuse
    /// both lower settings, Sonnet 5.5 refuses `disabled` and takes `between_tools` (« Only
    /// Claude Sonnet 5.5 accepts `between_tools` »), so every other model of the table, the
    /// 4.x models included, refuses `between_tools`; Haiku 5.5 and Opus 5 take `disabled`. An id
    /// the data does not know and another type claim nothing.
    #[test]
    fn a_thinking_type_the_model_does_not_take_is_refused() {
        for name in [
            "claude-fable-5-2",
            "claude-mythos-5-2",
            "claude-fable-5-1",
            "claude-mythos-5-1",
            "claude-fable-5",
            "claude-opus-5-5",
        ] {
            assert!(
                refuses_thinking_type("anthropic", name, "disabled"),
                "{name}"
            );
            assert!(
                refuses_thinking_type("anthropic", name, "between_tools"),
                "{name}"
            );
        }
        assert!(refuses_thinking_type(
            "anthropic",
            "claude-sonnet-5-5",
            "disabled"
        ));
        assert!(refuses_thinking_type("anthropic", "sonnet", "disabled"));
        assert!(!refuses_thinking_type(
            "anthropic",
            "claude-sonnet-5-5",
            "between_tools"
        ));
        for name in ["claude-haiku-5-5", "claude-opus-5", "claude-sonnet-5"] {
            assert!(
                !refuses_thinking_type("anthropic", name, "disabled"),
                "{name}"
            );
            assert!(
                refuses_thinking_type("anthropic", name, "between_tools"),
                "{name}"
            );
        }
        for name in [
            "claude-opus-4-8",
            "claude-opus-4-7",
            "claude-opus-4-6",
            "claude-sonnet-4-6",
            "claude-opus-4-5-20251101",
            "claude-haiku-4-5-20251001",
            "claude-sonnet-4-5-20250929",
        ] {
            assert!(
                refuses_thinking_type("anthropic", name, "between_tools"),
                "{name}"
            );
            assert!(
                !refuses_thinking_type("anthropic", name, "disabled"),
                "{name}"
            );
        }
        for (provider, name, kind) in [
            ("anthropic", "claude-fable-5-2", "adaptive"),
            ("anthropic", "claude-sonnet-4-6", "disabled"),
            ("anthropic", "some-future-model", "disabled"),
            ("anthropic", "claude-next-9", "between_tools"),
            ("anthropic", "claude-next-9", "disabled"),
            ("openrouter", "anthropic/claude-fable-5.2", "disabled"),
            ("openrouter", "anthropic/claude-opus-4.8", "between_tools"),
        ] {
            assert!(
                !refuses_thinking_type(provider, name, kind),
                "{provider}/{name} {kind}"
            );
        }
    }

    /// Claude Opus 4.8 and 4.7 fix their sampling and take no thinking budget (Anthropic's
    /// thinking page, 2026-11-24), while their reasoning stays off unless a request asks.
    #[test]
    fn opus_4_8_and_4_7_fix_their_sampling_and_refuse_a_budget() {
        for name in ["claude-opus-4-8", "claude-opus-4-7"] {
            assert!(refuses_sampling("anthropic", name), "{name}");
            assert!(refuses_thinking_budget("anthropic", name), "{name}");
            let facts = model_capabilities("anthropic", name).reasoning_facts;
            assert_eq!(facts.reasons_by_default, Some(false), "{name}");
            assert_eq!(facts.thinking_off, Some(ThinkingOff::ByDefault), "{name}");
            assert!(
                !refuses_thinking_type("anthropic", name, "disabled"),
                "{name}"
            );
        }
        assert!(!refuses_sampling("anthropic", "claude-opus-4-6"));
    }

    /// The Claude 5 models refuse both controls on Anthropic's API, under their wire id, a
    /// nickname that names them or the provider alias; earlier Claude models accept both.
    #[test]
    fn the_claude_5_models_refuse_sampling_and_a_thinking_budget() {
        for (provider, name) in [
            ("anthropic", "claude-sonnet-5-5"),
            ("anthropic", "claude-opus-5-5"),
            ("anthropic", "claude-haiku-5-5"),
            ("anthropic", "claude-fable-5-2"),
            ("anthropic", "claude-mythos-5-2"),
            ("anthropic", "claude-fable-5-1"),
            ("anthropic", "claude-mythos-5-1"),
            ("anthropic", "claude-fable-5"),
            ("anthropic", "claude-opus-5"),
            ("anthropic", "claude-sonnet-5"),
            ("anthropic", "sonnet"),
            ("anthropic", "opus"),
            ("anthropic", "haiku"),
            ("claude", "claude-sonnet-5-5"),
        ] {
            assert!(refuses_sampling(provider, name), "{provider}/{name}");
            assert!(refuses_thinking_budget(provider, name), "{provider}/{name}");
        }
        for name in [
            "claude-sonnet-4-6",
            "claude-sonnet-4-5-20250929",
            "claude-haiku-4-5-20251001",
        ] {
            assert!(!refuses_sampling("anthropic", name), "{name}");
            assert!(!refuses_thinking_budget("anthropic", name), "{name}");
        }
    }

    /// The `openai` provider's own API answers a temperature other than the default with
    /// HTTP 400 on the reasoning seats that fix their sampling (measured on 0.123.2): the
    /// o-series and GPT-5, GPT-5 mini and GPT-5 nano. A model that takes a temperature, a later
    /// dot variant included, keeps it, and a compatible route that drops the control claims no
    /// refusal.
    #[test]
    fn openai_reasoning_seats_refuse_sampling_on_openai_only() {
        for name in [
            "gpt-5",
            "gpt-5-mini",
            "gpt-5-nano",
            "gpt-5-mini-2025-08-07",
            "o3",
        ] {
            assert!(refuses_sampling("openai", name), "openai/{name}");
            assert!(
                refuses_sampling("gpt", name),
                "the provider alias: gpt/{name}"
            );
            assert!(!refuses_thinking_budget("openai", name), "openai/{name}");
        }
        for (provider, name) in [
            ("openai", "gpt-4o"),
            ("openai", "gpt-5.1"),
            ("openai", "gpt-5-chat-latest"),
            ("openrouter", "o3"),
            ("openrouter", "gpt-5-mini"),
        ] {
            assert!(!refuses_sampling(provider, name), "{provider}/{name}");
        }
    }

    /// A model the catalog records no refusal for gets none: an unrecorded Anthropic name, a
    /// Claude model reached through another dialect, and an OpenAI-compatible route whose model
    /// takes no temperature (that route ignores it rather than refusing).
    #[test]
    fn no_refusal_is_claimed_where_the_catalog_records_none() {
        for (provider, name) in [
            ("anthropic", "some-future-model"),
            ("openrouter", "anthropic/claude-sonnet-5.5"),
            ("deepseek", "deepseek-v4-pro"),
            ("openrouter", "o3"),
            ("mock", "echo"),
        ] {
            assert!(!refuses_sampling(provider, name), "{provider}/{name}");
            assert!(
                !refuses_thinking_budget(provider, name),
                "{provider}/{name}"
            );
        }
        assert_eq!(
            nickname_wire("anthropic", "sonnet"),
            Some("claude-sonnet-5-5")
        );
        assert_eq!(nickname_wire("claude", "opus"), Some("claude-opus-5-5"));
        assert_eq!(nickname_wire("anthropic", "claude-x"), None);
        assert_eq!(nickname_wire("nowhere", "sonnet"), None);
        assert_eq!(wire_model("anthropic", "claude-x"), "claude-x");
    }
}
