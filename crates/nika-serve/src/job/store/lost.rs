// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! What becomes of a running job whose owner is gone (ADR-156).
//!
//! The store owns the write and nothing else. Whether a lost run is
//! started again is the resident's decision ([`LostOwnerJudge`]); the store
//! asks it once per job and makes the answer durable in the same write as
//! the loss itself. A job that will be started again goes from `running`
//! to `queued` and never shows `interrupted`: the queue is the durable
//! mark, so a second crash before the new attempt starts loses nothing.

use serde_json::Value;

use super::{
    EventInstant, IncarnationGeneration, JobId, JobStatus, JobStore, JobStoreError, StoredJob,
    ValidatedEventBatch, attach_interrupted_receipt, has_complete_execution_identity,
};
use crate::job::{AttemptMode, ClosedAttempt, RetryRefusal, Spend};
use crate::{JobEventKind, JobRecord, ServerIncarnation};

/// What the resident decides for one job whose run lost its owner.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum LostVerdict {
    /// The job stays `interrupted`. A reason, when there is one to state,
    /// is the `code` of its `execution.interrupted` event.
    Stay(Option<RetryRefusal>),
    /// A new attempt is queued.
    Queue {
        /// How the new attempt runs.
        mode: AttemptMode,
        /// What the lost attempt committed against the job's ceiling.
        spend: Spend,
        /// The bound of attempts in force for the job, counting the first.
        bound: u32,
    },
}

/// Who decides. The store asks under its own lease, so an answer is never
/// judged against a record another writer has since moved.
pub(crate) trait LostOwnerJudge {
    /// Decide for `job`, whose status is `running` and whose owner is gone.
    /// `world` is its captured workflow world, when the store still holds it.
    fn judge(&self, job: &JobRecord, world: Option<&str>) -> LostVerdict;
}

/// The judge of a store no retry policy was stated for: every lost run
/// stays `interrupted`, as before attempts existed.
pub(super) struct NeverAgain;

impl LostOwnerJudge for NeverAgain {
    fn judge(&self, _job: &JobRecord, _world: Option<&str>) -> LostVerdict {
        LostVerdict::Stay(None)
    }
}

impl JobStore {
    /// Settle every running job when an incarnation takes the store: the
    /// runs of the incarnation before it have no owner. A new attempt is
    /// queued where `judge` allows one; the job is left `interrupted`
    /// otherwise. Returns how many jobs were left `interrupted`. Runs once
    /// per incarnation.
    ///
    /// # Errors
    /// Returns an error when locking, loading, validation, or durable
    /// writing fails. Nothing is written then.
    pub(crate) fn settle_lost_jobs(
        &self,
        incarnation: &ServerIncarnation,
        judge: &dyn LostOwnerJudge,
    ) -> Result<usize, JobStoreError> {
        let _local = self.local_guard()?;
        let _lease = self.kernel_lease()?;
        let mut state = self.load_state()?;
        if state.incarnation.current != incarnation.generation {
            return Err(JobStoreError::StaleServerIncarnation);
        }
        if state.incarnation.settled == Some(incarnation.generation) {
            return Ok(0);
        }
        let generations = Some((
            incarnation.generation.get(),
            state.incarnation.settled.map(IncarnationGeneration::get),
        ));
        let now = self.now();
        let mut interrupted = 0;
        for job in &mut state.jobs {
            if job.record.status != JobStatus::Running {
                continue;
            }
            if !has_complete_execution_identity(&job.record) {
                job.requeue_unstarted(generations, &now)?;
                continue;
            }
            let world = self.read_world(&job.record.id).ok();
            match judge.judge(&job.record, world.as_deref()) {
                LostVerdict::Queue { mode, spend, bound } => {
                    job.queue_next_attempt(mode, spend, bound, generations, &now)?;
                }
                LostVerdict::Stay(refusal) => {
                    job.interrupt_lost(refusal, generations, &now)?;
                    interrupted += 1;
                }
            }
        }
        state.incarnation.settled = Some(incarnation.generation);
        self.persist(&state)?;
        Ok(interrupted)
    }

    /// Settle one running job the live incarnation dropped while it stops.
    /// The same decision as [`Self::settle_lost_jobs`], for the one job.
    ///
    /// # Errors
    /// Returns a stale-incarnation, missing-job, illegal-status, or storage
    /// failure. Nothing is written then.
    pub(crate) fn settle_lost_job(
        &self,
        id: &JobId,
        incarnation: &ServerIncarnation,
        judge: &dyn LostOwnerJudge,
    ) -> Result<JobRecord, JobStoreError> {
        let _local = self.local_guard()?;
        let _lease = self.kernel_lease()?;
        let mut state = self.load_state()?;
        if state.incarnation.current != incarnation.generation {
            return Err(JobStoreError::StaleServerIncarnation);
        }
        let now = self.now();
        let job = state
            .jobs
            .iter_mut()
            .find(|job| job.record.id == *id)
            .ok_or_else(|| JobStoreError::JobNotFound(id.clone()))?;
        if job.record.status != JobStatus::Running {
            return Err(JobStoreError::IllegalTransition {
                from: job.record.status,
                to: JobStatus::Interrupted,
            });
        }
        let world = self.read_world(id).ok();
        match judge.judge(&job.record, world.as_deref()) {
            LostVerdict::Queue { mode, spend, bound } => {
                job.queue_next_attempt(mode, spend, bound, None, &now)?;
            }
            LostVerdict::Stay(refusal) => job.interrupt_lost(refusal, None, &now)?,
        }
        let record = job.record.clone();
        self.persist(&state)?;
        Ok(record)
    }

