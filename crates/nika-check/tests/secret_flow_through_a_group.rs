// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>
#![allow(clippy::expect_used)]

//! A `group.<name>` read is the plural of a `tasks.<id>` read (spec 03 §group): the fold holds
//! each member's record, so it carries whatever a member's output carries. The secret flow
//! judges it as it judges the same record read one member at a time: a secret a member
//! received and the fold sends where the secret's `egress:` does not allow is `NIKA-SEC-006`.

use nika_check::{CheckReport, check};
use nika_schema::{FileId, ParseMode, parse};

/// The rule that lets `token` go to crm.example.com through `nika:fetch`, and no further.
const CRM: &str = r#"{ to: "nika:fetch", host: "crm.example.com" }"#;

/// The rule that also lets it go to other.example.com.
const OTHER: &str = r#"{ to: "nika:fetch", host: "other.example.com" }"#;

/// `call_a` and `call_b` join `calls`; only `call_a` sends the token, to crm.example.com.
/// `relay` binds `READ` and posts it to other.example.com.
const RELAY: &str = r#"nika: group-relay
secrets:
  token: { source: env, key: T, egress: [EGRESS] }
permits:
  tools: ["nika:fetch"]
  net: { http: ["crm.example.com", "other.example.com"] }
tasks:
  call_a:
    group: calls
    invoke: { tool: "nika:fetch", args: { url: "https://crm.example.com/a", headers: { authorization: "Bearer ${{ secrets.token }}" } } }
  call_b:
    group: calls
    invoke: { tool: "nika:fetch", args: { url: "https://crm.example.com/b" } }
  relay:
    with: { legs: "READ" }
    invoke: { tool: "nika:fetch", args: { url: "https://other.example.com/in", method: POST, body: "${{ with.legs }}" } }
"#;

/// `calls` holds `call_b` and `call_c`; `call_a` sends the token but joins no group.
const NO_MEMBER_RECEIVED: &str = r#"nika: group-relay
secrets:
  token: { source: env, key: T, egress: [{ to: "nika:fetch", host: "crm.example.com" }] }
permits:
  tools: ["nika:fetch"]
  net: { http: ["crm.example.com", "other.example.com"] }
tasks:
  call_a:
    invoke: { tool: "nika:fetch", args: { url: "https://crm.example.com/a", headers: { authorization: "Bearer ${{ secrets.token }}" } } }
  call_b:
    group: calls
    invoke: { tool: "nika:fetch", args: { url: "https://crm.example.com/b" } }
  call_c:
    group: calls
    invoke: { tool: "nika:fetch", args: { url: "https://crm.example.com/c" } }
  relay:
    with: { legs: "${{ group.calls }}" }
    invoke: { tool: "nika:fetch", args: { url: "https://other.example.com/in", method: POST, body: "${{ with.legs }}" } }
"#;

/// The member that received the token stands between two that did not.
const ONE_AMONG_SEVERAL: &str = r#"nika: group-relay
secrets:
  token: { source: env, key: T, egress: [{ to: "nika:fetch", host: "crm.example.com" }] }
permits:
  tools: ["nika:fetch"]
  net: { http: ["crm.example.com", "other.example.com"] }
tasks:
  call_z:
    group: calls
    invoke: { tool: "nika:fetch", args: { url: "https://crm.example.com/z" } }
  call_a:
    group: calls
    invoke: { tool: "nika:fetch", args: { url: "https://crm.example.com/a", headers: { authorization: "Bearer ${{ secrets.token }}" } } }
  call_b:
    group: calls
    invoke: { tool: "nika:fetch", args: { url: "https://crm.example.com/b" } }
  call_c:
    group: calls
    invoke: { tool: "nika:fetch", args: { url: "https://crm.example.com/c" } }
  relay:
    with: { legs: "${{ group.calls }}" }
    invoke: { tool: "nika:fetch", args: { url: "https://other.example.com/in", method: POST, body: "${{ with.legs }}" } }
