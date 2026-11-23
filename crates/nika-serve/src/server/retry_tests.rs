// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! A job whose run lost its owner (ADR-156): what the resident decides, what
//! a request may state about it, and what the next resident runs.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use nika_dap::journal::TraceFileSink;
use nika_event::{Event, EventKind};
use nika_execution::SnapshotLimits;
use nika_runtime::EventSink as _;
use nika_types::id::{EventId, ExecutionId, TraceId};
use nika_types::resource::{KeyValue, Value as FieldValue};
use nika_types::timestamp::Timestamp;
use serde_json::{Value, json};

use super::retry::LostRunJudge;
use super::tests::{
    TestBackend, TestWorld, WORKFLOW, auth_header, cancel_request, events_request, get_request,
    limits, parse_sse_data, post_request, snapshot_body, wait_for_status,
};
use super::{ExecutionBackend, ExecutionDisposition, ExecutionOutcome, ServerError, ServerLimits};
use crate::job::{
    AttemptMode, ClosedAttempt, JobBindings, LostOwnerJudge, LostVerdict, OnInterrupted,
    RetryPolicy, RetryRefusal, RetryStatement, Spend,
};
use crate::{IdempotencyKey, JobRecord, JobStatus, JobStore, RequestDigest};

fn frame(kind: EventKind, fields: &[(&str, FieldValue)]) -> Event {
    let fields = fields
        .iter()
        .map(|(key, value)| KeyValue::new(*key, value.clone()))
        .collect();
    Event::new(
        EventId::generate(),
        Timestamp::from_unix_ms(1_792_000_000_000),
        kind,
    )
    .with_fields(fields)
}

fn admitted_record(admission: crate::Admission) -> JobRecord {
    match admission {
        crate::Admission::Created(record)
        | crate::Admission::Existing(record)
        | crate::Admission::Conflict(record) => record,
    }
}

/// The record a run writes before anything else.
fn boot() -> Event {
    frame(
        EventKind::WorkflowStarted,
        &[("workflow", FieldValue::string("root".to_owned()))],
    )
}

/// Write what a killed run leaves under `dir`: `events`, through the
/// resident's own sink, never settled and never sealed.
fn leave_journal(dir: &Path, execution: ExecutionId, trace: TraceId, events: &[Event]) {
    let mut journal = TraceFileSink::new(dir).for_execution(execution, trace);
    for event in events {
        journal.emit(event.clone());
    }
    assert!(journal.error().is_none(), "the journal was written");
}

// ---------------------------------------------------------------- the judge

fn restart(bound: u32) -> ServerLimits {
    limits().with_retry_policy(RetryPolicy::new(OnInterrupted::Restart, bound))
}

fn statement(value: &Value) -> Option<RetryStatement> {
    let stated = RetryStatement::stated(value);
    assert!(stated.is_some(), "a legal statement: {value}");
    stated
}

/// One job a resident started and then lost, as the store holds it.
struct Lost {
    _state: tempfile::TempDir,
    journals: tempfile::TempDir,
    job: JobRecord,
}

/// Admit and start one job, then leave what its killed run would: the
/// events of `journal`, or no journal at all.
fn lost(bindings: JobBindings, journal: Option<&[Event]>) -> Lost {
    let state = tempfile::tempdir().expect("state");
    let journals = tempfile::tempdir().expect("journals");
    let store = JobStore::open(state.path()).expect("store");
    let admitted = store
        .create_or_replay_captured_inputs(
            IdempotencyKey::new("judged").expect("key"),
            RequestDigest::from_bytes([7; 32]),
            usize::MAX,
            "root.nika".to_owned(),
            &snapshot_body(WORKFLOW),
            None,
            bindings,
        )
        .expect("admit");
    let record = admitted_record(admitted);
    let execution = ExecutionId::generate();
    let trace = TraceId::from(execution);
    store
        .start_execution(
            record.id(),
            execution.to_string(),
            trace.to_string(),
            world_digest(),
            &[json!({"kind": "execution.started", "status": "running"})],
        )
        .expect("start");
    if let Some(events) = journal {
        leave_journal(journals.path(), execution, trace, events);
    }
    let job = store.get(record.id()).expect("get").expect("job");
    Lost {
        _state: state,
        journals,
        job,
    }
}

