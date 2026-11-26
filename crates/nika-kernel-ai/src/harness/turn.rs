// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! How a delegated turn ended and what its adapter reported about it: the stop reason, and the
//! context occupancy and cost estimate beside the token counts, each with the report it came
//! from. Every fact here is the adapter's own word; an absent one is unknown, never zero, and
//! nothing here is billed spend.

/// How a delegated turn ended, as the agent said it (ACP: the `stopReason` of the
/// `session/prompt` result). Only [`Self::EndTurn`] is a completed turn by itself; the caller
/// decides what each other reason means for its task, and an unknown word is never read as a
/// completion.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum HarnessStopReason {
    /// The agent finished its turn (`end_turn`).
    EndTurn,
    /// The model stopped at its output limit (`max_tokens`): the answer is cut.
    MaxTokens,
    /// The agent stopped at its own limit of model requests for the turn
    /// (`max_turn_requests`).
    MaxTurnRequests,
    /// The model refused to continue (`refusal`).
    Refusal,
    /// The turn was cancelled (`cancelled`).
    Cancelled,
    /// A word this engine does not know. It holds the word when it is a protocol word (ASCII
    /// lowercase letters, digits and `_`, at most [`Self::MAX_WORD`] bytes) and
    /// `unrecognized` otherwise, so an adapter's text never rides it.
    Unknown(String),
}

impl HarnessStopReason {
    /// The longest unknown word kept.
    pub const MAX_WORD: usize = 40;

    /// The stop reason a wire word names; any other word is [`Self::Unknown`].
    #[must_use]
    pub fn parse(word: &str) -> Self {
        match word {
            "end_turn" => Self::EndTurn,
            "max_tokens" => Self::MaxTokens,
            "max_turn_requests" => Self::MaxTurnRequests,
            "refusal" => Self::Refusal,
            "cancelled" => Self::Cancelled,
            other => {
                let protocol = !other.is_empty()
                    && other.len() <= Self::MAX_WORD
                    && other
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
                Self::Unknown(if protocol { other } else { "unrecognized" }.to_owned())
            }
        }
    }

    /// The wire word (an unknown one as it was kept).
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::EndTurn => "end_turn",
            Self::MaxTokens => "max_tokens",
            Self::MaxTurnRequests => "max_turn_requests",
            Self::Refusal => "refusal",
            Self::Cancelled => "cancelled",
            Self::Unknown(word) => word,
        }
    }
}

/// Which adapter report a fact came from. Each is the adapter's own statement, never an engine
/// estimate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AdapterReport {
    /// The `usage` object of the `session/prompt` result (a draft ACP field): the turn's token
    /// counts.
    PromptResult,
    /// A `usage_update` notification (stable ACP): the session's context occupancy and its
    /// cumulative cost.
    UsageUpdate,
}

impl AdapterReport {
    /// The stable record spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PromptResult => "adapter_prompt_result",
            Self::UsageUpdate => "adapter_usage_update",
        }
    }
}

/// The session's context window as the adapter last reported it (`usage_update`): the tokens
/// it holds and its size. Occupancy, not spend: it counts no turn's consumption.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct HarnessContext {
    /// Tokens currently in the context.
    pub used: u64,
    /// The context window's size in tokens.
    pub size: u64,
}

impl HarnessContext {
    /// Construct (INV-019).
    #[must_use]
    pub const fn new(used: u64, size: u64) -> Self {
        Self { used, size }
    }

    /// The record's form, naming its report.
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({"used": self.used, "size": self.size,
            "source": AdapterReport::UsageUpdate.as_str()})
    }
}

/// The adapter's own estimate of the session's cumulative cost, as its last `usage_update` gave
/// it (`cost: {amount, currency}`). Never billed spend: the engine prices nothing from it and
/// no cost ceiling reads it. An update without a cost clears it, so an old value is never shown
/// as current. Only what [`Self::new`] keeps can be held, and it is read, never changed.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct HarnessCostEstimate {
    amount: f64,
    currency: String,
}

