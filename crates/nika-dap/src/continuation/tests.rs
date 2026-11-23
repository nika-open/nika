// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! The one resume judgment, over journals written the way a run's sink
//! writes them: what proceeds, what it says on the way, and in what order.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use nika_event::{Event, EventKind};
use nika_runtime::EventSink as _;
use nika_types::id::{EventId, ExecutionId, TraceId};
use nika_types::resource::{KeyValue, Value as FieldValue};
use nika_types::timestamp::Timestamp;

use super::*;

const ENGINE: &str = "9.9.9";
const HELLO: &str =
    "nika: hello\nmodel: mock/echo\ntasks:\n  greet:\n    infer:\n      prompt: hi\n";
const BRIEF: &str =
    "nika: compose-brief\nmodel: mock/echo\ntasks:\n  draft:\n    infer:\n      prompt: hi\n";

fn parsed(source: &str) -> RawWorkflow {
    nika_schema::parse(
        source,
        nika_schema::FileId::new(0),
        nika_schema::ParseMode::Strict,
    )
    .expect("a valid fixture")
}

fn text(value: &str) -> FieldValue {
    FieldValue::String(value.to_owned())
}

/// A boot manifest naming `workflow`, the hash of `source` and `engine`.
fn boot(workflow: &str, source: &str, engine: &str) -> Event {
    Event::new(
        EventId::new(uuid::Uuid::nil()),
        Timestamp::from_unix_ms(0),
        EventKind::WorkflowStarted,
    )
    .with_field(KeyValue::new("workflow", text(workflow)))
    .with_field(KeyValue::new(
        "workflow_sha256",
        text(&nika_event::source_id::sha256_hex(source.as_bytes())),
    ))
    .with_field(KeyValue::new("engine_version", text(engine)))
}

fn scheduled(task: &str) -> Event {
    Event::new(
        EventId::new(uuid::Uuid::nil()),
        Timestamp::from_unix_ms(1),
        EventKind::TaskScheduled,
    )
    .with_field(KeyValue::new("task", text(task)))
}

/// A chained journal holding `events`, written as a run's sink writes it.
fn journal(dir: &Path, events: &[Event]) -> PathBuf {
    let execution = ExecutionId::generate();
    let trace = TraceId::from(execution);
    let mut sink = crate::journal::TraceFileSink::new(dir).for_execution(execution, trace);
    for event in events {
        sink.emit(event.clone());
    }
    assert!(sink.error().is_none(), "the journal was written");
    crate::store::locate_trace(dir, &execution.to_string(), &trace.to_string())
        .expect("the journal is where a run leaves it")
}

/// Judge `ask` over the journal at `path`, keeping what it said.
fn judged(
    ask: &ResumeAsk<'_>,
    path: &Path,
) -> (Result<Continuation, ResumeRefusal>, Vec<ResumeNotice>) {
    let raw = std::fs::read_to_string(path).expect("the journal reads");
    let mut said = Vec::new();
    let result = continue_from(ask, &raw, &mut |notice| said.push(notice));
    (result, said)
}

#[test]
fn a_journal_of_this_workflow_and_engine_continues() {
    let dir = tempfile::tempdir().expect("dir");
    let path = journal(dir.path(), &[boot("hello", HELLO, ENGINE)]);
    let (wf, label) = (parsed(HELLO), path.display().to_string());
    let ask = ResumeAsk::new(&path, &label, &wf, HELLO, ENGINE);
    let (result, said) = judged(&ask, &path);
    let continuation = result.expect("it proceeds");
    assert!(continuation.plan.is_empty());
    assert!(continuation.unverified.is_none());
    assert_eq!(continuation.compat, None);
    assert_eq!(said, [ResumeNotice::NoKeys]);
}

#[test]
fn another_workflow_refuses_before_anything_is_said_of_it() {
    let dir = tempfile::tempdir().expect("dir");
    let path = journal(dir.path(), &[boot("compose-brief", BRIEF, ENGINE)]);
    let (wf, label) = (parsed(HELLO), path.display().to_string());
    let (result, said) = judged(&ResumeAsk::new(&path, &label, &wf, HELLO, ENGINE), &path);
    match result {
        Err(ResumeRefusal::Refused(message)) => assert!(message.contains("compose-brief")),
        other => panic!("another workflow refuses: {other:?}"),
    }
    assert!(said.is_empty(), "{said:?}");
    // The same workflow whose file changed proceeds, and says so first.
    let path = journal(dir.path(), &[boot("hello", BRIEF, ENGINE)]);
    let label = path.display().to_string();
    let (result, said) = judged(&ResumeAsk::new(&path, &label, &wf, HELLO, ENGINE), &path);
    assert!(result.is_ok());
    assert_eq!(said, [ResumeNotice::SourceChanged, ResumeNotice::NoKeys]);
}