/// A job classed free of external effects whose run wrote its boot record.
fn contained(bindings: JobBindings) -> Lost {
    lost(bindings.classed("none"), Some(&[boot()]))
}

fn judge(limits: ServerLimits, journals: Option<&Path>) -> LostRunJudge {
    LostRunJudge::new(
        limits,
        journals.map(Path::to_path_buf),
        SnapshotLimits::default(),
    )
}

/// The digest of the captured world every test job runs: the one a job
/// that started binds.
fn world_digest() -> String {
    nika_execution::ExecutionSnapshot::decode_with_limits(
        &snapshot_body(WORKFLOW),
        SnapshotLimits::default(),
    )
    .expect("the test world decodes")
    .digest()
    .to_owned()
}

fn judged(limits: ServerLimits, lost: &Lost) -> LostVerdict {
    judge(limits, Some(lost.journals.path())).judge(&lost.job, Some(&snapshot_body(WORKFLOW)))
}

fn queued(spend: Spend, bound: u32) -> LostVerdict {
    LostVerdict::Queue {
        mode: AttemptMode::Restart,
        spend,
        bound,
    }
}

#[test]
fn a_resident_with_no_policy_starts_nothing_again() {
    let lost = contained(JobBindings::default());
    assert_eq!(judged(limits(), &lost), LostVerdict::Stay(None));
}

#[test]
fn a_contained_workflow_that_dispatched_nothing_is_started_again() {
    let lost = contained(JobBindings::default());
    assert_eq!(judged(restart(3), &lost), queued(Spend::new(0.0, false), 3));
}

#[test]
fn a_job_states_its_own_mode_and_lowers_the_bound() {
    // The resident is off; the job asks for a restart.
    let lost = contained(
        JobBindings::default().retrying(statement(&json!({"on_interrupted": "restart"}))),
    );
    assert_eq!(judged(limits(), &lost), queued(Spend::new(0.0, false), 3));
    // The resident restarts; the job asks to be left alone.
    let lost =
        contained(JobBindings::default().retrying(statement(&json!({"on_interrupted": "off"}))));
    assert_eq!(judged(restart(3), &lost), LostVerdict::Stay(None));
    // One attempt is all the job allows itself.
    let lost = contained(JobBindings::default().retrying(statement(&json!({"max_attempts": 1}))));
    assert_eq!(
        judged(restart(3), &lost),
        LostVerdict::Stay(Some(RetryRefusal::Exhausted))
    );
    // It cannot raise the resident's bound.
    let lost = contained(JobBindings::default().retrying(statement(&json!({"max_attempts": 9}))));
    assert_eq!(judged(restart(2), &lost), queued(Spend::new(0.0, false), 2));
}

#[test]
fn effects_outside_the_engine_need_the_job_to_have_stated_retry() {
    for effects in [Some("external"), Some("a word of a later engine"), None] {
        let classed = |bindings: JobBindings| match effects {
            Some(word) => bindings.classed(word),
            None => bindings,
        };
        let lost_run = lost(classed(JobBindings::default()), Some(&[boot()]));
        assert_eq!(
            judged(restart(3), &lost_run),
            LostVerdict::Stay(Some(RetryRefusal::EffectsNeedConsent)),
            "{effects:?}"
        );
        let consented = lost(
            classed(JobBindings::default()).retrying(statement(&json!({}))),
            Some(&[boot()]),
        );
        assert_eq!(
            judged(restart(3), &consented),
            queued(Spend::new(0.0, false), 3),
            "the statement is the consent: {effects:?}"
        );
    }
}

