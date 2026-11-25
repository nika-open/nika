// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>
#![allow(clippy::expect_used)]

//! A `group.<name>` read is the plural of a `tasks.<id>` read (spec 03 §group): the fold holds
//! each member's record, so it carries whatever a member's output carries. The secret flow
//! judges it as it judges the same record read one member at a time: a secret a member
//! received and the fold sends where the secret's `egress:` does not allow is `NIKA-SEC-006`.
//!
//! A task's record is also what its `on_error.recover:` value makes it (spec 05 §recovery): the
//! recovered value replaces the output, so a secret the value reads rides the record. The
//! recover itself sends nothing and is no destination; each later reader is judged as usual.
//! A recover read is no scheduling edge, so its source may come later in the sweep: the flow
//! repeats until no record moves.
//!
//! A record carries one trace per secret, the most direct one, and every sink (a request, the
//! workflow `outputs:`, the data journey) reads all of them.

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

/// `call_a` sends the token to crm.example.com; `backup` fails over to `VALUE`; `relay` posts
/// `backup`'s record to other.example.com.
const RECOVER_RELAY: &str = r#"nika: recover-relay
secrets:
  token: { source: env, key: T, egress: [EGRESS] }
permits:
  tools: ["nika:fetch"]
  net: { http: ["crm.example.com", "other.example.com"] }
tasks:
  call_a:
    invoke: { tool: "nika:fetch", args: { url: "https://crm.example.com/a", headers: { authorization: "Bearer ${{ secrets.token }}" } } }
  backup:
    after: { call_a: success }
    invoke: { tool: "nika:fetch", args: { url: "https://crm.example.com/b" } }
    on_error: { recover: "VALUE" }
  relay:
    with: { legs: "${{ tasks.backup.output }}" }
    invoke: { tool: "nika:fetch", args: { url: "https://other.example.com/in", method: POST, body: "${{ with.legs }}" } }
"#;

/// `backup` binds the fold of `calls`, of which only `call_a` received the token, and recovers
/// to it.
const RECOVER_A_FOLD: &str = r#"nika: recover-relay
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
    invoke: { tool: "nika:fetch", args: { url: "https://crm.example.com/c" } }
  backup:
    with: { legs: "${{ group.calls }}" }
    invoke: { tool: "nika:fetch", args: { url: "https://crm.example.com/b" } }
    on_error: { recover: "${{ with.legs }}" }
  relay:
    with: { legs: "${{ tasks.backup.output }}" }
    invoke: { tool: "nika:fetch", args: { url: "https://other.example.com/in", method: POST, body: "${{ with.legs }}" } }
"#;

fn recover_relay(value: &str, egress: &str) -> String {
    RECOVER_RELAY
        .replace("EGRESS", egress)
        .replace("VALUE", value)
}

#[test]
fn a_recover_value_that_reads_a_secret_carries_it_into_the_record() {
    for (value, path) in [
        (
            "${{ tasks.call_a.output }}",
            "secrets.token → tasks.call_a.output → on_error.recover @ backup → \
             tasks.backup.output → with.legs @ relay",
        ),
        (
            "${{ secrets.token }}",
            "secrets.token → on_error.recover @ backup → tasks.backup.output → with.legs @ relay",
        ),
    ] {
        let report = run(&recover_relay(value, CRM));
        assert_eq!(leaks(&report), [leak("relay", path)], "{value}");
        assert_eq!(findings(&report), [sec_006("relay")], "{value}");
        assert_eq!(
            flows_to(&report),
            ["crm.example.com", "other.example.com"],
            "{value}"
        );
    }
}

/// The recover is no destination of its own: with the relay's host allowed, a secret read
/// directly in `recover:` gives no finding.
#[test]
fn the_recovered_record_sent_where_the_secret_may_go_is_clean() {
    for value in ["${{ tasks.call_a.output }}", "${{ secrets.token }}"] {
        let report = run(&recover_relay(value, &format!("{CRM}, {OTHER}")));
        assert_eq!(leaks(&report), Vec::<Leak>::new(), "{value}");
        assert_eq!(findings(&report), Vec::<Finding>::new(), "{value}");
    }
}

