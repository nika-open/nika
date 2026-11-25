// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>
#![allow(
    clippy::expect_used,
    reason = "fixture construction and required findings are test assertions"
)]

//! The untrusted-content label (the lethal trifecta `NIKA-SEC-009` and the
//! branch-steering hint) across the shapes it used to lose: a `group.<name>`
//! fold, a `recover:` whose source the sweep reaches later, an unwind cleanup
//! task, and a `with:` binding that reads the `for_each` item. Each group
//! shape is the one-line twin of a direct `tasks.<id>.output` read and must
//! be judged exactly like it: the same code, the same sink, the same named
//! source. Parse and check only: no filesystem, no network, no effect.

use nika_check::{CheckReport, check, check_composed};
use nika_schema::{ParseMode, parse, source::FileId};

/// The direct read of the untrusted member.
const DIRECT: &str = "${{ tasks.page.output }}";
/// The fold over the group the member declares.
const FOLD: &str = "${{ group.pages }}";

fn report(source: &str) -> CheckReport {
    let wf = parse(source, FileId::new(0), ParseMode::Strict).expect("the fixture parses");
    check(&wf)
}

/// Every `NIKA-SEC-009` finding as `(sink, source)`, after checking that the
/// file is well-formed, that the unified list carries the coded row for each
/// sink, and that the check is not clean exactly when a finding exists.
fn trifecta(report: &CheckReport) -> Vec<(String, String)> {
    assert!(
        report.conformance.is_empty(),
        "the fixture is valid Nika: {:?}",
        report.conformance
    );
    let rows: Vec<(String, String)> = report
        .trifecta_findings
        .iter()
        .map(|v| {
            (
                v.task.clone(),
                v.source.clone().expect("a refusal names its source"),
            )
        })
        .collect();
    for (sink, _) in &rows {
        assert!(
            report.findings.iter().any(|f| f.gate == "TRIFECTA"
                && f.code.as_deref() == Some("NIKA-SEC-009")
                && f.task.as_deref() == Some(sink.as_str())),
            "the unified list carries the coded row for `{sink}`: {:?}",
            report.findings
        );
    }
    assert_eq!(
        report.is_clean(),
        rows.is_empty(),
        "a SEC-009 finding gates the check: {:?}",
        report.findings
    );
    rows
}

fn pairs(rows: &[(&str, &str)]) -> Vec<(String, String)> {
    rows.iter()
        .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
        .collect()
}

/// The branch-steering hints as `(task, advice)`.
fn steering(report: &CheckReport) -> Vec<(String, String)> {
    report
        .hints
        .iter()
        .filter(|h| h.kind == "branch-steering")
        .map(|h| (h.task.clone(), h.advice.clone()))
        .collect()
}

/// Render a fixture whose `{read}` slot reads the member directly or through
/// the fold, and whose `{source}` slot is the member's verb.
fn render(template: &str, read: &str, source: &str) -> String {
    template.replace("{read}", read).replace("{source}", source)
}

/// The untrusted member: a fetched page.
const FETCH: &str =
    r#"invoke: { tool: "nika:fetch", args: { url: "https://feed.example.com/a" } }"#;
/// The clean member: an authored literal through jq.
const CLEAN: &str = r#"invoke: { tool: "nika:jq", args: { expression: ".", input: "fixed" } }"#;

const PERMITS: &str = r#"permits:
  tools: ["nika:fetch", "nika:jq"]
  fs: { read: ["./notes/**"] }
  net: { http: ["feed.example.com", "sink.example.com"] }
"#;

/// a · the fold reaches an egress task's arguments.
const A_TRIFECTA: &str = r#"nika: a-trifecta-fetch
{permits}tasks:
  page:
    group: pages
    {source}
  send:
    with: { got: "{read}" }
    invoke: { tool: "nika:fetch", args: { url: "https://sink.example.com/in", method: POST, body: "${{ with.got }}" } }
"#;

/// d · the fold is laundered through a jq hop before the egress.
const D_LAUNDER: &str = r#"nika: d-launder
{permits}tasks:
  page:
    group: pages
    {source}
  merge:
    with: { got: "{read}" }
    invoke: { tool: "nika:jq", args: { expression: ".", input: "${{ with.got }}" } }
  send:
    with: { body: "${{ tasks.merge.output }}" }
    invoke: { tool: "nika:fetch", args: { url: "https://sink.example.com/in", method: POST, body: "${{ with.body }}" } }
