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
    let confirm = swap(&confirm, "${{ with.pick }}\" }", "${{ with.ok }}\" }");
    let r = report(&confirm);
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
/// the tested one is credited. Templated `choices:` cannot be listed, so
/// the read of the answer alone is the test.
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
        "\"${{ with.pick != null }}\"",
    )));
    assert!(r.conformance.is_empty(), "{:?}", r.conformance);
    assert!(r.trifecta_findings.is_empty(), "{:?}", r.trifecta_findings);
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

/// An input gate follows the choice rule: only a confirm counts by position
/// alone. With no list to enumerate, a guard that reads the answer is the
/// test.
#[test]
fn an_input_gate_counts_only_where_the_effect_tests_its_answer() {
    let input = |yaml: &str| {
        swap(
            yaml,
            "args: { mode: \"choice\", message: \"Send this? ${{ with.brief }} / ${{ with.page }}\", choices: [\"send\", \"no\"] }",
            "args: { mode: \"input\", message: \"Type send to post this: ${{ with.brief }} / ${{ with.page }}\" }",
        )
    };
    let r = report(&input(CHOICE));
    assert!(r.conformance.is_empty(), "{:?}", r.conformance);
    assert_eq!(sinks(&r), [("send", Some("page"))]);
    assert!(credited(&r).is_empty(), "{:?}", r.trifecta_mitigations);
    let d = &r.trifecta_findings[0].detail;
    for part in [
        "the gate `human` dominates `send`, but `send` does not test its answer",
        "only a `mode: confirm` gate counts by its position alone",
    ] {
        assert!(d.contains(part), "missing `{part}`: {d}");
    }
    let r = report(&input(&choice_with_guard("\"${{ with.pick == 'send' }}\"")));
    assert!(r.conformance.is_empty(), "{:?}", r.conformance);
    assert!(r.trifecta_findings.is_empty(), "{:?}", r.trifecta_findings);
    assert_eq!(credited(&r), [("send", "human")]);
}
