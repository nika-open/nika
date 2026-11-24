// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! The job list read from the store (ADR-155): order, dated windows, origin
//! and pages that hold while admissions race.

use nika_cadence::firing::{ArmGeneration, SlotId};
use nika_cadence::{ScheduleDecision, ScheduleOrigin, ScheduleRevision};

use super::*;
use crate::job::{JobBindings, JobListFilter, JobPage, OriginKind};

/// A clock a test moves by hand.
#[derive(Debug)]
struct StepClock(Mutex<jiff::Zoned>);

impl StepClock {
    fn at(instant: &str) -> Arc<Self> {
        Arc::new(Self(Mutex::new(zoned(instant))))
    }

    fn set(&self, instant: &str) {
        *self.0.lock().expect("clock") = zoned(instant);
    }
}

impl crate::ResidentClock for StepClock {
    fn now(&self) -> jiff::Zoned {
        self.0.lock().expect("clock").clone()
    }

    fn sleep(
        &self,
        _: std::time::Duration,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        Box::pin(std::future::ready(()))
    }
}

fn instant(text: &str) -> jiff::Timestamp {
    text.parse().expect("instant")
}

fn zoned(text: &str) -> jiff::Zoned {
    instant(text).to_zoned(jiff::tz::TimeZone::UTC)
}

fn admit(store: &JobStore, name: &str, workflow: &str) -> JobRecord {
    let request = RequestDigest::from_bytes(Sha256::digest(name.as_bytes()).into());
    admitted_record(
        store
            .create_or_replay_captured_inputs(
                key(name),
                request,
                usize::MAX,
                workflow.to_owned(),
                "{}",
                None,
                JobBindings::default(),
            )
            .expect("admit"),
    )
}

fn all(store: &JobStore, filter: &JobListFilter) -> JobPage {
    store.list(filter, 200, None).expect("list")
}

fn ids(page: &JobPage) -> Vec<JobId> {
    page.jobs.iter().map(|job| job.id.clone()).collect()
}

#[test]
fn a_dated_window_places_each_job_by_its_admission() {
    let root = tempfile::tempdir().expect("root");
    let clock = StepClock::at("2026-11-16T09:00:00Z");
    let store = JobStore::open(root.path())
        .expect("store")
        .with_clock(clock.clone());
    let monday = admit(&store, "monday", "root.nika");
    clock.set("2026-11-18T09:00:00Z");
    let wednesday = admit(&store, "wednesday", "other.nika");
    clock.set("2026-11-20T09:00:00Z");
    let friday = admit(&store, "friday", "root.nika");

    let everything = all(&store, &JobListFilter::default());
    assert_eq!(
        ids(&everything),
        [friday.id(), wednesday.id(), monday.id()].map(Clone::clone)
    );
    assert_eq!(everything.next, None);
    assert_eq!(
        everything.jobs[0].created_at.as_deref(),
        Some("2026-11-20T09:00:00Z")
    );

    let window = |after: Option<&str>, before: Option<&str>| JobListFilter {
        created_after: after.map(instant),
        created_before: before.map(instant),
        ..JobListFilter::default()
    };
    for (after, before, expected) in [
        (
            Some("2026-11-16T09:00:00Z"),
            None,
            vec![&friday, &wednesday],
        ),
        (
            Some("2026-11-16T08:59:59Z"),
            None,
            vec![&friday, &wednesday, &monday],
        ),
        (
            None,
            Some("2026-11-20T09:00:00Z"),
            vec![&wednesday, &monday],
        ),
        (
            Some("2026-11-16T09:00:00Z"),
            Some("2026-11-20T09:00:00Z"),
            vec![&wednesday],
        ),
        (Some("2026-11-20T09:00:00Z"), None, vec![]),
    ] {
        let expected: Vec<JobId> = expected.into_iter().map(|job| job.id().clone()).collect();
        assert_eq!(
            ids(&all(&store, &window(after, before))),
            expected,
            "after {after:?} before {before:?}: both bounds are exclusive"
        );
    }

    let other = JobListFilter {
        workflow: Some("other.nika".to_owned()),
        ..JobListFilter::default()
    };
    assert_eq!(ids(&all(&store, &other)), [wednesday.id().clone()]);
}

/// A format-3 record carries no admission date. The list places it by its
/// first dated event, and leaves it out of a dated listing when it has none:
/// it never invents an instant.
#[test]
fn a_job_admitted_before_admissions_were_dated_is_placed_by_its_first_dated_event() {
    let root = tempfile::tempdir().expect("root");
    let clock = StepClock::at("2026-11-16T09:00:00Z");
    let store = JobStore::open(root.path())
        .expect("store")
        .with_clock(clock.clone());
    let started = admit(&store, "started", "root.nika");
    let waiting = admit(&store, "waiting", "root.nika");
    clock.set("2026-11-16T09:05:00Z");
    stamp_execution(&store, started.id(), 7);
    transition(&store, started.id(), JobStatus::Running);
    drop(store);

    let path = root.path().join("jobs/state.json");
    let mut state: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("state")).expect("json");
    state["version"] = json!(3);
    state.as_object_mut().expect("state").remove("next_ordinal");
    for job in state["jobs"].as_array_mut().expect("jobs") {
        let record = job["record"].as_object_mut().expect("record");
        record.remove("ordinal");
        record.remove("created_at");
    }
    std::fs::write(&path, format!("{state}\n")).expect("format-3 state");

    let store = JobStore::open(root.path()).expect("the format-3 store migrates");
    let everything = all(&store, &JobListFilter::default());
    assert_eq!(
        ids(&everything),
        [waiting.id(), started.id()].map(Clone::clone),
        "a migrated store is ranked in its stored order"
    );
    assert_eq!(everything.jobs[0].created_at, None);
    assert_eq!(
        everything.jobs[1].created_at.as_deref(),
        Some("2026-11-16T09:05:00Z"),
        "the first dated event stands in for the admission date"
    );
    let dated = JobListFilter {
        created_after: Some(instant("2026-11-16T09:00:00Z")),
        ..JobListFilter::default()
    };
    assert_eq!(
        ids(&all(&store, &dated)),
        [started.id().clone()],
        "an undated job is left out of a dated listing"
    );
}