#[test]
fn an_agent_whose_tools_only_read_is_contained_off_a_harness_only() {
    let plan = |plan: &str| {
        frame(
            EventKind::WorkflowStarted,
            &[("access_plan", FieldValue::string(plan.to_owned()))],
        )
    };
    let direct = plan(r#"[{"model":"mock/echo","access":"mock","chosen":"mock"}]"#);
    let harness = plan(r#"[{"model":"x/y","access":"codex","chosen":"harness"}]"#);
    let off = lost(
        JobBindings::default().classed("none_off_harness"),
        Some(&[direct]),
    );
    assert_eq!(judged(restart(3), &off), queued(Spend::new(0.0, false), 3));
    let on = lost(
        JobBindings::default().classed("none_off_harness"),
        Some(&[harness]),
    );
    assert_eq!(
        judged(restart(3), &on),
        LostVerdict::Stay(Some(RetryRefusal::EffectsNeedConsent))
    );
    // No journal names no route: the harness cannot be ruled out.
    let silent = lost(JobBindings::default().classed("none_off_harness"), None);
    assert_eq!(
        judged(restart(3).with_default_max_cost_usd(None), &silent),
        LostVerdict::Stay(Some(RetryRefusal::EffectsNeedConsent))
    );
}

#[test]
fn what_a_settled_task_reported_is_what_the_lost_attempt_spent() {
    let settled = frame(
        EventKind::TaskCompleted,
        &[
            ("task", FieldValue::string("value".to_owned())),
            ("cost_usd", FieldValue::Float(0.4)),
        ],
    );
    let lost = lost(
        JobBindings::default().classed("none"),
        Some(&[boot(), settled]),
    );
    assert_eq!(judged(restart(3), &lost), queued(Spend::new(0.4, false), 3));
    // Under a ceiling the attempt already crossed, nothing is left.
    assert_eq!(
        judged(restart(3).with_default_max_cost_usd(Some(0.3)), &lost),
        LostVerdict::Stay(Some(RetryRefusal::BudgetExhausted))
    );
}

#[test]
fn a_spend_nobody_can_read_refuses_under_a_ceiling_and_never_counts_for_zero() {
    // The run left no journal: killed before its first event, or its
    // journal never opened. The two cannot be told apart.
    let silent = lost(JobBindings::default().classed("none"), None);
    assert_eq!(
        judged(restart(3), &silent),
        LostVerdict::Stay(Some(RetryRefusal::UnknownSpend))
    );
    // A backend that keeps no journal says as little.
    let journaled = contained(JobBindings::default());
    assert_eq!(
        judge(restart(3), None).judge(&journaled.job, Some(&snapshot_body(WORKFLOW))),
        LostVerdict::Stay(Some(RetryRefusal::UnknownSpend))
    );
    // So does a journal whose captured workflow can no longer be read, for
    // a job that consented to run again whatever its class.
    let consented = contained(JobBindings::default().retrying(statement(&json!({}))));
    assert_eq!(
        judge(restart(3), Some(consented.journals.path())).judge(&consented.job, None),
        LostVerdict::Stay(Some(RetryRefusal::UnknownSpend))
    );
    // No ceiling bounds the job: there is nothing the unknown could cross.
    assert_eq!(
        judged(restart(3).with_default_max_cost_usd(None), &silent),
        queued(Spend::new(0.0, true), 3)
    );
    // A stated budget for the re-attempts accepts the unknown.
    let accepted = lost(
        JobBindings::default()
            .classed("none")
            .retrying(statement(&json!({"max_cost_usd": 0.5}))),
        None,
    );
    assert_eq!(
        judged(restart(3), &accepted),
        queued(Spend::new(0.0, true), 3)
    );
}

/// An attempt a job closed, having spent `usd` (or an unknown amount).
fn closed_attempt(usd: f64, unknown: bool, sequence: u64) -> ClosedAttempt {
    ClosedAttempt {
        status: JobStatus::Interrupted,
        execution_id: format!("exe-earlier-{sequence}"),
        trace_id: format!("trace-earlier-{sequence}"),
        snapshot_digest: world_digest(),
        outputs: None,
        receipt: None,
        identity_digest: None,
        requeued_sequence: sequence,
        spend: Spend::new(usd, unknown),
        next: AttemptMode::Restart,
    }
}

#[test]
fn the_attempt_about_to_run_gets_what_its_job_has_left() {
    use super::retry::attempt_ceiling;
    let mut lost = contained(JobBindings::default());
    // A first attempt runs under its job's ceiling, a zero one included.
    assert_eq!(attempt_ceiling(Some(0.0), &lost.job), Ok(Some(0.0)));
    assert_eq!(attempt_ceiling(None, &lost.job), Ok(None));
    lost.job.attempts.push(closed_attempt(0.25, false, 2));
    assert_eq!(attempt_ceiling(Some(1.0), &lost.job), Ok(Some(0.75)));
    assert_eq!(
        attempt_ceiling(Some(0.25), &lost.job),
        Err(RetryRefusal::BudgetExhausted)
    );
    lost.job.attempts.push(closed_attempt(0.0, true, 4));
    assert_eq!(
        attempt_ceiling(Some(1.0), &lost.job),
        Err(RetryRefusal::UnknownSpend)
    );
    assert_eq!(attempt_ceiling(None, &lost.job), Ok(None));
}

#[test]
fn a_class_its_captured_workflow_does_not_confirm_is_not_trusted() {
    // The stored word says contained, but the world the judge is handed is
    // not the one the job's identity bound.
    let mut lost = contained(JobBindings::default());
    lost.job.snapshot_digest = RequestDigest::from_bytes([3; 32]).as_str().to_owned();
    assert_eq!(
        judged(restart(3), &lost),
        LostVerdict::Stay(Some(RetryRefusal::EffectsNeedConsent))
    );
    // No world at all: nothing confirms the word, and nothing bounds the
    // tasks, even for a job that consented.
    let lost = contained(JobBindings::default());
    let judge = judge(restart(3), Some(lost.journals.path()));
    assert_eq!(
        judge.judge(&lost.job, None),
        LostVerdict::Stay(Some(RetryRefusal::EffectsNeedConsent))
    );
    let consented = contained(JobBindings::default().retrying(statement(&json!({}))));
    assert_eq!(
        judge.judge(&consented.job, None),
        LostVerdict::Stay(Some(RetryRefusal::UnknownSpend))
    );
}

#[test]
fn a_journal_whose_chain_is_broken_is_no_evidence_of_spend() {
    let settled = frame(
        EventKind::TaskCompleted,
        &[
            ("task", FieldValue::string("value".to_owned())),
            ("cost_usd", FieldValue::Float(0.4)),
        ],
    );
    let lost = lost(
        JobBindings::default().classed("none"),
        Some(&[boot(), settled]),
    );
    assert_eq!(judged(restart(3), &lost), queued(Spend::new(0.4, false), 3));
    let path = nika_dap::store::locate_trace(
        lost.journals.path(),
        lost.job.execution_id().expect("execution"),
        lost.job.trace_id().expect("trace"),
    )
    .expect("the run left its journal");
    // The same two lines in the other order: the settled task now reads
    // first, and the chain no longer holds anywhere.
    let text = std::fs::read_to_string(&path).expect("journal");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2, "{text}");
    std::fs::write(&path, format!("{}\n{}\n", lines[1], lines[0])).expect("rewrite");
    assert_eq!(
        judged(restart(3), &lost),
        LostVerdict::Stay(Some(RetryRefusal::UnknownSpend))
    );
}

#[test]
fn the_bound_and_the_ceiling_count_every_attempt_the_job_left() {
    let mut lost = contained(JobBindings::default());
    let closed = |usd: f64, sequence: u64| ClosedAttempt {
        status: JobStatus::Interrupted,
        execution_id: format!("exe-earlier-{sequence}"),
        trace_id: format!("trace-earlier-{sequence}"),
        snapshot_digest: RequestDigest::from_bytes([7; 32]).as_str().to_owned(),
        outputs: None,
        receipt: None,
        identity_digest: None,
        requeued_sequence: sequence,
        spend: Spend::new(usd, false),
        next: AttemptMode::Restart,
    };
    // The resident's ceiling is one dollar; the first attempt spent it.
    lost.job.attempts.push(closed(1.0, 2));
    assert_eq!(
        judged(restart(3), &lost),
        LostVerdict::Stay(Some(RetryRefusal::BudgetExhausted))
    );
    // Two attempts left, one lost now: the bound of three is reached.
    lost.job.attempts.clear();
    lost.job.attempts.push(closed(0.1, 2));
    lost.job.attempts.push(closed(0.1, 4));
    assert_eq!(
        judged(restart(3), &lost),
        LostVerdict::Stay(Some(RetryRefusal::Exhausted))
    );
    lost.job.attempts.pop();
    assert_eq!(judged(restart(3), &lost), queued(Spend::new(0.0, false), 3));
}

#[test]
fn a_cancelled_job_is_never_started_again() {
    let mut lost = contained(
        JobBindings::default().retrying(statement(&json!({"on_interrupted": "restart"}))),
    );
    lost.job.cancel_requested = true;
    assert_eq!(judged(restart(3), &lost), LostVerdict::Stay(None));
}

/// A fired schedule's job is created `running` with its identity, under the
/// ARM ledger's claim: what follows a lost fire is its schedule's catch-up,
/// never a second run the resident decides on its own.
#[test]
fn a_job_a_schedule_fired_is_never_started_again() {
    let state = tempfile::tempdir().expect("state");
    let journals = tempfile::tempdir().expect("journals");
    let store = JobStore::open(state.path()).expect("store");
    let execution = ExecutionId::generate();
    let trace = TraceId::from(execution);
    let fired = store
        .prepare_scheduled_captured(
            IdempotencyKey::new("fired").expect("key"),
            RequestDigest::from_bytes([5; 32]),
            usize::MAX,
            "root.nika".to_owned(),
            &snapshot_body(WORKFLOW),
            super::coordinator_tests::scheduled_origin(
                nika_cadence::ScheduleOrigin::Api,
                "nightly",
                '5',
            ),
            execution.to_string(),
            trace.to_string(),
            world_digest(),
            JobBindings::default()
                .classed("none")
                .retrying(statement(&json!({"on_interrupted": "restart"}))),
            &json!({"kind": "execution.started", "status": "running"}),
        )
        .expect("fire");
    let job = admitted_record(fired);
    leave_journal(journals.path(), execution, trace, &[boot()]);
    assert_eq!(
        judge(restart(3), Some(journals.path())).judge(&job, Some(&snapshot_body(WORKFLOW))),
        LostVerdict::Stay(None)
    );
}

// ------------------------------------------------------------- the resident

/// A backend whose run writes its boot record through the resident's own
/// sink and then never returns: what a run leaves when its owner goes.
struct StartedThenLost {
    project: PathBuf,
    calls: AtomicUsize,
}

impl StartedThenLost {
    fn over(world: &TestWorld) -> Arc<Self> {
        Arc::new(Self {
            project: world.workflows.clone(),
            calls: AtomicUsize::new(0),
        })
    }

    fn journals(&self) -> PathBuf {
        self.project.join(nika_dap::store::TRACE_DIR)
    }
}

impl ExecutionBackend for StartedThenLost {
    fn execute<'a>(
        &'a self,
        context: nika_execution::ExecutionContext<'a>,
    ) -> Pin<Box<dyn Future<Output = ExecutionOutcome> + Send + 'a>> {
        leave_journal(
            &self.journals(),
            context.execution_id(),
            context.trace_id(),
            &[boot()],
        );
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(std::future::pending::<ExecutionOutcome>())
    }

    fn trace_journal_dir(&self) -> Option<PathBuf> {
        Some(self.journals())
    }
}

