// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! Frozen bytes pin the durable reader and writer together (#1467).
//!
//! Format 3 is the frozen input of the one migration to format 4 (ADR-155);
//! format 4 is read and written byte for byte.

use super::*;

const V3: &str = include_str!("fixtures/v3.json");
const V4: &str = include_str!("fixtures/v4.json");

fn parsed(text: &str) -> Value {
    serde_json::from_str(text).expect("fixture json")
}

/// Every hash a stored job proves itself with, in stored order.
fn proofs(state: &Value) -> Vec<Value> {
    state["jobs"]
        .as_array()
        .expect("jobs")
        .iter()
        .map(|job| {
            serde_json::json!({
                "events": job["events"],
                "event_head": job["event_head"],
                "identity_digest": job["identity_digest"],
                "terminal_sequence": job["terminal_sequence"],
                "receipt": job["record"]["receipt"],
            })
        })
        .collect()
}

#[test]
fn version_three_golden_migrates_to_four_and_keeps_every_proof() {
    let decoded = decode_state(V3).expect("frozen v3 fixture validates");
    assert!(decoded.migrated, "v3 is lifted to the current format once");
    assert_eq!(decoded.state.version, STATE_VERSION);
    assert_eq!(decoded.state.incarnation.current.get(), 4);
    assert_eq!(
        decoded
            .state
            .incarnation
            .settled
            .expect("previous owner")
            .get(),
        3
    );
    let writer = decoded.state.writer.as_ref().expect("writer stamp");
    assert_eq!(
        (
            writer.engine_version.as_str(),
            writer.machine_protocol_version
        ),
        ("0.119.0", 1),
        "the stamp stays the last writer's until a resident holds the lease"
    );
    assert_eq!(decoded.state.jobs.len(), 2);
    assert_eq!(decoded.state.jobs[0].record.status(), JobStatus::Succeeded);
    assert_eq!(decoded.state.jobs[1].record.status(), JobStatus::Queued);
    assert_eq!(decoded.state.jobs[0].terminal_sequence, Some(1));
    assert_eq!(decoded.state.jobs[1].terminal_sequence, None);
    assert_eq!(
        decoded
            .state
            .jobs
            .iter()
            .map(|job| job.record.ordinal)
            .collect::<Vec<_>>(),
        [1, 2],
        "a migrated job is ranked in stored order, which is admission order"
    );
    assert_eq!(decoded.state.next_ordinal, 3);
    assert!(
        decoded.state.jobs.iter().all(|job| {
            job.record.owner().is_none()
                && job.record.labels().is_empty()
                && job.record.created_at().is_none()
        }),
        "a migrated job gains no attribution and no invented admission date"
    );
    assert!(
        decoded
            .state
            .jobs
            .iter()
            .flat_map(|job| job.events.iter())
            .all(|event| event.at().is_none()),
        "a v3 journal written before #1463 is undated, and stays so"
    );

    let migrated = prepare_snapshot(&decoded.state).expect("encode");
    assert_eq!(
        proofs(&parsed(migrated.as_str())),
        proofs(&parsed(V3)),
        "every event hash, identity digest and receipt is the one format 3 wrote"
    );

    let root = tempfile::tempdir().expect("root");
    drop(JobStore::open(root.path()).expect("initialize store"));
    let path = root.path().join("jobs/state.json");
    std::fs::write(&path, V3).expect("install frozen bytes");
    let store = JobStore::open(root.path()).expect("open frozen store");
    for frozen in &decoded.state.jobs {
        let loaded = store
            .get(frozen.record.id())
            .expect("read job")
            .expect("job exists");
        assert_eq!(loaded.status(), frozen.record.status());
        assert_eq!(loaded.receipt(), frozen.record.receipt());
        assert_eq!(loaded.outputs(), frozen.record.outputs());
    }
    drop(store);
    let persisted = std::fs::read_to_string(&path).expect("migrated store");
    assert_eq!(
        persisted,
        migrated.as_str(),
        "opening persists the migration once"
    );
    drop(JobStore::open(root.path()).expect("reopen the migrated store"));
    assert_eq!(
        std::fs::read_to_string(&path).expect("stable store"),
        persisted,
        "a format-4 store is not rewritten by opening it"
    );
}

