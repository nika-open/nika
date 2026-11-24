// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

use serde_json::Value;

use crate::JobOrigin;

use super::{
    Admission, EventHash, IdempotencyKey, JobBindings, JobId, JobMutation, JobReceipt, JobRecord,
    JobStatus, JobStore, JobStoreError, MAX_ENCODED_EXECUTION_SNAPSHOT_BYTES, PersistedState,
    RequestDigest, StoredJob, UNBOUND_CEILING, ValidatedEventBatch, ensure_receipt_matches,
    hash_execution_identity, unique_job_id, validate_snapshot_digest, world_file,
};

impl JobStore {
    /// Atomically stamp a readmitted identity and claim a legal running leg.
    ///
    /// This explicit transition also supports resuming a paused leg. Queue
    /// workers use a queued-only claim: a stale replay is not resume consent.
    /// Identity and `Running` become durable together, so interruption after
    /// restart can retain the execution's receipt.
    ///
    /// # Errors
    /// Returns a typed identity, transition, event, contention, or storage
    /// failure without mutating the durable record.
    pub fn start_execution(
        &self,
        id: &JobId,
        execution_id: String,
        trace_id: String,
        snapshot_digest: String,
        payloads: &[Value],
    ) -> Result<JobMutation, JobStoreError> {
        self.claim_execution(id, execution_id, trace_id, snapshot_digest, payloads, None)
    }

    pub(crate) fn start_queued_execution(
        &self,
        id: &JobId,
        execution_id: String,
        trace_id: String,
        snapshot_digest: String,
        payloads: &[Value],
    ) -> Result<JobMutation, JobStoreError> {
        self.claim_execution(
            id,
            execution_id,
            trace_id,
            snapshot_digest,
            payloads,
            Some(JobStatus::Queued),
        )
    }

    fn claim_execution(
        &self,
        id: &JobId,
        execution_id: String,
        trace_id: String,
        snapshot_digest: String,
        payloads: &[Value],
        expected: Option<JobStatus>,
    ) -> Result<JobMutation, JobStoreError> {
        if execution_id.is_empty()
            || trace_id.is_empty()
            || validate_snapshot_digest(&snapshot_digest).is_err()
        {
            return Err(JobStoreError::InvalidReceipt);
        }
        let batch = ValidatedEventBatch::for_transition(payloads)?;
        let _local = self.local_guard()?;
        let _lease = self.kernel_lease()?;
        let mut state = self.load_state()?;
        super::ensure_approval_claims_unused(&state, &batch)?;
        let job = state
            .jobs
            .iter_mut()
            .find(|job| job.record.id == *id)
            .ok_or_else(|| JobStoreError::JobNotFound(id.clone()))?;
        let current = job.record.status;
        if expected.is_some_and(|status| current != status) || !current.allows(JobStatus::Running) {
            return Err(JobStoreError::IllegalTransition {
                from: current,
                to: JobStatus::Running,
            });
        }
        job.record.execution_id = execution_id;
        job.record.trace_id = trace_id;
        job.record.snapshot_digest = snapshot_digest;
        job.record.status = JobStatus::Running;
        job.record.outputs = None;
        job.record.receipt = None;
        job.identity_digest = Some(hash_execution_identity(&job.record)?);
        job.terminal_sequence = None;
        let events = job.append_payloads(&batch, &self.now())?;
        let record = job.record.clone();
        self.persist_event_mutation(&state, &batch)?;
        Ok(JobMutation { record, events })
    }

    /// Reject only an unclaimed admission under the same lease as the run
    /// claim. A failed duplicate may not settle another execution's job.
    pub(crate) fn refuse_queued(
        &self,
        id: &JobId,
        event: &Value,
    ) -> Result<JobMutation, JobStoreError> {
        self.transition_inner(
            id,
            JobStatus::Failed,
            std::slice::from_ref(event),
            None,
            None,
            Some(JobStatus::Queued),
        )
    }