#[test]
fn a_recover_value_that_carries_no_secret_is_clean() {
    let report = run(&recover_relay("offline", CRM));
    assert_eq!(leaks(&report), Vec::<Leak>::new());
    assert_eq!(findings(&report), Vec::<Finding>::new());
    assert_eq!(flows_to(&report), ["crm.example.com"]);
}

#[test]
fn a_recover_value_that_reads_a_fold_with_one_tainted_member_is_a_leak() {
    let report = run(RECOVER_A_FOLD);
    assert_eq!(
        leaks(&report),
        [leak(
            "relay",
            "secrets.token → tasks.call_a.output → with.legs @ backup → on_error.recover @ \
             backup → tasks.backup.output → with.legs @ relay"
        )]
    );
    assert_eq!(findings(&report), [sec_006("relay")]);
}

/// A leak of any secret, as `(task, secret, path)`.
fn leak_of(task: &str, secret: &str, path: &str) -> Leak {
    (task.to_owned(), secret.to_owned(), path.to_owned())
}

/// The external destinations the journey says `secret` reaches.
fn flows_to_of(report: &CheckReport, secret: &str) -> Vec<String> {
    report
        .data_journey
        .secrets_used
        .iter()
        .find(|used| used.name == secret)
        .map(|used| used.flows_to.clone())
        .unwrap_or_default()
}

/// Every workflow-output egress, as `(output, secret, path)`.
fn egresses(report: &CheckReport) -> Vec<(String, String, String)> {
    report
        .secret_egresses
        .iter()
        .map(|e| (e.output.clone(), e.secret.clone(), e.trace.clone()))
        .collect()
}

fn sec_007() -> Finding {
    ("secret_egress", Some("NIKA-SEC-007".to_owned()), None)
}

/// `backup` recovers to `call_a`'s record, but runs in the first wave while `call_a` waits on
/// `prep`: a recover read is no scheduling edge, so the source comes after the reader.
const RECOVER_FROM_A_LATER_WAVE: &str = r#"nika: recover-before-its-source
secrets:
  token: { source: env, key: T, egress: [{ to: "nika:fetch", host: "crm.example.com" }] }
permits:
  tools: ["nika:fetch"]
  net: { http: ["crm.example.com", "other.example.com"] }
tasks:
  prep:
    invoke: { tool: "nika:fetch", args: { url: "https://crm.example.com/p" } }
  call_a:
    after: { prep: success }
    invoke: { tool: "nika:fetch", args: { url: "https://crm.example.com/a", headers: { authorization: "Bearer ${{ secrets.token }}" } } }
  backup:
    invoke: { tool: "nika:fetch", args: { url: "https://crm.example.com/b" } }
    on_error: { recover: "${{ tasks.call_a.output }}" }
  relay:
    with: { legs: "${{ tasks.backup.output }}" }
    invoke: { tool: "nika:fetch", args: { url: "https://other.example.com/in", method: POST, body: "${{ with.legs }}" } }
"#;

/// `backup` and `call_a` share the first wave, `backup` declared first.
const RECOVER_FROM_LATER_IN_THE_WAVE: &str = r#"nika: recover-relay
secrets:
  token: { source: env, key: T, egress: [{ to: "nika:fetch", host: "crm.example.com" }] }
permits:
  tools: ["nika:fetch"]
  net: { http: ["crm.example.com", "other.example.com"] }
tasks:
  backup:
    invoke: { tool: "nika:fetch", args: { url: "https://crm.example.com/b" } }
    on_error: { recover: "${{ tasks.call_a.output }}" }
  call_a:
    invoke: { tool: "nika:fetch", args: { url: "https://crm.example.com/a", headers: { authorization: "Bearer ${{ secrets.token }}" } } }
  relay:
    with: { legs: "${{ tasks.backup.output }}" }
    invoke: { tool: "nika:fetch", args: { url: "https://other.example.com/in", method: POST, body: "${{ with.legs }}" } }
