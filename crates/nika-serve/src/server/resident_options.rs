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

    /// The resident ceilings these options ask for.
    pub(crate) const fn limits(&self) -> ServerLimits {
        self.limits
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
