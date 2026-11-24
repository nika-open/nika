// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! Eviction of the settled jobs retention no longer keeps (ADR-155).

use super::{JobEvent, JobStore, JobStoreError, PersistedState, world_file};
use crate::JobId;
use crate::job::JobRetention;

/// What a captured world's file name ends with.
const WORLD_SUFFIX: &str = ".world";

impl JobStore {
    /// Set the windows after which a settled job leaves the store. Absent,
    /// the store keeps every job.
    #[must_use]
    pub(crate) fn with_retention(mut self, retention: JobRetention) -> Self {
        self.retention = retention;
        self
    }

    /// Evict, in a transaction of its own, every settled job that outlived
    /// its window, then remove every captured world no job owns: one left by
    /// a crash between two writes goes with them. The resident runs this
    /// once at startup; every admission evicts as well.
    ///
    /// # Errors
    /// Returns an error when locking, loading, writing or listing fails.
    pub(crate) fn evict_expired(&self) -> Result<usize, JobStoreError> {
        let _local = self.local_guard()?;
        let _lease = self.kernel_lease()?;
        let mut state = self.load_state()?;
        let evicted = self.evict_from(&mut state);
        if !evicted.is_empty() {
            self.persist(&state)?;
        }
        for name in self.dir.names()? {
            let owned = name
                .strip_suffix(WORLD_SUFFIX)
                .and_then(|id| JobId::parse(id).ok())
                .is_none_or(|id| state.jobs.iter().any(|job| *job.record.id() == id));
            if !owned {
                self.dir.remove(&name)?;
            }
        }
        Ok(evicted.len())
    }

    /// Remove from `state` every settled job that outlived its window. The
    /// caller holds the store's locks, persists `state`, then calls
    /// [`Self::forget_worlds`] with the result.
    ///
    /// A job is aged from the date of its last event, its settlement.
    pub(super) fn evict_from(&self, state: &mut PersistedState) -> Vec<JobId> {
        if self.retention.keeps_everything() {
            return Vec::new();
        }
        let now = self.clock.now().timestamp();
        let mut evicted = Vec::new();
        state.jobs.retain(|job| {
            let settled = job
                .events
                .last()
                .and_then(JobEvent::at)
                .and_then(|at| at.parse().ok());
            let expired = self.retention.expired(job.record.status(), settled, now);
            if expired {
                evicted.push(job.record.id().clone());
            }
            !expired
        });
        evicted
    }

    /// Remove the captured worlds of evicted jobs, after the state that no
    /// longer names them is durable. A removal that fails leaves a file no
    /// job owns, which the next startup removes.
    pub(super) fn forget_worlds(&self, evicted: &[JobId]) {
        for id in evicted {
            let _removed = self.dir.remove(&world_file(id));
        }
    }
}
