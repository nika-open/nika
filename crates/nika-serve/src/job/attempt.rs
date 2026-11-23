// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! The attempts of one job (ADR-156).
//!
//! A job keeps one identifier and one idempotency key for its whole life.
//! When the resident loses a run it may start the job's captured world
//! again: a new attempt, with its own execution, trace, journal and
//! receipt. An attempt the job left is never rewritten; it stays on the
//! record as a [`ClosedAttempt`], and the job's own identity, status and
//! result are those of its latest attempt.
//!
//! Nothing here reads a clock, a file or a journal. The facts come from
//! the caller; this module holds the words, the stored shapes and the one
//! decision ([`decide`]).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};

use super::model::EventHash;
use super::{JobReceipt, JobRecord, JobStatus};

/// No statement and no flag raises a job past this many attempts.
pub(crate) const MAX_RETRY_ATTEMPTS: u32 = 16;

/// The resident's bound when `--retry-max-attempts` is absent, counting
/// the first attempt.
pub(crate) const DEFAULT_RETRY_ATTEMPTS: u32 = 3;

/// How an attempt after the first runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AttemptMode {
    /// The captured world again, from its first task.
    Restart,
    /// The captured world again, serving the tasks the interrupted
    /// attempt's journal completed (ADR-099).
    Resume,
}

impl AttemptMode {
    /// The word a flag, a statement and a route use.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Restart => "restart",
            Self::Resume => "resume",
        }
    }

    /// Read the word of a retry request.
    pub(crate) fn parse(word: &str) -> Option<Self> {
        [Self::Restart, Self::Resume]
            .into_iter()
            .find(|mode| mode.as_str() == word)
    }
}

/// What becomes of a job whose run lost its owner.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OnInterrupted {
    /// It stays `interrupted`. The default, and the behaviour before
    /// attempts existed.
    #[default]
    Off,
    /// A new attempt runs the captured world from its first task.
    Restart,
    /// A new attempt serves what the interrupted attempt's journal
    /// completed and runs the rest.
    Resume,
}

impl OnInterrupted {
    /// Read the word of `--retry-interrupted` or of a job's statement.
    pub(crate) fn parse(word: &str) -> Option<Self> {
        [Self::Off, Self::Restart, Self::Resume]
            .into_iter()
            .find(|policy| policy.as_str() == word)
    }

    /// The word this policy is written as.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Restart => "restart",
            Self::Resume => "resume",
        }
    }

    /// How the new attempt runs; `None` when there is none.
    pub(crate) const fn mode(self) -> Option<AttemptMode> {
        match self {
            Self::Off => None,
            Self::Restart => Some(AttemptMode::Restart),
            Self::Resume => Some(AttemptMode::Resume),
        }
    }
}

/// The resident's policy, or the one in force for a job once its own
/// statement is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RetryPolicy {
    on_interrupted: OnInterrupted,
    max_attempts: u32,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            on_interrupted: OnInterrupted::Off,
            max_attempts: DEFAULT_RETRY_ATTEMPTS,
        }
    }
}

impl RetryPolicy {
    pub(crate) const fn new(on_interrupted: OnInterrupted, max_attempts: u32) -> Self {
        Self {
            on_interrupted,
            max_attempts,
        }
    }

    pub(crate) const fn on_interrupted(self) -> OnInterrupted {
        self.on_interrupted
    }

    pub(crate) const fn max_attempts(self) -> u32 {
        self.max_attempts
    }

    /// The policy in force for a job. Its statement chooses the mode; its
    /// bound may lower the resident's and never raises it.
    pub(crate) fn for_job(self, statement: Option<&RetryStatement>) -> Self {
        let bound = statement.and_then(|statement| statement.max_attempts);
        Self {
            on_interrupted: statement
                .and_then(|statement| statement.on_interrupted)
                .unwrap_or(self.on_interrupted),
            max_attempts: bound.map_or(self.max_attempts, |bound| bound.min(self.max_attempts)),
        }
    }
}

/// What a job stated about its own re-attempt when it was submitted. The
/// statement is also the consent a workflow with external effects needs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RetryStatement {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    on_interrupted: Option<OnInterrupted>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_attempts: Option<u32>,
    /// The ceiling of the re-attempts alone (USD).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_cost_usd: Option<Number>,
}

