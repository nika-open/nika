// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

use nika_types::id::EventId;
use nika_types::resource::KeyValue;
use nika_types::timestamp::Timestamp;
use uuid::Uuid;

use super::*;

fn parse(source: &str) -> RawWorkflow {
    nika_schema::parse(
        source,
        nika_schema::FileId::new(0),
        nika_schema::ParseMode::Strict,
    )
    .expect("the workflow parses")
}

fn class(source: &str) -> EffectsClass {
    let wf = parse(source);
    effects_class(&wf, &nika_check::check(&wf))
}

fn text(value: &str) -> FieldValue {
    FieldValue::String(value.to_owned())
}

fn frame(kind: EventKind, fields: &[(&str, FieldValue)]) -> Event {
    let blank = Event::new(EventId::new(Uuid::nil()), Timestamp::from_unix_ms(0), kind);
    fields.iter().fold(blank, |event, (key, value)| {
        event.with_field(KeyValue::new(*key, value.clone()))
    })
}

fn scheduled(task: &str) -> Event {
    frame(EventKind::TaskScheduled, &[("task", text(task))])
}

fn completed(task: &str, fields: &[(&str, FieldValue)]) -> Event {
    let mut all = vec![("task", text(task))];
    all.extend(fields.iter().cloned());
    frame(EventKind::TaskCompleted, &all)
}

fn worst_case(wf: &RawWorkflow, task: &str) -> f64 {
    nika_check::check(wf)
        .cost
        .tasks
        .iter()
        .find(|row| row.task == task)
        .and_then(|row| row.usd)
        .expect("the task has a static bound")
}

const MOCK: &str = "nika: t\nmodel: mock/echo\npermits: {}\ntasks:\n  ask:\n    infer: { prompt: hi, max_tokens: 64 }\n";

#[test]
fn a_workflow_that_only_reads_and_computes_is_contained() {
    assert_eq!(class(MOCK), EffectsClass::Contained);
    for method in ["", ", method: GET", ", method: head"] {
        let fetch = format!(
            "nika: t\npermits: {{ net: {{ http: [example.com] }}, tools: [\"nika:fetch\"] }}\ntasks:\n  page:\n    invoke: {{ tool: \"nika:fetch\", args: {{ url: \"https://example.com/a\"{method} }} }}\n"
        );
        assert_eq!(class(&fetch), EffectsClass::Contained, "{method}");
    }
    let read = "nika: t\npermits: { fs: { read: [notes.md] }, tools: [\"nika:read\"] }\ntasks:\n  open:\n    invoke: { tool: \"nika:read\", args: { path: notes.md } }\n";
    assert_eq!(class(read), EffectsClass::Contained);
}

#[test]
fn a_fetch_that_may_write_is_an_external_effect() {
    for method in ["POST", "DELETE", "${{ inputs.verb }}"] {
        let fetch = format!(
            "nika: t\ninputs:\n  verb: {{ type: string, default: GET }}\npermits: {{ net: {{ http: [example.com] }}, tools: [\"nika:fetch\"] }}\ntasks:\n  page:\n    invoke: {{ tool: \"nika:fetch\", args: {{ url: \"https://example.com/a\", method: \"{method}\" }} }}\n"
        );
        assert_eq!(class(&fetch), EffectsClass::External, "{method}");
    }
}

#[test]
fn a_program_a_written_file_and_a_child_workflow_are_external_effects() {
    let exec =
        "nika: t\npermits: { exec: [echo] }\ntasks:\n  run:\n    exec: { command: [echo, hi] }\n";
    let write = "nika: t\npermits: { fs: { write: [out.txt] }, tools: [\"nika:write\"] }\ntasks:\n  save:\n    invoke: { tool: \"nika:write\", args: { path: out.txt, content: x } }\n";
    let child =
        "nika: t\npermits: {}\ntasks:\n  call:\n    invoke: { workflow: \"./child.nika\" }\n";
    let computed = "nika: t\ninputs:\n  file: { type: string, default: notes.md }\npermits: { fs: { read: [\"*.md\"] }, tools: [\"nika:read\"] }\ntasks:\n  open:\n    invoke: { tool: \"nika:read\", args: { path: \"${{ inputs.file }}\" } }\n";
    for source in [exec, write, child, computed] {
        assert_eq!(class(source), EffectsClass::External, "{source}");
    }
}