"#;

/// g · a clean effect whose recover value is the fold's alias.
const G_RECOVER: &str = r#"nika: g-recover-alias
{permits}tasks:
  page:
    group: pages
    {source}
  stage:
    with: { got: "{read}" }
    invoke: { tool: "nika:jq", args: { expression: ".", input: "fixed" } }
    on_error: { recover: "${{ with.got }}" }
  send:
    with: { body: "${{ tasks.stage.output }}" }
    invoke: { tool: "nika:fetch", args: { url: "https://sink.example.com/in", method: POST, body: "${{ with.body }}" } }
"#;

/// i · a `for_each` over the fold's alias.
const I_FOREACH: &str = r#"nika: i-foreach
{permits}tasks:
  page:
    group: pages
    {source}
  send:
    with: { items: "{read}" }
    for_each: { items: "${{ with.items }}" }
    invoke: { tool: "nika:fetch", args: { url: "https://sink.example.com/in", method: POST, body: "${{ item }}" } }
"#;

/// k · the fold is written to a file another task reads and sends.
const K_FILE_CHANNEL: &str = r#"nika: k-file-channel
permits:
  tools: ["nika:fetch", "nika:jq", "nika:write", "nika:read"]
  fs: { read: ["./notes/**"], write: ["./notes/**"] }
  net: { http: ["feed.example.com", "sink.example.com"] }
tasks:
  page:
    group: pages
    {source}
  stash:
    with: { got: "{read}" }
    invoke: { tool: "nika:write", args: { path: "./notes/page.txt", content: "${{ with.got }}" } }
  load:
    invoke: { tool: "nika:read", args: { path: "./notes/page.txt" } }
  send:
    with: { body: "${{ tasks.load.output }}" }
    invoke: { tool: "nika:fetch", args: { url: "https://sink.example.com/in", method: POST, body: "${{ with.body }}" } }
"#;

/// b · an exec member's output reaches another exec's argv.
const B_EXEC: &str = r#"nika: b-exec
permits:
  exec: ["git", "tar"]
  tools: ["nika:jq"]
  fs: { read: ["./repo/**"] }
tasks:
  subjects:
    group: logs
    {source}
  pack:
    with: { got: "{read}" }
    exec: { command: ["tar", "-cf", "out.tar", "${{ with.got }}"] }
"#;

/// Check the group twin and the direct twin of one shape, with the untrusted
/// member and with a clean one. Returns the refusal both twins share.
fn twins(template: &str, direct: &str, fold: &str, untrusted: &str) -> Vec<(String, String)> {
    let template = template.replace("{permits}", PERMITS);
    let task = trifecta(&report(&render(&template, direct, untrusted)));
    let group = trifecta(&report(&render(&template, fold, untrusted)));
    assert_eq!(
        group, task,
        "the fold is judged exactly like the direct read"
    );
    for read in [direct, fold] {
        assert_eq!(
            trifecta(&report(&render(&template, read, CLEAN))),
            Vec::new(),
            "a clean member reaches no sink through `{read}`"
        );
    }
    group
}

#[test]
fn a_fold_reaching_an_egress_argument_is_refused_like_the_direct_read() {
    assert_eq!(
        twins(A_TRIFECTA, DIRECT, FOLD, FETCH),
        pairs(&[("send", "page")])
    );
}

#[test]
fn a_fold_laundered_through_a_jq_hop_is_refused_like_the_direct_read() {
    assert_eq!(
        twins(D_LAUNDER, DIRECT, FOLD, FETCH),
        pairs(&[("send", "page")])
    );
}

#[test]
fn a_recover_through_a_fold_alias_is_refused_like_the_direct_read() {
    assert_eq!(
        twins(G_RECOVER, DIRECT, FOLD, FETCH),
        pairs(&[("send", "page")])
    );
}

#[test]
fn a_for_each_over_a_fold_alias_is_refused_like_the_direct_read() {
    assert_eq!(
        twins(I_FOREACH, DIRECT, FOLD, FETCH),
        pairs(&[("send", "page")])
    );
}

