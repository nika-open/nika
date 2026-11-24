// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! The text-answer gates of the trifecta lane: only a confirm gate counts by
//! its position alone; a `choice` or an `input` gate counts before an egress
//! task only where that task's own `when:` tests the answer, directly or
//! through a `nika:jq` stage that transforms it.

use crate::check;
use nika_schema::parser::{ParseMode, parse};
use nika_schema::source::FileId;

fn report(yaml: &str) -> crate::CheckReport {
    check(&parse(yaml, FileId::new(0), ParseMode::Strict).expect("fixture parses"))
}

/// A private note and a fetched page, both shown to a `choice` gate,
/// then a POST of the answer that waits on the gate and tests nothing.
const CHOICE: &str = r#"nika: choice-gate
permits:
  tools: ["nika:read", "nika:fetch", "nika:prompt"]
  net: { http: ["www.corp-news-example.com", "api.corp-hooks-example.com"] }
  fs:
    read: ["./notes/private.md"]
tasks:
  brief:
    invoke: { tool: "nika:read", args: { path: "./notes/private.md" } }
  page:
    invoke: { tool: "nika:fetch", args: { url: "https://www.corp-news-example.com/today", mode: text } }
  human:
    with: { brief: "${{ tasks.brief.output }}", page: "${{ tasks.page.output }}" }
    invoke:
      tool: "nika:prompt"
      args: { mode: "choice", message: "Send this? ${{ with.brief }} / ${{ with.page }}", choices: ["send", "no"] }
  send:
    after: { human: success }
    with: { pick: "${{ tasks.human.output }}" }
    invoke:
      tool: "nika:fetch"
      args: { url: "https://api.corp-hooks-example.com/hook", method: POST, body: { pick: "${{ with.pick }}" } }
"#;

/// The `send` binding line of [`CHOICE`].
const SEND_PICK: &str = "    with: { pick: \"${{ tasks.human.output }}\" }\n    invoke:\n      tool: \"nika:fetch\"\n      args: { url: \"https://api.corp-hooks-example.com/hook\"";

/// One edit of a fixture, refused when the text it replaces is gone.
fn swap(yaml: &str, from: &str, to: &str) -> String {
    assert_eq!(yaml.matches(from).count(), 1, "fixture drift on `{from}`");
    yaml.replacen(from, to, 1)
}

/// [`CHOICE`] with `when` on `send`.
fn choice_with_guard(when: &str) -> String {
    swap(
        CHOICE,
        SEND_PICK,
        &SEND_PICK.replacen(
            "\n    invoke:",
            &format!("\n    when: {when}\n    invoke:"),
            1,
        ),
    )
}

fn sinks(r: &crate::CheckReport) -> Vec<(&str, Option<&str>)> {
    r.trifecta_findings
        .iter()
        .map(|v| (v.task.as_str(), v.source.as_deref()))
        .collect()
}

/// The choice's answer does not guard `send`: it runs whichever answer
/// the person picks, « no » included, so the gate is not credited and
/// the trifecta is refused as for no gate, saying why and how to repair.
#[test]
fn a_choice_gate_whose_answer_the_effect_does_not_test_is_not_credited() {
    let r = report(CHOICE);
    assert!(r.conformance.is_empty(), "{:?}", r.conformance);
    assert_eq!(
        sinks(&r),
        [("send", Some("page"))],
        "{:?}",
        r.trifecta_findings
    );
    assert!(
        r.trifecta_mitigations.is_empty(),
        "{:?}",
        r.trifecta_mitigations
    );
    let codes: Vec<String> = r
        .extra_conformance_codes()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(
        codes.iter().filter(|c| *c == "NIKA-SEC-009").count(),
        1,
        "{codes:?}"
    );
    assert!(!r.is_clean());
    let d = &r.trifecta_findings[0].detail;
    assert!(
        d.starts_with("lethal trifecta complete · human gate required"),
        "{d}"
    );
    for part in [
        "the gate `human` dominates `send`, but `send` does not test its answer",
        "`send` runs whatever the answer",
        "test the answer in the `when:` of `send`",
        "`with: { pick: \"${{ tasks.human.output }}\" }`",
        "`mode: confirm`",
    ] {
        assert!(d.contains(part), "missing `{part}`: {d}");
    }
    assert!(
        !d.contains("no blocking `invoke: nika:prompt` dominates"),
        "the gate dominates, so the generic clause would be false: {d}"
    );
}

