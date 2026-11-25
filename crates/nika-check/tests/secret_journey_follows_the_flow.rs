// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>
#![allow(clippy::expect_used)]

//! Where the data journey says a secret goes is where the secret flow found it (NIK-387).
//!
//! The journey used to attach a secret to every task naming it, in its request, its `with:` or
//! its `for_each:`, so it could claim a secret goes where the flow proves it never does: a
//! literal `for_each:` list whose item the request never reads. A secret's tasks and
//! destinations now come from the settled flow only, the facts the SECRETS verdict reads, so the
//! two cannot disagree. A secret the body names but never sends stays a used secret (the run
//! resolves it) that goes nowhere.

use nika_check::{CheckReport, DataClassification, check};
use nika_schema::{FileId, ParseMode, parse};

fn run(yaml: &str) -> CheckReport {
    check(&parse(yaml, FileId::new(0), ParseMode::Strict).expect("the fixture parses"))
}

/// Every used secret as `(name, tasks, destinations)`.
type Use = (String, Vec<String>, Vec<String>);

fn uses(report: &CheckReport) -> Vec<Use> {
    report
        .data_journey
        .secrets_used
        .iter()
        .map(|s| (s.name.clone(), s.tasks.clone(), s.flows_to.clone()))
        .collect()
}

/// `token`, used and sent nowhere.
fn token_unsent() -> Use {
    ("token".to_owned(), Vec::new(), Vec::new())
}

fn token_to(task: &str, dest: &str) -> Use {
    (
        "token".to_owned(),
        vec![task.to_owned()],
        vec![dest.to_owned()],
    )
}

/// `relay` iterates over `[secrets.token]`, binds `WITH` and posts to other.example.com with the
/// extra args `BODY`; `token` may go through `nika:fetch` to `HOST` only.
fn relay(host: &str, with: &str, body: &str) -> String {
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

const CRM: &str = "crm.example.com";

/// The witness of NIK-387: `token` is used, reaches no task and no destination, and the class
/// is `sensitive` (a secret is used), not `regulated` (none leaves).
#[test]
fn a_secret_iterated_over_but_never_sent_goes_nowhere() {
    let report = run(&relay(CRM, "", ""));
    assert_eq!(report.secret_leaks.len(), 0, "{:?}", report.secret_leaks);
    assert_eq!(uses(&report), [token_unsent()]);
    assert_eq!(
        report.data_journey.classification,
        DataClassification::Sensitive
    );
}

/// A secret a task binds in `with:` but never sends goes nowhere either.
#[test]
fn a_secret_bound_but_never_sent_goes_nowhere() {
    let report = run(&relay(
        CRM,
        "    with: { v: \"${{ secrets.token }}\" }\n",
        "",
    ));
    assert_eq!(report.secret_leaks.len(), 0, "{:?}", report.secret_leaks);
    assert_eq!(uses(&report), [token_unsent()]);
    assert_eq!(
        report.data_journey.classification,
        DataClassification::Sensitive
    );
}

/// The item sent, directly or through a binding, allowed or not: the journey names the task and
/// the destination, whatever the verdict says of the send.
#[test]
fn a_secret_sent_through_its_item_is_in_the_journey() {
    for (host, with, body, leaks) in [
        (CRM, "", r#", method: POST, body: "${{ item }}""#, 1),
        (
            CRM,
            "    with: { v: \"${{ item }}\" }\n",
            r#", method: POST, body: "${{ with.v }}""#,
            1,
        ),
        (
            "other.example.com",
            "    with: { v: \"${{ item }}\" }\n",
            r#", method: POST, body: "${{ with.v }}""#,
            0,
        ),
    ] {
        let report = run(&relay(host, with, body));
        assert_eq!(report.secret_leaks.len(), leaks, "{host} {body}");
        assert_eq!(
            uses(&report),
            [token_to("relay", "other.example.com")],
            "{host} {body}"
        );
        assert_eq!(
            report.data_journey.classification,
            DataClassification::Regulated
        );
    }
}

/// A secret that reaches a task through an upstream output is named on the task it reaches, as
/// before: the flow carries it there.
#[test]
fn a_secret_carried_by_an_upstream_output_is_in_the_journey() {
    let yaml = r#"nika: carried
secrets:
  token: { source: env, key: T, egress: [{ to: exec }, { to: "nika:fetch", host: "crm.example.com" }] }
permits:
  tools: ["nika:fetch"]
  net: { http: ["crm.example.com"] }
  exec: ["printf"]
tasks:
  pick:
    exec: { command: [printf, "${{ secrets.token }}"] }
  send:
    with: { v: "${{ tasks.pick.output }}" }
    invoke: { tool: "nika:fetch", args: { url: "https://crm.example.com/in", method: POST, body: "${{ with.v }}" } }
"#;
    let report = run(yaml);
    assert_eq!(report.secret_leaks.len(), 0, "{:?}", report.secret_leaks);
    assert_eq!(
        uses(&report),
        [(
            "token".to_owned(),
            vec!["pick".to_owned(), "send".to_owned()],
            vec![CRM.to_owned()],
        )]
    );
}