#[test]
fn a_fold_written_to_a_file_arms_the_channel_like_the_direct_read() {
    // The writer and the sink that reads the file back are both refused.
    assert_eq!(
        twins(K_FILE_CHANNEL, DIRECT, FOLD, FETCH),
        pairs(&[("stash", "page"), ("send", "page")])
    );
}

#[test]
fn an_exec_member_folded_into_an_exec_argv_is_refused_like_the_direct_read() {
    let exec = r#"exec: { command: ["git", "log", "--format=%s", "-n", "1"] }"#;
    let direct = "${{ tasks.subjects.output }}";
    let fold = "${{ group.logs }}";
    let task = trifecta(&report(&render(B_EXEC, direct, exec)));
    let group = trifecta(&report(&render(B_EXEC, fold, exec)));
    assert_eq!(task, pairs(&[("pack", "subjects")]));
    assert_eq!(
        group, task,
        "the fold is judged exactly like the direct read"
    );
    assert_eq!(
        trifecta(&report(&render(B_EXEC, fold, CLEAN))),
        Vec::new(),
        "a clean member keeps the exec sink green"
    );
}

#[test]
fn the_fold_names_the_first_untrusted_member_in_declaration_order() {
    let two = |first: &str, second: &str| {
        format!(
            "nika: order\n{PERMITS}tasks:\n  {first}:\n    group: pages\n    {FETCH}\n  {second}:\n    group: pages\n    {FETCH}\n  send:\n    with: {{ got: \"{FOLD}\" }}\n    invoke: {{ tool: \"nika:fetch\", args: {{ url: \"https://sink.example.com/in\", method: POST, body: \"${{{{ with.got }}}}\" }} }}\n"
        )
    };
    assert_eq!(
        trifecta(&report(&two("zeta", "alpha"))),
        pairs(&[("send", "zeta")]),
        "declaration order, not id order"
    );
    // A clean member declared first does not hide the untrusted one.
    let mixed = format!(
        "nika: order\n{PERMITS}tasks:\n  notes:\n    group: pages\n    {CLEAN}\n  page:\n    group: pages\n    {FETCH}\n  send:\n    with: {{ got: \"{FOLD}\" }}\n    invoke: {{ tool: \"nika:fetch\", args: {{ url: \"https://sink.example.com/in\", method: POST, body: \"${{{{ with.got }}}}\" }} }}\n"
    );
    assert_eq!(trifecta(&report(&mixed)), pairs(&[("send", "page")]));
}

/// h · the recover source `page` sits before, in a later wave than, or after
/// the recovering task. The run awaits the source in every case.
const H_STAGE: &str = r#"  stage:
    invoke: { tool: "nika:jq", args: { expression: ".", input: "fixed" } }
    on_error: { recover: "${{ tasks.page.output }}" }
"#;
const H_PAGE: &str = r#"  page:
    invoke: { tool: "nika:fetch", args: { url: "https://feed.example.com/a" } }
"#;
const H_SEND: &str = r#"  send:
    with: { body: "${{ tasks.stage.output }}" }
    invoke: { tool: "nika:fetch", args: { url: "https://sink.example.com/in", method: POST, body: "${{ with.body }}" } }
"#;

#[test]
fn a_recover_value_is_judged_whatever_order_its_source_takes() {
    let head = format!("nika: h-recover-order\n{PERMITS}tasks:\n");
    let source_first = format!("{head}{H_PAGE}{H_STAGE}{H_SEND}");
    let source_after = format!("{head}{H_STAGE}{H_PAGE}{H_SEND}");
    let later_wave = format!(
        "{head}  warm:\n    {CLEAN}\n{}{H_STAGE}{H_SEND}",
        H_PAGE.replace("  page:\n", "  page:\n    after: { warm: success }\n")
    );
    for (name, source) in [
        ("source first", source_first),
        ("source in a later wave", later_wave),
        ("source declared after", source_after),
    ] {
        assert_eq!(
            trifecta(&report(&source)),
            pairs(&[("send", "page")]),
            "{name}"
        );
    }
}

