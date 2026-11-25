// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! NIK-385 · a run keeps the child traces it started: the policy judges
//! roots only, and a child goes only with its root, for the root's reason.

use super::*;
use crate::store::tests::event;
use nika_event::Event;
use nika_types::resource::{KeyValue, Value};

/// A synthetic trace (10 bytes) that cites `children` by file name.
fn citing(
    name: &str,
    workflow: &str,
    state: TraceState,
    age_s: u64,
    children: &[&str],
) -> TraceMeta {
    meta(name, workflow, state, 10, age_s)
        .with_children(children.iter().copied().map(str::to_owned).collect())
}

/// The default caps, keeping the last `keep_last` runs per workflow.
fn keeping(keep_last: usize) -> RetentionConfig {
    let base = RetentionConfig::default();
    RetentionConfig::new(keep_last, base.max_age, base.budget_bytes)
}

/// The plan, as the names it removes and why.
fn plan_names(traces: &[TraceMeta], cfg: &RetentionConfig) -> BTreeMap<String, Reason> {
    plan(traces, cfg, now())
        .into_iter()
        .map(|(i, reason)| (traces[i].name.clone(), reason))
        .collect()
}

/// The given names, each removed for `reason`.
fn each(names: &[&str], reason: Reason) -> BTreeMap<String, Reason> {
    names
        .iter()
        .copied()
        .map(|name| (name.to_owned(), reason))
        .collect()
}

/// The file names left in `dir`, sorted.
fn file_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("store dir")
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    names.sort();
    names
}

/// The item rows of a fan-out whose iterations each ran a child, indexed
/// from `from`.
fn rows(names: &[String], from: usize) -> String {
    let rows: Vec<serde_json::Value> = names
        .iter()
        .zip(from..)
        .map(|(name, index)| {
            serde_json::json!({
                "child": name, "index": index, "item": format!("term-{index}"), "status": "ok",
            })
        })
        .collect();
    serde_json::to_string(&rows).expect("rows")
}

/// A `calibration` run whose fan-outs cite `children`: the first half
/// inline on the `warmup` terminal, the rest paged into two `task_items`
/// frames before the `rounds` terminal.
fn calibration_journal(children: &[String]) -> String {
    let (inline, paged) = children.split_at(children.len() / 2);
    let (first, second) = paged.split_at(paged.len() / 2);
    let with = |frame: Event, key: &str, value: Value| frame.with_field(KeyValue::new(key, value));
    let page = |n: i64, names: &[String], from: usize| {
        let frame = event(EventKind::TaskItems, "task", "rounds", 12);
        let frame = with(frame, "page", Value::Int(n));
        with(frame, "items", Value::String(rows(names, from)))
    };
    let warmup = event(EventKind::TaskCompleted, "task", "warmup", 11);
    let rounds = event(EventKind::TaskCompleted, "task", "rounds", 13);
    let mut events = run_events("calibration", None);
    events.push(with(warmup, "items", Value::String(rows(inline, 0))));
    events.push(page(0, first, 0));
    events.push(page(1, second, first.len()));
    events.push(with(rounds, "items_pages", Value::Int(2)));
    events.push(event(
        EventKind::WorkflowCompleted,
        "workflow",
        "calibration",
        20,
    ));
    ndjson(&events)
}

