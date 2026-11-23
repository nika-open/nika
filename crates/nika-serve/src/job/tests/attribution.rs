// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! Store format 4 (ADR-155): a stated attribution is durable and hash-bound,
//! an admission is dated and ranked, and an unattributed job is what it was.

use std::collections::BTreeMap;

use super::*;
use crate::job::{JobAttribution, JobBindings};

#[derive(Debug)]
struct FixedClock(jiff::Zoned);

impl crate::ResidentClock for FixedClock {
    fn now(&self) -> jiff::Zoned {
        self.0.clone()
    }

    fn sleep(
        &self,
        _: std::time::Duration,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        Box::pin(std::future::ready(()))
    }
}

fn dated_store(root: &std::path::Path, at: &str) -> JobStore {
    let at = at
        .parse::<jiff::Timestamp>()
        .expect("instant")
        .to_zoned(jiff::tz::TimeZone::UTC);
    JobStore::open(root)
        .expect("store")
        .with_clock(Arc::new(FixedClock(at)))
}

fn stated(owner: &str, labels: &[(&str, &str)]) -> JobBindings {
    let labels = labels
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect::<BTreeMap<_, _>>();
    JobBindings {
        attribution: JobAttribution::new(Some(owner.to_owned()), labels)
            .expect("legal attribution"),
        ..JobBindings::default()
    }
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

fn read_state(root: &std::path::Path) -> serde_json::Value {
    let text = std::fs::read_to_string(root.join("jobs/state.json")).expect("state");
    serde_json::from_str(&text).expect("state json")
}

fn write_state(root: &std::path::Path, state: &serde_json::Value) {
    let mut text = serde_json::to_string(state).expect("encode");
    text.push('\n');
    std::fs::write(root.join("jobs/state.json"), text).expect("write state");
}

#[test]
fn an_admission_is_dated_ranked_and_keeps_what_its_requester_stated() {
    let root = tempfile::tempdir().expect("root");
    let store = dated_store(root.path(), "2026-11-23T10:00:00Z");
    let first = admit(
        &store,
        "first",
        1,
        stated("user_123", &[("app", "translation"), ("plan", "pro")]),
    );
    let second = admit(&store, "second", 2, JobBindings::default());

    assert_eq!(first.owner(), Some("user_123"));
    assert_eq!(
        first.labels().get("app").map(String::as_str),
        Some("translation")
    );
    assert_eq!(first.labels().len(), 2);
    assert_eq!(first.created_at(), Some("2026-11-23T10:00:00Z"));
    assert_eq!(first.ordinal, 1);
    assert_eq!(second.owner(), None);
    assert!(second.labels().is_empty());
    assert_eq!(second.ordinal, 2);

    drop(store);
    let reopened = JobStore::open(root.path()).expect("reopen");
    let read = reopened.get(first.id()).expect("read").expect("job");
    assert_eq!(read, first, "the stated attribution survives a restart");

    let state = read_state(root.path());
    assert_eq!(state["version"], json!(4));
    assert_eq!(state["next_ordinal"], json!(3));
    assert_eq!(state["jobs"][0]["record"]["owner"], json!("user_123"));
    assert!(
        state["jobs"][1]["record"].get("owner").is_none()
            && state["jobs"][1]["record"].get("labels").is_none(),
        "an unattributed record writes no attribution member"
    );
}

#[test]
fn a_stated_attribution_is_bound_from_admission_and_cannot_be_rewritten() {
    let root = tempfile::tempdir().expect("root");
    let store = dated_store(root.path(), "2026-11-23T10:00:00Z");
    let record = admit(&store, "owned", 1, stated("user_123", &[("plan", "pro")]));
    let (events, _, _) = store
        .event_page(record.id(), 0, page_limit(8))
        .expect("events");
    assert_eq!(
        events.len(),
        1,
        "an attributed admission writes the event whose hash binds it"
    );
    drop(store);

    for (member, forged) in [
        ("owner", json!("user_456")),
        ("labels", json!({"plan": "free"})),
    ] {
        let original = read_state(root.path());
        let mut tampered = original.clone();
        tampered["jobs"][0]["record"][member] = forged;
        write_state(root.path(), &tampered);
        assert!(
            matches!(JobStore::open(root.path()), Err(JobStoreError::Corrupt(_))),
            "a rewritten {member} no longer matches the admission hash"
        );
        write_state(root.path(), &original);
    }
    let mut stripped = read_state(root.path());
    stripped["jobs"][0]["record"]
        .as_object_mut()
        .expect("record")
        .remove("owner");
    write_state(root.path(), &stripped);
    assert!(
        matches!(JobStore::open(root.path()), Err(JobStoreError::Corrupt(_))),
        "an erased owner no longer matches the admission hash"
    );
}

#[test]
fn the_same_key_under_another_attribution_is_its_own_request() {
    let root = tempfile::tempdir().expect("root");
    let store = JobStore::open(root.path()).expect("store");
    let created = admit(&store, "retry", 1, stated("user_123", &[]));
    // The request digest is the digest of the exact body: another owner is
    // another body, so the store sees another digest under the same key.
    let conflict = store
        .create_or_replay_captured_inputs(
            key("retry"),
            digest(2),
            usize::MAX,
            "root.nika".to_owned(),
            "{}",
            None,
            stated("user_456", &[]),
        )
        .expect("conflict");
    assert!(matches!(conflict, Admission::Conflict(ref record) if record.id() == created.id()));
    let replay = store
        .create_or_replay_captured_inputs(
            key("retry"),
            digest(1),
            usize::MAX,
            "root.nika".to_owned(),
            "{}",
            None,
            stated("user_123", &[]),
        )
        .expect("replay");
    assert!(
        matches!(replay, Admission::Existing(ref record) if record.owner() == Some("user_123"))
    );
}

#[test]
fn ordinals_are_never_reused_and_a_broken_rank_refuses_the_store() {
    let root = tempfile::tempdir().expect("root");
    let store = JobStore::open(root.path()).expect("store");
    for index in 0..3_u8 {
        admit(
            &store,
            &format!("job-{index}"),
            index,
            JobBindings::default(),
        );
    }
    drop(store);
    let original = read_state(root.path());
    assert_eq!(
        original["jobs"]
            .as_array()
            .expect("jobs")
            .iter()
            .map(|job| job["record"]["ordinal"].as_u64().expect("ordinal"))
            .collect::<Vec<_>>(),
        [1, 2, 3]
    );

    for (case, edit) in [
        (
            "a repeated ordinal",
            Box::new(|state: &mut serde_json::Value| {
                state["jobs"][1]["record"]["ordinal"] = json!(1);
            }) as Box<dyn Fn(&mut serde_json::Value)>,
        ),
        (
            "an ordinal at the next one",
            Box::new(|state: &mut serde_json::Value| {
                state["next_ordinal"] = json!(3);
            }),
        ),
        (
            "a missing ordinal",
            Box::new(|state: &mut serde_json::Value| {
                state["jobs"][0]["record"]
                    .as_object_mut()
                    .expect("record")
                    .remove("ordinal");
            }),
        ),
    ] {
        let mut broken = original.clone();
        edit(&mut broken);
        write_state(root.path(), &broken);
        assert!(
            matches!(JobStore::open(root.path()), Err(JobStoreError::Corrupt(_))),
            "{case} refuses the store"
        );
    }
    write_state(root.path(), &original);
    JobStore::open(root.path()).expect("the untouched state opens");
}

#[test]
fn a_stored_attribution_outside_its_bounds_refuses_the_store() {
    let root = tempfile::tempdir().expect("root");
    let store = JobStore::open(root.path()).expect("store");
    admit(&store, "owned", 1, stated("user_123", &[]));
    drop(store);
    let mut state = read_state(root.path());
    state["jobs"][0]["record"]["owner"] = json!("user 123");
    write_state(root.path(), &state);
    // The reason is named: the edit also breaks the admission event's hash,
    // which would refuse the store by itself were the bounds not judged.
    let refused = JobStore::open(root.path()).err();
    assert!(
        matches!(
            &refused,
            Some(JobStoreError::Corrupt(reason)) if reason == "job attribution is outside its bounds"
        ),
        "{refused:?}"
    );
}
