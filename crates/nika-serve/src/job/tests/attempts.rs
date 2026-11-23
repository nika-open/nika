// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! The attempts of one job in the store (ADR-156): a run that lost its
//! owner is queued again or left `interrupted` in the one write that records
//! the loss, the attempt it left stays whole and hash-bound, and a job no
//! policy starts again settles as it did before attempts existed, its
//! record carrying only the class every door keeps.

use super::*;
use crate::job::{
    AttemptMode, JobBindings, LostOwnerJudge, LostVerdict, RetryRefusal, RetryStatement, Spend,
};

/// A judge that answers what a test tells it to.
struct Fixed(LostVerdict);

impl LostOwnerJudge for Fixed {
    fn judge(&self, _job: &JobRecord, _world: Option<&str>) -> LostVerdict {
        self.0.clone()
    }
}

fn again(usd: f64) -> Fixed {
    Fixed(LostVerdict::Queue {
        mode: AttemptMode::Restart,
        spend: Spend::new(usd, false),
        bound: 3,
    })
}

fn admit(store: &JobStore, name: &str, byte: u8, bindings: JobBindings) -> JobRecord {
    admitted_record(
        store
            .create_or_replay_captured_inputs(
                key(name),
                digest(byte),
                usize::MAX,
                "root.nika".to_owned(),
                "{}",
                None,
                bindings,
            )
            .expect("admit"),
    )
}

fn start(store: &JobStore, id: &JobId, byte: u8) {
    store
        .start_execution(
            id,
            format!("execution-{byte}"),
            format!("trace-{byte}"),
            digest(byte).as_str().to_owned(),
            &[json!({"kind": "execution.started", "status": "running"})],
        )
        .expect("start");
}

fn events(store: &JobStore, id: &JobId) -> Vec<JobEvent> {
    store
        .events_after(id, 0, page_limit(MAX_EVENT_PAGE_LEN))
        .expect("events")
}

fn read_state(root: &std::path::Path) -> serde_json::Value {
    let text = std::fs::read_to_string(root.join("jobs/state.json")).expect("state");
    serde_json::from_str(&text).expect("state json")
}

fn write_state(root: &std::path::Path, state: &serde_json::Value) {
    let mut text = serde_json::to_string(state).expect("encode");
    text.push('\n');
    std::fs::write(root.join("jobs/state.json"), text).expect("write state");
}

/// One job, started by one incarnation that is then gone.
fn lost_run(root: &std::path::Path, bindings: JobBindings) -> JobId {
    let store = JobStore::open(root).expect("store");
    let first = store.claim_server_incarnation().expect("first incarnation");
    store
        .settle_interrupted_jobs(&first)
        .expect("nothing to settle");
    let record = admit(&store, "request-lost", 21, bindings);
    start(&store, record.id(), 21);
    record.id().clone()
}