"#;

#[test]
fn a_recover_whose_source_comes_later_in_the_sweep_still_carries_its_secret() {
    for yaml in [RECOVER_FROM_A_LATER_WAVE, RECOVER_FROM_LATER_IN_THE_WAVE] {
        let report = run(yaml);
        assert_eq!(
            leaks(&report),
            [leak(
                "relay",
                "secrets.token → tasks.call_a.output → on_error.recover @ backup → \
                 tasks.backup.output → with.legs @ relay"
            )],
            "{yaml}"
        );
        assert_eq!(findings(&report), [sec_006("relay")], "{yaml}");
        assert_eq!(
            flows_to(&report),
            ["crm.example.com", "other.example.com"],
            "{yaml}"
        );
    }
}

/// `left` sends `alpha` and recovers to `right`'s record; `right` sends `omega` and recovers to
/// `left`'s. Each relay posts one of them to other.example.com, where neither secret may go.
const RECOVER_FROM_EACH_OTHER: &str = r#"nika: recover-from-each-other
secrets:
  alpha: { source: env, key: A, egress: [{ to: "nika:fetch", host: "crm.example.com" }] }
  omega: { source: env, key: O, egress: [{ to: "nika:fetch", host: "crm.example.com" }] }
permits:
  tools: ["nika:fetch"]
  net: { http: ["crm.example.com", "other.example.com"] }
tasks:
  left:
    invoke: { tool: "nika:fetch", args: { url: "https://crm.example.com/l", headers: { authorization: "Bearer ${{ secrets.alpha }}" } } }
    on_error: { recover: "${{ tasks.right.output }}" }
  right:
    invoke: { tool: "nika:fetch", args: { url: "https://crm.example.com/r", headers: { authorization: "Bearer ${{ secrets.omega }}" } } }
    on_error: { recover: "${{ tasks.left.output }}" }
  relay_left:
    with: { legs: "${{ tasks.left.output }}" }
    invoke: { tool: "nika:fetch", args: { url: "https://other.example.com/l", method: POST, body: "${{ with.legs }}" } }
  relay_right:
    with: { legs: "${{ tasks.right.output }}" }
    invoke: { tool: "nika:fetch", args: { url: "https://other.example.com/r", method: POST, body: "${{ with.legs }}" } }
"#;

/// The pass settles on a recover cycle, and each record carries both secrets: its own send's,
/// directly, and the other's, through the recover.
#[test]
fn two_tasks_that_recover_from_each_other_each_carry_both_secrets() {
    let report = run(RECOVER_FROM_EACH_OTHER);
    assert_eq!(
        leaks(&report),
        [
            leak_of(
                "relay_left",
                "alpha",
                "secrets.alpha → tasks.left.output → with.legs @ relay_left"
            ),
            leak_of(
                "relay_left",
                "omega",
                "secrets.omega → tasks.right.output → on_error.recover @ left → \
                 tasks.left.output → with.legs @ relay_left"
            ),
            leak_of(
                "relay_right",
                "alpha",
                "secrets.alpha → tasks.left.output → on_error.recover @ right → \
                 tasks.right.output → with.legs @ relay_right"
            ),
            leak_of(
                "relay_right",
                "omega",
                "secrets.omega → tasks.right.output → with.legs @ relay_right"
            ),
        ]
    );
    assert_eq!(
        findings(&report),
        [
            sec_006("relay_left"),
            sec_006("relay_left"),
            sec_006("relay_right"),
            sec_006("relay_right")
        ]
    );
    for secret in ["alpha", "omega"] {
        assert_eq!(
            flows_to_of(&report, secret),
            ["crm.example.com", "other.example.com"],
            "{secret}"
        );
    }
}