/// The repair as taught: `send` tests the answer, the gate is credited
/// and named by the tick.
#[test]
fn a_choice_gate_whose_answer_the_effect_tests_is_credited() {
    let r = report(&choice_with_guard("\"${{ with.pick == 'send' }}\""));
    assert!(r.conformance.is_empty(), "{:?}", r.conformance);
    assert!(r.trifecta_findings.is_empty(), "{:?}", r.trifecta_findings);
    let credited: Vec<(&str, &str)> = r
        .trifecta_mitigations
        .iter()
        .map(|m| (m.sink.as_str(), m.gate.as_str()))
        .collect();
    assert_eq!(credited, [("send", "human")]);
    assert!(r.consent_findings.is_empty(), "{:?}", r.consent_findings);
}

/// The confirm twin keeps today's credit (the consent lane judges its
/// « no »): clean once `send` tests the yes.
#[test]
fn the_confirm_twin_stays_credited() {
    let r = report(&confirm_twin());
    assert!(r.conformance.is_empty(), "{:?}", r.conformance);
    assert!(r.trifecta_findings.is_empty(), "{:?}", r.trifecta_findings);
    assert!(r.consent_findings.is_empty(), "{:?}", r.consent_findings);
    let credited: Vec<(&str, &str)> = r
        .trifecta_mitigations
        .iter()
        .map(|m| (m.sink.as_str(), m.gate.as_str()))
        .collect();
    assert_eq!(credited, [("send", "human")]);
}

/// [`CHOICE`] with a confirm gate whose yes `send` tests.
fn confirm_twin() -> String {
    let confirm = swap(
        CHOICE,
        "args: { mode: \"choice\", message: \"Send this? ${{ with.brief }} / ${{ with.page }}\", choices: [\"send\", \"no\"] }",
        "args: { mode: \"confirm\", message: \"Send this? ${{ with.brief }} / ${{ with.page }}\" }",
    );
    let confirm = swap(
        &confirm,
        SEND_PICK,
        &SEND_PICK.replacen("pick:", "ok:", 1).replacen(
            "\n    invoke:",
            "\n    when: \"${{ with.ok == true }}\"\n    invoke:",
            1,
        ),
    );
    swap(&confirm, "${{ with.pick }}\" }", "${{ with.ok }}\" }")
}

/// Two effects behind one choice gate: the one that tests the answer
/// keeps the credit, the one that only waits on the gate is refused by
/// name.
#[test]
fn each_effect_behind_a_choice_gate_must_test_its_answer() {
    let two = choice_with_guard("\"${{ with.pick == 'send' }}\"");
    let two = format!(
        "{two}  copy:\n    after: {{ human: success }}\n    with: {{ pick: \"${{{{ tasks.human.output }}}}\" }}\n    invoke:\n      tool: \"nika:fetch\"\n      args: {{ url: \"https://api.corp-hooks-example.com/copy\", method: POST, body: {{ pick: \"${{{{ with.pick }}}}\" }} }}\n"
    );
    let r = report(&two);
    assert!(r.conformance.is_empty(), "{:?}", r.conformance);
    assert_eq!(
        sinks(&r),
        [("copy", Some("page"))],
        "{:?}",
        r.trifecta_findings
    );
    let d = &r.trifecta_findings[0].detail;
    assert!(
        d.contains("the gate `human` dominates `copy`, but `copy` does not test its answer"),
        "{d}"
    );
    let credited: Vec<(&str, &str)> = r
        .trifecta_mitigations
        .iter()
        .map(|m| (m.sink.as_str(), m.gate.as_str()))
        .collect();
    assert_eq!(credited, [("send", "human")]);
}

