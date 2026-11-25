// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! NIK-385 · `trace verify` names each child trace a run cites that is not
//! beside its journal, and the tier and the exit stay the journal's.

use super::*;

/// The `child` row a plain `invoke` of a workflow leaves on its terminal.
const SOLO: &str = r#"{"target":"./solo.nika","trace_id":"child-a.ndjson","chain_head":"abc","outcome":"success"}"#;
/// A `for_each` of two items whose iterations each ran a child workflow.
const ROWS_CITING: &str = r#"[{"child":"child-b.ndjson","index":0,"item":"alpha","status":"ok"},{"child":"child-c.ndjson","index":1,"item":"beta","status":"ok"}]"#;
/// The same two items, citing no child.
const ROWS_PLAIN: &str =
    r#"[{"index":0,"item":"alpha","status":"ok"},{"index":1,"item":"beta","status":"ok"}]"#;
/// How a missing-child line opens.
const MARK: &str = "missing child trace";

/// A parent journal: `solo` ran one child and `rounds` two, when `cites`.
fn parent(cites: bool) -> String {
    let solo: &[(&str, &str)] = if cites {
        &[("task", "solo"), ("child", SOLO)]
    } else {
        &[("task", "solo")]
    };
    let rows = if cites { ROWS_CITING } else { ROWS_PLAIN };
    chained_with(&[
        ("workflow_started", &[]),
        ("task_completed", solo),
        ("task_completed", &[("task", "rounds"), ("items", rows)]),
        ("workflow_completed", &[]),
    ])
}

/// The JSON `lines` of a verify report that name a missing child trace.
fn named(doc: &serde_json::Value) -> Vec<String> {
    doc["lines"]
        .as_array()
        .expect("lines")
        .iter()
        .filter_map(serde_json::Value::as_str)
        .filter(|line| line.starts_with(MARK))
        .map(str::to_owned)
        .collect()
}

/// A parent citing three child traces, one of them gone, says so in exactly
/// one line, in the prose and in the JSON `lines`, with the tier and the exit
/// of the same journal citing none. Once every child is beside it the line
/// is gone, and a plain `invoke` child names no item.
#[test]
fn a_missing_child_trace_is_named_without_moving_the_tier_or_the_exit() {
    let dir = std::env::temp_dir().join(format!("nika-verify-children-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("dir");
    let (cited, plain) = (dir.join("parent.ndjson"), dir.join("plain.ndjson"));
    std::fs::write(&cited, parent(true)).expect("parent");
    std::fs::write(&plain, parent(false)).expect("plain");
    for name in ["child-a.ndjson", "child-b.ndjson"] {
        std::fs::write(dir.join(name), "{}\n").expect("a present child");
    }
    let json = VerifyOptions {
        json: true,
        ..VerifyOptions::default()
    };
    let doc = |path: &std::path::Path| -> serde_json::Value {
        let out = verify_with(&path.to_string_lossy(), &json);
        serde_json::from_str(out.text.trim()).expect("one document")
    };
    let line = "missing child trace `child-c.ndjson` · task `rounds` item 1 — this run's evidence is incomplete";

    let prose = verify_with(&cited.to_string_lossy(), &VerifyOptions::default());
    let baseline = verify_with(&plain.to_string_lossy(), &VerifyOptions::default());
    assert_eq!(prose.code, super::super::super::exit::OK, "{}", prose.text);
    assert_eq!(prose.code, baseline.code, "{}", prose.text);
    let said: Vec<&str> = prose.text.lines().filter(|l| l.starts_with(MARK)).collect();
    assert_eq!(said, [line], "{}", prose.text);
    assert!(!prose.text.contains("FINDING"), "{}", prose.text);
    let (with_children, without) = (doc(&cited), doc(&plain));
    assert_eq!(named(&with_children), [line], "{with_children}");
    assert!(named(&without).is_empty(), "{without}");
    assert_eq!(with_children["tier"], without["tier"], "{with_children}");
    assert_eq!(with_children["exit"], without["exit"], "{with_children}");

    std::fs::write(dir.join("child-c.ndjson"), "{}\n").expect("the last child");
    assert!(named(&doc(&cited)).is_empty(), "every child is beside it");
    std::fs::remove_file(dir.join("child-a.ndjson")).expect("a child gone");
    assert_eq!(
        named(&doc(&cited)),
        ["missing child trace `child-a.ndjson` · task `solo` — this run's evidence is incomplete"]
    );
    let _ = std::fs::remove_dir_all(&dir);
}