#[test]
fn another_engine_refuses_unless_its_exact_version_is_declared() {
    let dir = tempfile::tempdir().expect("dir");
    let path = journal(dir.path(), &[boot("hello", HELLO, "0.1.0")]);
    let (wf, label) = (parsed(HELLO), path.display().to_string());
    let ask = ResumeAsk::new(&path, &label, &wf, HELLO, ENGINE);
    match judged(&ask, &path).0 {
        Err(ResumeRefusal::Refused(message)) => {
            assert!(
                message.contains("0.1.0") && message.contains(ENGINE),
                "{message}"
            );
        }
        other => panic!("another engine refuses: {other:?}"),
    }
    let (result, said) = judged(&ask.with_compat(Some("0.1.0")), &path);
    assert_eq!(result.expect("declared").compat.as_deref(), Some("0.1.0"));
    assert_eq!(
        said,
        [
            ResumeNotice::CrossVersion {
                recorded: "0.1.0".to_owned()
            },
            ResumeNotice::NoKeys
        ]
    );
}

#[test]
fn a_broken_chain_refuses_unless_waived_and_the_waiver_is_attested() {
    let dir = tempfile::tempdir().expect("dir");
    let path = journal(
        dir.path(),
        &[boot("hello", HELLO, ENGINE), scheduled("greet")],
    );
    // The same two lines in the other order: the chain no longer holds.
    let raw = std::fs::read_to_string(&path).expect("journal");
    let lines: Vec<&str> = raw.lines().collect();
    assert_eq!(lines.len(), 2, "{raw}");
    std::fs::write(&path, format!("{}\n{}\n", lines[1], lines[0])).expect("rewrite");
    let (wf, label) = (parsed(HELLO), path.display().to_string());
    let ask = ResumeAsk::new(&path, &label, &wf, HELLO, ENGINE);
    let (result, said) = judged(&ask, &path);
    assert!(
        matches!(result, Err(ResumeRefusal::BrokenChain { .. })),
        "{result:?}"
    );
    assert!(said.is_empty(), "{said:?}");
    let (result, said) = judged(&ask.waiving_a_broken_chain(true), &path);
    let continuation = result.expect("waived");
    assert!(matches!(
        continuation.unverified,
        Some(ResumeUnverified::Declared(_))
    ));
    assert!(
        matches!(said.first(), Some(ResumeNotice::BrokenChainWaived { .. })),
        "{said:?}"
    );
    assert_eq!(said.get(1), Some(&ResumeNotice::UnprotectedProject));
}

#[test]
fn a_journal_without_a_chain_is_trusted_unverified_or_refused_when_one_is_required() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("capture.ndjson");
    let line = serde_json::to_string(&boot("hello", HELLO, ENGINE)).expect("event json");
    std::fs::write(&path, format!("{line}\n")).expect("capture");
    let (wf, label) = (parsed(HELLO), path.display().to_string());
    let ask = ResumeAsk::new(&path, &label, &wf, HELLO, ENGINE);
    let (result, said) = judged(&ask, &path);
    assert!(matches!(
        result.expect("trusted unverified").unverified,
        Some(ResumeUnverified::Unchained(_))
    ));
    assert_eq!(
        said,
        [
            ResumeNotice::Unchained,
            ResumeNotice::UnprotectedProject,
            ResumeNotice::NoKeys
        ]
    );
    let (result, said) = judged(&ask.requiring_a_chain(), &path);
    assert!(
        matches!(result, Err(ResumeRefusal::Refused(_))),
        "{result:?}"
    );
    assert!(said.is_empty(), "{said:?}");
}

#[test]
fn a_moved_access_lane_refuses_unless_the_resume_judges_none() {
    let dir = tempfile::tempdir().expect("dir");
    let manifest = boot("hello", HELLO, ENGINE).with_field(KeyValue::new(
        "access_plan",
        text(r#"[{"model":"mock/echo","access":"mock","chosen":"mock"}]"#),
    ));
    let path = journal(dir.path(), &[manifest]);
    let (wf, label) = (parsed(HELLO), path.display().to_string());
    let moved: BTreeMap<String, LaneCarry> = BTreeMap::from([(
        "mock/echo".to_owned(),
        ("openai".to_owned(), "api".to_owned()),
    )]);
    let ask = ResumeAsk::new(&path, &label, &wf, HELLO, ENGINE);
    let (result, _) = judged(&ask.with_access(&moved, None), &path);
    assert!(
        matches!(result, Err(ResumeRefusal::AccessMoved(_))),
        "{result:?}"
    );
    assert!(
        judged(&ask, &path).0.is_ok(),
        "no access judged, none refused"
    );
}

#[test]
fn from_a_task_the_workflow_does_not_have_refuses() {
    let dir = tempfile::tempdir().expect("dir");
    let path = journal(dir.path(), &[boot("hello", HELLO, ENGINE)]);
    let (wf, label) = (parsed(HELLO), path.display().to_string());
    let ask = ResumeAsk::new(&path, &label, &wf, HELLO, ENGINE).from_task(Some("nowhere"));
    assert!(matches!(
        judged(&ask, &path).0,
        Err(ResumeRefusal::Refused(_))
    ));
}