/// `call_a` sends `alpha` and `call_b` sends `omega`, both to crm.example.com. `hop` posts the
/// fold to crm.example.com, where both may go; `relay` posts `hop`'s record to
/// other.example.com, where only `alpha` may go.
const TWO_SECRETS_ONE_FOLD: &str = r#"nika: two-secrets-one-fold
secrets:
  alpha: { source: env, key: A, egress: [{ to: "nika:fetch", host: "crm.example.com" }, { to: "nika:fetch", host: "other.example.com" }] }
  omega: { source: env, key: O, egress: [{ to: "nika:fetch", host: "crm.example.com" }] }
permits:
  tools: ["nika:fetch"]
  net: { http: ["crm.example.com", "other.example.com"] }
tasks:
  call_a:
    group: calls
    invoke: { tool: "nika:fetch", args: { url: "https://crm.example.com/a", headers: { authorization: "Bearer ${{ secrets.alpha }}" } } }
  call_b:
    group: calls
    invoke: { tool: "nika:fetch", args: { url: "https://crm.example.com/b", headers: { authorization: "Bearer ${{ secrets.omega }}" } } }
  hop:
    with: { legs: "${{ group.calls }}" }
    invoke: { tool: "nika:fetch", args: { url: "https://crm.example.com/h", method: POST, body: "${{ with.legs }}" } }
  relay:
    with: { legs: "${{ tasks.hop.output }}" }
    invoke: { tool: "nika:fetch", args: { url: "https://other.example.com/in", method: POST, body: "${{ with.legs }}" } }
"#;

#[test]
fn a_record_past_the_first_hop_carries_every_secret_its_fold_held() {
    let report = run(TWO_SECRETS_ONE_FOLD);
    assert_eq!(
        leaks(&report),
        [leak_of(
            "relay",
            "omega",
            "secrets.omega → tasks.call_b.output → with.legs @ hop → tasks.hop.output → \
             with.legs @ relay"
        )]
    );
    assert_eq!(findings(&report), [sec_006("relay")]);
    // The journey reads every secret at every effect, not the first one only.
    for secret in ["alpha", "omega"] {
        assert_eq!(
            flows_to_of(&report, secret),
            ["crm.example.com", "other.example.com"],
            "{secret}"
        );
    }
}

/// `backup` sends `alpha` and recovers to `omega`; `hop` posts `backup`'s record to
/// crm.example.com and `relay` posts `hop`'s to other.example.com, where only `alpha` may go.
const EFFECT_AND_RECOVER: &str = r#"nika: effect-and-recover
secrets:
  alpha: { source: env, key: A, egress: [{ to: "nika:fetch", host: "crm.example.com" }, { to: "nika:fetch", host: "other.example.com" }] }
  omega: { source: env, key: O, egress: [{ to: "nika:fetch", host: "crm.example.com" }] }
permits:
  tools: ["nika:fetch"]
  net: { http: ["crm.example.com", "other.example.com"] }
tasks:
  call_o:
    invoke: { tool: "nika:fetch", args: { url: "https://crm.example.com/o", headers: { authorization: "Bearer ${{ secrets.omega }}" } } }
  backup:
    invoke: { tool: "nika:fetch", args: { url: "https://crm.example.com/b", headers: { authorization: "Bearer ${{ secrets.alpha }}" } } }
    on_error: { recover: "VALUE" }
  hop:
    with: { legs: "${{ tasks.backup.output }}" }
    invoke: { tool: "nika:fetch", args: { url: "https://crm.example.com/h", method: POST, body: "${{ with.legs }}" } }
  relay:
    with: { legs: "${{ tasks.hop.output }}" }
    invoke: { tool: "nika:fetch", args: { url: "https://other.example.com/in", method: POST, body: "${{ with.legs }}" } }
"#;