/// A resident that drops a run 200 ms into its stop and never times it out.
fn dropping(policy: OnInterrupted) -> ServerLimits {
    ServerLimits::new(
        1024,
        Duration::from_secs(2),
        Duration::from_secs(120),
        Duration::from_millis(200),
        4,
        16,
        64,
        32,
    )
    .with_retry_policy(RetryPolicy::new(policy, 3))
}

fn stored_jobs(world: &TestWorld) -> Vec<Value> {
    let text = std::fs::read_to_string(world.state.join("jobs/state.json")).expect("job state");
    let state: Value = serde_json::from_str(&text).expect("job state json");
    state["jobs"].as_array().expect("jobs").clone()
}

fn named(extra: &Value) -> String {
    let mut body = json!({"workflow": "root.nika"});
    for (key, value) in extra.as_object().expect("members") {
        body[key] = value.clone();
    }
    body.to_string()
}

async fn running_job(server: &super::tests::TestServer, body: &str, key: &str) -> (String, Value) {
    let created = server
        .request(&post_request(body, key, &auth_header()))
        .await;
    assert_eq!(created.status, 202, "{}", created.body);
    let id = created.json()["id"].as_str().expect("job id").to_owned();
    wait_for_status(server, &id, "running")
        .await
        .expect("running");
    let job = server
        .request(&get_request(&format!("/v1/jobs/{id}")))
        .await
        .json();
    (id, job)
}

