// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! How long the resident keeps a settled job in its store (ADR-155).
//!
//! Without a window the store keeps every job, and at its ceiling it refuses
//! every admission for ever. A window lets a busy resident forget what it
//! settled long ago. The store row is an index of a run, never its
//! evidence: the trace journal has its own life and does not follow.

use std::time::Duration;

use super::JobStatus;

/// The shortest window an operator may set. An evicted job's idempotency
/// key is forgotten, so a window must outlive the caller's retries.
pub(crate) const MIN_JOB_RETENTION: Duration = Duration::from_secs(60);

/// The windows after which a settled job leaves the store. The default
/// keeps every job.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct JobRetention {
    /// How long a `succeeded` job stays after it settles.
    pub(crate) succeeded: Option<Duration>,
    /// How long a `failed`, `cancelled` or `interrupted` job stays.
    pub(crate) unsuccessful: Option<Duration>,
}

impl JobRetention {
    /// Whether no job ever leaves the store.
    pub(crate) const fn keeps_everything(self) -> bool {
        self.succeeded.is_none() && self.unsuccessful.is_none()
    }

    /// Whether every window that is set is at least [`MIN_JOB_RETENTION`].
    pub(crate) fn valid(self) -> bool {
        [self.succeeded, self.unsuccessful]
            .into_iter()
            .flatten()
            .all(|window| window >= MIN_JOB_RETENTION)
    }

    /// How long a job in `status` stays after it settles. A job that is
    /// queued, running or paused never leaves: something still owns it.
    const fn window(self, status: JobStatus) -> Option<Duration> {
        match status {
            JobStatus::Succeeded => self.succeeded,
            JobStatus::Failed | JobStatus::Cancelled | JobStatus::Interrupted => self.unsuccessful,
            JobStatus::Queued | JobStatus::Running | JobStatus::Paused => None,
        }
    }

    /// Whether a job that reached `status` at `settled` has outlived its
    /// window at `now`. A settlement without a date is never aged: the
    /// store does not destroy what it cannot date.
    pub(crate) fn expired(
        self,
        status: JobStatus,
        settled: Option<jiff::Timestamp>,
        now: jiff::Timestamp,
    ) -> bool {
        let (Some(window), Some(settled)) = (self.window(status), settled) else {
            return false;
        };
        let age = now.as_second().saturating_sub(settled.as_second());
        u64::try_from(age).is_ok_and(|age| age >= window.as_secs())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> jiff::Timestamp {
        text.parse().expect("instant")
    }

    #[test]
    fn a_window_ages_only_the_statuses_it_names() {
        let retention = JobRetention {
            succeeded: Some(Duration::from_secs(3_600)),
            unsuccessful: Some(Duration::from_secs(7 * 86_400)),
        };
        let settled = Some(at("2026-11-16T10:00:00Z"));
        let hour = at("2026-11-16T11:00:00Z");
        let week = at("2026-11-23T10:00:00Z");
        assert!(!retention.expired(JobStatus::Succeeded, settled, at("2026-11-16T10:59:59Z")));
        assert!(retention.expired(JobStatus::Succeeded, settled, hour));
        for status in [
            JobStatus::Failed,
            JobStatus::Cancelled,
            JobStatus::Interrupted,
        ] {
            assert!(!retention.expired(status, settled, hour), "{status}");
            assert!(retention.expired(status, settled, week), "{status}");
        }
        for owned in [JobStatus::Queued, JobStatus::Running, JobStatus::Paused] {
            assert!(!retention.expired(owned, settled, week), "{owned}");
        }
        assert!(
            !retention.expired(JobStatus::Succeeded, None, week),
            "an undated settlement is never aged"
        );
        assert!(
            !retention.expired(JobStatus::Succeeded, Some(week), hour),
            "a settlement dated after now is not old"
        );
    }

    #[test]
    fn the_default_keeps_everything_and_a_window_is_at_least_a_minute() {
        let keep = JobRetention::default();
        assert!(keep.keeps_everything() && keep.valid());
        assert!(!keep.expired(
            JobStatus::Succeeded,
            Some(at("2020-01-01T00:00:00Z")),
            at("2026-11-23T10:00:00Z")
        ));
        let minute = JobRetention {
            succeeded: Some(MIN_JOB_RETENTION),
            unsuccessful: None,
        };
        assert!(minute.valid() && !minute.keeps_everything());
        let short = JobRetention {
            succeeded: None,
            unsuccessful: Some(Duration::from_secs(59)),
        };
        assert!(!short.valid());
    }
}