#[test]
fn a_record_carries_its_recovered_secret_beside_its_effect_two_hops_on() {
    for (value, root) in [
        ("${{ secrets.omega }}", "secrets.omega"),
        (
            "${{ tasks.call_o.output }}",
            "secrets.omega → tasks.call_o.output",
        ),
    ] {
        let report = run(&EFFECT_AND_RECOVER.replace("VALUE", value));
        let path = format!(
            "{root} → on_error.recover @ backup → tasks.backup.output → with.legs @ hop → \
             tasks.hop.output → with.legs @ relay"
        );
        assert_eq!(
            leaks(&report),
            [leak_of("relay", "omega", &path)],
            "{value}"
        );
        assert_eq!(findings(&report), [sec_006("relay")], "{value}");
    }
}

/// `backup` sends `alpha` and recovers to `omega`; the workflow returns `backup`'s record.
/// `ALPHA_RULES` decide whether `alpha` may leave as the return value.
const RECOVER_TO_OUTPUTS: &str = r#"nika: recover-to-outputs
secrets:
  alpha: { source: env, key: A, egress: [ALPHA_RULES] }
  omega: { source: env, key: O, egress: [{ to: "nika:fetch", host: "crm.example.com" }] }
permits:
  tools: ["nika:fetch"]
  net: { http: ["crm.example.com"] }
tasks:
  backup:
    invoke: { tool: "nika:fetch", args: { url: "https://crm.example.com/b", headers: { authorization: "Bearer ${{ secrets.alpha }}" } } }
    on_error: { recover: "${{ secrets.omega }}" }
outputs:
  r: "${{ tasks.backup.output }}"
"#;

/// `to: outputs` clears the return value for `alpha` only: `omega`, which the same record
/// carries through the recover, is still `NIKA-SEC-007`.
#[test]
fn the_outputs_boundary_judges_every_secret_the_record_carries() {
    let report = run(&RECOVER_TO_OUTPUTS.replace(
        "ALPHA_RULES",
        r#"{ to: "nika:fetch", host: "crm.example.com" }, { to: "outputs" }"#,
    ));
    assert_eq!(
        egresses(&report),
        [(
            "r".to_owned(),
            "omega".to_owned(),
            "secrets.omega → on_error.recover @ backup → tasks.backup.output → outputs".to_owned()
        )]
    );
    assert_eq!(leaks(&report), Vec::<Leak>::new());
    assert_eq!(findings(&report), [sec_007()]);
}

/// One entry carrying two secrets neither of which may leave is two refusals, one per secret,
/// as a request carrying two unsanctioned secrets is two leaks.
#[test]
fn an_outputs_entry_carrying_two_forbidden_secrets_is_refused_for_each() {
    let report = run(&RECOVER_TO_OUTPUTS.replace(
        "ALPHA_RULES",
        r#"{ to: "nika:fetch", host: "crm.example.com" }"#,
    ));
    assert_eq!(
        egresses(&report),
        [
            (
                "r".to_owned(),
                "alpha".to_owned(),
                "secrets.alpha → tasks.backup.output → outputs".to_owned()
            ),
            (
                "r".to_owned(),
                "omega".to_owned(),
                "secrets.omega → on_error.recover @ backup → tasks.backup.output → outputs"
                    .to_owned()
            ),
        ]
    );
    assert_eq!(findings(&report), [sec_007(), sec_007()]);
}

/// `hop` loops over a literal list that holds its fold of `calls`, posts each item to
/// crm.example.com, and `relay` posts `hop`'s record to other.example.com.
const LITERAL_LIST_OF_A_FOLD: &str = r#"nika: list-of-a-fold
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
    with: { legs: "${{ group.calls }}" }
    for_each: { items: ["${{ with.legs }}"] }
    invoke: { tool: "nika:fetch", args: { url: "https://crm.example.com/h", method: POST, body: "${{ item }}" } }
  relay:
    with: { legs: "${{ tasks.hop.output }}" }
    invoke: { tool: "nika:fetch", args: { url: "https://other.example.com/in", method: POST, body: "${{ with.legs }}" } }
"#;

