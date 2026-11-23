// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! What a schedule row states about its jobs, the one-snapshot overview and
//! the conditional removal (ADR-155).

use std::collections::BTreeMap;

use sha2::{Digest as _, Sha256};

use super::*;

fn attributed(id: &str) -> ScheduleDraft {
    let mut draft = once_draft(id, "base.nika");
    draft.owner = Some("user_123".to_owned());
    draft.labels = BTreeMap::from([
        ("plan".to_owned(), "pro".to_owned()),
        ("app".to_owned(), "translation".to_owned()),
    ]);
    draft
}

fn rows(root: &std::path::Path) -> Vec<serde_json::Value> {
    let path = root.join("schedules/state.json");
    let state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path).expect("read")).expect("json");
    state["schedules"].as_array().expect("rows").clone()
}

/// A schedule row as every engine before attribution wrote and read it.
/// Closed, so a row that carries a member this shape does not know cannot
/// be read through it; serialized, it is that engine's integrity preimage
/// (the stored `integrity` is not part of its own preimage).
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoricalRow {
    id: String,
    origin: String,
    workflow: String,
    when: HistoricalWhen,
    max_cost_usd: f64,
    missed: String,
    max_lateness_seconds: Option<u64>,
    overlap: String,
    after_skip: String,
    jitter: Option<String>,
    tolerance: Option<String>,
    active: bool,
    pause_reason: Option<String>,
    pause_until: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    inputs: BTreeMap<String, String>,
    revision: String,
    #[serde(skip_serializing)]
    integrity: String,
    history: Vec<serde_json::Value>,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum HistoricalWhen {
    Once { at: String },
    Cadence { expression: String },
    Webhook,
}

impl HistoricalRow {
    fn integrity_of_preimage(&self) -> String {
        let preimage = serde_json::to_vec(self).expect("preimage");
        format!("sha256:{:x}", Sha256::digest(preimage))
    }
}

/// A schedule that states nobody is stored exactly as before attribution
/// existed: no new member in its row, and the integrity of its historical
/// preimage. A resident that upgrades reads its schedules unchanged.
#[test]
fn an_unattributed_row_keeps_its_historical_bytes() {
    let root = tempfile::tempdir().expect("root");
    let store = ScheduleStore::open(root.path()).expect("store");
    store
        .apply(
            once_draft("plain", "base.nika"),
            ScheduleApplyPrecondition::Create,
        )
        .expect("create");
    let mut inputs = once_draft("bound", "base.nika");
    inputs.inputs = BTreeMap::from([("tenant".to_owned(), "acme".to_owned())]);
    store
        .apply(inputs, ScheduleApplyPrecondition::Create)
        .expect("create");
    let rows = rows(root.path());
    assert_eq!(rows.len(), 2);
    for row in rows {
        let historical: HistoricalRow =
            serde_json::from_value(row).expect("a row an earlier engine reads whole");
        assert_eq!(
            historical.integrity,
            historical.integrity_of_preimage(),
            "{}: the preimage of an unattributed row has no attribution member",
            historical.id
        );
    }
}

/// Attribution is stored with the definition, bound in its revision and its
/// integrity, and restored by a reopen.
#[test]
fn attribution_is_stored_bound_and_restored() {
    let root = tempfile::tempdir().expect("root");
    let store = ScheduleStore::open(root.path()).expect("store");
    let plain = created_revision(
        store
            .apply(
                once_draft("plain", "base.nika"),
                ScheduleApplyPrecondition::Create,
            )
            .expect("create"),
    )
    .expect("revision");
    let stated = created_definition(
        store
            .apply(attributed("stated"), ScheduleApplyPrecondition::Create)
            .expect("create"),
    )
    .expect("definition");
    assert_eq!(stated.owner(), Some("user_123"));

    let rows_now = rows(root.path());
    let row = rows_now
        .iter()
        .find(|row| row["id"] == "stated")
        .expect("stated row");
    assert_eq!(row["owner"], "user_123");
    assert_eq!(
        row["labels"],
        serde_json::json!({"app": "translation", "plan": "pro"})
    );
    assert!(
        serde_json::from_value::<HistoricalRow>(row.clone()).is_err(),
        "an engine before attribution refuses the row rather than drop what it states"
    );

    // Stating an owner on the plain schedule is an update under its revision.
    let mut owned = once_draft("plain", "base.nika");
    owned.owner = Some("user_456".to_owned());
    let updated = store
        .apply(owned, ScheduleApplyPrecondition::Revision(plain.clone()))
        .expect("update");
    let updated = match updated {
        ScheduleApplyOutcome::Updated(updated) => Some(updated),
        _ => None,
    }
    .expect("stating an owner changes the revision");
    assert_ne!(updated.revision(), plain);
    drop(store);

    let reopened = ScheduleStore::open(root.path()).expect("reopen");
    let restored = reopened.get("stated").expect("read").expect("schedule");
    assert_eq!(restored.owner(), Some("user_123"));
    let labels: Vec<(&str, &str)> = restored.labels().collect();
    assert_eq!(labels, [("app", "translation"), ("plan", "pro")]);
    assert_eq!(restored.revision(), stated.revision());

    // A row whose attribution was edited outside the law refuses recovery.
    drop(reopened);
    let path = root.path().join("schedules/state.json");
    let mut state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).expect("read")).expect("json");
    let rows = state["schedules"].as_array_mut().expect("rows");
    let row = rows
        .iter_mut()
        .find(|row| row["id"] == "stated")
        .expect("stated row");
    row["owner"] = serde_json::json!("user 123");
    std::fs::write(&path, serde_json::to_vec(&state).expect("encode")).expect("write");
    assert!(matches!(
        ScheduleStore::open(root.path()),
        Err(ScheduleStoreError::Corrupt(_))
    ));
}

