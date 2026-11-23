// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! The job list, read from one validated snapshot (ADR-155).

use super::{JobEvent, JobStatus, JobStore, JobStoreError, StoredJob};
use crate::JobOrigin;
use crate::job::{JobListFilter, JobPage, JobSummary, OriginKind};

/// A job that was `queued` when the resident last stopped, as its queue
/// takes it back: the access pin its task carries and the owner whose lane
/// it waits in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QueuedJob {
    pub(crate) id: crate::JobId,
    pub(crate) access_pin: Option<String>,
    pub(crate) owner: Option<String>,
}

impl JobStore {
    /// The queued jobs that still name a workflow, in admission order.
    ///
    /// # Errors
    /// Returns an error when the store cannot be locked or validated.
    pub(crate) fn queued_for_recovery(&self) -> Result<Vec<QueuedJob>, JobStoreError> {
        let _local = self.local_guard()?;
        let _lease = self.kernel_lease()?;
        let state = self.load_state()?;
        Ok(state
            .jobs
            .iter()
            .filter(|job| job.record.status == JobStatus::Queued && !job.record.workflow.is_empty())
            .map(|job| QueuedJob {
                id: job.record.id.clone(),
                access_pin: job.record.access_pin.clone(),
                owner: job.record.owner.clone(),
            })
            .collect())
    }

    /// One page of the jobs `filter` keeps, newest first.
    ///
    /// `before` is the ordinal of the last job the previous page served:
    /// the page holds strictly older jobs. Ordinals are never reused, so a
    /// job admitted or evicted between two pages moves no other job.
    ///
    /// # Errors
    /// Returns an error when locking, loading or validation fails.
    pub(crate) fn list(
        &self,
        filter: &JobListFilter,
        limit: usize,
        before: Option<u64>,
    ) -> Result<JobPage, JobStoreError> {
        let _local = self.local_guard()?;
        let _lease = self.kernel_lease()?;
        let state = self.load_state()?;
        let mut jobs: Vec<JobSummary> = Vec::new();
        let mut next = None;
        let older = state
            .jobs
            .iter()
            .rev()
            .filter(|job| before.is_none_or(|before| job.record.ordinal < before))
            .filter(|job| keeps(filter, job));
        for job in older {
            if jobs.len() >= limit {
                next = jobs.last().map(|served| served.ordinal);
                break;
            }
            jobs.push(summary(job));
        }
        Ok(JobPage { jobs, next })
    }
}

fn keeps(filter: &JobListFilter, job: &StoredJob) -> bool {
    let record = &job.record;
    if filter
        .owner
        .as_deref()
        .is_some_and(|owner| record.owner() != Some(owner))
        || (!filter.statuses.is_empty() && !filter.statuses.contains(&record.status()))
        || filter
            .workflow
            .as_deref()
            .is_some_and(|workflow| record.workflow() != workflow)
    {
        return false;
    }
    let scheduled = matches!(record.origin(), JobOrigin::Schedule { .. });
    if filter
        .origin
        .is_some_and(|origin| (origin == OriginKind::Schedule) != scheduled)
    {
        return false;
    }
    if !filter
        .labels
        .iter()
        .all(|(key, value)| record.labels().get(key) == Some(value))
    {
        return false;
    }
    if filter.created_after.is_none() && filter.created_before.is_none() {
        return true;
    }
    // An undated job cannot be placed in a window: it is left out of a dated
    // listing rather than given an instant it never recorded.
    let Some(created) = admitted_at(job).and_then(|at| at.parse::<jiff::Timestamp>().ok()) else {
        return false;
    };
    filter.created_after.is_none_or(|after| created > after)
        && filter.created_before.is_none_or(|before| created < before)
}

/// When the resident admitted the job: its admission date, or for a record
/// written before format 4 the date of its first dated event.
fn admitted_at(job: &StoredJob) -> Option<&str> {
    job.record
        .created_at()
        .or_else(|| job.events.iter().find_map(JobEvent::at))
}

fn summary(job: &StoredJob) -> JobSummary {
    let record = &job.record;
    let error = (record.status() == JobStatus::Failed)
        .then(|| record.error())
        .flatten()
        .map(|(code, message)| (code.to_owned(), message.to_owned()));
    JobSummary {
        id: record.id().clone(),
        status: record.status(),
        workflow: record.workflow().to_owned(),
        origin: record.origin().clone(),
        owner: record.owner().map(str::to_owned),
        labels: record.labels().clone(),
        created_at: admitted_at(job).map(str::to_owned),
        execution_id: record.execution_id().map(str::to_owned),
        trace_id: record.trace_id().map(str::to_owned),
        error,
        attempts: record.attempt(),
        retry_state: job.retry_state(),
        ordinal: record.ordinal,
    }
}