#[test]
fn a_literal_for_each_list_passes_its_fold_on_to_the_output() {
    let report = run(LITERAL_LIST_OF_A_FOLD);
    assert_eq!(
        leaks(&report),
        [leak(
            "relay",
            "secrets.token → tasks.call_a.output → with.legs @ hop → item @ hop → \
             tasks.hop.output → with.legs @ relay"
        )]
    );
    assert_eq!(findings(&report), [sec_006("relay")]);
    assert_eq!(flows_to(&report), ["crm.example.com", "other.example.com"]);
}

/// The same literal list over one task's record, no fold: `hop`'s request was already judged
/// with the item, its output was not.
const LITERAL_LIST_OF_A_TASK: &str = r#"nika: list-of-a-task
secrets:
  token: { source: env, key: T, egress: [{ to: "nika:fetch", host: "crm.example.com" }] }
permits:
  tools: ["nika:fetch"]
  net: { http: ["crm.example.com", "other.example.com"] }
tasks:
  call_a:
    invoke: { tool: "nika:fetch", args: { url: "https://crm.example.com/a", headers: { authorization: "Bearer ${{ secrets.token }}" } } }
  hop:
    with: { a: "${{ tasks.call_a.output }}" }
    for_each: { items: ["${{ with.a }}"] }
    invoke: { tool: "nika:fetch", args: { url: "https://crm.example.com/h", method: POST, body: "${{ item }}" } }
  relay:
    with: { legs: "${{ tasks.hop.output }}" }
    invoke: { tool: "nika:fetch", args: { url: "https://other.example.com/in", method: POST, body: "${{ with.legs }}" } }
"#;

#[test]
fn a_literal_for_each_list_passes_a_task_read_on_to_the_output() {
    let report = run(LITERAL_LIST_OF_A_TASK);
    assert_eq!(
        leaks(&report),
        [leak(
            "relay",
            "secrets.token → tasks.call_a.output → with.a @ hop → item @ hop → \
             tasks.hop.output → with.legs @ relay"
        )]
    );
    assert_eq!(findings(&report), [sec_006("relay")]);
    assert_eq!(flows_to(&report), ["crm.example.com", "other.example.com"]);
}

/// `backup` loops over `call_a`'s record without sending an item, and recovers to the item.
const RECOVER_TO_THE_ITEM: &str = r#"nika: recover-to-the-item
secrets:
  token: { source: env, key: T, egress: [{ to: "nika:fetch", host: "crm.example.com" }] }
permits:
  tools: ["nika:fetch"]
  net: { http: ["crm.example.com", "other.example.com"] }
tasks:
  call_a:
    invoke: { tool: "nika:fetch", args: { url: "https://crm.example.com/a", headers: { authorization: "Bearer ${{ secrets.token }}" } } }
  backup:
    with: { legs: "${{ tasks.call_a.output }}" }
    for_each: { items: "${{ with.legs }}", max_items: 10 }
    invoke: { tool: "nika:fetch", args: { url: "https://crm.example.com/b" } }
    on_error: { recover: "${{ item }}" }
  relay:
    with: { legs: "${{ tasks.backup.output }}" }
    invoke: { tool: "nika:fetch", args: { url: "https://other.example.com/in", method: POST, body: "${{ with.legs }}" } }
"#;

#[test]
fn an_item_read_in_a_recover_value_rides_the_record() {
    let report = run(RECOVER_TO_THE_ITEM);
    assert_eq!(
        leaks(&report),
        [leak(
            "relay",
            "secrets.token → tasks.call_a.output → with.legs @ backup → item @ backup → \
             on_error.recover @ backup → tasks.backup.output → with.legs @ relay"
        )]
    );
    assert_eq!(findings(&report), [sec_006("relay")]);
}

/// `backup` is a model task (`VERB`) that recovers to the token; `relay` posts its record.
const MODEL_TASK_RECOVERS: &str = r#"nika: model-recovers
secrets:
  token: { source: env, key: T, egress: [{ to: "nika:fetch", host: "crm.example.com" }] }