    /// Cancel an unclaimed job without taking ownership from a running engine.
    /// Identity, receipt and cancellation become durable under the same lease
    /// that excludes `start_execution`; a stale queued read grants no authority.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn cancel_queued(
        &self,
        id: &JobId,
        execution_id: String,
        trace_id: String,
        snapshot_digest: String,
        event: &Value,
        receipt: JobReceipt,
    ) -> Result<JobMutation, JobStoreError> {
        if execution_id.is_empty()
            || trace_id.is_empty()
            || validate_snapshot_digest(&snapshot_digest).is_err()
        {
            return Err(JobStoreError::InvalidReceipt);
        }
        receipt.validate()?;
        let batch = ValidatedEventBatch::for_transition(std::slice::from_ref(event))?;
        let _local = self.local_guard()?;
        let _lease = self.kernel_lease()?;
        let mut state = self.load_state()?;
        super::ensure_approval_claims_unused(&state, &batch)?;
        let job = state
            .jobs
            .iter_mut()
            .find(|job| job.record.id == *id)
            .ok_or_else(|| JobStoreError::JobNotFound(id.clone()))?;
        if job.record.status != JobStatus::Queued {
            return Err(JobStoreError::IllegalTransition {
                from: job.record.status,
                to: JobStatus::Cancelled,
            });
        }
        job.record.execution_id = execution_id;
        job.record.trace_id = trace_id;
        job.record.snapshot_digest = snapshot_digest;
        ensure_receipt_matches(&job.record, &receipt)?;
        job.record.status = JobStatus::Cancelled;
        job.record.outputs = None;
        job.record.receipt = Some(receipt);
        job.identity_digest = Some(hash_execution_identity(&job.record)?);
        job.terminal_sequence = Some(job.final_sequence_after(&batch)?);
        let events = job.append_payloads(&batch, &self.now())?;
        let record = job.record.clone();
        self.persist_event_mutation(&state, &batch)?;
        Ok(JobMutation { record, events })
    }

    /// The admission already bound to this key, without creating one
    /// (ADR-132 · the freeze audit): a lost-response retry finds its job
    /// BEFORE the resident touches the registry or the body again, so a
    /// workflow that changed, vanished or went red since the first request
    /// can neither re-run nor lose the job. `None` when the key is unbound.
    ///
    /// # Errors
    /// Returns an error when locking, loading or validation fails.
    pub fn replay(
        &self,
        key: &IdempotencyKey,
        digest: &RequestDigest,
    ) -> Result<Option<Admission>, JobStoreError> {
        key.validate()?;
        digest.validate()?;
        let _local = self.local_guard()?;
        let _lease = self.kernel_lease()?;
        Ok(bound_admission(&self.load_state()?, key, digest))
    }

    /// The one transaction that creates a manual job, or answers a key that
    /// is already bound.
    pub(super) fn create_or_replay_inner(
        &self,
        key: IdempotencyKey,
        digest: RequestDigest,
        max_jobs: usize,
        workflow: String,
        world: Option<&str>,
        access_pin: Option<String>,
        bindings: JobBindings,
    ) -> Result<Admission, JobStoreError> {
        let JobBindings {
            inputs,
            max_cost_usd,
            attribution,
        } = bindings;
        let (owner, labels) = attribution.into_parts();
        let max_cost_usd = max_cost_usd.map(ceiling_number).transpose()?;
        key.validate()?;
        digest.validate()?;
        if let Some(world) = world
            && world.len() > MAX_ENCODED_EXECUTION_SNAPSHOT_BYTES
        {
            return Err(JobStoreError::SnapshotTooLarge {
                bytes: world.len() as u64,
                maximum: MAX_ENCODED_EXECUTION_SNAPSHOT_BYTES,
            });
        }
        let _local = self.local_guard()?;
        let _lease = self.kernel_lease()?;
        let mut state = self.load_state()?;
        // A job that outlived its retention window is gone before this
        // admission is judged: its key is forgotten and its place is free.
        let evicted = self.evict_from(&mut state);

        if let Some(bound) = bound_admission(&state, &key, &digest) {
            return Ok(bound);
        }

        if state.jobs.len() >= max_jobs {
            return Err(JobStoreError::CapacityExceeded);
        }

        let record = JobRecord {
            id: unique_job_id(&state),
            idempotency_key: key,
            request_digest: digest,
            status: JobStatus::Queued,
            origin: JobOrigin::Manual,
            workflow,
            access_pin,
            inputs,
            max_cost_usd,
            owner,
            labels,
            created_at: Some(self.now()),
            ordinal: state.take_ordinal()?,
            execution_id: String::new(),
            trace_id: String::new(),
            snapshot_digest: String::new(),
            error_code: String::new(),
            error_message: String::new(),
            outputs: None,
            receipt: None,
            settlement: None,
            evidence: None,
            paused_outputs: None,
            paused_receipt: None,
        };
        if let Some(world) = world {
            self.dir.write_atomic(&world_file(&record.id), world)?;
        }
        let mut stored = StoredJob {
            record: record.clone(),
            events: Vec::new(),
            event_count: 0,
            event_head: None,
            identity_digest: None,
            terminal_sequence: None,
        };
        // Bind caller values, a requested ceiling and a stated attribution before execution,
        // including during durable queue recovery. The payload carries no values; the event
        // preimage binds them.
        if record.binds_admission() {
            let queued = serde_json::json!({"kind": crate::JobEventKind::Queued, "status": JobStatus::Queued});
            let batch = ValidatedEventBatch::for_transition(std::slice::from_ref(&queued))?;
            stored.append_payloads(&batch, &self.now())?;
        }
        state.jobs.push(stored);
        self.persist(&state)?;
        self.forget_worlds(&evicted);
        Ok(Admission::Created(record))
    }

    /// Atomically create one scheduled run already claimed as `running`.
    ///
    /// The caller has reserved the shared queue before entering this method.
    /// Persisting `running` before ARM's claim makes a crash visible as
    /// interrupted ambiguity instead of silently replaying an ownerless effect.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn prepare_scheduled_captured(
        &self,
        key: IdempotencyKey,
        digest: RequestDigest,
        max_jobs: usize,
        workflow: String,
        world: &str,
        origin: JobOrigin,
        execution_id: String,
        trace_id: String,
        snapshot_digest: String,
        bindings: JobBindings,
        event: &Value,
    ) -> Result<Admission, JobStoreError> {
        let (owner, labels) = bindings.attribution.into_parts();
        let inputs = bindings.inputs;
        key.validate()?;
        digest.validate()?;
        origin.validate()?;
        if !matches!(origin, JobOrigin::Schedule { .. })
            || execution_id.is_empty()
            || trace_id.is_empty()
            || RequestDigest::new(snapshot_digest.clone()).is_err()
        {
            return Err(JobStoreError::InvalidReceipt);
        }
        if world.len() > MAX_ENCODED_EXECUTION_SNAPSHOT_BYTES {
            return Err(JobStoreError::SnapshotTooLarge {
                bytes: world.len() as u64,
                maximum: MAX_ENCODED_EXECUTION_SNAPSHOT_BYTES,
            });
        }
        let batch = ValidatedEventBatch::for_transition(std::slice::from_ref(event))?;
        let _local = self.local_guard()?;
        let _lease = self.kernel_lease()?;
        let mut state = self.load_state()?;
        // A job that outlived its retention window is gone before this
        // admission is judged: its key is forgotten and its place is free.
        let evicted = self.evict_from(&mut state);

        if let Some(bound) = bound_admission(&state, &key, &digest) {
            return Ok(bound);
        }
        if state.jobs.len() >= max_jobs {
            return Err(JobStoreError::CapacityExceeded);
        }

        let record = JobRecord {
            id: unique_job_id(&state),
            idempotency_key: key,
            request_digest: digest,
            status: JobStatus::Running,
            origin,
            workflow,
            access_pin: None,
            inputs,
            max_cost_usd: None,
            owner,
            labels,
            created_at: Some(self.now()),
            ordinal: state.take_ordinal()?,
            execution_id,
            trace_id,
            snapshot_digest,
            error_code: String::new(),
            error_message: String::new(),
            outputs: None,
            receipt: None,
            settlement: None,
            evidence: None,
            paused_outputs: None,
            paused_receipt: None,
        };
        let identity_digest = Some(hash_execution_identity(&record)?);
        let mut stored = StoredJob {
            record: record.clone(),
            events: Vec::new(),
            event_count: 0,
            event_head: None::<EventHash>,
            identity_digest,
            terminal_sequence: None,
        };
        stored.append_payloads(&batch, &self.now())?;
        self.dir.write_atomic(&world_file(&record.id), world)?;
        state.jobs.push(stored);
        self.persist(&state)?;
        self.forget_worlds(&evicted);
        Ok(Admission::Created(record))
    }
}

/// What a key already bound answers: the same request replays its job,
/// another request conflicts with it. `None` when the key is unbound.
fn bound_admission(
    state: &PersistedState,
    key: &IdempotencyKey,
    digest: &RequestDigest,
) -> Option<Admission> {
    let existing = state
        .jobs
        .iter()
        .find(|job| job.record.idempotency_key == *key)?;
    Some(if existing.record.request_digest == *digest {
        Admission::Existing(existing.record.clone())
    } else {
        Admission::Conflict(existing.record.clone())
    })
}

/// A requested per-job ceiling as the record keeps it: a finite,
/// non-negative amount, or a refusal.
fn ceiling_number(usd: f64) -> Result<serde_json::Number, JobStoreError> {
    serde_json::Number::from_f64(usd)
        .filter(|_| usd >= 0.0)
        .ok_or_else(|| JobStoreError::Corrupt(UNBOUND_CEILING.to_owned()))
}