/// What « tests its answer » means: a `when:` that reads an exact
/// binding of the gate's output and is not true for every listed
/// choice. A guard true whatever the person picks tests nothing; a
/// binding the substitution cannot resolve is not a test either.
#[test]
fn a_guard_true_on_every_choice_tests_nothing() {
    for when in [
        "\"${{ with.pick != 'no' }}\"",
        "\"${{ with.pick == 'send' && with.pick != null }}\"",
        "\"${{ with.pick in ['send'] }}\"",
        "\"${{ with.pick.startsWith('s') }}\"",
    ] {
        let r = report(&choice_with_guard(when));
        assert!(r.conformance.is_empty(), "{when}: {:?}", r.conformance);
        assert!(
            r.trifecta_findings.is_empty(),
            "{when}: {:?}",
            r.trifecta_findings
        );
    }
    for when in [
        "\"${{ with.pick != null }}\"",
        "\"${{ with.pick == 'send' || true }}\"",
        "\"${{ with.pick in ['send', 'no'] }}\"",
        "\"${{ with.pick != 'maybe' }}\"",
    ] {
        let r = report(&choice_with_guard(when));
        assert!(r.conformance.is_empty(), "{when}: {:?}", r.conformance);
        assert_eq!(sinks(&r), [("send", Some("page"))], "{when}");
    }
    let unresolved = swap(
        &choice_with_guard("\"${{ with.pick == 'send' }}\""),
        "with: { pick: \"${{ tasks.human.output }}\" }",
        "with: { pick: \"answer=${{ tasks.human.output }}\" }",
    );
    let r = report(&unresolved);
    assert!(r.conformance.is_empty(), "{:?}", r.conformance);
    assert_eq!(sinks(&r), [("send", Some("page"))]);
}

/// A `mode:` the check cannot read as a literal may resolve to `choice`:
/// it is read as one (fail-closed), so the untested POST is refused and
/// the tested one is credited. Templated `choices:` cannot be listed, so a
/// read of the answer is the test, unless it is only a `null` test of the
/// binding, which every answer passes.
#[test]
fn a_templated_mode_is_read_as_a_choice() {
    let templated = |yaml: &str| {
        let y = swap(
            yaml,
            "nika: choice-gate\n",
            "nika: choice-gate\ninputs:\n  mode: { type: string, default: choice }\n  yes: { type: string, default: send }\n",
        );
        swap(
            &y,
            "args: { mode: \"choice\", message: \"Send this? ${{ with.brief }} / ${{ with.page }}\", choices: [\"send\", \"no\"] }",
            "args: { mode: \"${{ inputs.mode }}\", message: \"Send this? ${{ with.brief }} / ${{ with.page }}\", choices: [\"${{ inputs.yes }}\", \"no\"] }",
        )
    };
    let r = report(&templated(CHOICE));
    assert!(r.conformance.is_empty(), "{:?}", r.conformance);
    assert_eq!(sinks(&r), [("send", Some("page"))]);
    let r = report(&templated(&choice_with_guard(
        "\"${{ with.pick == inputs.yes }}\"",
    )));
    assert!(r.conformance.is_empty(), "{:?}", r.conformance);
    assert!(r.trifecta_findings.is_empty(), "{:?}", r.trifecta_findings);
    let r = report(&templated(&choice_with_guard(
        "\"${{ with.pick != null }}\"",
    )));
    assert!(r.conformance.is_empty(), "{:?}", r.conformance);
    assert_eq!(sinks(&r), [("send", Some("page"))]);
}

/// The shipped Foundry block, read from the bundled release: its `review`
/// choice is transformed by the `plan` jq stage (`accepted = .answer != null
/// and .answer != "no"`), and each effect's `when:` reads `plan`. The answer
/// is tested through a derived stage, so the gate is credited before all
/// three effects.
const WATCH_BLOCK: &str = include_str!(
    "../../../nika-onboard-knowledge/assets/knowledge-release-0.123.0-r3.5-foundry-20261016/blocks/watch-api-state-notify.nika"
);

fn credited(r: &crate::CheckReport) -> Vec<(&str, &str)> {
    r.trifecta_mitigations
        .iter()
        .map(|m| (m.sink.as_str(), m.gate.as_str()))
        .collect()
}

