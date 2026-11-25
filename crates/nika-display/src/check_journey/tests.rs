// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! The SECRETS and JOURNEY rungs say only what the secret flow found (NIK-387): a secret a
//! task iterates over but never sends reaches no destination, and a row never says a rule
//! « pins no host » when the rule pins one. A secret in a task's `model:` goes to the model
//! route of that task, which the JOURNEY rung names (NIK-386).

use nika_check::CheckReport;
use nika_schema::parser::{ParseMode, parse};
use nika_schema::raw::RawWorkflow;
use nika_schema::source::FileId;

use crate::check_render::{ModelsAudit, RepairTarget, VerdictLayers, render};
use crate::theme::Theme;

fn parsed(yaml: &str) -> RawWorkflow {
    parse(yaml, FileId::new(0), ParseMode::Strict).expect("the fixture parses")
}

fn console_of(report: &CheckReport, wf: &RawWorkflow, yaml: &str) -> String {
    render(
        report,
        wf,
        yaml,
        "w.nika",
        RepairTarget::WorkspaceFile,
        Theme::new(false, false, false),
        &ModelsAudit::new(Vec::new(), 0, 0),
        &nika_schema::ResolvedSkills::default(),
        &[],
        report.is_clean(),
        &VerdictLayers::default(),
    )
}

fn console(yaml: &str) -> String {
    let wf = parsed(yaml);
    console_of(&nika_check::check(&wf), &wf, yaml)
}

/// Every line of one rung, as printed.
fn rung<'a>(out: &'a str, label: &str) -> Vec<&'a str> {
    out.lines()
        .filter(|line| line.split_whitespace().nth(1) == Some(label))
        .collect()
}

/// `relay` iterates over `[secrets.token]` and posts to other.example.com, sending its item when
/// `sent`; `token` may go through `nika:fetch` to `host` only.
fn relay(host: &str, sent: bool) -> String {
    let (with, body) = if sent {
        (
            "    with: { v: \"${{ item }}\" }\n",
            r#", method: POST, body: "${{ with.v }}""#,
        )
    } else {
        ("", "")
    };
    format!(
        r#"nika: item-relay
secrets:
  token: {{ source: env, key: T, egress: [{{ to: "nika:fetch", host: "{host}" }}] }}
permits:
  tools: ["nika:fetch"]
  net: {{ http: ["crm.example.com", "other.example.com"] }}
tasks:
  relay:
    for_each: {{ items: ["${{{{ secrets.token }}}}"] }}
{with}    invoke: {{ tool: "nika:fetch", args: {{ url: "https://other.example.com/in"{body} }} }}
"#
    )
}

/// The request of `relay` never reads its item, so the token reaches no effect: the verdict
/// finds nothing, and neither rung claims the token goes to other.example.com. The token is
/// still a used secret (the run resolves it), so the class is `sensitive`.
#[test]
fn a_secret_iterated_over_but_never_sent_reaches_no_destination() {
    let out = console(&relay("crm.example.com", false));
    assert_eq!(
        rung(&out, "SECRETS"),
        [" ✔ SECRETS  declared secrets used · 1 egress rule declared · model echo untracked"],
        "{out}"
    );
    assert_eq!(
        rung(&out, "JOURNEY"),
        [
            " ✔ JOURNEY sensitive · 0 sources · 1 destination · 0 model endpoints · no secret \
             reaches an external destination"
        ],
        "{out}"
    );
    assert!(!out.contains("sanctioned secret flow"), "{out}");
}

/// The item the request reads through a binding, to the host the rule pins: the flow and both
/// rungs name it, pinned.
#[test]
fn a_secret_sent_through_its_item_to_the_pinned_host_is_named() {
    let out = console(&relay("other.example.com", true));
    assert_eq!(
        rung(&out, "SECRETS"),
        [
            " ⚠ SECRETS  1 sanctioned secret flow by declaration · review consent before the run \
             · model echo untracked",
            " ⚠ SECRETS  sanctioned · secret `token` → other.example.com · egress.to nika:fetch · \
             host pinned",
        ],
        "{out}"
    );
    assert_eq!(
        rung(&out, "JOURNEY"),
        [
            " ⚠ JOURNEY regulated · 0 sources · 1 destination · 0 model endpoints",
            " ⚠ JOURNEY secret `token` flows to other.example.com · review consent before the run",
        ],
        "{out}"
    );
}

/// The item sent where the rule does not allow: the leak finding is the SECRETS row, and the
/// journey names the destination the flow found.
#[test]
fn a_secret_sent_through_its_item_elsewhere_is_the_finding() {
    let yaml = relay("crm.example.com", true);
    let wf = parsed(&yaml);
    let report = nika_check::check(&wf);
    let leaks: Vec<String> = report
        .findings
        .iter()
        .filter(|f| f.kind == "secret_leak")
        .map(|f| format!(" ✖ SECRETS  [NIKA-SEC-006] {}", f.message))
        .collect();
    assert_eq!(leaks.len(), 1, "{:?}", report.findings);
    let out = console_of(&report, &wf, &yaml);
    assert_eq!(rung(&out, "SECRETS"), leaks, "{out}");
    assert_eq!(
        rung(&out, "JOURNEY"),
        [
            " ⚠ JOURNEY regulated · 0 sources · 1 destination · 0 model endpoints",
            " ⚠ JOURNEY secret `token` flows to other.example.com · review consent before the run",
        ],
        "{out}"
    );
}