#[test]
fn a_lost_run_is_queued_again_in_the_write_that_records_its_loss() {
    let root = tempfile::tempdir().expect("root");
    let id = lost_run(root.path(), JobBindings::default());

    let store = JobStore::open(root.path()).expect("reopen");
    let second = store
        .claim_server_incarnation()
        .expect("second incarnation");
    let left_interrupted = store
        .settle_lost_jobs(&second, &again(0.25))
        .expect("settle");
    assert_eq!(left_interrupted, 0, "the job was queued, not interrupted");

    let record = store.get(&id).expect("get").expect("job");
    assert_eq!(record.status(), JobStatus::Queued);
    assert_eq!(
        record.execution_id(),
        None,
        "the new attempt has no identity yet"
    );
    assert_eq!(record.trace_id(), None);
    assert_eq!(record.receipt(), None);
    assert_eq!(record.attempt(), 2);

    // The attempt the job left is whole: identity, interrupted receipt, spend.
    let closed = &record.attempts[0];
    assert_eq!(closed.status, JobStatus::Interrupted);
    assert_eq!(closed.execution_id, "execution-21");
    assert_eq!(closed.trace_id, "trace-21");
    let receipt = closed.receipt.as_ref().expect("the interrupted receipt");
    assert_eq!(receipt.execution_id(), "execution-21");
    assert_eq!(receipt.job_id(), &id);
    assert!(closed.identity_digest.is_some());
    assert_eq!(closed.spend, Spend::new(0.25, false));
    assert_eq!(closed.next, AttemptMode::Restart);

    // The stream carries no interrupted frame: a client that waits for a
    // terminal status keeps waiting for the new attempt.
    let events = events(&store, &id);
    let last = events.last().expect("the requeue event");
    assert_eq!(last.payload()["kind"], "execution.requeued");
    assert_eq!(last.payload()["status"], "queued");
    assert_eq!(
        last.payload()["message"],
        "attempt 2 of 3 (restart): attempt 1 lost its owner"
    );
    assert_eq!(closed.requeued_sequence, last.sequence());
    assert!(
        events
            .iter()
            .all(|event| event.payload()["status"] != "interrupted"),
        "{events:?}"
    );

    // The same key answers the same job, whatever its attempt.
    let replay = store
        .create_or_replay(key("request-lost"), digest(21))
        .expect("replay");
    assert!(matches!(replay, Admission::Existing(existing) if existing.id() == &id));
}

#[test]
fn a_new_attempt_takes_its_own_identity_and_the_history_still_validates() {
    let root = tempfile::tempdir().expect("root");
    let id = lost_run(root.path(), JobBindings::default());
    let store = JobStore::open(root.path()).expect("reopen");
    let second = store
        .claim_server_incarnation()
        .expect("second incarnation");
    store
        .settle_lost_jobs(&second, &again(0.0))
        .expect("settle");

    // The closed attempt's identity is never claimed again.
    for (execution, trace) in [("execution-21", "trace-22"), ("execution-22", "trace-21")] {
        assert!(
            matches!(
                store.start_execution(
                    &id,
                    execution.to_owned(),
                    trace.to_owned(),
                    digest(22).as_str().to_owned(),
                    &[json!({"kind": "execution.started", "status": "running"})],
                ),
                Err(JobStoreError::InvalidReceipt)
            ),
            "{execution} {trace}"
        );
    }
    start(&store, &id, 22);
    let receipt = JobReceipt::new(
        id.clone(),
        "execution-22",
        "trace-22",
        digest(22).as_str(),
        None,
    )
    .expect("receipt");
    store
        .settle_with_events(
            &id,
            JobStatus::Succeeded,
            &[json!({"kind": "execution.settled", "status": "succeeded"})],
            Some(std::collections::BTreeMap::new()),
            Some(receipt),
        )
        .expect("the second attempt settles");
    drop(second);
    drop(store);

    // Every hash of both attempts is recomputed from the stored state.
    let reopened = JobStore::open(root.path()).expect("the history validates");
    let record = reopened.get(&id).expect("get").expect("job");
    assert_eq!(record.status(), JobStatus::Succeeded);
    assert_eq!(record.execution_id(), Some("execution-22"));
    assert_eq!(record.attempts.len(), 1);
    assert_eq!(record.attempts[0].execution_id, "execution-21");
    let kinds: Vec<String> = events(&reopened, &id)
        .iter()
        .map(|event| event.payload()["kind"].as_str().unwrap_or("").to_owned())
        .collect();
    assert_eq!(
        kinds,
        [
            "execution.started",
            "execution.requeued",
            "execution.started",
            "execution.settled"
        ]
    );
}