#[test]
fn the_shipped_watch_block_tests_its_answer_through_a_jq_stage() {
    let r = report(WATCH_BLOCK);
    assert!(r.conformance.is_empty(), "{:?}", r.conformance);
    assert!(r.trifecta_findings.is_empty(), "{:?}", r.trifecta_findings);
    assert_eq!(
        credited(&r),
        [
            ("notify", "review"),
            ("append_journal", "review"),
            ("save_state", "review")
        ]
    );
}

/// The twin: the jq stage reads the answer inside a string
/// (`"answer=${{ … }}"`), not a binding of the gate's output, so it is no
/// derived stage. `review` still dominates every effect and its answer still
/// reaches them, and every effect behind the choice is refused, naming the
/// gate.
#[test]
fn a_jq_stage_that_does_not_bind_the_answer_tests_nothing() {
    let twin = swap(
        WATCH_BLOCK,
        "with: { answer: \"${{ tasks.review.output }}\",",
        "with: { answer: \"answer=${{ tasks.review.output }}\",",
    );
    let r = report(&twin);
    assert!(r.conformance.is_empty(), "{:?}", r.conformance);
    assert_eq!(
        sinks(&r),
        [
            ("notify", Some("current")),
            ("append_journal", Some("current")),
            ("save_state", Some("current"))
        ],
        "{:?}",
        r.trifecta_findings
    );
    assert!(credited(&r).is_empty(), "{:?}", r.trifecta_mitigations);
    for v in &r.trifecta_findings {
        let part = format!(
            "the gate `review` dominates `{}`, but `{}` does not test its answer",
            v.task, v.task
        );
        assert!(v.detail.contains(&part), "missing `{part}`: {}", v.detail);
    }
}

/// [`CHOICE`] asked as an input: the person types the answer.
fn input(yaml: &str) -> String {
    swap(
        yaml,
        "args: { mode: \"choice\", message: \"Send this? ${{ with.brief }} / ${{ with.page }}\", choices: [\"send\", \"no\"] }",
        "args: { mode: \"input\", message: \"Type send to post this: ${{ with.brief }} / ${{ with.page }}\" }",
    )
}

/// `send` also posts a caller input beside the answer, so it sends more
/// than what the person typed.
fn with_caller_field(yaml: &str) -> String {
    let y = swap(
        yaml,
        "nika: choice-gate\n",
        "nika: choice-gate\ninputs:\n  to: { type: string, default: ops }\n",
    );
    let y = swap(
        &y,
        "with: { pick: \"${{ tasks.human.output }}\" }",
        "with: { pick: \"${{ tasks.human.output }}\", to: \"${{ inputs.to }}\" }",
    );
    swap(
        &y,
        "body: { pick: \"${{ with.pick }}\" }",
        "body: { pick: \"${{ with.pick }}\", to: \"${{ with.to }}\" }",
    )
}

/// An input gate follows the choice rule: only a confirm counts by position
/// alone, so an effect that posts more than the typed answer counts only
/// where its `when:` tests that answer. With no list to enumerate, a guard
/// that reads the answer is the test. An effect that posts nothing but the
/// typed answer counts too: what it sends is what the person typed.
#[test]
fn an_input_gate_counts_where_the_effect_tests_or_only_sends_its_answer() {
    let r = report(&input(&with_caller_field(CHOICE)));
    assert!(r.conformance.is_empty(), "{:?}", r.conformance);
    assert_eq!(sinks(&r), [("send", Some("page"))]);
    assert!(credited(&r).is_empty(), "{:?}", r.trifecta_mitigations);
    let d = &r.trifecta_findings[0].detail;
    for part in [
        "the gate `human` dominates `send`, but `send` does not test its answer",
        "only a `mode: confirm` gate counts by its position alone",
        "an input gate also counts before a `nika:` tool call that sends only the typed answer",
    ] {
        assert!(d.contains(part), "missing `{part}`: {d}");
    }
    let r = report(&input(&with_caller_field(&choice_with_guard(
        "\"${{ with.pick == 'send' }}\"",
    ))));
    assert!(r.conformance.is_empty(), "{:?}", r.conformance);
    assert!(r.trifecta_findings.is_empty(), "{:?}", r.trifecta_findings);
    assert_eq!(credited(&r), [("send", "human")]);
    let r = report(&input(CHOICE));
    assert!(r.conformance.is_empty(), "{:?}", r.conformance);
    assert!(r.trifecta_findings.is_empty(), "{:?}", r.trifecta_findings);
    assert_eq!(credited(&r), [("send", "human")]);
}