#[test]
fn an_unwind_cleanup_task_is_judged_like_a_success_follower() {
    let file = |edge: &str| {
        format!(
            "nika: u-unwind\npermits:\n  tools: [\"nika:fetch\"]\n  fs: {{ read: [\"./notes/**\"] }}\n  net: {{ http: [\"feed.example.com\", \"sink.example.com\"] }}\ntasks:\n{H_PAGE}  send:\n    after: {{ page: {edge} }}\n    with: {{ got: \"{DIRECT}\" }}\n    invoke: {{ tool: \"nika:fetch\", args: {{ url: \"https://sink.example.com/in\", method: POST, body: \"${{{{ with.got }}}}\" }} }}\n"
        )
    };
    let success = trifecta(&report(&file("success")));
    assert_eq!(success, pairs(&[("send", "page")]));
    assert_eq!(
        trifecta(&report(&file("unwind"))),
        success,
        "the cleanup task is a sink like any other"
    );
}

/// f · the fold decides whether an egress task runs, through a jq hop.
const F_STEER: &str = r#"nika: f-steer-transitive
{permits}tasks:
  page:
    group: pages
    {source}
  digest:
    with: { got: "{read}" }
    invoke: { tool: "nika:jq", args: { expression: ".", input: "${{ with.got }}" } }
  send:
    with: { flag: "${{ tasks.digest.output }}" }
    when: "${{ with.flag != null }}"
    invoke: { tool: "nika:fetch", args: { url: "https://sink.example.com/in", method: POST, body: "fixed" } }
"#;

#[test]
fn a_fold_steering_an_egress_branch_gets_the_hint_like_the_direct_read() {
    let template = F_STEER.replace("{permits}", PERMITS);
    let task = steering(&report(&render(&template, DIRECT, FETCH)));
    let group = steering(&report(&render(&template, FOLD, FETCH)));
    assert_eq!(task.len(), 1, "{task:?}");
    assert_eq!(task[0].0, "send");
    assert!(
        task[0]
            .1
            .contains("the output of `digest`, which carries untrusted content from `page`"),
        "{}",
        task[0].1
    );
    assert_eq!(group, task, "the fold steers exactly like the direct read");
    assert_eq!(
        steering(&report(&render(&template, FOLD, CLEAN))),
        Vec::new(),
        "a clean member steers nothing"
    );
}

/// The `with:` binding reads the `for_each` item (spec 03 · rendered once per
/// iteration): it carries what the item carries. `{body}` is the sink's read.
const ITEM_BINDING: &str = r#"nika: item-binding
{permits}tasks:
  page:
    group: pages
    {source}
  relay:
    with: { all: "{read}", one: "${{ item }}" }
    for_each: { items: "${{ with.all }}", max_items: 10 }
    invoke: { tool: "nika:fetch", args: { url: "https://sink.example.com/in", method: POST, body: "{body}" } }
"#;

#[test]
fn a_with_binding_reading_the_item_is_refused_like_the_item_itself() {
    let template = ITEM_BINDING.replace("{permits}", PERMITS);
    let file = |read: &str, body: &str, source: &str| {
        report(&render(&template, read, source).replace("{body}", body))
    };
    let direct_item = trifecta(&file(DIRECT, "${{ item }}", FETCH));
    assert_eq!(direct_item, pairs(&[("relay", "page")]));
    for read in [DIRECT, FOLD] {
        assert_eq!(
            trifecta(&file(read, "${{ with.one }}", FETCH)),
            direct_item,
            "the binding over the item, collection read through `{read}`"
        );
        assert_eq!(
            trifecta(&file(read, "${{ with.one }}", CLEAN)),
            Vec::new(),
            "a clean collection through `{read}`"
        );
    }
}

#[test]
fn a_recover_through_an_item_binding_is_refused_like_the_item_itself() {
    let file = |recover: &str| {
        format!(
            "nika: item-recover\n{PERMITS}tasks:\n{H_PAGE}  relay:\n    with: {{ all: \"{DIRECT}\", one: \"${{{{ item }}}}\" }}\n    for_each: {{ items: \"${{{{ with.all }}}}\", max_items: 10 }}\n    invoke: {{ tool: \"nika:jq\", args: {{ expression: \".\", input: \"fixed\" }} }}\n    on_error: {{ recover: \"{recover}\" }}\n  send:\n    with: {{ body: \"${{{{ tasks.relay.output }}}}\" }}\n    invoke: {{ tool: \"nika:fetch\", args: {{ url: \"https://sink.example.com/in\", method: POST, body: \"${{{{ with.body }}}}\" }} }}\n"
        )
    };
    let direct = trifecta(&report(&file("${{ item }}")));
    assert_eq!(direct, pairs(&[("send", "page")]));
    assert_eq!(trifecta(&report(&file("${{ with.one }}"))), direct);
}