#[test]
fn a_refused_retry_leaves_the_job_interrupted_and_says_why() {
    for refusal in [
        RetryRefusal::Exhausted,
        RetryRefusal::EffectsNeedConsent,
        RetryRefusal::UnknownSpend,
        RetryRefusal::BudgetExhausted,
    ] {
        let root = tempfile::tempdir().expect("root");
        let id = lost_run(root.path(), JobBindings::default());
        let store = JobStore::open(root.path()).expect("reopen");
        let second = store
            .claim_server_incarnation()
            .expect("second incarnation");
        let left = store
            .settle_lost_jobs(&second, &Fixed(LostVerdict::Stay(Some(refusal))))
            .expect("settle");
        assert_eq!(left, 1);
        let record = store.get(&id).expect("get").expect("job");
        assert_eq!(record.status(), JobStatus::Interrupted);
        assert!(record.attempts.is_empty());
        assert_eq!(
            record.error(),
            None,
            "the reason is the event's, the job did not fail"
        );
        let events = events(&store, &id);
        let last = events.last().expect("the interruption");
        assert_eq!(last.payload()["kind"], "interrupted");
        assert_eq!(last.payload()["status"], "interrupted");
        assert_eq!(last.payload()["code"], refusal.code());
        assert_eq!(last.payload()["message"], refusal.message());
    }
}

#[test]
fn a_store_no_policy_was_stated_for_settles_as_it_always_did() {
    let root = tempfile::tempdir().expect("root");
    // Classed, as every door that admits a job classes it.
    let id = lost_run(root.path(), JobBindings::default().classed("none"));
    let store = JobStore::open(root.path()).expect("reopen");
    let second = store
        .claim_server_incarnation()
        .expect("second incarnation");
    assert_eq!(store.settle_interrupted_jobs(&second).expect("settle"), 1);
    assert_eq!(
        store.settle_interrupted_jobs(&second).expect("settle once"),
        0,
        "one pass per incarnation"
    );
    let record = store.get(&id).expect("get").expect("job");
    assert_eq!(record.status(), JobStatus::Interrupted);
    let events = events(&store, &id);
    let payload = events.last().expect("the interruption").payload();
    let mut keys: Vec<&str> = payload
        .as_object()
        .expect("payload")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "incarnation_generation",
            "kind",
            "previous_incarnation_generation",
            "status"
        ],
        "no key was added to the interruption of a job nobody retries"
    );
    // Nothing of attempts is written on a job that never needs it: the
    // class kept at admission is the one member it gains.
    let state = read_state(root.path());
    let stored = state["jobs"][0]["record"].as_object().expect("record");
    for member in ["retry", "cancel_requested", "attempts"] {
        assert!(!stored.contains_key(member), "{member} is absent");
    }
    assert_eq!(stored.get("effects"), Some(&serde_json::json!("none")));
}

#[test]
fn a_run_dropped_at_a_stop_is_settled_by_the_same_decision() {
    let root = tempfile::tempdir().expect("root");
    let store = JobStore::open(root.path()).expect("store");
    let live = store.claim_server_incarnation().expect("incarnation");
    store
        .settle_interrupted_jobs(&live)
        .expect("nothing to settle");
    let queued_again = admit(&store, "request-stop-a", 31, JobBindings::default());
    let stays = admit(&store, "request-stop-b", 32, JobBindings::default());
    start(&store, queued_again.id(), 31);
    start(&store, stays.id(), 32);

    let record = store
        .settle_lost_job(queued_again.id(), &live, &again(0.0))
        .expect("the stop queues it");
    assert_eq!(record.status(), JobStatus::Queued);
    let last = events(&store, queued_again.id());
    let payload = last.last().expect("requeue").payload();
    assert_eq!(payload["kind"], "execution.requeued");
    assert!(
        payload.get("incarnation_generation").is_none(),
        "a stop crosses no incarnation"
    );

    let record = store
        .settle_lost_job(stays.id(), &live, &Fixed(LostVerdict::Stay(None)))
        .expect("the stop interrupts it");
    assert_eq!(record.status(), JobStatus::Interrupted);
    let last = events(&store, stays.id());
    assert_eq!(
        last.last().expect("interruption").payload(),
        &json!({"kind": "execution.interrupted", "status": "interrupted"}),
        "the interruption a stop has always written"
    );

    // A job that is not running is not this door's to settle.
    assert!(matches!(
        store.settle_lost_job(stays.id(), &live, &again(0.0)),
        Err(JobStoreError::IllegalTransition { .. })
    ));
}