async fn until_called(backend: &StartedThenLost) {
    for _ in 0..500 {
        if backend.calls.load(Ordering::SeqCst) > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        backend.calls.load(Ordering::SeqCst) > 0,
        "the backend entered its run"
    );
}

fn kinds_and_statuses(frames: &[Value]) -> Vec<(String, String)> {
    frames
        .iter()
        .map(|frame| {
            (
                frame["kind"].as_str().unwrap_or("").to_owned(),
                frame["status"].as_str().unwrap_or("").to_owned(),
            )
        })
        .collect()
}

fn pair(kind: &str, status: &str) -> (String, String) {
    (kind.to_owned(), status.to_owned())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_run_dropped_by_a_stop_is_started_again_by_the_next_resident() {
    let world = TestWorld::new();
    let lost = StartedThenLost::over(&world);
    let server = world
        .start(lost.clone(), dropping(OnInterrupted::Restart))
        .await;
    let (id, first) = running_job(&server, &named(&json!({})), "lost-1").await;
    let first_execution = first["execution_id"]
        .as_str()
        .expect("execution")
        .to_owned();
    until_called(&lost).await;

    // The stop outlives its grace and the resident drops the run.
    assert!(matches!(
        server.stop().await,
        Err(ServerError::ShutdownTimeout)
    ));
    let stored = stored_jobs(&world);
    let record = &stored[0]["record"];
    assert_eq!(record["status"], "queued", "{record}");
    assert_eq!(record["effects"], "none");
    assert_eq!(record["attempts"][0]["status"], "interrupted");
    assert_eq!(record["attempts"][0]["execution_id"], first_execution);
    assert_eq!(
        record["attempts"][0]["receipt"]["execution_id"],
        first_execution
    );
    assert_eq!(record["attempts"][0]["spend"], json!({"usd": 0.0}));

    // The next resident finds the job in its queue and runs it again.
    let backend = Arc::new(TestBackend::completes(ExecutionDisposition::Succeeded));
    let server = world
        .start(backend.clone(), dropping(OnInterrupted::Restart))
        .await;
    wait_for_status(&server, &id, "succeeded")
        .await
        .expect("the second attempt settles");
    assert_eq!(backend.calls(), 1);
    let job = server
        .request(&get_request(&format!("/v1/jobs/{id}")))
        .await
        .json();
    assert_eq!(job["id"], id.as_str(), "one job for both attempts");
    assert_ne!(job["execution_id"], first_execution.as_str());
    assert_eq!(job["receipt"]["execution_id"], job["execution_id"]);

    // The job's stream never shows a terminal status before its real end:
    // a client that waits for one sees the requeue and keeps waiting.
    let frames = parse_sse_data(&server.request(&events_request(&id, None)).await.body);
    assert_eq!(
        kinds_and_statuses(&frames),
        [
            pair("execution.started", "running"),
            pair("execution.requeued", "queued"),
            pair("execution.started", "running"),
            pair("execution.settled", "succeeded"),
        ],
        "{frames:?}"
    );
    assert_eq!(
        frames[1]["message"],
        "attempt 2 of 3 (restart): attempt 1 lost its owner"
    );
    for frame in &frames {
        super::test_support::assert_allowlisted(frame);
    }

    // The same key still answers the same job.
    let replay = server
        .request(&post_request(&named(&json!({})), "lost-1", &auth_header()))
        .await;
    assert_eq!(replay.status, 200, "{}", replay.body);
    assert_eq!(replay.json()["id"], id.as_str());
    server.stop().await.expect("a clean stop");
}

/// A judge that has already decided.
struct Decided(LostVerdict);

impl LostOwnerJudge for Decided {
    fn judge(&self, _job: &JobRecord, _world: Option<&str>) -> LostVerdict {
        self.0.clone()
    }
}

/// The decision that queued an attempt held under the ceiling of its day. A
/// resident restarted under a lower one fails the attempt before its backend
/// is called.
#[tokio::test(flavor = "multi_thread")]
async fn a_queued_attempt_its_ceiling_no_longer_covers_is_refused_before_it_runs() {
    let world = TestWorld::new();
    let id = {
        let store = JobStore::open(&world.state).expect("store");
        let first = store.claim_server_incarnation().expect("incarnation");
        store.settle_interrupted_jobs(&first).expect("first start");
        let admitted = store
            .create_or_replay_captured_inputs(
                IdempotencyKey::new("spent-1").expect("key"),
                RequestDigest::from_bytes([11; 32]),
                usize::MAX,
                "root.nika".to_owned(),
                &snapshot_body(WORKFLOW),
                None,
                JobBindings::default().classed("none"),
            )
            .expect("admit");
        let record = admitted_record(admitted);
        let execution = ExecutionId::generate();
        store
            .start_execution(
                record.id(),
                execution.to_string(),
                TraceId::from(execution).to_string(),
                world_digest(),
                &[json!({"kind": "execution.started", "status": "running"})],
            )
            .expect("start");
        let queued = store
            .settle_lost_job(
                record.id(),
                &first,
                &Decided(queued(Spend::new(0.5, false), 3)),
            )
            .expect("queued again");
        assert_eq!(queued.status(), JobStatus::Queued);
        record.id().as_str().to_owned()
    };

    let backend = Arc::new(TestBackend::completes(ExecutionDisposition::Succeeded));
    let server = world
        .start(
            backend.clone(),
            dropping(OnInterrupted::Restart).with_default_max_cost_usd(Some(0.3)),
        )
        .await;
    wait_for_status(&server, &id, "failed")
        .await
        .expect("the attempt is refused");
    assert_eq!(backend.calls(), 0, "the backend never ran the attempt");
    let frames = parse_sse_data(&server.request(&events_request(&id, None)).await.body);
    assert_eq!(
        kinds_and_statuses(&frames),
        [
            pair("execution.started", "running"),
            pair("execution.requeued", "queued"),
            pair("execution.refused", "failed"),
        ],
        "{frames:?}"
    );
    let refused = frames.last().expect("the refusal");
    assert_eq!(refused["code"], RetryRefusal::BudgetExhausted.code());
    for frame in &frames {
        super::test_support::assert_allowlisted(frame);
    }
    server.stop().await.expect("a clean stop");
}

/// What a killed resident leaves: a job still `running` in the store, and
/// the journal its run had written. No stop path ran.
#[tokio::test(flavor = "multi_thread")]
async fn a_job_a_killed_resident_left_running_is_started_again_at_the_next_start() {
    let world = TestWorld::new();
    let journals = world.workflows.join(nika_dap::store::TRACE_DIR);
    let id = {
        let store = JobStore::open(&world.state).expect("store");
        let killed = store.claim_server_incarnation().expect("incarnation");
        store.settle_interrupted_jobs(&killed).expect("first start");
        let admitted = store
            .create_or_replay_captured_inputs(
                IdempotencyKey::new("killed-1").expect("key"),
                RequestDigest::from_bytes([9; 32]),
                usize::MAX,
                "root.nika".to_owned(),
                &snapshot_body(WORKFLOW),
                None,
                JobBindings::default().classed("none"),
            )
            .expect("admit");
        let record = admitted_record(admitted);
        let execution = ExecutionId::generate();
        let trace = TraceId::from(execution);
        store
            .start_execution(
                record.id(),
                execution.to_string(),
                trace.to_string(),
                world_digest(),
                &[json!({"kind": "execution.started", "status": "running"})],
            )
            .expect("start");
        leave_journal(&journals, execution, trace, &[boot()]);
        record.id().as_str().to_owned()
    };

    // The next resident judges the lost run with its own journal reader.
    let backend = Arc::new(JournalingBackend {
        inner: TestBackend::completes(ExecutionDisposition::Succeeded),
        journals,
    });
    let server = world
        .start(backend.clone(), dropping(OnInterrupted::Restart))
        .await;
    wait_for_status(&server, &id, "succeeded")
        .await
        .expect("the second attempt settles");
    assert_eq!(backend.inner.calls(), 1);
    let frames = parse_sse_data(&server.request(&events_request(&id, None)).await.body);
    assert_eq!(
        kinds_and_statuses(&frames),
        [
            pair("execution.started", "running"),
            pair("execution.requeued", "queued"),
            pair("execution.started", "running"),
            pair("execution.settled", "succeeded"),
        ],
        "{frames:?}"
    );
    let record = &stored_jobs(&world)[0]["record"];
    assert_eq!(record["attempts"].as_array().expect("attempts").len(), 1);
    server.stop().await.expect("a clean stop");
}

/// The test backend, naming the directory its runs' journals live in.
struct JournalingBackend {
    inner: TestBackend,
    journals: PathBuf,
}

impl ExecutionBackend for JournalingBackend {
    fn execute<'a>(
        &'a self,
        context: nika_execution::ExecutionContext<'a>,
    ) -> Pin<Box<dyn Future<Output = ExecutionOutcome> + Send + 'a>> {
        self.inner.execute(context)
    }

    fn trace_journal_dir(&self) -> Option<PathBuf> {
        Some(self.journals.clone())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn without_a_policy_a_dropped_run_is_interrupted_as_before() {
    let world = TestWorld::new();
    let lost = StartedThenLost::over(&world);
    let server = world
        .start(lost.clone(), dropping(OnInterrupted::Off))
        .await;
    let (_id, _job) = running_job(&server, &named(&json!({})), "lost-off").await;
    until_called(&lost).await;
    assert!(matches!(
        server.stop().await,
        Err(ServerError::ShutdownTimeout)
    ));
    let stored = stored_jobs(&world);
    let record = stored[0]["record"].as_object().expect("record");
    assert_eq!(record["status"], "interrupted");
    assert!(!record.contains_key("attempts"));
    let interruption = stored[0]["events"]
        .as_array()
        .expect("events")
        .last()
        .expect("the interruption")
        .clone();
    assert_eq!(
        interruption["payload"],
        json!({"kind": "execution.interrupted", "status": "interrupted"})
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_job_cancelled_while_it_ran_is_not_started_again() {
    let world = TestWorld::new();
    let lost = StartedThenLost::over(&world);
    let server = world
        .start(lost.clone(), dropping(OnInterrupted::Restart))
        .await;
    let (id, _job) = running_job(&server, &named(&json!({})), "lost-cancelled").await;
    until_called(&lost).await;
    let cancelled = server.request(&cancel_request(&id)).await;
    assert!(
        matches!(cancelled.status, 200 | 202),
        "{}: {}",
        cancelled.status,
        cancelled.body
    );
    // The resident stops before the cancelled run settles.
    let _stopped = server.stop().await;
    let stored = stored_jobs(&world);
    let record = &stored[0]["record"];
    assert_eq!(record["cancel_requested"], true, "{record}");
    assert_eq!(record["status"], "interrupted");
    assert!(record.get("attempts").is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_review_reference_and_a_retry_statement_do_not_meet() {
    let world = TestWorld::new();
    let backend = Arc::new(TestBackend::completes(ExecutionDisposition::Succeeded));
    let server = world.start(backend, limits()).await;
    let body = named(&json!({
        "cost_review": {"review_id": "review-1", "witness_sha256": "0".repeat(64)},
        "retry": {},
    }));
    let response = server
        .request(&post_request(&body, "reviewed-retry", &auth_header()))
        .await;
    assert_eq!(response.status, 422, "{}", response.body);
    assert_eq!(response.json()["error"]["code"], "invalid_job_retry");
    assert!(stored_jobs(&world).is_empty());
    server.stop().await.expect("a clean stop");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_retry_statement_is_judged_at_the_door_and_bound_to_the_key() {
    let world = TestWorld::new();
    let backend = Arc::new(TestBackend::completes(ExecutionDisposition::Succeeded));
    let server = world.start(backend, limits()).await;
    for refused in [
        json!({"retry": null}),
        json!({"retry": "restart"}),
        json!({"retry": {"on_interrupted": "always"}}),
        json!({"retry": {"max_attempts": 0}}),
        json!({"retry": {"max_attempts": 17}}),
        json!({"retry": {"max_cost_usd": -1}}),
        json!({"retry": {"mode": "restart"}}),
    ] {
        let response = server
            .request(&post_request(
                &named(&refused),
                "stated-refused",
                &auth_header(),
            ))
            .await;
        assert_eq!(response.status, 422, "{refused}: {}", response.body);
        assert_eq!(response.json()["error"]["code"], "invalid_job_retry");
    }
    assert!(stored_jobs(&world).is_empty(), "a refusal admits nothing");

    let stated = named(&json!({"retry": {"on_interrupted": "restart", "max_attempts": 2}}));
    let created = server
        .request(&post_request(&stated, "stated-1", &auth_header()))
        .await;
    assert_eq!(created.status, 202, "{}", created.body);
    assert!(
        created.json().get("retry").is_none(),
        "the published Job object stays closed"
    );
    let stored = stored_jobs(&world);
    assert_eq!(
        stored[0]["record"]["retry"],
        json!({"on_interrupted": "restart", "max_attempts": 2})
    );
    // The statement is part of the request: the same key with another one conflicts.
    let other = named(&json!({"retry": {"on_interrupted": "restart", "max_attempts": 3}}));
    let conflict = server
        .request(&post_request(&other, "stated-1", &auth_header()))
        .await;
    assert_eq!(conflict.status, 409, "{}", conflict.body);
    server.stop().await.expect("a clean stop");
}