    /// Record that a cancellation was accepted for a running job, so that
    /// no later incarnation starts it again. Returns whether the job was
    /// running. No event is written: the job's own settlement says how the
    /// cancellation ended.
    ///
    /// # Errors
    /// Returns a missing-job or storage failure.
    pub(crate) fn mark_cancel_requested(&self, id: &JobId) -> Result<bool, JobStoreError> {
        let _local = self.local_guard()?;
        let _lease = self.kernel_lease()?;
        let mut state = self.load_state()?;
        let job = state
            .jobs
            .iter_mut()
            .find(|job| job.record.id == *id)
            .ok_or_else(|| JobStoreError::JobNotFound(id.clone()))?;
        if job.record.status != JobStatus::Running {
            return Ok(false);
        }
        if !job.record.cancel_requested {
            job.record.cancel_requested = true;
            self.persist(&state)?;
        }
        Ok(true)
    }
}

/// Name the two incarnations a start crossed on an event payload.
fn name_generations(payload: &mut Value, generations: Option<(u64, Option<u64>)>) {
    if let Some((current, prior)) = generations {
        payload["incarnation_generation"] = Value::from(current);
        payload["previous_incarnation_generation"] = prior.map_or(Value::Null, Value::from);
    }
}

impl StoredJob {
    /// A job claimed as `running` that never took an execution identity ran
    /// nothing: it returns to the queue as the same first attempt.
    fn requeue_unstarted(
        &mut self,
        generations: Option<(u64, Option<u64>)>,
        at: &EventInstant,
    ) -> Result<(), JobStoreError> {
        if self.record.workflow.is_empty() {
            return Err(JobStoreError::Corrupt(
                "running job lacks both durable execution identity and restart schedule".to_owned(),
            ));
        }
        let mut payload = serde_json::json!({
            "kind": JobEventKind::Requeued,
            "status": JobStatus::Queued,
        });
        name_generations(&mut payload, generations);
        let batch = ValidatedEventBatch::for_transition(std::slice::from_ref(&payload))?;
        self.record.status = JobStatus::Queued;
        self.record.outputs = None;
        self.record.receipt = None;
        self.terminal_sequence = None;
        self.append_payloads(&batch, at)?;
        Ok(())
    }

    /// The lost run stays what it is: evidence. The job is `interrupted`.
    fn interrupt_lost(
        &mut self,
        refusal: Option<RetryRefusal>,
        generations: Option<(u64, Option<u64>)>,
        at: &EventInstant,
    ) -> Result<(), JobStoreError> {
        // The interruption a start writes has always carried the bare word
        // and its two incarnations; a stop names neither.
        let mut payload = if generations.is_some() {
            serde_json::json!({"kind": "interrupted", "status": JobStatus::Interrupted})
        } else {
            serde_json::json!({
                "kind": JobEventKind::Interrupted,
                "status": JobStatus::Interrupted,
            })
        };
        name_generations(&mut payload, generations);
        if let Some(refusal) = refusal {
            payload["code"] = Value::from(refusal.code());
            payload["message"] = Value::from(refusal.message());
        }
        let batch = ValidatedEventBatch::for_transition(std::slice::from_ref(&payload))?;
        self.record.status = JobStatus::Interrupted;
        attach_interrupted_receipt(&mut self.record)?;
        self.terminal_sequence = Some(self.final_sequence_after(&batch)?);
        self.append_payloads(&batch, at)?;
        Ok(())
    }

    /// Close the lost attempt into the job's history and queue the next one,
    /// in the one write that records the loss. The closed attempt keeps its
    /// identity and its interrupted receipt, and the event that queues the
    /// next attempt binds them in its hash.
    fn queue_next_attempt(
        &mut self,
        mode: AttemptMode,
        spend: Spend,
        bound: u32,
        generations: Option<(u64, Option<u64>)>,
        at: &EventInstant,
    ) -> Result<(), JobStoreError> {
        let lost = self.record.attempt();
        let mut payload = serde_json::json!({
            "kind": JobEventKind::Requeued,
            "status": JobStatus::Queued,
            "message": format!(
                "attempt {} of {bound} ({}): attempt {lost} lost its owner",
                lost.saturating_add(1),
                mode.as_str(),
            ),
        });
        name_generations(&mut payload, generations);
        let batch = ValidatedEventBatch::for_transition(std::slice::from_ref(&payload))?;
        attach_interrupted_receipt(&mut self.record)?;
        let closed = ClosedAttempt {
            status: JobStatus::Interrupted,
            execution_id: std::mem::take(&mut self.record.execution_id),
            trace_id: std::mem::take(&mut self.record.trace_id),
            snapshot_digest: std::mem::take(&mut self.record.snapshot_digest),
            outputs: None,
            receipt: self.record.receipt.take(),
            identity_digest: self.identity_digest.take(),
            requeued_sequence: self.final_sequence_after(&batch)?,
            terminal_sequence: None,
            spend,
            next: mode,
            requested: None,
        };
        self.record.attempts.push(closed);
        self.record.status = JobStatus::Queued;
        self.record.outputs = None;
        self.terminal_sequence = None;
        self.append_payloads(&batch, at)?;
        Ok(())
    }
}