/// A child that folds its fetched page into its own egress task.
const CHILD: &str = r#"nika: child
permits:
  tools: ["nika:fetch"]
  net: { http: ["feed.example.com", "sink.example.com"] }
tasks:
  page:
    group: pages
    invoke: { tool: "nika:fetch", args: { url: "https://feed.example.com/a" } }
  send:
    with: { got: "{read}" }
    invoke: { tool: "nika:fetch", args: { url: "https://sink.example.com/in", method: POST, body: "${{ with.got }}" } }
"#;

const PARENT: &str = r#"nika: parent
permits:
  tools: ["nika:read", "nika:fetch"]
  fs: { read: ["./notes/**"] }
  net: { http: ["feed.example.com", "sink.example.com"] }
tasks:
  call:
    invoke:
      workflow: "./child.nika"
"#;

#[test]
fn a_child_that_folds_untrusted_content_into_egress_is_refused_like_its_direct_twin() {
    let composed = |read: &str| {
        let child = CHILD.replace("{read}", read);
        let parent = parse(PARENT, FileId::new(0), ParseMode::Strict).expect("parent parses");
        check_composed(&parent, "parent.nika", &mut |path| {
            if path == "child.nika" {
                Ok(child.clone())
            } else {
                Err(format!("No such file or directory: {path}"))
            }
        })
    };
    let task = composed(DIRECT);
    assert!(task.composition.is_empty(), "{:?}", task.composition);
    let task = trifecta(&task);
    assert_eq!(task, pairs(&[("call", "call")]));
    let group = composed(FOLD);
    assert!(group.composition.is_empty(), "{:?}", group.composition);
    assert_eq!(
        trifecta(&group),
        task,
        "the child's fold reaches its egress"
    );
}

/// A literal `for_each:` list is rendered as a template tree: a string in it
/// that reads the fold's alias carries the fold's label into the item.
const LITERAL_LIST: &str = r#"nika: literal-list
{permits}tasks:
  page:
    group: pages
    {source}
  relay:
    with: { all: "{read}" }
    for_each: { items: ["${{ with.all }}", "fixed"] }
    invoke: { tool: "nika:fetch", args: { url: "https://sink.example.com/in", method: POST, body: "{body}" } }
"#;

#[test]
fn a_literal_list_reading_a_fold_is_refused_like_the_direct_read() {
    let template = LITERAL_LIST.replace("{permits}", PERMITS);
    let file = |read: &str, body: &str, source: &str| {
        report(&render(&template, read, source).replace("{body}", body))
    };
    for body in ["${{ item }}", "${{ with.all }}"] {
        let task = trifecta(&file(DIRECT, body, FETCH));
        assert_eq!(task, pairs(&[("relay", "page")]), "{body}");
        assert_eq!(
            trifecta(&file(FOLD, body, FETCH)),
            task,
            "{body} through the fold"
        );
    }
    assert_eq!(
        trifecta(&file(FOLD, "${{ item }}", CLEAN)),
        Vec::new(),
        "a list over a clean fold stays clean"
    );
}

#[test]
fn a_per_iteration_binding_over_an_authored_list_carries_only_what_it_reads() {
    // The items are authored, so a binding of the item alone stays clean; a
    // per-iteration binding that also reads the fetched page carries it.
    let file = |one: &str| {
        format!(
            "nika: per-item\n{PERMITS}tasks:\n{H_PAGE}  relay:\n    with: {{ one: \"{one}\" }}\n    for_each: {{ items: [\"a\", \"b\"] }}\n    invoke: {{ tool: \"nika:fetch\", args: {{ url: \"https://sink.example.com/in\", method: POST, body: \"${{{{ with.one }}}}\" }} }}\n"
        )
    };
    assert_eq!(trifecta(&report(&file("${{ item }}"))), Vec::new());
    assert_eq!(
        trifecta(&report(&file("${{ item }} ${{ tasks.page.output }}"))),
        pairs(&[("relay", "page")])
    );
}
