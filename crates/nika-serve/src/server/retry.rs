// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! The resident's side of a job's attempts (ADR-156): what a request
//! states, the class of the workflow an admission captured, and the
//! decision for a run that lost its owner.
//!
//! The decision reads three things the engine already wrote: the effects
//! class kept on the job at admission, the journal of the lost attempt,
//! and the cost rows of the job's captured workflow. It guesses none of
//! them. Whatever it cannot read counts against starting the job again,
//! a missing journal included.

use std::path::PathBuf;

use hyper::StatusCode;
use nika_dap::attempt::{EffectsClass, effects_class, interrupted_spend, rode_harness};
use nika_execution::{AdmittedExecution, ExecutionService, ExecutionSnapshot, SnapshotLimits};
use serde_json::Value;

use super::error::ApiError;
use super::{AuthorityState, ExecutionTask, ServerError, ServerLimits};
use crate::job::{
    LostOwnerJudge, LostRun, LostVerdict, Remaining, RetryPolicy, RetryRefusal, RetryStatement,
    Spend, decide, remaining,
};
use crate::{JobOrigin, JobRecord};

/// What a request stated about a re-attempt, validated.
///
/// # Errors
/// `422 invalid_job_retry`: nothing is clamped or repaired.
pub(super) fn stated(retry: Option<&Value>) -> Result<Option<RetryStatement>, ApiError> {
    retry
        .map(|value| RetryStatement::stated(value).ok_or_else(invalid_retry))
        .transpose()
}

fn invalid_retry() -> ApiError {
    ApiError::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid_job_retry",
        "retry must be an object of on_interrupted (off or restart), max_attempts (1 to 16) and max_cost_usd (a non-negative amount), each optional",
    )
}

/// A review covers the one run it was shown. Its job is never started again
/// on its own, so a request that consumes a review states no retry.
pub(super) fn reviewed_states_no_retry() -> ApiError {
    ApiError::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid_job_retry",
        "a job admitted through a cost review states no retry: the review covers one run",
    )
}

/// The stored word of the effects class of the workflow an admission
/// captured, read from its own check.
pub(super) fn effects_word(admitted: &AdmittedExecution) -> &'static str {
    effects_class(admitted.workflow(), admitted.check()).as_str()
}

/// The ceiling that bounds every attempt of a job: the resident's own, a
/// lower one its requester bound, or none.
pub(super) fn job_ceiling(resident: Option<f64>, job: &JobRecord) -> Option<f64> {
    match (resident, job.max_cost_usd()) {
        (Some(resident), Some(job)) => Some(resident.min(job)),
        (resident, job) => resident.or(job),
    }
}

/// The ceiling the attempt about to run is given: what its job has left
/// once its earlier attempts are counted (ADR-156 section 6). `None` is no
/// ceiling at all. A first attempt runs under the job's ceiling as it
/// always has, a zero one included: a zero ceiling binds a run, it does
/// not refuse one that spends nothing.
///
/// # Errors
/// The reason the attempt must not run: nothing is left, or an earlier
/// attempt's spend is unknown under a ceiling nobody lifted.
pub(super) fn attempt_ceiling(
    resident: Option<f64>,
    job: &JobRecord,
) -> Result<Option<f64>, RetryRefusal> {
    if job.attempts.is_empty() {
        return Ok(job_ceiling(resident, job));
    }
    let left = remaining(
        job_ceiling(resident, job),
        job.retry.as_ref().and_then(RetryStatement::budget),
        job.attempts.iter().map(|attempt| &attempt.spend),
    );
    match left {
        Remaining::Unbounded => Ok(None),
        Remaining::Usd(left) => Ok(Some(left)),
        Remaining::UnknownSpend => Err(RetryRefusal::UnknownSpend),
        Remaining::Exhausted => Err(RetryRefusal::BudgetExhausted),
    }
}

/// A queued attempt its job's ceiling no longer covers does not run. The
/// decision that queued it held then; a resident restarted under a lower
/// ceiling is how it stops holding. A job another transition already moved
/// stays as it is.
pub(super) async fn refuse_attempt(
    state: &AuthorityState,
    task: &ExecutionTask,
    refusal: RetryRefusal,
) -> Result<(), ServerError> {
    super::refuse_queued_job(state, task, refusal.code(), refusal.message()).await
}

/// The resident's decision for a run that lost its owner.
#[derive(Debug, Clone)]
pub(super) struct LostRunJudge {
    policy: RetryPolicy,
    /// The resident's own per-run ceiling (USD).
    ceiling: Option<f64>,
    /// Where the backend leaves a job's journal, when it leaves one.
    journals: Option<PathBuf>,
    service: ExecutionService,
    snapshots: SnapshotLimits,
}