impl HarnessCostEstimate {
    /// The estimate of `amount` in `currency` (INV-019), kept when `amount` is a finite number
    /// of zero or more (a negative zero reads as zero) and `currency` has the shape of a
    /// currency code, three ASCII capital letters (`USD`), since it rides the record; `None`
    /// otherwise. No upper bound is set: the amount stays the adapter's own claim.
    #[must_use]
    pub fn new(amount: f64, currency: impl Into<String>) -> Option<Self> {
        let currency = currency.into();
        let code = currency.len() == 3 && currency.bytes().all(|b| b.is_ascii_uppercase());
        if amount.is_finite() && amount >= 0.0 && code {
            Some(Self {
                amount: amount.abs(),
                currency,
            })
        } else {
            None
        }
    }

    /// The amount, in [`Self::currency`]: finite, zero or more.
    #[must_use]
    pub const fn amount(&self) -> f64 {
        self.amount
    }

    /// The currency of [`Self::amount`] as the adapter named it: three capital letters (`USD`).
    #[must_use]
    pub fn currency(&self) -> &str {
        &self.currency
    }

    /// The record's form: the estimate, its unit, its report, its scope, and that it was not
    /// billed.
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({"amount": self.amount, "currency": self.currency,
            "source": AdapterReport::UsageUpdate.as_str(), "scope": "session", "billed": false})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every spec word reads as itself; an unknown word is kept only as a protocol word, and
    /// is never a completion.
    #[test]
    fn a_stop_word_reads_as_itself_and_an_unknown_one_stays_unknown() {
        for word in [
            "end_turn",
            "max_tokens",
            "max_turn_requests",
            "refusal",
            "cancelled",
        ] {
            assert_eq!(HarnessStopReason::parse(word).as_str(), word);
        }
        assert_eq!(
            HarnessStopReason::parse("end_turn"),
            HarnessStopReason::EndTurn
        );
        let future = HarnessStopReason::parse("paused_2");
        assert_eq!(future, HarnessStopReason::Unknown("paused_2".to_owned()));
        assert_ne!(future, HarnessStopReason::EndTurn);
        for hostile in ["End_Turn", "", "stop\u{1b}[2J", "a".repeat(41).as_str()] {
            assert_eq!(
                HarnessStopReason::parse(hostile),
                HarnessStopReason::Unknown("unrecognized".to_owned()),
                "{hostile:?}"
            );
        }
    }

    /// The record names each report by its own word, and the estimate says it was not billed.
    #[test]
    fn each_report_names_its_source_and_the_estimate_is_never_billed() {
        assert_eq!(
            AdapterReport::PromptResult.as_str(),
            "adapter_prompt_result"
        );
        assert_eq!(AdapterReport::UsageUpdate.as_str(), "adapter_usage_update");
        assert_eq!(
            HarnessContext::new(53_000, 200_000).to_json(),
            serde_json::json!({"used": 53_000, "size": 200_000, "source": "adapter_usage_update"})
        );
        assert_eq!(
            HarnessCostEstimate::new(0.045, "USD").map(|cost| cost.to_json()),
            Some(serde_json::json!({"amount": 0.045, "currency": "USD",
                "source": "adapter_usage_update", "scope": "session", "billed": false}))
        );
    }

    /// An estimate holds only a finite amount of zero or more in a currency of three capital
    /// ASCII letters: anything else is refused at construction, and a negative zero reads as
    /// zero, so no caller can record a value the wire would have dropped.
    #[test]
    fn a_cost_estimate_holds_only_a_finite_amount_of_zero_or_more_in_a_currency_code() {
        for amount in [
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            -1.0,
            -f64::MIN_POSITIVE,
        ] {
            assert_eq!(HarnessCostEstimate::new(amount, "USD"), None, "{amount}");
        }
        for currency in ["usd", "Usd", "US D", "USDT", "US", "", "827", "\u{c9}U"] {
            assert_eq!(
                HarnessCostEstimate::new(0.1, currency),
                None,
                "{currency:?}"
            );
        }
        for amount in [0.0, 1e-12, 0.0412, f64::MAX] {
            let kept = HarnessCostEstimate::new(amount, "EUR").expect("an estimate");
            assert_eq!(kept.amount().to_bits(), amount.to_bits(), "{amount}");
            assert_eq!(kept.currency(), "EUR");
        }
        let zero = HarnessCostEstimate::new(-0.0, "USD").expect("a negative zero is zero");
        assert!(
            zero.amount().is_sign_positive(),
            "the record never reads -0"
        );
    }
}
