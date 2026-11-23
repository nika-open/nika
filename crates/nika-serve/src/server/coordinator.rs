// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

use std::collections::BTreeMap;
use std::fmt;
use std::time::{Duration, Instant};

use nika_execution::AdmittedExecution;
use serde_json::json;
use sha2::{Digest as _, Sha256};

use crate::job::JobBindings;
use crate::{
    Admission, IdempotencyKey, JobId, JobOrigin, JobReceipt, JobRecord, JobStatus, RequestDigest,
};

use super::fair_queue::{FairQueue, QueueLoad, QueueRefusal, Reservation};
use super::provider_gate::{ProviderGates, ProviderLoad};
use super::{ExecutionTask, ServerError, ServerLimits, StoreHandle};

/// A place in the execution queue that was not given: a FULL queue is
/// back-pressure (`ExecutionQueueFull`), an owner at its own bound is that
/// owner's to wait out (`OwnerQueueFull`), and a CLOSED queue means the
/// authority is stopping (`Stopping`), which no caller may read as capacity.
fn queue_refusal(refusal: QueueRefusal) -> ServerError {
    match refusal {
        QueueRefusal::Full => ServerError::ExecutionQueueFull,
        QueueRefusal::OwnerFull => ServerError::OwnerQueueFull,
        QueueRefusal::Closed => ServerError::Stopping,
    }
}

const OBSERVATION_POLL: Duration = Duration::from_millis(5);
const SCHEDULE_KEY_DOMAIN: &[u8] = b"nika/resident-schedule-idempotency@1\0";
/// The resident's own idempotency namespace (a scheduled slot's key):
/// `schedule:<namespace>:<sha256>`. A manual caller may not enter it.
pub(super) const SCHEDULE_KEY_PREFIX: &str = "schedule:";

/// One resident admission, queue, concurrency, and observation authority.
///
/// This coordinator does not claim exactly-once execution. A crash after the
/// ARM claim or backend effect but before its terminal receipt remains durable
/// `interrupted`/unsettled ambiguity for an operator to observe.
#[derive(Clone)]
pub struct ResidentExecutionCoordinator {
    store: StoreHandle,
    jobs: std::sync::Arc<FairQueue<ExecutionTask>>,
    limits: ServerLimits,
    reviewed: std::sync::Arc<super::cost_review::Reviewed>,
    /// The operator's caps on provider calls in flight, when any is set.
    provider_gates: Option<std::sync::Arc<ProviderGates>>,
}