impl RetryStatement {
    /// Read the `retry` member a caller wrote. `None` when it is not an
    /// object of the three known members inside their bounds: nothing is
    /// repaired or clamped here.
    pub(crate) fn stated(value: &Value) -> Option<Self> {
        let object = value.as_object()?;
        let known = ["on_interrupted", "max_attempts", "max_cost_usd"];
        if object.keys().any(|key| !known.contains(&key.as_str())) {
            return None;
        }
        let on_interrupted = match object.get("on_interrupted") {
            Some(word) => Some(OnInterrupted::parse(word.as_str()?)?),
            None => None,
        };
        let max_attempts = match object.get("max_attempts") {
            Some(bound) => Some(
                u32::try_from(bound.as_u64()?)
                    .ok()
                    .filter(|bound| (1..=MAX_RETRY_ATTEMPTS).contains(bound))?,
            ),
            None => None,
        };
        let max_cost_usd = match object.get("max_cost_usd") {
            Some(Value::Number(usd)) if usd.as_f64().is_some_and(|usd| usd >= 0.0) => {
                Some(usd.clone())
            }
            Some(_) => return None,
            None => None,
        };
        Some(Self {
            on_interrupted,
            max_attempts,
            max_cost_usd,
        })
    }

    /// The ceiling of the re-attempts alone, when one was stated (USD).
    pub(crate) fn budget(&self) -> Option<f64> {
        self.max_cost_usd.as_ref().and_then(Number::as_f64)
    }

    /// Add what a job stated, and the effects class its admission
    /// computed, to a hash preimage. A job with neither adds nothing, so
    /// its hashes are what they were before attempts existed.
    pub(crate) fn bind(record: &JobRecord, preimage: &mut Value) {
        if let Some(statement) = &record.retry {
            preimage["retry"] = serde_json::json!({
                "max_attempts": statement.max_attempts,
                "max_cost_usd": statement.max_cost_usd,
                "on_interrupted": statement.on_interrupted,
            });
        }
        if let Some(effects) = &record.effects {
            preimage["effects"] = Value::String(effects.clone());
        }
    }
}

/// What a retry request stated when it closed an attempt (ADR-156 §9).
/// Kept on the attempt it closed: the same request sent again finds the
/// attempt it created, and its budget holds for the attempts after it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RetryRequest {
    /// The ceiling of the re-attempts alone (USD).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_cost_usd: Option<Number>,
}

impl RetryRequest {
    /// The ceiling of the re-attempts alone, when the request stated one.
    pub(crate) fn budget(&self) -> Option<f64> {
        self.max_cost_usd.as_ref().and_then(Number::as_f64)
    }
}

/// A retry request, validated (ADR-156 §9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RetryAsk {
    /// The attempt the request retries; the first is 1.
    pub(crate) attempt: u32,
    /// How the new attempt runs.
    pub(crate) mode: AttemptMode,
    /// What the request states beside the attempt and the mode.
    pub(crate) request: RetryRequest,
}

impl RetryAsk {
    /// Read a request body: `attempt` (1 or more) and `mode` (`restart` or
    /// `resume`), and optionally `max_cost_usd` (a non-negative amount).
    /// `None` for anything else: nothing is repaired or clamped here.
    pub(crate) fn read(value: &Value) -> Option<Self> {
        let object = value.as_object()?;
        let known = ["attempt", "mode", "max_cost_usd"];
        if object.keys().any(|key| !known.contains(&key.as_str())) {
            return None;
        }
        let attempt = u32::try_from(object.get("attempt")?.as_u64()?)
            .ok()
            .filter(|attempt| *attempt >= 1)?;
        let mode = AttemptMode::parse(object.get("mode")?.as_str()?)?;
        let max_cost_usd = match object.get("max_cost_usd") {
            Some(Value::Number(usd)) if usd.as_f64().is_some_and(|usd| usd >= 0.0) => {
                Some(usd.clone())
            }
            Some(_) => return None,
            None => None,
        };
        Some(Self {
            attempt,
            mode,
            request: RetryRequest { max_cost_usd },
        })
    }
}

/// What one attempt committed against the job's ceiling.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Spend {
    usd: Number,
    /// Some of the spend is real and not bounded. Never read as zero.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    unknown: bool,
}

impl Spend {
    /// A spend that is not a finite, non-negative amount is unknown.
    pub(crate) fn new(usd: f64, unknown: bool) -> Self {
        match Number::from_f64(usd).filter(|_| usd >= 0.0) {
            Some(usd) => Self { usd, unknown },
            None => Self {
                usd: Number::from(0_u8),
                unknown: true,
            },
        }
    }