/// The shipped teaching block, read from the bundled release: the person
/// edits the draft in an input gate, and `write_dossier` writes exactly the
/// text typed there.
const BLOCKING_GATE_BLOCK: &str = include_str!(
    "../../../nika-onboard-knowledge/assets/knowledge-release-0.123.0-r3.5-foundry-20261016/blocks/trifecta-blocking-gate.nika"
);

/// The `write_dossier` task of [`BLOCKING_GATE_BLOCK`].
const WRITE_DOSSIER: &str = "  write_dossier:\n    with: { text: \"${{ tasks.review.output }}\" }\n    invoke: { tool: \"nika:write\", args: { path: \"./out/dossier.md\", content: \"${{ with.text }}\" } }\n";

#[test]
fn the_shipped_blocking_gate_block_writes_only_the_typed_answer() {
    let r = report(BLOCKING_GATE_BLOCK);
    assert!(r.conformance.is_empty(), "{:?}", r.conformance);
    assert!(r.trifecta_findings.is_empty(), "{:?}", r.trifecta_findings);
    assert_eq!(credited(&r), [("write_dossier", "review")]);
    assert!(r.is_clean(), "{r:?}");
}

/// The twins: a write that also carries the draft, or carries only the
/// draft after the review, takes untrusted content around the person, and
/// a write that carries the output of a task ordered after the review sends
/// more than the typed answer while the gate still dominates it. Each one
/// is refused.
#[test]
fn a_write_that_carries_more_than_the_typed_answer_is_refused() {
    for (why, task) in [
        (
            "the draft beside the answer",
            WRITE_DOSSIER
                .replacen(
                    "with: { text: \"${{ tasks.review.output }}\" }",
                    "with: { text: \"${{ tasks.review.output }}\", draft: \"${{ tasks.draft.output }}\" }",
                    1,
                )
                .replacen("${{ with.text }}", "${{ with.text }} ${{ with.draft }}", 1),
        ),
        (
            "the draft alone, after the review",
            WRITE_DOSSIER.replacen(
                "with: { text: \"${{ tasks.review.output }}\" }",
                "after: { review: success }\n    with: { text: \"${{ tasks.draft.output }}\" }",
                1,
            ),
        ),
    ] {
        let r = report(&swap(BLOCKING_GATE_BLOCK, WRITE_DOSSIER, &task));
        assert!(r.conformance.is_empty(), "{why}: {:?}", r.conformance);
        assert_eq!(sinks(&r), [("write_dossier", Some("lookup"))], "{why}");
        assert!(credited(&r).is_empty(), "{why}: {:?}", r.trifecta_mitigations);
    }
    let stamped = swap(
        BLOCKING_GATE_BLOCK,
        "tools: [\"nika:read\", \"nika:fetch\", \"nika:prompt\", \"nika:write\"]",
        "tools: [\"nika:read\", \"nika:fetch\", \"nika:prompt\", \"nika:write\", \"nika:jq\"]",
    );
    let stamped = swap(
        &stamped,
        WRITE_DOSSIER,
        &format!(
            "  stamp:\n    after: {{ review: success }}\n    invoke: {{ tool: \"nika:jq\", args: {{ input: \"x\", expression: \"now\" }} }}\n{}",
            WRITE_DOSSIER
                .replacen(
                    "with: { text: \"${{ tasks.review.output }}\" }",
                    "with: { text: \"${{ tasks.review.output }}\", at: \"${{ tasks.stamp.output }}\" }",
                    1,
                )
                .replacen("${{ with.text }}", "${{ with.text }} ${{ with.at }}", 1)
        ),
    );
    let r = report(&stamped);
    assert!(r.conformance.is_empty(), "{:?}", r.conformance);
    assert_eq!(sinks(&r), [("write_dossier", Some("lookup"))]);
    let d = &r.trifecta_findings[0].detail;
    assert!(
        d.contains("the gate `review` dominates `write_dossier`, but `write_dossier` does not test its answer"),
        "{d}"
    );
}