"#;

/// `relay` runs once per member record and posts each record's output.
const THROUGH_ITEMS: &str = r#"nika: group-relay
secrets:
  token: { source: env, key: T, egress: [{ to: "nika:fetch", host: "crm.example.com" }] }
permits:
  tools: ["nika:fetch"]
  net: { http: ["crm.example.com", "other.example.com"] }
tasks:
  call_a:
    group: calls
    invoke: { tool: "nika:fetch", args: { url: "https://crm.example.com/a", headers: { authorization: "Bearer ${{ secrets.token }}" } } }
  call_b:
    group: calls
    invoke: { tool: "nika:fetch", args: { url: "https://crm.example.com/b" } }
  relay:
    with: { legs: "${{ group.calls }}" }
    for_each: { items: "${{ with.legs }}", max_items: 10 }
    invoke: { tool: "nika:fetch", args: { url: "https://other.example.com/in", method: POST, body: "${{ item.output }}" } }
"#;

/// `hop` folds `calls` and sends it to crm.example.com, where the token may go; `relay` folds the
/// group `hop` joins.
const THROUGH_A_MEMBER_THAT_FOLDS: &str = r#"nika: group-relay
secrets:
  token: { source: env, key: T, egress: [{ to: "nika:fetch", host: "crm.example.com" }] }
permits:
  tools: ["nika:fetch"]
  net: { http: ["crm.example.com", "other.example.com"] }
tasks:
  call_a:
    group: calls
    invoke: { tool: "nika:fetch", args: { url: "https://crm.example.com/a", headers: { authorization: "Bearer ${{ secrets.token }}" } } }
  call_b:
    group: calls
    invoke: { tool: "nika:fetch", args: { url: "https://crm.example.com/b" } }
  hop:
    group: hops
    with: { legs: "${{ group.calls }}" }
    invoke: { tool: "nika:fetch", args: { url: "https://crm.example.com/h", method: POST, body: "${{ with.legs }}" } }
  relay:
    with: { legs: "${{ group.hops }}" }
    invoke: { tool: "nika:fetch", args: { url: "https://other.example.com/in", method: POST, body: "${{ with.legs }}" } }
"#;

/// The only member is a local command the token may reach; `push` sends the fold to the only
/// external host of the file.
const ONLY_THROUGH_THE_FOLD: &str = r#"nika: group-push
secrets:
  token: { source: env, key: T, egress: [{ to: exec }] }
permits:
  exec: ["printenv"]
  tools: ["nika:fetch"]
  net: { http: ["other.example.com"] }
tasks:
  sign:
    group: signed
    exec: { command: ["printenv", "TOKEN"], env: { TOKEN: "${{ secrets.token }}" } }
  push:
    with: { legs: "${{ group.signed }}" }
    invoke: { tool: "nika:fetch", args: { url: "https://other.example.com/in", method: POST, body: "${{ with.legs }}" } }
"#;

fn relay(read: &str, egress: &str) -> String {
    RELAY.replace("EGRESS", egress).replace("READ", read)
}

fn run(yaml: &str) -> CheckReport {
    check(&parse(yaml, FileId::new(0), ParseMode::Strict).expect("the fixture parses"))
}

/// A leak as `(task, secret, path)`.
type Leak = (String, String, String);

