// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! What format 4 adds to the durable snapshot (ADR-155): a stated
//! attribution, an admission date and an admission ordinal per job.
//!
//! Format 3 is read once and persisted as format 4. Nothing it proved is
//! rewritten: an unattributed job hashes exactly as it did, so every event
//! hash, receipt and identity digest of a migrated store is the one it had.

use super::{JobStoreError, PREVIOUS_STATE_VERSION, PersistedState, STATE_VERSION};
use crate::job::JobAttribution;

impl PersistedState {
    /// The ordinal a newly admitted job takes.
    pub(super) fn take_ordinal(&mut self) -> Result<u64, JobStoreError> {
        let ordinal = self.next_ordinal;
        self.next_ordinal = ordinal
            .checked_add(1)
            .ok_or_else(|| JobStoreError::Corrupt("job ordinals are exhausted".to_owned()))?;
        Ok(ordinal)
    }

    /// Rank every job in stored order, which is admission order: the store
    /// only ever appends.
    pub(super) fn assign_ordinals(&mut self) -> Result<(), JobStoreError> {
        self.next_ordinal = 1;
        for index in 0..self.jobs.len() {
            let ordinal = self.take_ordinal()?;
            self.jobs[index].record.ordinal = ordinal;
        }
        Ok(())
    }

    /// Lift a format-3 snapshot to format 4.
    ///
    /// A format-3 writer knew none of the format-4 members, so a snapshot
    /// that claims format 3 and carries one is refused rather than believed.
    pub(super) fn upgrade_from_previous(mut self) -> Result<Self, JobStoreError> {
        if self.version != PREVIOUS_STATE_VERSION {
            return Err(JobStoreError::Corrupt(
                "previous state version is unsupported".to_owned(),
            ));
        }
        let carries_newer_members = self.next_ordinal != 0
            || self.jobs.iter().any(|job| {
                job.record.owner.is_some()
                    || !job.record.labels.is_empty()
                    || job.record.created_at.is_some()
                    || job.record.ordinal != 0
            });
        if carries_newer_members {
            return Err(JobStoreError::Corrupt(
                "a format-3 state carries format-4 members".to_owned(),
            ));
        }
        self.version = STATE_VERSION;
        self.assign_ordinals()?;
        self.validate()?;
        Ok(self)
    }

    /// Ordinals rise strictly in stored order and stay under the next one;
    /// a stored attribution is one the door would have admitted.
    pub(super) fn validate_format(&self) -> Result<(), JobStoreError> {
        let mut previous = 0;
        for job in &self.jobs {
            let record = &job.record;
            if record.ordinal <= previous || record.ordinal >= self.next_ordinal {
                return Err(JobStoreError::Corrupt(
                    "job ordinals are not strictly increasing".to_owned(),
                ));
            }
            previous = record.ordinal;
            if JobAttribution::new(record.owner.clone(), record.labels.clone()).is_err() {
                return Err(JobStoreError::Corrupt(
                    "job attribution is outside its bounds".to_owned(),
                ));
            }
        }
        Ok(())
    }
}