#[test]
fn version_four_golden_reads_and_roundtrips_byte_for_byte() {
    let decoded = decode_state(V4).expect("frozen v4 fixture validates");
    assert!(!decoded.migrated, "v4 is read without migration");
    assert_eq!(decoded.state.version, 4);
    assert_eq!(decoded.state.next_ordinal, 4);
    let jobs = &decoded.state.jobs;
    assert_eq!(jobs.len(), 3);

    let owned = &jobs[0].record;
    assert_eq!(owned.status(), JobStatus::Succeeded);
    assert_eq!(owned.owner(), Some("user_123"));
    assert_eq!(
        owned
            .labels()
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect::<Vec<_>>(),
        [("app", "translation"), ("plan", "pro")]
    );
    assert_eq!(owned.created_at(), Some("2026-11-23T10:00:00Z"));
    assert_eq!(owned.ordinal, 1);

    let plain = &jobs[1].record;
    assert_eq!(plain.status(), JobStatus::Queued);
    assert_eq!((plain.owner(), plain.labels().len()), (None, 0));
    assert_eq!(plain.ordinal, 2);

    let labelled = &jobs[2].record;
    assert_eq!(labelled.status(), JobStatus::Failed);
    assert_eq!(labelled.owner(), None);
    assert_eq!(
        labelled.labels().get("team").map(String::as_str),
        Some("support")
    );
    assert_eq!(
        labelled.error(),
        Some(("NIKA-1704", "the run stopped at its cost ceiling"))
    );
    assert_eq!(labelled.ordinal, 3);

    assert_eq!(
        prepare_snapshot(&decoded.state).expect("encode").as_str(),
        V4
    );

    let root = tempfile::tempdir().expect("root");
    drop(JobStore::open(root.path()).expect("initialize store"));
    let path = root.path().join("jobs/state.json");
    std::fs::write(&path, V4).expect("install frozen bytes");
    let store = JobStore::open(root.path()).expect("open frozen store");
    for frozen in jobs {
        let loaded = store
            .get(frozen.record.id())
            .expect("read job")
            .expect("job exists");
        assert_eq!(loaded.owner(), frozen.record.owner());
        assert_eq!(loaded.labels(), frozen.record.labels());
        assert_eq!(loaded.receipt(), frozen.record.receipt());
    }
    assert_eq!(std::fs::read_to_string(path).expect("unchanged store"), V4);
}

#[test]
fn a_format_three_state_carrying_format_four_members_is_refused() {
    for (case, edit) in [
        (
            "an owner",
            Box::new(|state: &mut Value| {
                state["jobs"][0]["record"]["owner"] = serde_json::json!("user_123");
            }) as Box<dyn Fn(&mut Value)>,
        ),
        (
            "labels",
            Box::new(|state: &mut Value| {
                state["jobs"][0]["record"]["labels"] = serde_json::json!({"app": "x"});
            }),
        ),
        (
            "an admission date",
            Box::new(|state: &mut Value| {
                state["jobs"][1]["record"]["created_at"] =
                    serde_json::json!("2026-11-23T10:00:00Z");
            }),
        ),
        (
            "an ordinal",
            Box::new(|state: &mut Value| {
                state["jobs"][1]["record"]["ordinal"] = serde_json::json!(7);
            }),
        ),
        (
            "a next ordinal",
            Box::new(|state: &mut Value| {
                state["next_ordinal"] = serde_json::json!(3);
            }),
        ),
    ] {
        let mut state = parsed(V3);
        edit(&mut state);
        let text = serde_json::to_string(&state).expect("encode");
        assert!(
            matches!(decode_state(&text), Err(JobStoreError::Corrupt(_))),
            "a format-3 state with {case} is refused"
        );
    }
}

#[test]
fn an_unknown_format_is_refused() {
    for version in [0, 1, 5, 99] {
        let mut state = parsed(V4);
        state["version"] = serde_json::json!(version);
        let text = serde_json::to_string(&state).expect("encode");
        assert!(
            matches!(decode_state(&text), Err(JobStoreError::Corrupt(_))),
            "format {version} is refused"
        );
    }
}