permits:
  tools: ["nika:fetch"]
  net: { http: ["other.example.com"] }
tasks:
  backup:
    VERB
    on_error: { recover: "${{ secrets.token }}" }
  relay:
    with: { legs: "${{ tasks.backup.output }}" }
    invoke: { tool: "nika:fetch", args: { url: "https://other.example.com/in", method: POST, body: "${{ with.legs }}" } }
"#;

/// The model carve-out covers what the provider answers, not a recovered value: whatever the
/// verb, the recovered record carries the secret.
#[test]
fn a_model_task_whose_recover_reads_a_secret_carries_it() {
    for verb in [
        r#"infer: { model: "mock/echo", prompt: "summarize the day", max_tokens: 10 }"#,
        r#"agent: { model: "mock/echo", prompt: "summarize the day", max_turns: 2 }"#,
    ] {
        let report = run(&MODEL_TASK_RECOVERS.replace("VERB", verb));
        assert_eq!(
            leaks(&report),
            [leak(
                "relay",
                "secrets.token → on_error.recover @ backup → tasks.backup.output → \
                 with.legs @ relay"
            )],
            "{verb}"
        );
        assert_eq!(findings(&report), [sec_006("relay")], "{verb}");
    }
}

/// `ask` is an `infer:` member of `calls` whose prompt the token may reach; `RECOVER` is its
/// error handling. `relay` posts the fold to other.example.com.
const INFER_MEMBER: &str = r#"nika: infer-member
secrets:
  token: { source: env, key: T, egress: [{ to: "infer" }] }
permits:
  tools: ["nika:fetch"]
  net: { http: ["other.example.com"] }
tasks:
  ask:
    group: calls
    infer: { model: "mock/echo", prompt: "use ${{ secrets.token }}", max_tokens: 10 }
    RECOVER
  call_b:
    group: calls
    invoke: { tool: "nika:fetch", args: { url: "https://other.example.com/b" } }
  relay:
    with: { legs: "${{ group.calls }}" }
    invoke: { tool: "nika:fetch", args: { url: "https://other.example.com/in", method: POST, body: "${{ with.legs }}" } }
"#;

/// A fold reads an `infer:` member as a task read does: the answer is not tainted by the
/// prompt's secret, a recovered value is.
#[test]
fn an_infer_member_of_a_fold_is_judged_as_its_task_read() {
    let answered = run(&INFER_MEMBER.replace("RECOVER", ""));
    assert_eq!(leaks(&answered), Vec::<Leak>::new());
    assert_eq!(findings(&answered), Vec::<Finding>::new());

    let recovered = run(&INFER_MEMBER.replace(
        "RECOVER",
        r#"on_error: { recover: "${{ secrets.token }}" }"#,
    ));
    assert_eq!(
        leaks(&recovered),
        [leak(
            "relay",
            "secrets.token → on_error.recover @ ask → tasks.ask.output → with.legs @ relay"
        )]
    );
    assert_eq!(findings(&recovered), [sec_006("relay")]);
}

/// A `with:` binding that reads `item` renders once per iteration with the item bound (spec 03,
/// `for_each` semantics), so it carries what the item carries: here a literal list's secret.
const ITEM_THROUGH_A_BINDING: &str = r#"nika: item-through-a-binding
secrets:
  token: { source: env, key: T, egress: [{ to: "nika:fetch", host: "crm.example.com" }] }
permits:
  tools: ["nika:fetch"]
  net: { http: ["crm.example.com", "other.example.com"] }
tasks:
  relay:
    for_each: { items: ["${{ secrets.token }}"] }
    with: { v: "${{ item }}" }
    invoke: { tool: "nika:fetch", args: { url: "https://other.example.com/in", method: POST, body: "${{ with.v }}" } }
"#;