#[test]
fn a_stated_retry_and_the_effects_class_are_durable_and_bound() {
    let root = tempfile::tempdir().expect("root");
    let store = JobStore::open(root.path()).expect("store");
    let statement =
        RetryStatement::stated(&json!({"on_interrupted": "restart", "max_attempts": 2}))
            .expect("a legal statement");
    let record = admit(
        &store,
        "request-stated",
        41,
        JobBindings::default()
            .retrying(Some(statement.clone()))
            .classed("none"),
    );
    assert_eq!(record.retry, Some(statement));
    assert_eq!(record.effects.as_deref(), Some("none"));
    assert_eq!(
        events(&store, record.id()).len(),
        1,
        "a stated retry is bound by an admission event before the job runs"
    );
    drop(store);

    // The statement is in the hash: another one is not the job that was admitted.
    for (member, forged) in [
        (
            "retry",
            json!({"on_interrupted": "restart", "max_attempts": 9}),
        ),
        ("effects", json!("external")),
    ] {
        let mut state = read_state(root.path());
        let original = state.clone();
        state["jobs"][0]["record"][member] = forged;
        write_state(root.path(), &state);
        assert!(
            matches!(JobStore::open(root.path()), Err(JobStoreError::Corrupt(_))),
            "a forged {member} is refused"
        );
        write_state(root.path(), &original);
    }
    JobStore::open(root.path()).expect("the untouched state reads back");
}

#[test]
fn a_closed_attempt_cannot_be_rewritten() {
    let root = tempfile::tempdir().expect("root");
    let id = lost_run(root.path(), JobBindings::default());
    {
        let store = JobStore::open(root.path()).expect("reopen");
        let second = store
            .claim_server_incarnation()
            .expect("second incarnation");
        store
            .settle_lost_jobs(&second, &again(0.5))
            .expect("settle");
        assert_eq!(store.get(&id).expect("get").expect("job").attempt(), 2);
    }
    let original = read_state(root.path());
    let forgeries: [(&str, serde_json::Value); 4] = [
        ("execution_id", json!("execution-99")),
        ("spend", json!({"usd": 0.0})),
        ("status", json!("succeeded")),
        ("requeued_sequence", json!(1)),
    ];
    for (member, forged) in forgeries {
        let mut state = original.clone();
        state["jobs"][0]["record"]["attempts"][0][member] = forged;
        write_state(root.path(), &state);
        assert!(
            matches!(JobStore::open(root.path()), Err(JobStoreError::Corrupt(_))),
            "a forged {member} is refused"
        );
    }
    // Dropping the history is refused too: the requeue event bound it.
    let mut state = original.clone();
    state["jobs"][0]["record"]
        .as_object_mut()
        .expect("record")
        .remove("attempts");
    write_state(root.path(), &state);
    assert!(matches!(
        JobStore::open(root.path()),
        Err(JobStoreError::Corrupt(_))
    ));
    write_state(root.path(), &original);
    JobStore::open(root.path()).expect("the untouched state reads back");
}

#[test]
fn an_accepted_cancellation_is_durable_while_the_job_runs() {
    let root = tempfile::tempdir().expect("root");
    let store = JobStore::open(root.path()).expect("store");
    let record = admit(&store, "request-cancel", 51, JobBindings::default());
    assert!(
        !store.mark_cancel_requested(record.id()).expect("mark"),
        "a queued job is cancelled by its own door"
    );
    start(&store, record.id(), 51);
    assert!(store.mark_cancel_requested(record.id()).expect("mark"));
    assert!(
        store
            .mark_cancel_requested(record.id())
            .expect("mark twice")
    );
    drop(store);
    let reopened = JobStore::open(root.path()).expect("reopen");
    let record = reopened.get(record.id()).expect("get").expect("job");
    assert!(record.cancel_requested);
    assert_eq!(record.status(), JobStatus::Running);
}