    pub(crate) fn usd(&self) -> f64 {
        self.usd.as_f64().unwrap_or(0.0)
    }

    pub(crate) const fn unknown(&self) -> bool {
        self.unknown
    }
}

/// One attempt the job has left. Every member is bound into the hash of
/// the event that queued the attempt after it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ClosedAttempt {
    /// `interrupted` or `failed`: how the attempt ended.
    pub(crate) status: JobStatus,
    pub(crate) execution_id: String,
    pub(crate) trace_id: String,
    pub(crate) snapshot_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) outputs: Option<BTreeMap<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) receipt: Option<JobReceipt>,
    /// The binding digest of the attempt's execution identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) identity_digest: Option<EventHash>,
    /// The `execution.requeued` event that queued the next attempt.
    pub(crate) requeued_sequence: u64,
    /// The attempt's own terminal event, when it settled before a request
    /// retried it. That event's hash binds the attempt's identity and
    /// result, which this closed attempt keeps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) terminal_sequence: Option<u64>,
    pub(crate) spend: Spend,
    /// How the attempt after this one runs.
    pub(crate) next: AttemptMode,
    /// The retry request that closed the attempt, when a request did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) requested: Option<RetryRequest>,
}

/// Why a lost run is not started again. The word is the `code` of the
/// job's `execution.interrupted` event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RetryRefusal {
    /// The job already had as many attempts as its policy allows.
    Exhausted,
    /// The job stated no `retry`, and its workflow has external effects or
    /// was never classed as having none.
    EffectsNeedConsent,
    /// An earlier attempt's spend cannot be bounded.
    UnknownSpend,
    /// Nothing is left under the job's ceiling.
    BudgetExhausted,
    /// The interrupted attempt's journal cannot be resumed the way `nika
    /// run --resume` would judge it.
    ResumeRefused,
}

impl RetryRefusal {
    pub(crate) const fn code(self) -> &'static str {
        match self {
            Self::Exhausted => "retry_exhausted",
            Self::EffectsNeedConsent => "effects_need_consent",
            Self::UnknownSpend => "unknown_spend",
            Self::BudgetExhausted => "budget_exhausted",
            Self::ResumeRefused => "resume_refused",
        }
    }

    pub(crate) const fn message(self) -> &'static str {
        match self {
            Self::Exhausted => "the job reached its bound of attempts; it is not started again",
            Self::EffectsNeedConsent => {
                "the job stated no retry and its workflow is not known to be free of external effects; a retry request is its consent"
            }
            Self::UnknownSpend => {
                "an earlier attempt's spend cannot be bounded; a retry request that states max_cost_usd accepts it"
            }
            Self::BudgetExhausted => "nothing is left under the job's cost ceiling",
            Self::ResumeRefused => {
                "the interrupted attempt's journal cannot be resumed: its chain, its project, its workflow or its engine is not the one this resident runs; the resident never starts it again from the first task on its own"
            }
        }
    }
}

/// What the attempt after `spent` may spend.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Remaining {
    /// No ceiling bounds the job.
    Unbounded,
    /// This much is left (USD, above zero).
    Usd(f64),
    /// A ceiling bounds the job and an attempt's spend is unknown.
    UnknownSpend,
    /// Nothing is left.
    Exhausted,
}

/// What is left for a new attempt (ADR-156 §6). `ceiling` covers every
/// attempt; `budget` is the stated ceiling of the re-attempts alone, and
/// whoever stated it accepted the unknown spend. `spent` lists every
/// attempt so far, oldest first; with none so far the new attempt is the
/// first, which the budget of the re-attempts does not bound.
pub(crate) fn remaining<'s>(
    ceiling: Option<f64>,
    budget: Option<f64>,
    spent: impl Iterator<Item = &'s Spend> + Clone,
) -> Remaining {
    let budget = budget.filter(|_| spent.clone().next().is_some());
    if ceiling.is_some() && budget.is_none() && spent.clone().any(Spend::unknown) {
        return Remaining::UnknownSpend;
    }
    let all: f64 = spent.clone().map(Spend::usd).sum();
    let retried: f64 = spent.skip(1).map(Spend::usd).sum();
    let under_ceiling = ceiling.map(|ceiling| ceiling - all);
    let under_budget = budget.map(|budget| budget - retried);
    let left = match (under_ceiling, under_budget) {
        (Some(ceiling), Some(budget)) => Some(ceiling.min(budget)),
        (ceiling, budget) => ceiling.or(budget),
    };
    match left {
        None => Remaining::Unbounded,
        Some(left) if left > 0.0 => Remaining::Usd(left),
        Some(_) => Remaining::Exhausted,
    }
}

