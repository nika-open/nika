// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! A job's attempts as a reader sees them (ADR-156 section 10), read from
//! one validated snapshot: the closed attempts the record keeps, the one in
//! hand, the instants their events carry, and where the job stands about
//! being started again.

use super::{JobEvent, JobId, JobRecord, JobStatus, JobStore, JobStoreError, StoredJob};
use crate::job::{AttemptMode, RetryRefusal, RetryState, Spend};
use crate::{JobEventKind, JobReceipt};

/// One attempt of a job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AttemptView {
    /// Its number; the first is 1.
    pub(crate) attempt: u32,
    pub(crate) status: JobStatus,
    pub(crate) execution_id: Option<String>,
    pub(crate) trace_id: Option<String>,
    /// How it was started again; `None` on the first attempt.
    pub(crate) mode: Option<AttemptMode>,
    /// The trace it continues, when it resumed the attempt before it.
    pub(crate) resumed_from: Option<String>,
    pub(crate) receipt: Option<JobReceipt>,
    /// What the decision that closed it counted; `None` on the attempt in
    /// hand, whose settlement carries its own spend.
    pub(crate) spend: Option<Spend>,
    pub(crate) started_at: Option<String>,
    pub(crate) ended_at: Option<String>,
}

/// Every attempt of a job, oldest first, and where it stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct JobAttempts {
    pub(crate) record: JobRecord,
    pub(crate) attempts: Vec<AttemptView>,
    pub(crate) retry_state: RetryState,
}

impl JobStore {
    /// The attempts of one job, `None` when the store has no such job.
    ///
    /// # Errors
    /// Returns an error when the store cannot be locked or validated.
    pub(crate) fn attempts(&self, id: &JobId) -> Result<Option<JobAttempts>, JobStoreError> {
        let _local = self.local_guard()?;
        let _lease = self.kernel_lease()?;
        let state = self.load_state()?;
        Ok(state
            .jobs
            .iter()
            .find(|job| job.record.id == *id)
            .map(|job| JobAttempts {
                record: job.record.clone(),
                attempts: job.attempt_views(),
                retry_state: job.retry_state(),
            }))
    }
}

impl StoredJob {
    /// Where the job stands about being started again. A refusal is read
    /// from the `interrupted` event that ended the job, or from the error of
    /// a re-attempt refused before it ran.
    pub(super) fn retry_state(&self) -> RetryState {
        let record = &self.record;
        match record.status {
            JobStatus::Queued if !record.attempts.is_empty() => RetryState::Queued,
            JobStatus::Interrupted => self
                .event_at(self.terminal_sequence)
                .and_then(|event| event.payload.get("code")?.as_str())
                .and_then(RetryRefusal::parse)
                .map_or(RetryState::None, RetryState::refused),
            JobStatus::Failed if !record.attempts.is_empty() => {
                RetryRefusal::parse(&record.error_code)
                    .map_or(RetryState::None, RetryState::refused)
            }
            _ => RetryState::None,
        }
    }

    fn event_at(&self, sequence: Option<u64>) -> Option<&JobEvent> {
        let index = usize::try_from(sequence?.checked_sub(1)?).ok()?;
        self.events.get(index)
    }

    /// The instant of the first `execution.started` event after `after`
    /// and up to `until` (the end of the events when `None`).
    fn started_between(&self, after: u64, until: Option<u64>) -> Option<String> {
        self.events
            .iter()
            .filter(|event| event.sequence > after && until.is_none_or(|end| event.sequence <= end))
            .find(|event| JobEventKind::Started.is(&event.payload))
            .and_then(JobEvent::at)
            .map(str::to_owned)
    }

    fn attempt_views(&self) -> Vec<AttemptView> {
        let record = &self.record;
        let mut views = Vec::with_capacity(record.attempts.len().saturating_add(1));
        let mut after = 0;
        let mut before: Option<&crate::job::ClosedAttempt> = None;
        for (number, closed) in (1_u32..).zip(&record.attempts) {
            // A settled attempt a request retried ended at its terminal
            // event; a lost one ended where it was queued again.
            let ended = closed.terminal_sequence.unwrap_or(closed.requeued_sequence);
            views.push(AttemptView {
                attempt: number,
                status: closed.status,
                execution_id: Some(closed.execution_id.clone()),
                trace_id: Some(closed.trace_id.clone()),
                mode: before.map(|earlier| earlier.next),
                resumed_from: resumed_from(before),
                receipt: closed.receipt.clone(),
                spend: Some(closed.spend.clone()),
                started_at: self.started_between(after, Some(closed.requeued_sequence)),
                ended_at: self
                    .event_at(Some(ended))
                    .and_then(JobEvent::at)
                    .map(str::to_owned),
            });
            after = closed.requeued_sequence;
            before = Some(closed);
        }
        let settled = record.status.is_settled();
        views.push(AttemptView {
            attempt: record.attempt(),
            status: record.status,
            execution_id: record.execution_id().map(str::to_owned),
            trace_id: record.trace_id().map(str::to_owned),
            mode: before.map(|earlier| earlier.next),
            resumed_from: resumed_from(before),
            receipt: record.receipt.clone().filter(|_| settled),
            spend: None,
            started_at: self.started_between(after, None),
            ended_at: settled
                .then(|| self.event_at(self.terminal_sequence))
                .flatten()
                .and_then(JobEvent::at)
                .map(str::to_owned),
        });
        views
    }
}

/// The trace an attempt continues: the one before it, when that one was
/// closed to be resumed.
fn resumed_from(before: Option<&crate::job::ClosedAttempt>) -> Option<String> {
    before
        .filter(|earlier| earlier.next == AttemptMode::Resume)
        .map(|earlier| earlier.trace_id.clone())
}