/// A rule pinned to crm.example.com pins a host: a row naming another destination says which
/// host the rule pins, never « pins no host ». Since the journey reads the secret flow, no real
/// file reaches this case: a send the flow finds to a host no rule pins is a leak finding, never
/// a sanctioned row. So the report here is retouched: the sent one with its finding removed,
/// the journey a projection that disagreed with the flow used to print.
#[test]
fn a_row_names_the_host_the_rule_pins() {
    let yaml = relay("crm.example.com", true);
    let wf = parsed(&yaml);
    let mut report = nika_check::check(&wf);
    report.secret_leaks.clear();
    report.findings.retain(|f| f.kind != "secret_leak");
    let out = console_of(&report, &wf, &yaml);
    assert_eq!(
        rung(&out, "SECRETS")[1..],
        [
            " ⚠ SECRETS  sanctioned · secret `token` → other.example.com · egress.to nika:fetch \
             pins crm.example.com, not other.example.com"
        ],
        "{out}"
    );
}

/// One rule pins crm.example.com, one beside it pins no host and clears the send to
/// other.example.com: the row names both, and the sink once.
#[test]
fn a_row_names_the_pinned_host_and_the_open_rule_beside_it() {
    let yaml = relay("crm.example.com", true).replace(
        r#"egress: [{ to: "nika:fetch", host: "crm.example.com" }]"#,
        r#"egress: [{ to: "nika:fetch", host: "crm.example.com" }, { to: "nika:fetch" }]"#,
    );
    let out = console(&yaml);
    assert_eq!(
        rung(&out, "SECRETS")[1..],
        [
            " ⚠ SECRETS  sanctioned · secret `token` → other.example.com · egress.to nika:fetch \
             pins crm.example.com, not other.example.com, and a rule with no host clears any \
             host the tool reaches · exact form: egress: [{ to: \"nika:fetch\", host: \
             \"other.example.com\" }]"
        ],
        "{out}"
    );
}

/// The witnesses q1 (`infer:`) and q2 (`agent:`) of NIK-386, the token in the task's `model:`.
fn routed(verb: &str) -> String {
    let budget = if verb == "agent" {
        "max_tokens_total"
    } else {
        "max_tokens"
    };
    format!(
        r#"nika: q-{verb}-model
secrets:
  token: {{ source: env, key: T, egress: [{{ to: "nika:fetch", host: "crm.example.com" }}] }}
tasks:
  ask:
    {verb}: {{ model: "${{{{ secrets.token }}}}", prompt: hi, {budget}: 10 }}
"#
    )
}

/// The token in a task's `model:` is refused on the SECRETS rung, and the JOURNEY rung says where
/// it goes, the route of `ask`, in place of « no secret reaches an external destination ».
#[test]
fn a_secret_in_a_task_model_flows_to_its_model_route() {
    for verb in ["infer", "agent"] {
        let yaml = routed(verb);
        let wf = parsed(&yaml);
        let report = nika_check::check(&wf);
        let leaks: Vec<String> = report
            .findings
            .iter()
            .filter(|f| f.kind == "secret_leak")
            .map(|f| format!(" ✖ SECRETS  [NIKA-SEC-006] {}", f.message))
            .collect();
        assert_eq!(leaks.len(), 1, "{verb}: {:?}", report.findings);
        let out = console_of(&report, &wf, &yaml);
        assert_eq!(rung(&out, "SECRETS"), leaks, "{out}");
        assert_eq!(
            rung(&out, "JOURNEY"),
            [
                " ⚠ JOURNEY regulated · 0 sources · 0 destinations · 0 model endpoints",
                " ⚠ JOURNEY secret `token` flows to the model route of `ask` · review consent \
                 before the run",
            ],
            "{out}"
        );
    }
}

/// The witness q3: the run never renders the workflow `model:`, so a secret there goes nowhere
/// and the JOURNEY rung is as it was.
#[test]
fn a_secret_in_the_envelope_model_leaves_the_journey_as_it_was() {
    let out = console(
        r#"nika: q-envelope-model
model: "${{ secrets.token }}"
secrets:
  token: { source: env, key: T, egress: [{ to: "nika:fetch", host: "crm.example.com" }] }
tasks:
  ask:
    infer: { prompt: hi, max_tokens: 10 }
"#,
    );
    assert_eq!(
        rung(&out, "JOURNEY"),
        [
            " ✔ JOURNEY internal · 0 sources · 0 destinations · 0 model endpoints · no secret \
             reaches an external destination"
        ],
        "{out}"
    );
}