#[test]
fn a_binding_that_reads_the_item_carries_its_secret() {
    let report = run(ITEM_THROUGH_A_BINDING);
    assert_eq!(
        leaks(&report),
        [leak(
            "relay",
            "secrets.token → item @ relay → with.v @ relay"
        )]
    );
    assert_eq!(findings(&report), [sec_006("relay")]);
    assert_eq!(flows_to(&report), ["other.example.com"]);

    // The same send where the secret may go stays allowed.
    let allowed = run(&ITEM_THROUGH_A_BINDING.replace(
        r#"host: "crm.example.com" }] }"#,
        r#"host: "other.example.com" }] }"#,
    ));
    assert_eq!(leaks(&allowed), Vec::<Leak>::new());
    assert_eq!(findings(&allowed), Vec::<Finding>::new());
}

/// The loop runs over a fold, and a binding reads the item: the run renders the binding with
/// each member's record bound.
const ITEM_OF_A_FOLD_THROUGH_A_BINDING: &str = r#"nika: item-of-a-fold-through-a-binding
secrets:
  token: { source: env, key: T, egress: [{ to: "nika:fetch", host: "crm.example.com" }] }
permits:
  tools: ["nika:fetch"]
  net: { http: ["crm.example.com", "other.example.com"] }
tasks:
  call_a:
    group: calls
    invoke: { tool: "nika:fetch", args: { url: "https://crm.example.com/a", headers: { authorization: "Bearer ${{ secrets.token }}" } } }
  relay:
    with: { legs: "${{ group.calls }}", one: "${{ item }}" }
    for_each: { items: "${{ with.legs }}", max_items: 10 }
    invoke: { tool: "nika:fetch", args: { url: "https://other.example.com/in", method: POST, body: "${{ with.one }}" } }
"#;

#[test]
fn a_binding_that_reads_the_item_of_a_fold_carries_its_secret() {
    let report = run(ITEM_OF_A_FOLD_THROUGH_A_BINDING);
    assert_eq!(
        leaks(&report),
        [leak(
            "relay",
            "secrets.token → tasks.call_a.output → with.legs @ relay → item @ relay → \
             with.one @ relay"
        )]
    );
    assert_eq!(findings(&report), [sec_006("relay")]);
    assert_eq!(flows_to(&report), ["crm.example.com", "other.example.com"]);
}

/// A recover value reads a binding that reads the item: the recovered record carries it.
const ITEM_THROUGH_A_RECOVERED_BINDING: &str = r#"nika: item-through-a-recovered-binding
secrets:
  token: { source: env, key: T, egress: [{ to: "nika:fetch", host: "crm.example.com" }] }
permits:
  tools: ["nika:fetch"]
  net: { http: ["crm.example.com", "other.example.com"] }
tasks:
  call_a:
    invoke: { tool: "nika:fetch", args: { url: "https://crm.example.com/a", headers: { authorization: "Bearer ${{ secrets.token }}" } } }
  backup:
    with: { legs: "${{ tasks.call_a.output }}", one: "${{ item }}" }
    for_each: { items: "${{ with.legs }}", max_items: 10 }
    invoke: { tool: "nika:fetch", args: { url: "https://crm.example.com/b" } }
    on_error: { recover: "${{ with.one }}" }
  relay:
    with: { legs: "${{ tasks.backup.output }}" }
    invoke: { tool: "nika:fetch", args: { url: "https://other.example.com/in", method: POST, body: "${{ with.legs }}" } }
"#;

#[test]
fn a_recover_value_that_reads_an_item_binding_carries_its_secret() {
    let report = run(ITEM_THROUGH_A_RECOVERED_BINDING);
    assert_eq!(
        leaks(&report),
        [leak(
            "relay",
            "secrets.token → tasks.call_a.output → with.legs @ backup → item @ backup → \
             with.one @ backup → on_error.recover @ backup → tasks.backup.output → \
             with.legs @ relay"
        )]
    );
    assert_eq!(findings(&report), [sec_006("relay")]);
    assert_eq!(flows_to(&report), ["crm.example.com", "other.example.com"]);
}
