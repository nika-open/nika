// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! What the operator sets on the resident itself at startup.
//!
//! These are the `nika serve` flags that need no listener: they shape the
//! resident's durable job state, which scheduled runs fill as well.

use std::time::Duration;

use super::ServerLimits;
use super::production::ServerLaunchRefuse;
use crate::job::MIN_JOB_RETENTION;

/// The resident's own startup options, with or without an HTTP listener.
/// The default changes nothing.
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct ResidentOptions {
    limits: ServerLimits,
}

impl ResidentOptions {
    /// Apply `nika serve --job-retention` and `--job-retention-unsuccessful`:
    /// how long a settled job stays in the store. Each is a whole number and
    /// a unit (`s`, `m`, `h`, `d`), at least one minute. Absent, the store
    /// keeps every job; unsuccessful jobs follow the first window unless the
    /// second names theirs.
    ///
    /// # Errors
    /// A value that is not such a duration, or is under one minute.
    pub fn with_job_retention(
        mut self,
        succeeded: Option<&str>,
        unsuccessful: Option<&str>,
    ) -> Result<Self, ServerLaunchRefuse> {
        let window = |stated: &str| {
            stated_duration(stated)
                .filter(|window| *window >= MIN_JOB_RETENTION)
                .ok_or(ServerLaunchRefuse::InvalidJobRetention)
        };
        if let Some(stated) = succeeded {
            self.limits = self.limits.with_job_retention(window(stated)?);
        }
        if let Some(stated) = unsuccessful {
            self.limits = self.limits.with_unsuccessful_job_retention(window(stated)?);
        }
        Ok(self)
    }

    /// Apply `nika serve --max-concurrent-jobs`, `--queue-capacity` and
    /// `--max-jobs`: how many jobs run at once, how many may wait, and how
    /// many the store keeps. Absent, each keeps its default.
    ///
    /// # Errors
    /// A count of zero.
    pub fn with_job_capacity(
        mut self,
        max_concurrent: Option<usize>,
        queue: Option<usize>,
        max_jobs: Option<usize>,
    ) -> Result<Self, ServerLaunchRefuse> {
        if let Some(workers) = stated_count(max_concurrent)? {
            self.limits = self.limits.with_max_concurrent_jobs(workers);
        }
        if let Some(waiting) = stated_count(queue)? {
            self.limits = self.limits.with_queue_capacity(waiting);
        }
        if let Some(kept) = stated_count(max_jobs)? {
            self.limits = self.limits.with_max_jobs(kept);
        }
        Ok(self)
    }

    /// Apply `nika serve --max-queued-per-owner` and
    /// `--max-concurrent-per-owner`: what one owner may hold. Absent, an
    /// owner is bounded by the resident's own limits only.
    ///
    /// # Errors
    /// A count of zero.
    pub fn with_owner_limits(
        mut self,
        max_queued: Option<usize>,
        max_concurrent: Option<usize>,
    ) -> Result<Self, ServerLaunchRefuse> {
        if let Some(waiting) = stated_count(max_queued)? {
            self.limits = self.limits.with_max_queued_per_owner(waiting);
        }
        if let Some(running) = stated_count(max_concurrent)? {
            self.limits = self.limits.with_max_concurrent_per_owner(running);
        }
        Ok(self)
    }

    /// Apply `nika serve --shutdown-grace`: how long a stop waits for running
    /// and queued jobs. A whole number and a unit (`s`, `m`, `h`, `d`), at
    /// least one second. Absent, thirty seconds.
    ///
    /// # Errors
    /// A value that is not such a duration, or is under one second.
    pub fn with_shutdown_grace(mut self, stated: Option<&str>) -> Result<Self, ServerLaunchRefuse> {
        if let Some(stated) = stated {
            let grace = stated_duration(stated)
                .filter(|grace| !grace.is_zero())
                .ok_or(ServerLaunchRefuse::InvalidShutdownGrace)?;
            self.limits = self.limits.with_shutdown_grace(grace);
        }
        Ok(self)
    }

    /// The resident ceilings these options ask for.
    pub(crate) const fn limits(&self) -> ServerLimits {
        self.limits
    }
}

/// One count as an operator states it on a limit flag: at least one.
fn stated_count(stated: Option<usize>) -> Result<Option<usize>, ServerLaunchRefuse> {
    match stated {
        Some(0) => Err(ServerLaunchRefuse::InvalidJobLimit),
        other => Ok(other),
    }
}

