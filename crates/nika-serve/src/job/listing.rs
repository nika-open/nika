// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! One page of the job list (ADR-155): newest first, by admission ordinal.
//!
//! A summary is the light projection of a job a listing shows. It never
//! carries inputs, outputs, a receipt or a settlement; those stay on the
//! job itself.

use std::collections::BTreeMap;

use super::{JobId, JobOrigin, JobStatus};

/// Most jobs one page holds.
pub(crate) const MAX_PAGE_LEN: usize = 200;
/// Jobs one page holds when the caller names no limit.
pub(crate) const DEFAULT_PAGE_LEN: usize = 50;

/// How a job entered the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OriginKind {
    /// A caller submitted it through the job API.
    Manual,
    /// A resident schedule fired it.
    Schedule,
}

/// Which jobs a listing keeps. Every member that is set must match.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct JobListFilter {
    /// The exact stated owner.
    pub(crate) owner: Option<String>,
    /// Any of these statuses; empty keeps every status.
    pub(crate) statuses: Vec<JobStatus>,
    /// The exact workflow name captured at admission.
    pub(crate) workflow: Option<String>,
    /// Manual or scheduled.
    pub(crate) origin: Option<OriginKind>,
    /// Every pair must be a label of the job.
    pub(crate) labels: Vec<(String, String)>,
    /// Admitted strictly after this instant.
    pub(crate) created_after: Option<jiff::Timestamp>,
    /// Admitted strictly before this instant.
    pub(crate) created_before: Option<jiff::Timestamp>,
}

/// What a listing shows of one job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct JobSummary {
    pub(crate) id: JobId,
    pub(crate) status: JobStatus,
    pub(crate) workflow: String,
    pub(crate) origin: JobOrigin,
    pub(crate) owner: Option<String>,
    pub(crate) labels: BTreeMap<String, String>,
    /// When the resident admitted the job; for a record written before
    /// format 4, the date of its first dated event.
    pub(crate) created_at: Option<String>,
    pub(crate) execution_id: Option<String>,
    pub(crate) trace_id: Option<String>,
    /// The redacted diagnosis, on a failed job only.
    pub(crate) error: Option<(String, String)>,
    /// How many attempts the job has taken or holds, the first included.
    pub(crate) attempts: u32,
    /// Where the job stands about being started again (ADR-156 §10), as
    /// the store reads it; the policy in force adds the bound.
    pub(crate) retry_state: super::RetryState,
    /// The job's own retry statement, which narrows the policy in force.
    pub(crate) retry: Option<super::RetryStatement>,
    /// A request could retry the job's latest attempt as it stands.
    pub(crate) retryable: bool,
    /// Admission rank: what the page cursor names.
    pub(crate) ordinal: u64,
}

/// One page, newest first, and where the next one starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct JobPage {
    pub(crate) jobs: Vec<JobSummary>,
    /// The ordinal of the last job served, when an older match exists.
    pub(crate) next: Option<u64>,
}