/// The jq stage of [`WATCH_BLOCK`] earns the credit only while the answer
/// reaches its program, the program reads it, and the effect's guard can
/// close: a constant program, a guard true whatever `plan` holds, or an
/// answer bound in `with:` but left out of `args.input` (which reads the
/// answer inside a string instead) tests nothing. Dropping the answer from
/// `args.input` altogether leaves no untrusted content in the effects, so
/// that twin is clean with no credit claimed.
#[test]
fn the_watch_block_credit_needs_the_answer_read_and_a_guard_that_can_close() {
    let plan = WATCH_BLOCK.find("\n  plan:\n").expect("the jq stage");
    let at = plan
        + WATCH_BLOCK
            .get(plan..)
            .and_then(|rest| rest.find("        expression: >-\n"))
            .expect("its program");
    let end = WATCH_BLOCK.find("\n  notify:\n").expect("the first effect");
    let constant = format!(
        "{}        expression: '{{accepted: true, changed: true, payload: {{}}, journal: \"\", state: \"\"}}'{}",
        WATCH_BLOCK.get(..at).expect("prefix"),
        WATCH_BLOCK.get(end..).expect("suffix"),
    );
    let unbound = swap(
        &swap(
            WATCH_BLOCK,
            "with: { answer: \"${{ tasks.review.output }}\",",
            "with: { answer: \"${{ tasks.review.output }}\", raw: \"answer=${{ tasks.review.output }}\",",
        ),
        "input: { answer: \"${{ with.answer }}\",",
        "input: { answer: \"${{ with.raw }}\",",
    );
    let all = [
        ("notify", Some("current")),
        ("append_journal", Some("current")),
        ("save_state", Some("current")),
    ];
    for (why, twin) in [
        ("a constant program", constant),
        ("an answer left out of the input", unbound),
    ] {
        let r = report(&twin);
        assert!(r.conformance.is_empty(), "{why}: {:?}", r.conformance);
        assert_eq!(sinks(&r), all, "{why}: {:?}", r.trifecta_findings);
        assert!(
            credited(&r).is_empty(),
            "{why}: {:?}",
            r.trifecta_mitigations
        );
    }
    let dropped = swap(
        WATCH_BLOCK,
        "input: { answer: \"${{ with.answer }}\",",
        "input: { answer: \"none\",",
    );
    let r = report(&dropped);
    assert!(r.conformance.is_empty(), "{:?}", r.conformance);
    assert!(r.trifecta_findings.is_empty(), "{:?}", r.trifecta_findings);
    assert!(credited(&r).is_empty(), "{:?}", r.trifecta_mitigations);
    let tautology = swap(
        WATCH_BLOCK,
        "when: \"${{ with.plan.changed == true }}\"",
        "when: \"${{ with.plan.changed == true || true }}\"",
    );
    let r = report(&tautology);
    assert!(r.conformance.is_empty(), "{:?}", r.conformance);
    assert_eq!(sinks(&r), [("notify", Some("current"))]);
    assert_eq!(
        credited(&r),
        [("append_journal", "review"), ("save_state", "review")]
    );
}

/// [`CHOICE`] with `line` added to the `human` gate task.
fn on_gate(yaml: &str, line: &str) -> String {
    swap(yaml, "  human:\n", &format!("  human:\n{line}"))
}