#[test]
fn a_scheduled_job_is_told_from_a_submitted_one() {
    let root = tempfile::tempdir().expect("root");
    let store = JobStore::open(root.path()).expect("store");
    let submitted = admit(&store, "submitted", "root.nika");
    let revision =
        ScheduleRevision::from_wire(&format!("sha256:{}", "a".repeat(64))).expect("revision");
    let slot = SlotId::from_wire(&"b".repeat(64)).expect("slot id");
    let generation = ArmGeneration::from_wire(&"c".repeat(64)).expect("generation");
    let origin = JobOrigin::schedule(
        ScheduleOrigin::Api,
        "daily",
        &revision,
        &slot,
        ScheduleDecision::Scheduled,
        instant("2026-11-20T10:00:00Z"),
        instant("2026-11-20T10:00:01Z"),
        &generation,
    )
    .expect("scheduled origin");
    let fired = admitted_record(
        store
            .prepare_scheduled_captured(
                key("slot-b"),
                digest(9),
                usize::MAX,
                "root.nika".to_owned(),
                "{}",
                origin,
                "execution-9".to_owned(),
                "trace-9".to_owned(),
                digest(9).as_str().to_owned(),
                JobBindings::default(),
                &json!({"status": "running"}),
            )
            .expect("scheduled admission"),
    );

    let of = |origin| JobListFilter {
        origin: Some(origin),
        ..JobListFilter::default()
    };
    assert_eq!(
        ids(&all(&store, &of(OriginKind::Schedule))),
        [fired.id().clone()]
    );
    assert_eq!(
        ids(&all(&store, &of(OriginKind::Manual))),
        [submitted.id().clone()]
    );
    let running = JobListFilter {
        statuses: vec![JobStatus::Running, JobStatus::Paused],
        ..JobListFilter::default()
    };
    let page = all(&store, &running);
    assert_eq!(ids(&page), [fired.id().clone()]);
    assert_eq!(page.jobs[0].execution_id.as_deref(), Some("execution-9"));
    assert_eq!(page.jobs[0].trace_id.as_deref(), Some("trace-9"));
    assert!(matches!(page.jobs[0].origin, JobOrigin::Schedule { .. }));
}

/// While admissions race a caller that pages, every job admitted before the
/// first page is served exactly once, newest first: a new admission is newer
/// than every page still to come.
#[test]
fn pages_hold_every_older_job_once_while_admissions_race() {
    let root = tempfile::tempdir().expect("root");
    let store = JobStore::open(root.path()).expect("store");
    let before: Vec<JobId> = (0..30)
        .map(|index| {
            admit(&store, &format!("before-{index}"), "root.nika")
                .id()
                .clone()
        })
        .collect();
    let writer = JobStore::open(root.path()).expect("second handle");
    let barrier = Arc::new(Barrier::new(2));

    let served = std::thread::scope(|scope| {
        let gate = Arc::clone(&barrier);
        let racing = scope.spawn(move || {
            gate.wait();
            for index in 0..20 {
                admit(&writer, &format!("racing-{index}"), "root.nika");
            }
        });
        let first = store
            .list(&JobListFilter::default(), 7, None)
            .expect("first page");
        barrier.wait();
        let mut served = first.jobs;
        let mut next = first.next;
        while let Some(cursor) = next {
            let page = store
                .list(&JobListFilter::default(), 7, Some(cursor))
                .expect("next page");
            assert!(page.jobs.len() <= 7);
            served.extend(page.jobs);
            next = page.next;
        }
        racing.join().expect("racing admissions");
        served
    });

    let expected: Vec<JobId> = before.iter().rev().cloned().collect();
    assert_eq!(
        served.iter().map(|job| job.id.clone()).collect::<Vec<_>>(),
        expected,
        "no job repeats, none is skipped, none admitted later slips in"
    );
    assert!(
        served
            .windows(2)
            .all(|pair| pair[0].ordinal > pair[1].ordinal),
        "ranks strictly decrease"
    );
    let total = store
        .list(&JobListFilter::default(), 200, None)
        .expect("everything");
    assert_eq!(total.jobs.len(), 50);
    assert_eq!(total.next, None);

    let one = store
        .list(&JobListFilter::default(), 1, Some(2))
        .expect("the oldest job");
    assert_eq!(ids(&one), [before[0].clone()]);
    assert_eq!(one.next, None, "nothing is older than the first admission");
    let none = store
        .list(&JobListFilter::default(), 1, Some(1))
        .expect("past the end");
    assert!(none.jobs.is_empty());
}
