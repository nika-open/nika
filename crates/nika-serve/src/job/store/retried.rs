// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! A settled job a request starts again (ADR-156 section 9).
//!
//! The store owns the write and nothing else: the decision (the bound, the
//! cost, the resume judgment) is the resident's, made before it asks. The
//! store checks again, under its lease, that the request still names the
//! job's latest attempt and that the attempt settled in a status a retry
//! is allowed from, and makes the new attempt durable in one write.

use super::{
    JobId, JobMutation, JobRecord, JobStatus, JobStore, JobStoreError, StoredJob,
    ValidatedEventBatch, has_complete_execution_identity,
};
use crate::JobEventKind;
use crate::job::{ClosedAttempt, RetryAsk, Spend};

/// What a request asks of the store, once the resident has decided.
#[derive(Debug, Clone)]
pub(crate) struct RetriedAttempt {
    /// The request, validated.
    pub(crate) ask: RetryAsk,
    /// What the attempt it retries spent.
    pub(crate) spend: Spend,
    /// The bound of attempts in force for the job, counting the first.
    pub(crate) bound: u32,
}

/// What the store did with a retry request.
#[derive(Debug, Clone)]
pub(crate) enum Retried {
    /// The attempt is closed and a new one is queued.
    Queued(JobMutation),
    /// The same request already retried this attempt: nothing is written.
    Replayed(JobRecord),
    /// The job is not where the request found it, or not retryable.
    NotRetryable,
}

impl JobStore {
    /// Close the job's latest attempt and queue the next one, in one write,
    /// when `retried` names that attempt and it ended `interrupted` or
    /// `failed` with an execution identity and no accepted cancellation.
    ///
    /// # Errors
    /// Returns a missing-job, event, contention, or storage failure.
    /// Nothing is written then.
    pub(crate) fn retry_settled(
        &self,
        id: &JobId,
        retried: &RetriedAttempt,
    ) -> Result<Retried, JobStoreError> {
        let _local = self.local_guard()?;
        let _lease = self.kernel_lease()?;
        let mut state = self.load_state()?;
        let now = self.now();
        let job = state
            .jobs
            .iter_mut()
            .find(|job| job.record.id == *id)
            .ok_or_else(|| JobStoreError::JobNotFound(id.clone()))?;
        if is_replay(&job.record, &retried.ask) {
            return Ok(Retried::Replayed(job.record.clone()));
        }
        if !retryable(&job.record) || job.record.attempt() != retried.ask.attempt {
            return Ok(Retried::NotRetryable);
        }
        let mutation = job.queue_retried(retried, &now)?;
        self.persist(&state)?;
        Ok(Retried::Queued(mutation))
    }
}

/// Whether a request may retry the job's latest attempt as it stands: it
/// ended `interrupted` or `failed`, it has the identity a closed attempt
/// keeps, and its caller did not cancel it.
pub(crate) fn retryable(record: &JobRecord) -> bool {
    matches!(record.status, JobStatus::Interrupted | JobStatus::Failed)
        && has_complete_execution_identity(record)
        && !record.cancel_requested
}

/// Whether the same request already retried the attempt it names: the
/// attempt after it exists, and the request that created it stated the same
/// mode and the same budget.
pub(crate) fn is_replay(record: &JobRecord, ask: &RetryAsk) -> bool {
    let retried = usize::try_from(ask.attempt)
        .ok()
        .and_then(|attempt| attempt.checked_sub(1))
        .and_then(|index| record.attempts.get(index));
    record.attempt() == ask.attempt.saturating_add(1)
        && retried.is_some_and(|closed| {
            closed.next == ask.mode && closed.requested.as_ref() == Some(&ask.request)
        })
}

impl StoredJob {
    /// The attempt is evidence: it keeps its identity, its result, its
    /// receipt and the sequence of its terminal event, whose hash binds
    /// them. The job goes back to `queued`.
    fn queue_retried(
        &mut self,
        retried: &RetriedAttempt,
        at: &super::EventInstant,
    ) -> Result<JobMutation, JobStoreError> {
        let attempt = self.record.attempt();
        let payload = serde_json::json!({
            "kind": JobEventKind::Requeued,
            "status": JobStatus::Queued,
            "message": format!(
                "attempt {} of {} ({}): attempt {attempt} retried by request",
                attempt.saturating_add(1),
                retried.bound,
                retried.ask.mode.as_str(),
            ),
        });
        let batch = ValidatedEventBatch::for_transition(std::slice::from_ref(&payload))?;
        let closed = ClosedAttempt {
            status: self.record.status,
            execution_id: std::mem::take(&mut self.record.execution_id),
            trace_id: std::mem::take(&mut self.record.trace_id),
            snapshot_digest: std::mem::take(&mut self.record.snapshot_digest),
            outputs: self.record.outputs.take(),
            receipt: self.record.receipt.take(),
            identity_digest: self.identity_digest.take(),
            requeued_sequence: self.final_sequence_after(&batch)?,
            terminal_sequence: self.terminal_sequence.take(),
            spend: retried.spend.clone(),
            next: retried.ask.mode,
            requested: Some(retried.ask.request.clone()),
        };
        self.record.attempts.push(closed);
        self.record.status = JobStatus::Queued;
        self.record.error_code.clear();
        self.record.error_message.clear();
        let events = self.append_payloads(&batch, at)?;
        Ok(JobMutation {
            record: self.record.clone(),
            events,
        })
    }
}
