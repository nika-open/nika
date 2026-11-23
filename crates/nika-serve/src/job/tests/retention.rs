// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! Retention of settled jobs (ADR-155): what an admission and a startup
//! evict, what they never touch, and what an eviction leaves behind.

use std::time::Duration;

use super::*;
use crate::job::{JobBindings, JobListFilter, JobRetention};

const HOUR: Duration = Duration::from_secs(3_600);
const DAY: Duration = Duration::from_secs(86_400);

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

fn zoned(text: &str) -> jiff::Zoned {
    text.parse::<jiff::Timestamp>()
        .expect("instant")
        .to_zoned(jiff::tz::TimeZone::UTC)
}

fn store(root: &std::path::Path, clock: &Arc<StepClock>, retention: JobRetention) -> JobStore {
    JobStore::open(root)
        .expect("store")
        .with_clock(clock.clone())
        .with_retention(retention)
}

fn windows(succeeded: Duration, unsuccessful: Duration) -> JobRetention {
    JobRetention {
        succeeded: Some(succeeded),
        unsuccessful: Some(unsuccessful),
    }
}

fn request(name: &str) -> RequestDigest {
    RequestDigest::from_bytes(Sha256::digest(name.as_bytes()).into())
}

fn admit_within(store: &JobStore, name: &str, max_jobs: usize) -> Result<Admission, JobStoreError> {
    store.create_or_replay_captured_inputs(
        key(name),
        request(name),
        max_jobs,
        "root.nika".to_owned(),
        "{}",
        None,
        JobBindings::default(),
    )
}

fn admit(store: &JobStore, name: &str) -> JobRecord {
    admitted_record(admit_within(store, name, usize::MAX).expect("admit"))
}

/// Admit one job and settle it as `status`.
fn settled(store: &JobStore, name: &str, byte: u8, status: JobStatus) -> JobRecord {
    let record = admit(store, name);
    stamp_execution(store, record.id(), byte);
    transition(store, record.id(), JobStatus::Running);
    let receipt = JobReceipt::new(
        record.id().clone(),
        format!("execution-{byte}"),
        format!("trace-{byte}"),
        digest(byte).as_str().to_owned(),
        None,
    )
    .expect("receipt");
    store
        .settle_with_events(
            record.id(),
            status,
            &[json!({"status": status.to_string()})],
            None,
            Some(receipt),
        )
        .expect("settle");
    record
}

fn kept(store: &JobStore) -> Vec<JobId> {
    let page = store
        .list(&JobListFilter::default(), 200, None)
        .expect("list");
    page.jobs.into_iter().rev().map(|job| job.id).collect()
}

fn world_exists(root: &std::path::Path, id: &JobId) -> bool {
    root.join("jobs")
        .join(format!("{}.world", id.as_str()))
        .exists()
}

#[test]
fn an_admission_evicts_the_settled_jobs_that_outlived_their_window() {
    let root = tempfile::tempdir().expect("root");
    let clock = StepClock::at("2026-11-16T10:00:00Z");
    let store = store(root.path(), &clock, windows(HOUR, DAY));
    let succeeded = settled(&store, "succeeded", 1, JobStatus::Succeeded);
    let failed = settled(&store, "failed", 2, JobStatus::Failed);
    let cancelled = settled(&store, "cancelled", 3, JobStatus::Cancelled);
    let waiting = admit(&store, "waiting");
    let running = admit(&store, "running");
    stamp_execution(&store, running.id(), 5);
    transition(&store, running.id(), JobStatus::Running);
    assert!(world_exists(root.path(), succeeded.id()));

    clock.set("2026-11-16T10:59:59Z");
    let early = admit(&store, "early");
    assert_eq!(
        kept(&store),
        [&succeeded, &failed, &cancelled, &waiting, &running, &early].map(|job| job.id().clone()),
        "nothing leaves before its window ends"
    );

    clock.set("2026-11-16T11:00:00Z");
    let hourly = admit(&store, "hourly");
    assert_eq!(
        kept(&store),
        [&failed, &cancelled, &waiting, &running, &early, &hourly].map(|job| job.id().clone()),
        "the succeeded job left; unsuccessful and owned jobs stay"
    );
    assert!(store.get(succeeded.id()).expect("read").is_none());
    assert!(
        !world_exists(root.path(), succeeded.id()),
        "the captured world leaves with its job"
    );
    assert!(world_exists(root.path(), failed.id()));

    // The evicted job's key is forgotten: the same request is a new job.
    let again = admit_within(&store, "succeeded", usize::MAX).expect("admit");
    assert!(
        matches!(again, Admission::Created(_)),
        "an evicted key creates a job: {again:?}"
    );
    let again = admitted_record(again);
    assert_ne!(again.id(), succeeded.id());
    assert!(
        again.ordinal > hourly.ordinal,
        "a rank is never given twice, even after an eviction"
    );
    // A job that is still kept replays as before.
    let replay = admit_within(&store, "failed", usize::MAX).expect("replay");
    assert!(matches!(replay, Admission::Existing(record) if record.id() == failed.id()));

    clock.set("2026-11-17T10:00:00Z");
    let daily = admit(&store, "daily");
    assert_eq!(
        kept(&store),
        [&waiting, &running, &early, &hourly, &again, &daily].map(|job| job.id().clone()),
        "a day later the failed and the cancelled job left; \
         a queued and a running job never leave, however old"
    );
    drop(store);
    JobStore::open(root.path()).expect("the store an eviction wrote validates");
}