impl LostRunJudge {
    pub(super) fn new(
        limits: ServerLimits,
        journals: Option<PathBuf>,
        snapshots: SnapshotLimits,
    ) -> Self {
        Self {
            policy: limits.retry_policy(),
            ceiling: limits.default_max_cost_usd(),
            journals,
            service: ExecutionService::new(snapshots),
            snapshots,
        }
    }

    /// What the lost attempt of `job` committed, and whether running its
    /// workflow again can repeat an effect outside the engine.
    fn lost_attempt(&self, job: &JobRecord, world: Option<&str>) -> (Spend, bool) {
        let admitted = self.workflow(job, world);
        // The word kept at admission is bound to the job's chain only once
        // it started. The class is read again from the captured workflow,
        // which the job's identity binds by its digest, and the stricter of
        // the two holds: neither alone can make a job contained.
        let stored = job.effects.as_deref().and_then(EffectsClass::parse);
        let read = admitted
            .as_ref()
            .map(|admitted| effects_class(admitted.workflow(), admitted.check()));
        let class = stored
            .zip(read)
            .map(|(stored, read)| stricter(stored, read));
        let events = self.journal(job);
        let contained = match (class, &events) {
            (Some(EffectsClass::Contained), _) => true,
            // The route is in no snapshot: only the journal states it.
            (Some(EffectsClass::ContainedOffHarness), Some(events)) => !rode_harness(events),
            _ => false,
        };
        // No journal is not "nothing spent". A run killed before its first
        // event leaves none, and so does a run whose journal could not be
        // opened: the two cannot be told apart, so neither counts for zero.
        let known = events.and_then(|events| {
            let admitted = admitted.as_ref()?;
            let spend = interrupted_spend(admitted.workflow(), &job.inputs, &events);
            Some(Spend::new(spend.usd, spend.unknown))
        });
        (known.unwrap_or_else(|| Spend::new(0.0, true)), contained)
    }

    /// The events of the lost attempt's journal, when it left one that reads
    /// and whose chain holds.
    fn journal(&self, job: &JobRecord) -> Option<Vec<nika_event::Event>> {
        let path = nika_dap::store::locate_trace(
            self.journals.as_deref()?,
            job.execution_id()?,
            job.trace_id()?,
        )?;
        nika_dap::attempt::lost_journal(&path)
    }

    /// The workflow the job ran, readmitted from its captured world. A world
    /// whose digest is not the one the job's identity bound is not its own.
    fn workflow(&self, job: &JobRecord, world: Option<&str>) -> Option<AdmittedExecution> {
        let snapshot = ExecutionSnapshot::decode_with_limits(world?, self.snapshots).ok()?;
        if Some(snapshot.digest()) != job.snapshot_digest() {
            return None;
        }
        self.service
            .readmit_snapshot_with_model_override_over(snapshot, None, &super::admission_probes())
            .ok()
    }
}

/// The class of the two that lets less run again on its own.
fn stricter(a: EffectsClass, b: EffectsClass) -> EffectsClass {
    let rank = |class: EffectsClass| match class {
        EffectsClass::Contained => 0,
        EffectsClass::ContainedOffHarness => 1,
        _ => 2,
    };
    if rank(a) >= rank(b) { a } else { b }
}

impl LostOwnerJudge for LostRunJudge {
    fn judge(&self, job: &JobRecord, world: Option<&str>) -> LostVerdict {
        // A job a schedule fired on its beat belongs to its schedule: what
        // follows a lost fire is the schedule's to decide.
        if job.origin() != &JobOrigin::Manual {
            return LostVerdict::Stay(None);
        }
        let policy = self.policy.for_job(job.retry.as_ref());
        // Nothing is read for a job no policy starts again.
        if job.cancel_requested || policy.on_interrupted().mode().is_none() {
            return LostVerdict::Stay(None);
        }
        let (spend, contained) = self.lost_attempt(job, world);
        let mut every_attempt: Vec<Spend> = job
            .attempts
            .iter()
            .map(|attempt| attempt.spend.clone())
            .collect();
        every_attempt.push(spend.clone());
        let lost = LostRun {
            policy,
            consent: job.retry.is_some(),
            contained,
            cancel_requested: job.cancel_requested,
            ceiling: job_ceiling(self.ceiling, job),
            budget: job.retry.as_ref().and_then(RetryStatement::budget),
            spent: &every_attempt,
        };
        match decide(&lost) {
            Ok(mode) => LostVerdict::Queue {
                mode,
                spend,
                bound: policy.max_attempts(),
            },
            Err(refusal) => LostVerdict::Stay(refusal),
        }
    }
}