/// A gate whose answer a recovery value or a loop can supply is not a
/// person's answer, so it counts before no effect: an input before an
/// answer-only `nika:fetch`, and a choice before a `when:` that tests its
/// answer. The refusal names the cause, not an untested guard.
#[test]
fn a_gate_whose_answer_a_stand_in_can_supply_counts_before_nothing() {
    for (why, yaml) in [
        (
            "an input recovered on error",
            on_gate(&input(CHOICE), "    on_error: { recover: \"send\" }\n"),
        ),
        (
            "a choice asked in a loop",
            on_gate(
                &choice_with_guard("\"${{ with.pick == 'send' }}\""),
                "    for_each: { items: [\"one\"] }\n",
            ),
        ),
    ] {
        let r = report(&yaml);
        assert!(r.conformance.is_empty(), "{why}: {:?}", r.conformance);
        assert_eq!(sinks(&r), [("send", Some("page"))], "{why}");
        assert!(
            credited(&r).is_empty(),
            "{why}: {:?}",
            r.trifecta_mitigations
        );
        let d = &r.trifecta_findings[0].detail;
        for part in [
            "the gate `human` dominates `send`, but an `on_error:` or a `for_each:` on `human` can stand in for its answer",
            "remove `on_error:` and `for_each:` from `human` so that only a person answers it",
        ] {
            assert!(d.contains(part), "{why}: missing `{part}`: {d}");
        }
    }
}

/// [`CHOICE`] asked as an input, with `send` running `command` instead of
/// its `nika:fetch`.
fn input_exec(command: &str) -> String {
    let y = input(CHOICE);
    let y = swap(&y, "permits:\n", "permits:\n  exec: [\"curl\"]\n");
    swap(
        &y,
        "    invoke:\n      tool: \"nika:fetch\"\n      args: { url: \"https://api.corp-hooks-example.com/hook\", method: POST, body: { pick: \"${{ with.pick }}\" } }\n",
        &format!("    exec: {{ command: {command} }}\n"),
    )
}

/// Rule (b) reads only a builtin `nika:` tool call, whose payload its
/// `args:` determine. A subprocess that carries the typed answer and posts
/// the private note sends what no template shows, so it is refused. The
/// same subprocess with the answer bound but unused receives nothing
/// untrusted: clean, and no credit is claimed.
#[test]
fn a_subprocess_is_never_an_answer_only_effect() {
    let r = report(&input_exec(
        "[\"curl\", \"-H\", \"X-Pick: ${{ with.pick }}\", \"--data-binary\", \"@./notes/private.md\", \"https://api.corp-hooks-example.com/hook\"]",
    ));
    assert!(r.conformance.is_empty(), "{:?}", r.conformance);
    assert_eq!(
        sinks(&r),
        [("send", Some("page"))],
        "{:?}",
        r.trifecta_findings
    );
    assert!(credited(&r).is_empty(), "{:?}", r.trifecta_mitigations);
    assert!(
        r.trifecta_findings[0]
            .detail
            .contains("the gate `human` dominates `send`, but `send` does not test its answer"),
        "{:?}",
        r.trifecta_findings
    );
    let r = report(&input_exec(
        "[\"curl\", \"--data-binary\", \"@./notes/private.md\", \"https://api.corp-hooks-example.com/hook\"]",
    ));
    assert!(r.conformance.is_empty(), "{:?}", r.conformance);
    assert!(r.trifecta_findings.is_empty(), "{:?}", r.trifecta_findings);
    assert!(credited(&r).is_empty(), "{:?}", r.trifecta_mitigations);
}

/// A confirm gate counts by its position alone only while a person alone
/// answers it: recovered on error, its yes may be nobody's, so the POST it
/// precedes is refused with the cause named. The plain confirm keeps its
/// credit.
#[test]
fn a_confirm_whose_answer_a_recovery_can_supply_counts_before_nothing() {
    let r = report(&on_gate(
        &confirm_twin(),
        "    on_error: { recover: true }\n",
    ));
    assert!(r.conformance.is_empty(), "{:?}", r.conformance);
    assert_eq!(
        sinks(&r),
        [("send", Some("page"))],
        "{:?}",
        r.trifecta_findings
    );
    assert!(credited(&r).is_empty(), "{:?}", r.trifecta_mitigations);
    let d = &r.trifecta_findings[0].detail;
    for part in [
        "the gate `human` dominates `send`, but an `on_error:` or a `for_each:` on `human` can stand in for its answer",
        "remove `on_error:` and `for_each:` from `human` so that only a person answers it",
    ] {
        assert!(d.contains(part), "missing `{part}`: {d}");
    }
    let r = report(&confirm_twin());
    assert!(r.trifecta_findings.is_empty(), "{:?}", r.trifecta_findings);
    assert_eq!(credited(&r), [("send", "human")]);
}