/// A finding as `(kind, code, task)`.
type Finding = (&'static str, Option<String>, Option<String>);

/// Every leak.
fn leaks(report: &CheckReport) -> Vec<Leak> {
    report
        .secret_leaks
        .iter()
        .map(|leak| (leak.task.clone(), leak.secret.clone(), leak.trace.clone()))
        .collect()
}

fn leak(task: &str, path: &str) -> Leak {
    (task.to_owned(), "token".to_owned(), path.to_owned())
}

/// Every finding.
fn findings(report: &CheckReport) -> Vec<Finding> {
    report
        .findings
        .iter()
        .map(|finding| (finding.kind, finding.code.clone(), finding.task.clone()))
        .collect()
}

fn sec_006(task: &str) -> Finding {
    (
        "secret_leak",
        Some("NIKA-SEC-006".to_owned()),
        Some(task.to_owned()),
    )
}

/// The external destinations the journey says `token` reaches.
fn flows_to(report: &CheckReport) -> Vec<String> {
    report
        .data_journey
        .secrets_used
        .iter()
        .find(|used| used.name == "token")
        .map(|used| used.flows_to.clone())
        .unwrap_or_default()
}

#[test]
fn a_group_read_sent_where_the_secret_may_not_go_is_a_leak() {
    let folded = run(&relay("${{ group.calls }}", CRM));
    assert_eq!(
        leaks(&folded),
        [leak(
            "relay",
            "secrets.token → tasks.call_a.output → with.legs @ relay"
        )]
    );
    assert_eq!(findings(&folded), [sec_006("relay")]);
    // The same record read by its task is refused the same way.
    let by_task = run(&relay("${{ tasks.call_a.output }}", CRM));
    assert_eq!(leaks(&by_task), leaks(&folded));
    assert_eq!(findings(&by_task), findings(&folded));
}

#[test]
fn the_same_read_sent_where_the_secret_may_go_is_clean() {
    let report = run(&relay("${{ group.calls }}", &format!("{CRM}, {OTHER}")));
    assert_eq!(leaks(&report), Vec::<Leak>::new());
    assert_eq!(findings(&report), Vec::<Finding>::new());
    // The journey still names the destination the rule allows.
    assert_eq!(flows_to(&report), ["crm.example.com", "other.example.com"]);
}

#[test]
fn a_group_none_of_whose_members_received_a_secret_is_clean() {
    let report = run(NO_MEMBER_RECEIVED);
    assert_eq!(leaks(&report), Vec::<Leak>::new());
    assert_eq!(findings(&report), Vec::<Finding>::new());
    assert_eq!(flows_to(&report), ["crm.example.com"]);
}

#[test]
fn one_member_that_received_the_secret_taints_the_whole_fold() {
    let report = run(ONE_AMONG_SEVERAL);
    assert_eq!(
        leaks(&report),
        [leak(
            "relay",
            "secrets.token → tasks.call_a.output → with.legs @ relay"
        )]
    );
    assert_eq!(findings(&report), [sec_006("relay")]);
}

#[test]
fn a_member_record_read_through_for_each_items_is_a_leak() {
    let report = run(THROUGH_ITEMS);
    assert_eq!(
        leaks(&report),
        [leak(
            "relay",
            "secrets.token → tasks.call_a.output → with.legs @ relay → item @ relay"
        )]
    );
    assert_eq!(findings(&report), [sec_006("relay")]);
}

#[test]
fn a_member_that_folds_another_group_passes_the_secret_on() {
    let report = run(THROUGH_A_MEMBER_THAT_FOLDS);
    assert_eq!(
        leaks(&report),
        [leak(
            "relay",
            "secrets.token → tasks.call_a.output → with.legs @ hop → tasks.hop.output → \
             with.legs @ relay"
        )]
    );
    assert_eq!(findings(&report), [sec_006("relay")]);
}

/// The journey reads the same flow facts: a file whose only external send is a fold of the
/// secret no longer reads as one where no secret reaches an external destination.
#[test]
fn the_journey_counts_the_host_a_fold_sends_the_secret_to() {
    let report = run(ONLY_THROUGH_THE_FOLD);
    assert_eq!(
        leaks(&report),
        [leak(
            "push",
            "secrets.token → tasks.sign.output → with.legs @ push"
        )]
    );
    assert_eq!(findings(&report), [sec_006("push")]);
    assert_eq!(flows_to(&report), ["other.example.com"]);
    assert!(!report.is_clean());
}