/// NIK-385 point 4 · a `calibration` run's fan-outs ran 41 `round`
/// children, cited inline and through pages. Ten more `round` runs and one
/// more `calibration` run arrive: the kept parent keeps all 41 children,
/// and only the older `round` run the ten newer ones outrank rotates.
#[test]
fn a_kept_parent_keeps_every_child_it_cites() {
    let dir = temp_store("children-kept");
    let hours = |h: u64| Duration::from_secs(h * 3_600);
    let round = ndjson(&run_events("round", Some(EventKind::WorkflowCompleted)));
    let children: Vec<String> = (0..41).map(|i| format!("child-{i:02}.ndjson")).collect();
    stage_trace(&dir, "round-old.ndjson", &round, hours(200));
    for (name, i) in children.iter().zip(0u64..) {
        // The children ran inside the parent's run, the first one first.
        let age = hours(100) + Duration::from_secs(60 * (41 - i));
        stage_trace(&dir, name, &round, age);
    }
    let parent = calibration_journal(&children);
    stage_trace(&dir, "calibration-parent.ndjson", &parent, hours(99));
    for i in 0..10u64 {
        stage_trace(&dir, &format!("round-{i:02}.ndjson"), &round, hours(50 + i));
    }
    let newer = ndjson(&run_events(
        "calibration",
        Some(EventKind::WorkflowCompleted),
    ));
    stage_trace(&dir, "calibration-new.ndjson", &newer, hours(1));

    let report = collect(&dir, &RetentionConfig::default(), SystemTime::now())
        .expect("the older round run rotates");
    let kept_bytes = 51 * round.len() as u64 + (parent.len() + newer.len()) as u64;
    assert_eq!(
        report,
        GcReport::new(1, 0, 0, 53, kept_bytes),
        "one rotated · the parent, its 41 children, ten round runs and the newer calibration kept"
    );
    let mut kept = children;
    kept.extend((0..10).map(|i| format!("round-{i:02}.ndjson")));
    kept.extend(["calibration-new.ndjson", "calibration-parent.ndjson"].map(str::to_owned));
    kept.sort();
    assert_eq!(
        file_names(&dir),
        kept,
        "exactly round-old.ndjson is removed"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// Once the parent itself rotates out, it goes with every trace it cites,
/// a grandchild too, all for its reason; a `round` run of its own stays.
#[test]
fn a_rotated_parent_takes_its_children_with_it() {
    let ok = TraceState::Succeeded;
    let mut traces: Vec<TraceMeta> = (0..10u64)
        .map(|i| {
            meta(
                &format!("cal-{i:02}"),
                "calibration",
                ok,
                10,
                (i + 1) * 3_600,
            )
        })
        .collect();
    let children: Vec<String> = (0..41).map(|i| format!("round-{i:02}")).collect();
    let cited: Vec<&str> = children.iter().map(String::as_str).collect();
    traces.push(citing("cal-parent", "calibration", ok, 20 * 3_600, &cited));
    for (name, i) in cited.iter().zip(0u64..) {
        let below: &[&str] = if i == 0 { &["leaf-0"] } else { &[] };
        traces.push(citing(name, "round", ok, 21 * 3_600 + 60 * i, below));
    }
    traces.push(meta("leaf-0", "leaf", ok, 10, 21 * 3_600));
    traces.push(meta("round-solo", "round", ok, 10, 1_800));

    let mut gone = cited;
    gone.extend(["cal-parent", "leaf-0"]);
    assert_eq!(
        plan_names(&traces, &RetentionConfig::default()),
        each(&gone, Reason::Rotated),
        "the eleventh calibration run takes its 41 children and its grandchild"
    );
}

/// The budget removes a whole run, its children with it, at its root's
/// place in the order, and stops once the run's summed bytes fit.
#[test]
fn the_budget_removes_a_whole_run() {
    let ok = TraceState::Succeeded;
    let cfg = RetentionConfig::new(10, Duration::from_secs(365 * DAY), 350);
    let traces = vec![
        meta("c", "w", ok, 100, DAY),
        meta("b", "w", ok, 100, 2 * DAY),
        meta("a", "w", ok, 100, 9 * DAY).with_children(vec!["a-1".to_owned(), "a-2".to_owned()]),
        meta("a-1", "step", ok, 100, 9 * DAY + 3_600),
        meta("a-2", "step", ok, 100, 9 * DAY + 7_200),
    ];
    // 500 bytes → the oldest run's 300 go at once → 200 ≤ 350: `b` stays.
    assert_eq!(
        plan_names(&traces, &cfg),
        each(&["a", "a-1", "a-2"], Reason::OverBudget)
    );
}

/// A child whose parent is gone from disk is an ordinary trace of its own
/// workflow, and a cited child that is gone is simply absent.
#[test]
fn an_orphan_child_rotates_as_an_ordinary_trace() {
    let ok = TraceState::Succeeded;
    let traces = vec![
        meta("round-new", "round", ok, 10, 3_600),
        citing(
            "cal",
            "calibration",
            ok,
            5 * 3_600,
            &["round-kid", "round-gone"],
        ),
        meta("round-kid", "round", ok, 10, 6 * 3_600),
        meta("round-orphan", "round", ok, 10, 8 * 3_600),
    ];
    assert_eq!(
        plan_names(&traces, &keeping(1)),
        each(&["round-orphan"], Reason::Rotated),
        "the orphan counts in its workflow's window; the cited child rides with its parent"
    );
}

/// NIK-385 · a failed root the rotation keeps first keeps its children,
/// and the budget, which never removes the newest failure, leaves them too.
#[test]
fn a_kept_failed_root_keeps_its_children() {
    let (ok, fail) = (TraceState::Succeeded, TraceState::Failed);
    let mut traces: Vec<TraceMeta> = (0..5u64)
        .map(|i| meta(&format!("ok-{i}"), "w", ok, 10, (i + 1) * 3_600))
        .collect();
    traces.push(citing(
        "fail-old",
        "w",
        fail,
        10 * 3_600,
        &["step-a", "step-b"],
    ));
    traces.push(meta("step-a", "step", ok, 10, 10 * 3_600 + 60));
    traces.push(meta("step-b", "step", ok, 10, 10 * 3_600 + 120));
    traces.extend((0..3u64).map(|i| meta(&format!("step-{i}"), "step", ok, 10, (i + 2) * 3_600)));

    assert_eq!(
        plan_names(&traces, &keeping(2)),
        each(&["ok-1", "ok-2", "ok-3", "ok-4", "step-2"], Reason::Rotated),
        "the failure and the newest success fill the window of w; its children count in none"
    );
    let tight = RetentionConfig::new(10, Duration::from_secs(365 * DAY), 1);
    assert_eq!(
        plan_names(&traces, &tight),
        each(
            &["ok-1", "ok-2", "ok-3", "ok-4", "step-1", "step-2"],
            Reason::OverBudget
        ),
        "every removable success goes; the newest failure and its children stay"
    );
}