/// One duration as an operator writes it on a flag: a whole number without
/// sign or leading zero, then one unit: `s`, `m`, `h` or `d` (24 hours).
pub(crate) fn stated_duration(text: &str) -> Option<Duration> {
    let unit = text.chars().next_back()?;
    let digits = &text[..text.len() - unit.len_utf8()];
    let seconds_per_unit: u64 = match unit {
        's' => 1,
        'm' => 60,
        'h' => 3_600,
        'd' => 86_400,
        _ => return None,
    };
    let canonical = !digits.is_empty()
        && digits.bytes().all(|byte| byte.is_ascii_digit())
        && !(digits.len() > 1 && digits.starts_with('0'));
    let count = digits.parse::<u64>().ok().filter(|_| canonical)?;
    count.checked_mul(seconds_per_unit).map(Duration::from_secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_duration_is_one_whole_number_and_one_unit() {
        for (text, seconds) in [
            ("90s", 90),
            ("5m", 300),
            ("36h", 129_600),
            ("7d", 604_800),
            ("0s", 0),
        ] {
            assert_eq!(
                stated_duration(text),
                Some(Duration::from_secs(seconds)),
                "{text}"
            );
        }
        for text in [
            "",
            "7",
            "d",
            "7 d",
            "7D",
            "1.5h",
            "-1h",
            "+1h",
            "07d",
            "1h30m",
            "7days",
            "7j",
            "99999999999999999999d",
            "18446744073709551615d",
        ] {
            assert_eq!(stated_duration(text), None, "{text:?}");
        }
    }

    #[test]
    fn limit_flags_set_the_limits_or_are_refused() {
        let unset = ResidentOptions::default()
            .with_job_capacity(None, None, None)
            .and_then(|options| options.with_owner_limits(None, None))
            .and_then(|options| options.with_shutdown_grace(None))
            .expect("no flag");
        assert_eq!(unset, ResidentOptions::default());
        let limits = unset.limits();
        assert_eq!(limits.max_concurrent_jobs(), 4);
        assert_eq!(limits.queue_capacity(), 64);
        assert_eq!(limits.max_jobs(), 10_000);
        assert_eq!(limits.shutdown_grace(), Duration::from_secs(30));
        assert_eq!(limits.max_queued_per_owner(), None);
        assert_eq!(limits.max_concurrent_per_owner(), None);

        let stated = ResidentOptions::default()
            .with_job_capacity(Some(16), Some(5_000), Some(20_000))
            .and_then(|options| options.with_owner_limits(Some(50), Some(2)))
            .and_then(|options| options.with_shutdown_grace(Some("5m")))
            .expect("every flag");
        let limits = stated.limits();
        assert!(limits.valid());
        assert_eq!(limits.max_concurrent_jobs(), 16);
        assert_eq!(limits.queue_capacity(), 5_000);
        assert_eq!(limits.max_jobs(), 20_000);
        assert_eq!(limits.shutdown_grace(), Duration::from_secs(300));
        assert_eq!(limits.max_queued_per_owner(), Some(50));
        assert_eq!(limits.max_concurrent_per_owner(), Some(2));

        let options = ResidentOptions::default;
        for refused in [
            options().with_job_capacity(Some(0), None, None),
            options().with_job_capacity(None, Some(0), None),
            options().with_job_capacity(None, None, Some(0)),
            options().with_owner_limits(Some(0), None),
            options().with_owner_limits(None, Some(0)),
        ] {
            assert_eq!(refused, Err(ServerLaunchRefuse::InvalidJobLimit));
        }
        for grace in ["0s", "30", "soon", "1.5m", ""] {
            assert_eq!(
                options().with_shutdown_grace(Some(grace)),
                Err(ServerLaunchRefuse::InvalidShutdownGrace),
                "{grace:?}"
            );
        }
    }

    #[test]
    fn job_retention_flags_set_the_windows_or_are_refused() {
        let unset = ResidentOptions::default()
            .with_job_retention(None, None)
            .expect("no flag");
        assert_eq!(unset, ResidentOptions::default());
        assert!(unset.limits().job_retention().keeps_everything());

        let one = ResidentOptions::default()
            .with_job_retention(Some("24h"), None)
            .expect("one window");
        let retention = one.limits().job_retention();
        assert_eq!(retention.succeeded, Some(Duration::from_secs(86_400)));
        assert_eq!(
            retention.unsuccessful, retention.succeeded,
            "unsuccessful jobs follow the first window"
        );

        let both = ResidentOptions::default()
            .with_job_retention(Some("24h"), Some("7d"))
            .expect("both windows");
        let retention = both.limits().job_retention();
        assert_eq!(retention.succeeded, Some(Duration::from_secs(86_400)));
        assert_eq!(retention.unsuccessful, Some(Duration::from_secs(604_800)));

        let failures_only = ResidentOptions::default()
            .with_job_retention(None, Some("1m"))
            .expect("unsuccessful only");
        let retention = failures_only.limits().job_retention();
        assert_eq!(retention.succeeded, None);
        assert_eq!(retention.unsuccessful, Some(Duration::from_secs(60)));

        for (succeeded, unsuccessful) in [
            (Some("59s"), None),
            (Some("soon"), None),
            (Some("24h"), Some("0d")),
            (None, Some("1 week")),
        ] {
            assert_eq!(
                ResidentOptions::default().with_job_retention(succeeded, unsuccessful),
                Err(ServerLaunchRefuse::InvalidJobRetention),
                "{succeeded:?} {unsuccessful:?}"
            );
        }
    }
}