#[test]
fn an_eviction_frees_the_place_a_full_store_refused() {
    let root = tempfile::tempdir().expect("root");
    let clock = StepClock::at("2026-11-16T10:00:00Z");
    let kept_for_ever = store(root.path(), &clock, JobRetention::default());
    settled(&kept_for_ever, "first", 1, JobStatus::Succeeded);
    settled(&kept_for_ever, "second", 2, JobStatus::Failed);
    clock.set("2030-01-01T00:00:00Z");
    assert!(
        matches!(
            admit_within(&kept_for_ever, "third", 2),
            Err(JobStoreError::CapacityExceeded)
        ),
        "without a window a full store refuses for ever"
    );
    assert_eq!(kept(&kept_for_ever).len(), 2);
    drop(kept_for_ever);

    let retaining = store(root.path(), &clock, windows(HOUR, HOUR));
    let third = admit_within(&retaining, "third", 2).expect("the window freed a place");
    assert_eq!(kept(&retaining), [third.record().id().clone()]);
}

#[test]
fn a_startup_evicts_and_removes_the_worlds_no_job_owns() {
    let root = tempfile::tempdir().expect("root");
    let clock = StepClock::at("2026-11-16T10:00:00Z");
    let store = store(root.path(), &clock, windows(HOUR, DAY));
    let old = settled(&store, "old", 1, JobStatus::Succeeded);
    let failed = settled(&store, "failed", 2, JobStatus::Failed);
    let waiting = admit(&store, "waiting");
    let jobs = root.path().join("jobs");
    let orphan = "00000000-0000-4000-8000-0000000000aa.world";
    std::fs::write(jobs.join(orphan), "{}").expect("a world a crash left");
    std::fs::write(jobs.join("notes.world"), "kept").expect("not a job's world");
    let before: Vec<_> = std::fs::read_dir(&jobs)
        .expect("jobs dir")
        .map(|entry| entry.expect("entry").file_name())
        .collect();

    assert_eq!(store.evict_expired().expect("nothing is old yet"), 0);
    assert!(!jobs.join(orphan).exists(), "the orphan world is removed");
    assert!(jobs.join("notes.world").exists());
    assert!(world_exists(root.path(), old.id()));

    clock.set("2026-11-16T12:00:00Z");
    assert_eq!(store.evict_expired().expect("startup eviction"), 1);
    assert_eq!(kept(&store), [failed.id(), waiting.id()].map(Clone::clone));
    assert!(!world_exists(root.path(), old.id()));
    assert!(world_exists(root.path(), failed.id()));
    assert!(world_exists(root.path(), waiting.id()));
    for name in before {
        let name = name.to_string_lossy().into_owned();
        let gone = name == orphan || name == format!("{}.world", old.id().as_str());
        assert_eq!(jobs.join(&name).exists(), !gone, "{name}");
    }
    assert_eq!(store.evict_expired().expect("idempotent"), 0);
}

/// A page cursor names a rank, so an eviction between two pages moves no
/// other job: the evicted one simply no longer appears.
#[test]
fn a_listing_pages_across_an_eviction() {
    let root = tempfile::tempdir().expect("root");
    let clock = StepClock::at("2026-11-16T10:00:00Z");
    let store = store(root.path(), &clock, windows(HOUR, DAY));
    let first = admit(&store, "first");
    let second = admit(&store, "second");
    let short = settled(&store, "short", 3, JobStatus::Succeeded);
    let fourth = admit(&store, "fourth");
    let fifth = admit(&store, "fifth");

    let page = store
        .list(&JobListFilter::default(), 2, None)
        .expect("first page");
    assert_eq!(
        page.jobs.iter().map(|job| &job.id).collect::<Vec<_>>(),
        [fifth.id(), fourth.id()]
    );
    clock.set("2026-11-16T11:30:00Z");
    assert_eq!(store.evict_expired().expect("evict"), 1);
    assert!(store.get(short.id()).expect("read").is_none());
    let rest = store
        .list(&JobListFilter::default(), 2, page.next)
        .expect("second page");
    assert_eq!(
        rest.jobs.iter().map(|job| &job.id).collect::<Vec<_>>(),
        [second.id(), first.id()]
    );
    assert_eq!(rest.next, None);
}

/// The store never destroys what it cannot date: a job whose settlement
/// carries no date (written before the resident dated its journal) stays.
#[test]
fn an_undated_settlement_is_never_aged() {
    let root = tempfile::tempdir().expect("root");
    drop(JobStore::open(root.path()).expect("initialized store"));
    let frozen = include_str!("../store/fixtures/v3.json");
    std::fs::write(root.path().join("jobs/state.json"), frozen).expect("format-3 state");
    let clock = StepClock::at("2099-01-01T00:00:00Z");
    let store = store(root.path(), &clock, windows(HOUR, HOUR));
    assert_eq!(store.evict_expired().expect("startup"), 0);
    let page = store
        .list(&JobListFilter::default(), 200, None)
        .expect("list");
    assert_eq!(page.jobs.len(), 2);
    assert!(
        page.jobs
            .iter()
            .any(|job| job.status == JobStatus::Succeeded)
    );
}