impl fmt::Debug for ResidentExecutionCoordinator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResidentExecutionCoordinator")
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl ResidentExecutionCoordinator {
    pub(super) fn new(
        store: StoreHandle,
        jobs: std::sync::Arc<FairQueue<ExecutionTask>>,
        limits: ServerLimits,
        reviewed: std::sync::Arc<super::cost_review::Reviewed>,
    ) -> Self {
        Self {
            store,
            jobs,
            limits,
            reviewed,
            provider_gates: None,
        }
    }

    /// Report the provider caps the resident's jobs run under.
    pub(super) fn with_provider_gates(
        mut self,
        gates: Option<std::sync::Arc<ProviderGates>>,
    ) -> Self {
        self.provider_gates = gates;
        self
    }

    /// The resident's ceilings and what its queue holds now, with the `most`
    /// busiest owners.
    pub(super) fn occupancy(&self, most: usize) -> (ServerLimits, QueueLoad) {
        (self.limits, self.jobs.load(most))
    }

    /// What each capped provider holds now. Empty when none is capped.
    pub(super) fn provider_load(&self) -> Vec<ProviderLoad> {
        self.provider_gates
            .as_ref()
            .map(|gates| gates.load())
            .unwrap_or_default()
    }

    /// Give back the queue place of a job that left `queued` without
    /// running.
    pub(super) fn forget_queued(&self, id: &JobId) {
        self.jobs.forget(id);
    }

    /// The task that runs a queued record.
    fn queued_task(&self, record: &JobRecord) -> ExecutionTask {
        ExecutionTask::new(record.id().clone(), self.limits.default_max_cost_usd())
            .with_access_pin(record.access_pin().map(str::to_owned))
    }

    /// The job already bound to this key, without capturing anything
    /// (ADR-132 · the freeze audit): a retry replays its record, and a
    /// queued replay is queued again unless it waits already, so a replay
    /// takes no second place in its owner's lane.
    pub(super) async fn replay_manual(
        &self,
        key: IdempotencyKey,
        digest: RequestDigest,
    ) -> Result<Option<Admission>, ServerError> {
        let Some(admission) = self.store.replay(key, digest).await? else {
            return Ok(None);
        };
        if let Admission::Existing(record) = &admission
            && record.status() == JobStatus::Queued
        {
            self.jobs
                .requeue(
                    record.owner(),
                    record.id().clone(),
                    self.queued_task(record),
                )
                .map_err(queue_refusal)?;
        }
        Ok(Some(admission))
    }

    #[cfg(test)]
    pub(super) async fn admit_manual(
        &self,
        key: IdempotencyKey,
        digest: RequestDigest,
        workflow: String,
        world: String,
        access_pin: Option<String>,
    ) -> Result<Admission, ServerError> {
        self.admit_manual_inputs(
            key,
            digest,
            workflow,
            world,
            access_pin,
            JobBindings::default(),
            None,
        )
        .await
    }

    /// A reviewed admission (`reviewed`) attaches its captured world and cost
    /// authority to the created job before its task is queued; any other
    /// outcome drops it, which settles its account with nothing sent. A
    /// ceiling in `bindings` (a Session's run) is kept with the job: it
    /// restricts the server's at every run of it, never widens it.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn admit_manual_inputs(
        &self,
        key: IdempotencyKey,
        digest: RequestDigest,
        workflow: String,
        world: String,
        access_pin: Option<String>,
        bindings: JobBindings,
        reviewed: Option<(nika_execution::ExecutionSession, super::CostAuthority)>,
    ) -> Result<Admission, ServerError> {
        // The place is taken in the owner's lane before the record exists:
        // an owner at its bound is refused with nothing written.
        let permit = self
            .jobs
            .reserve(bindings.attribution.owner())
            .map_err(queue_refusal)?;
        let admission = self
            .store
            .create_or_replay(
                key,
                digest,
                self.limits.max_jobs(),
                workflow,
                world,
                access_pin,
                bindings,
            )
            .await?;
        match &admission {
            Admission::Created(record) => {
                if let Some(reviewed) = reviewed {
                    self.reviewed
                        .attach(record.id().as_str(), Box::new(reviewed));
                }
                permit.send(record.id().clone(), self.queued_task(record));
            }
            Admission::Existing(record) if record.status() == JobStatus::Queued => {
                permit.send(record.id().clone(), self.queued_task(record));
            }
            Admission::Existing(_) | Admission::Conflict(_) => drop(permit),
        }
        Ok(admission)
    }

    /// Prepare a normal durable run before ARM appends its fenced claim.
    ///
    /// The exact encoded snapshot is persisted and the run is marked running
    /// before returning. Dropping a newly prepared value before `execute`
    /// settles it failed without invoking the backend or retaining queue space.
    ///
    /// # Errors
    /// Returns a typed queue, store, identity, or snapshot refusal.
    pub fn prepare_scheduled(
        &self,
        admitted: AdmittedExecution,
        origin: JobOrigin,
    ) -> Result<PreparedScheduledRun, ServerError> {
        self.prepare_scheduled_with_max_cost(admitted, origin, None, BTreeMap::new())
    }

    /// Prepare a scheduled run carrying its required per-fire spend ceiling.
    ///
    /// The declared ceiling is validated, then folded with the server-level
    /// default: when both exist the LOWER wins — a schedule restricts the
    /// server's ceiling, never widens it (#1349).
    ///
    /// # Errors
    /// Returns the same typed refusals as [`Self::prepare_scheduled`].
    pub fn prepare_scheduled_with_max_cost(
        &self,
        admitted: AdmittedExecution,
        origin: JobOrigin,
        max_cost_usd: Option<f64>,
        inputs: BTreeMap<String, serde_json::Value>,
    ) -> Result<PreparedScheduledRun, ServerError> {
        self.prepare_scheduled_bound(
            admitted,
            origin,
            max_cost_usd,
            JobBindings::of(inputs, None),
        )
    }

    /// [`Self::prepare_scheduled_with_max_cost`] under everything the
    /// schedule binds onto its run: the inputs and the attribution it states.
    pub(crate) fn prepare_scheduled_bound(
        &self,
        admitted: AdmittedExecution,
        origin: JobOrigin,
        max_cost_usd: Option<f64>,
        bindings: JobBindings,
    ) -> Result<PreparedScheduledRun, ServerError> {
        if max_cost_usd.is_some_and(|cost| !cost.is_finite() || cost <= 0.0) {
            return Err(ServerError::ScheduledAdmission);
        }
        let max_cost_usd = self.limits.effective_max_cost_usd(max_cost_usd);
        let key = scheduled_key(&origin)?;
        let world = admitted
            .snapshot()
            .encode()
            .map_err(|_| ServerError::ScheduledAdmission)?;
        let digest = RequestDigest::from_bytes(Sha256::digest(world.as_bytes()).into());
        let workflow = admitted.snapshot().root().to_owned();
        let execution_id = admitted.execution_id().to_string();
        let trace_id = admitted.trace_id().to_string();
        let snapshot_digest = admitted.snapshot().digest().to_owned();
        let event_origin = origin.clone();
        // A run the resident starts itself is bounded by the whole queue
        // alone and served before every lane (see `fair_queue`).
        let permit = self
            .jobs
            .reserve_started(bindings.attribution.owner())
            .map_err(queue_refusal)?;
        let admission = self.store.prepare_scheduled_blocking(
            key,
            digest,
            self.limits.max_jobs(),
            workflow,
            world,
            origin,
            execution_id,
            trace_id,
            snapshot_digest,
            bindings,
            json!({
                "kind": crate::JobEventKind::Prepared,
                "origin": event_origin,
                "status": JobStatus::Running
            }),
        )?;
        match admission {
            Admission::Created(record) => Ok(PreparedScheduledRun::created(
                self.clone(),
                record,
                admitted,
                permit,
                max_cost_usd,
            )),
            // The slot already has its job. Under another world it means the
            // workflow changed after that fire: a deploy between a crash and
            // its restart, or a schedule removed and declared again while
            // the slot is still on time. The slot stays consumed against the
            // run it had; it never fires twice and never stops the resident.
            Admission::Existing(record) | Admission::Conflict(record) => {
                drop(permit);
                Ok(PreparedScheduledRun::existing(self.clone(), record))
            }
        }
    }

    fn observe_terminal(&self, id: &JobId) -> Result<JobRecord, ServerError> {
        let queue_waves = self
            .limits
            .queue_capacity()
            .div_ceil(self.limits.max_concurrent_jobs())
            .saturating_add(1);
        let queue_waves = u32::try_from(queue_waves).unwrap_or(u32::MAX);
        let observation_window = self
            .limits
            .execution_timeout()
            .saturating_mul(queue_waves)
            .checked_add(self.limits.shutdown_grace())
            .ok_or(ServerError::ScheduledObservationTimeout)?;
        let deadline = Instant::now()
            .checked_add(observation_window)
            .ok_or(ServerError::ScheduledObservationTimeout)?;
        loop {
            let record = self
                .store
                .get_blocking(id.clone())?
                .ok_or_else(|| crate::JobStoreError::JobNotFound(id.clone()))?;
            if !matches!(record.status(), JobStatus::Queued | JobStatus::Running) {
                return Ok(record);
            }
            if Instant::now() >= deadline {
                return Err(ServerError::ScheduledObservationTimeout);
            }
            std::thread::sleep(OBSERVATION_POLL);
        }
    }

    fn abort_prepared(&self, record: &JobRecord) {
        if record.status() != JobStatus::Running {
            return;
        }
        let Ok(receipt) = JobReceipt::with_origin(
            record.id().clone(),
            record.execution_id().unwrap_or_default(),
            record.trace_id().unwrap_or_default(),
            record.snapshot_digest().unwrap_or_default(),
            None,
            record.origin().clone(),
        ) else {
            return;
        };
        let _result = self.store.settle_with_result_blocking(
            record.id().clone(),
            JobStatus::Failed,
            json!({
                "code": "scheduled_claim_aborted",
                "kind": crate::JobEventKind::AbortedBeforeClaim,
                "message": "prepared schedule admission lost its ARM claim",
                "status": JobStatus::Failed
            }),
            None,
            Some(receipt),
        );
    }
}