/// The overview pairs every definition with the decisions of the same
/// snapshot, for both origins.
#[test]
fn the_overview_reads_definitions_and_decisions_together() {
    let root = tempfile::tempdir().expect("root");
    let store = ScheduleStore::open(root.path()).expect("store");
    assert_eq!(store.overview().expect("empty"), (Vec::new(), Vec::new()));
    let fired = created_definition(
        store
            .apply(
                once_draft("fired", "base.nika"),
                ScheduleApplyPrecondition::Create,
            )
            .expect("create"),
    )
    .expect("definition");
    store
        .apply(attributed("waiting"), ScheduleApplyPrecondition::Create)
        .expect("create");
    let decided_at: Timestamp = "2026-09-01T09:00:01Z".parse().expect("decision time");
    for origin in [ScheduleOrigin::Api, ScheduleOrigin::Project] {
        assert!(
            store
                .consume_slot(
                    origin,
                    &fired,
                    &due_once(&fired),
                    ScheduleDecision::Scheduled,
                    ScheduleSlotAction::Skipped,
                    decided_at,
                    Some("overview".to_owned()),
                )
                .expect("consume")
        );
    }
    let (definitions, decisions) = store.overview().expect("overview");
    let ids: Vec<&str> = definitions
        .iter()
        .map(nika_cadence::ScheduleDefinition::id)
        .collect();
    assert_eq!(ids, ["fired", "waiting"], "sorted by id");
    let mut decided: Vec<(ScheduleOrigin, &str)> = decisions
        .iter()
        .map(|record| (record.origin(), record.schedule_id()))
        .collect();
    decided.sort_by_key(|(origin, _)| origin.key(""));
    assert_eq!(
        decided,
        [
            (ScheduleOrigin::Api, "fired"),
            (ScheduleOrigin::Project, "fired")
        ]
    );
}

/// A removal is conditional on the revision the caller read, takes the
/// schedule's last-slot decision with it, and leaves a project beat of the
/// same id alone. A schedule declared again starts with no history.
#[test]
fn a_removal_is_conditional_and_takes_its_decision() {
    let root = tempfile::tempdir().expect("root");
    let store = ScheduleStore::open(root.path()).expect("store");
    assert_eq!(
        store.remove("absent", None).expect("absent"),
        ScheduleRemoval::Absent
    );
    let definition = created_definition(
        store
            .apply(
                once_draft("same", "base.nika"),
                ScheduleApplyPrecondition::Create,
            )
            .expect("create"),
    )
    .expect("definition");
    let kept = created_revision(
        store
            .apply(attributed("kept"), ScheduleApplyPrecondition::Create)
            .expect("create"),
    )
    .expect("revision");
    let decided_at: Timestamp = "2026-09-01T09:00:01Z".parse().expect("decision time");
    for origin in [ScheduleOrigin::Api, ScheduleOrigin::Project] {
        assert!(
            store
                .consume_slot(
                    origin,
                    &definition,
                    &due_once(&definition),
                    ScheduleDecision::Scheduled,
                    ScheduleSlotAction::Claimed,
                    decided_at,
                    None,
                )
                .expect("consume")
        );
    }

    // Another revision than the current one changes nothing.
    assert_eq!(
        store.remove("same", Some(&kept)).expect("stale"),
        ScheduleRemoval::Stale(definition.revision())
    );
    assert!(store.get("same").expect("read").is_some());

    assert_eq!(
        store
            .remove("same", Some(&definition.revision()))
            .expect("remove"),
        ScheduleRemoval::Removed(definition.revision())
    );
    assert!(store.get("same").expect("read").is_none());
    assert!(
        store
            .last_decision(ScheduleOrigin::Api, "same")
            .expect("api decision")
            .is_none(),
        "the removed schedule's decision left with it"
    );
    assert!(
        store
            .last_decision(ScheduleOrigin::Project, "same")
            .expect("project decision")
            .is_some(),
        "a project beat of the same id keeps its own decision"
    );
    assert_eq!(
        store.remove("same", None).expect("again"),
        ScheduleRemoval::Absent
    );
    drop(store);

    // Durable, and a schedule declared again under the id has no history.
    let reopened = ScheduleStore::open(root.path()).expect("reopen");
    assert!(reopened.get("same").expect("read").is_none());
    assert!(reopened.get("kept").expect("read").is_some());
    let again = created_definition(
        reopened
            .apply(
                once_draft("same", "base.nika"),
                ScheduleApplyPrecondition::Create,
            )
            .expect("create again"),
    )
    .expect("definition");
    let state = reopened
        .decision_state(ScheduleOrigin::Api, "same")
        .expect("state");
    let now: Zoned = "2026-09-01T09:00:01Z[UTC]".parse().expect("now");
    let plan = plan_schedule(&again, &now, &state, 1).expect("plan");
    assert!(
        matches!(
            plan.due(),
            ScheduleDueVerdict::ScheduledOnTime { .. } | ScheduleDueVerdict::CatchUp { .. }
        ),
        "no decision survives the removal"
    );
    assert_eq!(
        reopened.remove("kept", None).expect("any revision"),
        ScheduleRemoval::Removed(kept)
    );
}