/// The facts one decision reads.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LostRun<'a> {
    /// The policy in force for the job.
    pub(crate) policy: RetryPolicy,
    /// The job stated `retry` when it was submitted.
    pub(crate) consent: bool,
    /// Running the workflow twice repeats no external effect.
    pub(crate) contained: bool,
    /// A cancellation was accepted while the job ran.
    pub(crate) cancel_requested: bool,
    /// The ceiling over every attempt (USD).
    pub(crate) ceiling: Option<f64>,
    /// The stated ceiling of the re-attempts alone (USD).
    pub(crate) budget: Option<f64>,
    /// Every attempt so far, the lost one last.
    pub(crate) spent: &'a [Spend],
}

/// Decide whether a run that lost its owner is started again (ADR-156
/// §§2, 4 to 6 and 8). `Err(None)` leaves the job `interrupted` with no
/// reason to state: the policy is off, or its caller cancelled it.
pub(crate) fn decide(lost: &LostRun<'_>) -> Result<AttemptMode, Option<RetryRefusal>> {
    if lost.cancel_requested {
        return Err(None);
    }
    let mode = lost.policy.on_interrupted().mode().ok_or(None)?;
    let attempts = u32::try_from(lost.spent.len()).unwrap_or(u32::MAX);
    if attempts >= lost.policy.max_attempts() {
        return Err(Some(RetryRefusal::Exhausted));
    }
    if !lost.contained && !lost.consent {
        return Err(Some(RetryRefusal::EffectsNeedConsent));
    }
    match remaining(lost.ceiling, lost.budget, lost.spent.iter()) {
        Remaining::UnknownSpend => Err(Some(RetryRefusal::UnknownSpend)),
        Remaining::Exhausted => Err(Some(RetryRefusal::BudgetExhausted)),
        Remaining::Unbounded | Remaining::Usd(_) => Ok(mode),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spend(usd: f64) -> Spend {
        Spend::new(usd, false)
    }

    fn lost(policy: OnInterrupted, spent: &[Spend]) -> LostRun<'_> {
        LostRun {
            policy: RetryPolicy::new(policy, 3),
            consent: false,
            contained: true,
            cancel_requested: false,
            ceiling: Some(1.0),
            budget: None,
            spent,
        }
    }

    #[test]
    fn a_statement_is_read_exactly_or_refused() {
        let stated = RetryStatement::stated(&serde_json::json!({
            "on_interrupted": "restart", "max_attempts": 2, "max_cost_usd": 0.5
        }))
        .expect("a legal statement");
        assert_eq!(stated.on_interrupted, Some(OnInterrupted::Restart));
        assert_eq!(stated.max_attempts, Some(2));
        assert_eq!(stated.budget(), Some(0.5));
        assert_eq!(
            RetryStatement::stated(&serde_json::json!({})),
            Some(RetryStatement::default()),
            "an empty statement is consent to the resident's policy"
        );
        for refused in [
            serde_json::json!(null),
            serde_json::json!("restart"),
            serde_json::json!({"on_interrupted": "always"}),
            serde_json::json!({"on_interrupted": null}),
            serde_json::json!({"max_attempts": 0}),
            serde_json::json!({"max_attempts": 17}),
            serde_json::json!({"max_attempts": 1.5}),
            serde_json::json!({"max_attempts": "2"}),
            serde_json::json!({"max_cost_usd": -0.01}),
            serde_json::json!({"max_cost_usd": "1"}),
            serde_json::json!({"mode": "restart"}),
        ] {
            assert_eq!(RetryStatement::stated(&refused), None, "{refused}");
        }
    }

    #[test]
    fn a_job_lowers_the_resident_bound_and_never_raises_it() {
        let resident = RetryPolicy::new(OnInterrupted::Off, 3);
        let stated = |value: Value| RetryStatement::stated(&value).expect("legal");
        let lowered = resident.for_job(Some(&stated(
            serde_json::json!({"on_interrupted": "restart", "max_attempts": 2}),
        )));
        assert_eq!(lowered, RetryPolicy::new(OnInterrupted::Restart, 2));
        let capped = resident.for_job(Some(&stated(serde_json::json!({"max_attempts": 9}))));
        assert_eq!(capped, RetryPolicy::new(OnInterrupted::Off, 3));
        assert_eq!(resident.for_job(None), resident);
        let off = RetryPolicy::new(OnInterrupted::Restart, 3)
            .for_job(Some(&stated(serde_json::json!({"on_interrupted": "off"}))));
        assert_eq!(off.on_interrupted(), OnInterrupted::Off);
    }

    #[test]
    fn a_spend_is_stored_with_its_unknown_part() {
        let known = Spend::new(0.25, false);
        assert_eq!(
            serde_json::to_value(&known).expect("encodes"),
            serde_json::json!({"usd": 0.25})
        );
        assert_eq!(
            serde_json::to_value(Spend::new(0.0, true)).expect("encodes"),
            serde_json::json!({"usd": 0.0, "unknown": true})
        );
        let not_an_amount = Spend::new(f64::NAN, false);
        assert!(
            not_an_amount.unknown(),
            "a spend that is not an amount is unknown"
        );
        assert!(Spend::new(-1.0, false).unknown());
    }

    #[test]
    fn what_is_left_counts_every_attempt_and_never_reads_unknown_as_zero() {
        let two = [spend(0.25), spend(0.5)];
        assert_eq!(remaining(Some(1.0), None, two.iter()), Remaining::Usd(0.25));
        assert_eq!(remaining(None, None, two.iter()), Remaining::Unbounded);
        assert_eq!(
            remaining(Some(0.75), None, two.iter()),
            Remaining::Exhausted
        );
        let unknown = [spend(0.25), Spend::new(0.0, true)];
        assert_eq!(
            remaining(Some(1.0), None, unknown.iter()),
            Remaining::UnknownSpend
        );
        assert_eq!(
            remaining(None, None, unknown.iter()),
            Remaining::Unbounded,
            "no ceiling bounds the job: there is nothing the unknown could cross"
        );
        // A stated budget bounds the re-attempts alone: the first attempt
        // does not count against it, and it accepts the unknown.
        assert_eq!(
            remaining(Some(1.0), Some(0.5), unknown.iter()),
            Remaining::Usd(0.5)
        );
        let retried = [spend(0.25), spend(0.25), spend(0.25)];
        assert_eq!(
            remaining(Some(10.0), Some(0.5), retried.iter()),
            Remaining::Exhausted
        );
        // The first attempt runs under the job's ceiling alone.
        assert_eq!(
            remaining(Some(1.0), Some(0.25), [].iter()),
            Remaining::Usd(1.0)
        );
        // Both bounds hold; the tighter one is what is left.
        assert_eq!(
            remaining(Some(0.875), Some(1.0), [spend(0.25), spend(0.5)].iter()),
            Remaining::Usd(0.125)
        );
    }

    #[test]
    fn a_lost_run_is_started_again_only_when_every_rule_allows_it() {
        let one = [spend(0.25)];
        assert_eq!(
            decide(&lost(OnInterrupted::Restart, &one)),
            Ok(AttemptMode::Restart)
        );
        assert_eq!(decide(&lost(OnInterrupted::Off, &one)), Err(None));
        assert_eq!(
            decide(&LostRun {
                cancel_requested: true,
                ..lost(OnInterrupted::Restart, &one)
            }),
            Err(None),
            "a job its caller cancelled is never started again"
        );
        let three = [spend(0.0), spend(0.0), spend(0.0)];
        assert_eq!(
            decide(&lost(OnInterrupted::Restart, &three)),
            Err(Some(RetryRefusal::Exhausted))
        );
        let with_effects = LostRun {
            contained: false,
            ..lost(OnInterrupted::Restart, &one)
        };
        assert_eq!(
            decide(&with_effects),
            Err(Some(RetryRefusal::EffectsNeedConsent))
        );
        assert_eq!(
            decide(&LostRun {
                consent: true,
                ..with_effects
            }),
            Ok(AttemptMode::Restart)
        );
        let unknown = [Spend::new(0.0, true)];
        assert_eq!(
            decide(&lost(OnInterrupted::Restart, &unknown)),
            Err(Some(RetryRefusal::UnknownSpend))
        );
        assert_eq!(
            decide(&LostRun {
                budget: Some(0.5),
                ..lost(OnInterrupted::Restart, &unknown)
            }),
            Ok(AttemptMode::Restart)
        );
        let spent = [spend(1.0)];
        assert_eq!(
            decide(&lost(OnInterrupted::Restart, &spent)),
            Err(Some(RetryRefusal::BudgetExhausted))
        );
        // The bound is judged before the consent: an exhausted job says so.
        let exhausted_with_effects = LostRun {
            contained: false,
            ..lost(OnInterrupted::Restart, &three)
        };
        assert_eq!(
            decide(&exhausted_with_effects),
            Err(Some(RetryRefusal::Exhausted))
        );
    }

    #[test]
    fn every_refusal_has_its_own_word() {
        let words = [
            RetryRefusal::Exhausted,
            RetryRefusal::EffectsNeedConsent,
            RetryRefusal::UnknownSpend,
            RetryRefusal::BudgetExhausted,
            RetryRefusal::ResumeRefused,
        ]
        .map(RetryRefusal::code);
        assert_eq!(
            words,
            [
                "retry_exhausted",
                "effects_need_consent",
                "unknown_spend",
                "budget_exhausted",
                "resume_refused"
            ]
        );
    }

    #[test]
    fn resume_is_a_mode_of_its_own() {
        assert_eq!(OnInterrupted::parse("resume"), Some(OnInterrupted::Resume));
        assert_eq!(OnInterrupted::Resume.mode(), Some(AttemptMode::Resume));
        assert_eq!(OnInterrupted::Resume.as_str(), "resume");
        assert_eq!(AttemptMode::Resume.as_str(), "resume");
        assert_eq!(
            serde_json::to_value(AttemptMode::Resume).ok(),
            Some(serde_json::json!("resume")),
            "the word the store keeps"
        );
        let stated = RetryStatement::stated(&serde_json::json!({"on_interrupted": "resume"}));
        assert_eq!(
            stated.and_then(|stated| stated.on_interrupted),
            Some(OnInterrupted::Resume)
        );
        for word in ["Resume", "resume ", "resumed"] {
            assert_eq!(OnInterrupted::parse(word), None, "{word:?}");
        }
    }

    #[test]
    fn a_retry_request_is_read_exactly_or_refused() {
        let ask = RetryAsk::read(&serde_json::json!({
            "attempt": 2, "mode": "resume", "max_cost_usd": 0.25
        }))
        .expect("a legal request");
        assert_eq!(ask.attempt, 2);
        assert_eq!(ask.mode, AttemptMode::Resume);
        assert_eq!(ask.request.budget(), Some(0.25));
        let bare = RetryAsk::read(&serde_json::json!({"attempt": 1, "mode": "restart"}))
            .expect("a request with no budget");
        assert_eq!(bare.mode, AttemptMode::Restart);
        assert_eq!(bare.request, RetryRequest::default());
        assert_eq!(bare.request.budget(), None);
        let zero = RetryAsk::read(&serde_json::json!({
            "attempt": 1, "mode": "restart", "max_cost_usd": 0
        }));
        assert_eq!(zero.and_then(|ask| ask.request.budget()), Some(0.0));
        for refused in [
            serde_json::json!({}),
            serde_json::json!([]),
            serde_json::json!({"mode": "restart"}),
            serde_json::json!({"attempt": 1}),
            serde_json::json!({"attempt": 0, "mode": "restart"}),
            serde_json::json!({"attempt": -1, "mode": "restart"}),
            serde_json::json!({"attempt": 1.5, "mode": "restart"}),
            serde_json::json!({"attempt": "1", "mode": "restart"}),
            serde_json::json!({"attempt": 4_294_967_296_u64, "mode": "restart"}),
            serde_json::json!({"attempt": 1, "mode": "Restart"}),
            serde_json::json!({"attempt": 1, "mode": "off"}),
            serde_json::json!({"attempt": 1, "mode": null}),
            serde_json::json!({"attempt": 1, "mode": "restart", "max_cost_usd": -0.5}),
            serde_json::json!({"attempt": 1, "mode": "restart", "max_cost_usd": null}),
            serde_json::json!({"attempt": 1, "mode": "restart", "max_cost_usd": "1"}),
            serde_json::json!({"attempt": 1, "mode": "restart", "max_attempts": 2}),
        ] {
            assert_eq!(RetryAsk::read(&refused), None, "{refused}");
        }
        assert_eq!(AttemptMode::parse("restart"), Some(AttemptMode::Restart));
        assert_eq!(AttemptMode::parse("resume"), Some(AttemptMode::Resume));
        assert_eq!(AttemptMode::parse("off"), None, "off starts nothing");
    }
}