/// Prepared half of the two-phase scheduled admission transaction.
pub struct PreparedScheduledRun {
    coordinator: ResidentExecutionCoordinator,
    record: JobRecord,
    admitted: Option<AdmittedExecution>,
    permit: Option<Reservation<ExecutionTask>>,
    abort_on_drop: bool,
    max_cost_usd: Option<f64>,
}

impl fmt::Debug for PreparedScheduledRun {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedScheduledRun")
            .field("run_id", self.record.id())
            .field("status", &self.record.status())
            .finish_non_exhaustive()
    }
}

impl PreparedScheduledRun {
    fn created(
        coordinator: ResidentExecutionCoordinator,
        record: JobRecord,
        admitted: AdmittedExecution,
        permit: Reservation<ExecutionTask>,
        max_cost_usd: Option<f64>,
    ) -> Self {
        Self {
            coordinator,
            record,
            admitted: Some(admitted),
            permit: Some(permit),
            abort_on_drop: true,
            max_cost_usd,
        }
    }

    fn existing(coordinator: ResidentExecutionCoordinator, record: JobRecord) -> Self {
        Self {
            coordinator,
            record,
            admitted: None,
            permit: None,
            abort_on_drop: false,
            max_cost_usd: None,
        }
    }

    /// Normal durable run id allocated before the ARM claim.
    #[must_use]
    pub fn run_id(&self) -> &JobId {
        self.record.id()
    }