#[test]
fn an_agent_is_contained_only_off_a_harness_and_with_reading_tools() {
    let agent = |tools: &str| {
        format!(
            "nika: t\nmodel: mock/echo\npermits: {{ fs: {{ read: [\"*.md\"] }}, tools: [{tools}] }}\ntasks:\n  dig:\n    agent: {{ prompt: look, tools: [{tools}], max_turns: 2, max_tokens_total: 256 }}\n"
        )
    };
    assert_eq!(
        class(&agent("\"nika:read\"")),
        EffectsClass::ContainedOffHarness
    );
    assert_eq!(
        class(&agent("\"nika:read\", \"!nika:write\"")),
        EffectsClass::ContainedOffHarness
    );
    for tools in ["\"nika:fetch\"", "\"nika:*\"", "\"nika:write\""] {
        assert_eq!(class(&agent(tools)), EffectsClass::External, "{tools}");
    }
}

#[test]
fn the_stored_word_of_a_class_names_it_back() {
    for class in [
        EffectsClass::Contained,
        EffectsClass::ContainedOffHarness,
        EffectsClass::External,
    ] {
        assert_eq!(EffectsClass::parse(class.as_str()), Some(class));
    }
    assert_eq!(EffectsClass::parse("unknown"), None);
}

#[test]
fn only_a_stated_plain_route_is_read_as_off_a_harness() {
    let boot = |plan: &str| frame(EventKind::WorkflowStarted, &[("access_plan", text(plan))]);
    assert!(
        !rode_harness(&[]),
        "a journal with no boot record ran nothing"
    );
    assert!(!rode_harness(&[boot(
        r#"[{"model":"mock/echo","access":"mock","chosen":"mock"}]"#
    )]));
    assert!(!rode_harness(&[boot(
        r#"[{"model":"openai/gpt-4o-mini","access":"openai","chosen":"api"}]"#
    )]));
    assert!(rode_harness(&[boot(
        r#"[{"model":"openai/gpt-4o-mini","access":"openai","chosen":"api"},{"model":"x/y","access":"codex","chosen":"harness"}]"#
    )]));
    assert!(
        rode_harness(&[frame(EventKind::WorkflowStarted, &[])]),
        "a started run that states no plan is not read as a plain route"
    );
    // An older row that names no chosen route, in either shape.
    assert!(rode_harness(&[boot(
        r#"[{"model":"x/y","access":"some-agent"}]"#
    )]));
    assert!(rode_harness(&[boot(r#"{"x/y":{"access":"some-agent"}}"#)]));
    assert!(!rode_harness(&[boot(r#"{"mock/echo":{"access":"mock"}}"#)]));
    assert!(rode_harness(&[boot("not json")]));
}

const PRICED: &str = "nika: t\nmodel: anthropic/claude-sonnet-4-6\npermits: {}\ntasks:\n  first:\n    infer: { prompt: hi, max_tokens: 100 }\n  second:\n    with: { seen: \"${{ tasks.first.output }}\" }\n    infer: { prompt: \"again ${{ with.seen }}\", max_tokens: 200 }\n  third:\n    with: { seen: \"${{ tasks.second.output }}\" }\n    infer: { prompt: \"last ${{ with.seen }}\", max_tokens: 400 }\noutputs:\n  last: \"${{ tasks.third.output }}\"\n";

#[test]
fn a_journal_that_scheduled_nothing_spent_nothing() {
    let wf = parse(PRICED);
    let boot = frame(EventKind::WorkflowStarted, &[]);
    let spend = interrupted_spend(&wf, &BTreeMap::new(), &[boot]);
    assert_eq!((spend.usd, spend.unknown), (0.0, false));
}

#[test]
fn a_task_that_may_have_been_dispatched_counts_at_its_bound() {
    let wf = parse(PRICED);
    let journal = [
        scheduled("first"),
        scheduled("second"),
        scheduled("third"),
        completed("first", &[("cost_usd", FieldValue::Float(0.02))]),
    ];
    let spend = interrupted_spend(&wf, &BTreeMap::new(), &journal);
    let second = worst_case(&wf, "second");
    assert!(second > 0.0);
    // `third` waits on `second`: its wave never existed.
    assert!((spend.usd - (0.02 + second)).abs() < 1e-12, "{spend:?}");
    assert!(!spend.unknown);

    // Killed before the first task settled: only the first wave counts.
    let spend = interrupted_spend(&wf, &BTreeMap::new(), &journal[..3]);
    assert!(
        (spend.usd - worst_case(&wf, "first")).abs() < 1e-12,
        "{spend:?}"
    );
}

#[test]
fn a_call_in_flight_through_an_agent_application_is_unknown_spend() {
    let wf = parse(PRICED);
    let boot = |plan: &str| frame(EventKind::WorkflowStarted, &[("access_plan", text(plan))]);
    let journal = |plan: &str| {
        [
            boot(plan),
            scheduled("first"),
            scheduled("second"),
            scheduled("third"),
        ]
    };
    let api = r#"[{"model":"anthropic/claude-sonnet-4-6","access":"anthropic","chosen":"api"}]"#;
    let priced = interrupted_spend(&wf, &BTreeMap::new(), &journal(api));
    assert!(
        (priced.usd - worst_case(&wf, "first")).abs() < 1e-12,
        "{priced:?}"
    );
    assert!(!priced.unknown, "a plain route is priced at its bound");
    let harness =
        r#"[{"model":"anthropic/claude-sonnet-4-6","access":"claude","chosen":"harness"}]"#;
    let quota = interrupted_spend(&wf, &BTreeMap::new(), &journal(harness));
    assert!(
        quota.unknown,
        "a subscription lane bills no catalog price: {quota:?}"
    );
    // Nothing in flight: what settled is all there is to count.
    let mut settled = journal(harness).to_vec();
    for task in ["first", "second", "third"] {
        settled.push(completed(task, &[("cost_usd", FieldValue::Float(0.0))]));
    }
    let done = interrupted_spend(&wf, &BTreeMap::new(), &settled);
    assert_eq!((done.usd, done.unknown), (0.0, false));
    // A tool in flight calls no model: no route bills it.
    let tool = parse(
        "nika: t\npermits: { tools: [\"nika:wait\"] }\ntasks:\n  pause:\n    invoke: { tool: \"nika:wait\", args: { duration: \"1s\" } }\n",
    );
    let unstated = frame(EventKind::WorkflowStarted, &[]);
    let waiting = interrupted_spend(&tool, &BTreeMap::new(), &[unstated, scheduled("pause")]);
    assert_eq!((waiting.usd, waiting.unknown), (0.0, false));
}

#[test]
fn a_route_that_reasons_beyond_its_bound_leaves_its_call_in_flight_unknown() {
    let reasoning = PRICED.replace("anthropic/claude-sonnet-4-6", "xai/grok-4.7");
    let wf = parse(&reasoning);
    let report = nika_check::check(&wf);
    let row = report
        .cost
        .tasks
        .iter()
        .find(|row| row.task == "first")
        .expect("the first task has a cost row");
    assert!(row.reasoning_beyond, "{row:?}");
    let in_flight = interrupted_spend(&wf, &BTreeMap::new(), &[scheduled("first")]);
    assert!(in_flight.unknown, "{in_flight:?}");
    if let Some(bound) = row.usd {
        assert!((in_flight.usd - bound).abs() < 1e-12, "{in_flight:?}");
    }
    // A call of the same route that settled stated its charge: known.
    let settled = interrupted_spend(
        &wf,
        &BTreeMap::new(),
        &[
            scheduled("first"),
            completed("first", &[("cost_usd", FieldValue::Float(0.03))]),
            completed("second", &[("cost_usd", FieldValue::Float(0.01))]),
            completed("third", &[("cost_usd", FieldValue::Float(0.01))]),
        ],
    );
    assert!((settled.usd - 0.05).abs() < 1e-12, "{settled:?}");
    assert!(!settled.unknown, "{settled:?}");
}

#[test]
fn a_child_workflow_in_flight_is_unknown_spend() {
    let parent = "nika: parent\nmodel: anthropic/claude-sonnet-4-6\npermits: {}\ntasks:\n  call:\n    invoke:\n      workflow: \"./child.nika\"\n";
    let wf = parse(parent);
    let in_flight = interrupted_spend(&wf, &BTreeMap::new(), &[scheduled("call")]);
    assert!(in_flight.unknown, "{in_flight:?}");
    // Settled, the child's spend is what its parent's record states.
    let settled = interrupted_spend(
        &wf,
        &BTreeMap::new(),
        &[
            scheduled("call"),
            completed("call", &[("cost_usd", FieldValue::Float(0.04))]),
        ],
    );
    assert_eq!((settled.usd, settled.unknown), (0.04, false));
}

#[test]
fn a_deferred_recovery_counts_every_unsettled_task() {
    let recovering = PRICED.replace(
        "  second:\n",
        "  second:\n    on_error: { recover: fallback }\n",
    );
    let wf = parse(&recovering);
    let journal = [
        scheduled("first"),
        scheduled("second"),
        scheduled("third"),
        completed("first", &[("cost_usd", FieldValue::Float(0.02))]),
    ];
    let spend = interrupted_spend(&wf, &BTreeMap::new(), &journal);
    let expected = 0.02 + worst_case(&wf, "second") + worst_case(&wf, "third");
    assert!((spend.usd - expected).abs() < 1e-12, "{spend:?}");
}

#[test]
fn spend_that_cannot_be_bounded_is_unknown_and_never_zero() {
    // A settled task whose provider did not meter it.
    let wf = parse(PRICED);
    for (reason, unknown) in [
        ("subscription_quota", true),
        ("missing_catalog_price", true),
        ("provider_did_not_report_usage", true),
        ("mock_provider", false),
        ("local_model", false),
    ] {
        let journal = [
            scheduled("first"),
            completed("first", &[("cost_unpriced", text(reason))]),
            completed("second", &[]),
            completed("third", &[]),
        ];
        let spend = interrupted_spend(&wf, &BTreeMap::new(), &journal);
        assert_eq!(spend.unknown, unknown, "{reason}");
    }
    let uncounted = [
        scheduled("first"),
        completed("first", &[("cost_unknown_calls", FieldValue::Int(1))]),
        completed("second", &[]),
        completed("third", &[]),
    ];
    assert!(interrupted_spend(&wf, &BTreeMap::new(), &uncounted).unknown);

    // A task in flight with no token limit has no bound.
    let unbounded = parse(
        "nika: t\nmodel: anthropic/claude-sonnet-4-6\npermits: {}\ntasks:\n  ask:\n    infer: { prompt: hi }\n",
    );
    let spend = interrupted_spend(&unbounded, &BTreeMap::new(), &[scheduled("ask")]);
    assert!(spend.unknown, "{spend:?}");

    // The mock in flight is bounded at nothing, and known.
    let mock = parse(MOCK);
    let spend = interrupted_spend(&mock, &BTreeMap::new(), &[scheduled("ask")]);
    assert_eq!((spend.usd, spend.unknown), (0.0, false));
}