    /// Execution identity bound to the normal run.
    #[must_use]
    pub fn execution_id(&self) -> Option<&str> {
        self.record.execution_id()
    }

    /// Direct trace identity bound to the normal run.
    #[must_use]
    pub fn trace_id(&self) -> Option<&str> {
        self.record.trace_id()
    }

    /// Enqueue through the shared lane and block only on durable observation.
    ///
    /// # Errors
    /// Returns if the durable store disappears or the bounded execution and
    /// shutdown window elapses without a visible terminal/paused record.
    pub fn execute(mut self) -> Result<JobRecord, ServerError> {
        if let (Some(permit), Some(admitted)) = (self.permit.take(), self.admitted.take()) {
            self.abort_on_drop = false;
            permit.send(
                self.record.id().clone(),
                ExecutionTask::scheduled(
                    self.record.id().clone(),
                    admitted,
                    self.record.origin().clone(),
                    self.max_cost_usd,
                ),
            );
        }
        self.coordinator.observe_terminal(self.record.id())
    }

    #[cfg(test)]
    pub(super) fn abandon_for_restart_test(mut self) {
        self.abort_on_drop = false;
    }
}

impl Drop for PreparedScheduledRun {
    fn drop(&mut self) {
        if self.abort_on_drop {
            self.coordinator.abort_prepared(&self.record);
        }
    }
}

fn scheduled_key(origin: &JobOrigin) -> Result<IdempotencyKey, ServerError> {
    let (namespace, schedule_id, slot_id) = origin
        .schedule_key_parts()
        .ok_or(ServerError::ScheduledAdmission)?;
    let mut hasher = Sha256::new();
    hasher.update(SCHEDULE_KEY_DOMAIN);
    hasher.update(namespace.as_bytes());
    hasher.update([0]);
    hasher.update(schedule_id.as_bytes());
    hasher.update([0]);
    hasher.update(slot_id.as_bytes());
    IdempotencyKey::new(format!(
        "{SCHEDULE_KEY_PREFIX}{namespace}:{:x}",
        hasher.finalize()
    ))
    .map_err(ServerError::JobStore)
}

#[cfg(test)]
mod queue_refusal_tests {
    use super::super::fair_queue::QueueBounds;
    use super::*;

    /// A full queue is back-pressure; an owner at its bound is that owner's
    /// refusal; a closed queue is the authority stopping. The first and the
    /// last were one error before, and a fire that met the closed queue at
    /// shutdown ended the resident with « queue full ».
    #[test]
    fn a_closed_queue_is_stopping_and_a_full_one_is_capacity() {
        let queue: std::sync::Arc<FairQueue<u8>> = FairQueue::new(QueueBounds {
            capacity: 2,
            queued_per_owner: Some(1),
            running_per_owner: None,
        });
        let held = queue.reserve(Some("a")).expect("one place");
        let owner = queue.reserve(Some("a")).err().expect("the owner's bound");
        assert!(matches!(queue_refusal(owner), ServerError::OwnerQueueFull));
        let other = queue.reserve(Some("b")).expect("another owner's place");
        let full = queue.reserve(Some("c")).err().expect("no third place");
        assert!(matches!(
            queue_refusal(full),
            ServerError::ExecutionQueueFull
        ));
        drop((held, other));
        queue.close();
        let closed = queue.reserve(None).err().expect("closed queue");
        assert!(matches!(queue_refusal(closed), ServerError::Stopping));
    }
}
