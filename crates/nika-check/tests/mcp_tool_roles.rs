// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>
#![allow(
    clippy::expect_used,
    reason = "fixture construction and required findings are test assertions"
)]

//! NIKA-SEC-009 over the roles declared at `nika mcp approve`: an MCP tool
//! reads and sends on its server, outside `permits.fs` and `permits.net`.
//! A workflow that reads a private inbox through one tool and sends mail
//! through another armed the ingress leg only. Once the operator declares
//! the two roles on the pins, the same file is refused.
//!
//! Every source comes from the injected reader of the public composed-check
//! door: no filesystem, no network, no MCP server.

use nika_check::mcp_roles::McpToolRoles;
use nika_check::{CheckReport, check, check_composed};
use nika_schema::{ParseMode, parse, source::FileId};

const WORKFLOW: &str = "mail.nika";

/// Read the inbox through one MCP tool, send through another. No `fs`, no
/// `net`, no `exec`: every effect is server-side.
const MAIL: &str = r#"nika: mail-digest
model: mock/echo
permits:
  tools: ["mcp:mail/read_inbox", "mcp:mail/send_email"]
tasks:
  inbox:
    invoke:
      tool: "mcp:mail/read_inbox"
      args: {}
  send:
    with:
      mail: ${{ tasks.inbox.output }}
    invoke:
      tool: "mcp:mail/send_email"
      args:
        to: "${{ with.mail }}"
        body: "digest"
"#;

/// The same flow behind a blocking confirm that dominates it: the gate is
/// asked first, the read binds its answer, and the send runs only after a
/// read that succeeded (a refused confirm skips the read, which cancels it).
const MAIL_GATED: &str = r#"nika: mail-digest-gated
model: mock/echo
permits:
  tools: ["mcp:mail/read_inbox", "mcp:mail/send_email", "nika:prompt"]
tasks:
  approve:
    invoke:
      tool: "nika:prompt"
      args:
        mode: confirm
        message: "Read the inbox and email the digest?"
  inbox:
    with:
      go: "${{ tasks.approve.output }}"
    when: ${{ with.go == true }}
    invoke:
      tool: "mcp:mail/read_inbox"
      args: {}
  send:
    after: { inbox: success }
    with:
      mail: ${{ tasks.inbox.output }}
    invoke:
      tool: "mcp:mail/send_email"
      args:
        to: "${{ with.mail }}"
        body: "digest"
"#;

const REGISTRY: &str = r#"{"mcp_servers_format":1,"servers":{"mail":{"command":"mail-srv"}}}"#;

/// A pin lockfile for server `mail`; each argument is the tool's `roles`
/// member (`None` = the key is absent: nobody declared a role).
fn pins(read_inbox: Option<&str>, send_email: Option<&str>) -> String {
    let member =
        |roles: Option<&str>| roles.map_or_else(String::new, |r| format!(r#","roles":{r}"#));
    format!(
        r#"{{"mcp_pins_format":1,"servers":{{"mail":{{"pinned_at":"t","tools":{{
            "read_inbox":{{"pin":"blake3:00","description":"","inputSchema":{{}}{}}},
            "send_email":{{"pin":"blake3:01","description":"","inputSchema":{{}}{}}}}}}}}}}}"#,
        member(read_inbox),
        member(send_email)
    )
}

fn parsed(source: &str) -> nika_schema::raw::RawWorkflow {
    parse(source, FileId::new(0), ParseMode::Strict).expect("the fixture parses")
}

/// The composed check over `source` with the registry and `lockfile` served.
fn composed(source: &str, lockfile: &str) -> CheckReport {
    check_composed(&parsed(source), WORKFLOW, &mut |path| match path {
        ".nika/mcp_servers.json" => Ok(REGISTRY.to_owned()),
        ".nika/mcp_pins.json" => Ok(lockfile.to_owned()),
        other => Err(format!("No such file or directory: {other}")),
    })
}

fn sec_009(report: &CheckReport) -> Vec<&str> {
    report
        .findings
        .iter()
        .filter(|f| f.code.as_deref() == Some("NIKA-SEC-009"))
        .filter_map(|f| f.task.as_deref())
        .collect()
}

fn role_hints(report: &CheckReport) -> Vec<(&str, &str)> {
    report
        .hints
        .iter()
        .filter(|h| h.kind == "mcp-roles")
        .map(|h| (h.task.as_str(), h.advice.as_str()))
        .collect()
}

/// The gap, as it stands with no declaration: the check passes, and now
/// says which tools its trifecta row did not judge.
#[test]
fn with_no_declared_role_the_flow_passes_and_the_check_names_what_it_did_not_judge() {
    let report = composed(MAIL, &pins(None, None));
    assert!(report.is_clean(), "{:#?}", report.findings);
    assert!(report.mcp_tool_roles.is_empty(), "nothing was declared");
    let hints = role_hints(&report);
    assert_eq!(hints.len(), 2, "{hints:?}");
    assert_eq!(hints[0].0, "inbox");
    assert!(
        hints[0]
            .1
            .starts_with("`mcp:mail/read_inbox` is approved with no declared role"),
        "{}",
        hints[0].1
    );
    assert_eq!(hints[1].0, "send");
    assert!(
        hints[1]
            .1
            .starts_with("`mcp:mail/send_email` is approved with no declared role"),
        "{}",
        hints[1].1
    );
}

/// The declared roles complete the trifecta: the send is refused, and the
/// refusal says which pins armed which leg.
#[test]
fn declared_roles_refuse_the_mcp_only_exfiltration_path() {
    let report = composed(
        MAIL,
        &pins(Some(r#"["private-read"]"#), Some(r#"["egress"]"#)),
    );
    assert_eq!(
        report.mcp_tool_roles.get("mcp:mail/read_inbox"),
        Some(&McpToolRoles::new(true, false))
    );
    assert_eq!(
        report.mcp_tool_roles.get("mcp:mail/send_email"),
        Some(&McpToolRoles::new(false, true))
    );
    assert!(!report.is_clean(), "the lane gates the check");
    assert_eq!(sec_009(&report), vec!["send"], "{:#?}", report.findings);
    assert_eq!(report.trifecta_findings.len(), 1);
    let finding = &report.trifecta_findings[0];
    assert_eq!(finding.task, "send");
    assert_eq!(finding.source.as_deref(), Some("inbox"));
    assert!(
        finding
            .detail
            .contains("an MCP tool declared a private read on its pin (task `inbox`)"),
        "{}",
        finding.detail
    );
    assert!(
        finding
            .detail
            .contains("an MCP tool declared an egress on its pin (task `send`)"),
        "{}",
        finding.detail
    );
    assert!(
        role_hints(&report).is_empty(),
        "both tools are declared: nothing is left unjudged"
    );
}

/// One declared role is one leg: with the read undeclared (or the send
/// declared inert) the trifecta is incomplete and the file passes.
#[test]
fn one_declared_leg_alone_does_not_refuse() {
    let egress_only = composed(MAIL, &pins(None, Some(r#"["egress"]"#)));
    assert!(egress_only.is_clean(), "{:#?}", egress_only.findings);
    assert_eq!(
        role_hints(&egress_only).len(),
        1,
        "the read is still unknown"
    );

    let inert_send = composed(MAIL, &pins(Some(r#"["private-read"]"#), Some("[]")));
    assert!(inert_send.is_clean(), "{:#?}", inert_send.findings);
    assert_eq!(
        inert_send.mcp_tool_roles.get("mcp:mail/send_email"),
        Some(&McpToolRoles::default()),
        "a declared-inert tool is recorded, not unknown"
    );
    assert!(role_hints(&inert_send).is_empty());
}

/// A blocking confirm that dominates the flow clears the declared-role
/// trifecta, as it clears a `permits`-armed one, and the gate is credited:
/// the lane judged the flow and found it gated, it did not skip it.
#[test]
fn a_dominating_confirm_gate_clears_the_declared_trifecta_and_is_credited() {
    let report = composed(
        MAIL_GATED,
        &pins(Some(r#"["private-read"]"#), Some(r#"["egress"]"#)),
    );
    assert!(report.is_clean(), "{:#?}", report.findings);
    assert!(report.trifecta_findings.is_empty());
    let credited: Vec<(&str, &str)> = report
        .trifecta_mitigations
        .iter()
        .map(|m| (m.sink.as_str(), m.gate.as_str()))
        .collect();
    assert_eq!(credited, vec![("send", "approve")]);

    // With no declared role there is no trifecta, so there is nothing to
    // credit: the two empty verdicts are different facts.
    let undeclared = composed(MAIL_GATED, &pins(None, None));
    assert!(undeclared.is_clean(), "{:#?}", undeclared.findings);
    assert!(undeclared.trifecta_mitigations.is_empty());
}

/// A role value this engine cannot read never loosens the lane: it counts
/// as both roles.
#[test]
fn an_unreadable_role_counts_as_both_legs() {
    let report = composed(
        MAIL,
        &pins(Some(r#"["from-the-future"]"#), Some(r#"["egress"]"#)),
    );
    assert_eq!(
        report.mcp_tool_roles.get("mcp:mail/read_inbox"),
        Some(&McpToolRoles::new(true, true))
    );
    assert_eq!(sec_009(&report), vec!["send"], "{:#?}", report.findings);
}

/// The reader-less check reads no lockfile: it carries no role and keeps
/// its verdict on the bytes alone.
#[test]
fn the_reader_less_check_carries_no_role() {
    let report = check(&parsed(MAIL));
    assert!(report.mcp_tool_roles.is_empty());
    assert!(report.trifecta_findings.is_empty());
    assert!(role_hints(&report).is_empty());
}

/// The roles ride the report's JSON as the lockfile spells them, and are
/// absent from it when nothing was declared.
#[test]
fn the_report_json_carries_the_roles_only_when_declared() {
    let declared = composed(
        MAIL,
        &pins(Some(r#"["private-read"]"#), Some(r#"["egress"]"#)),
    );
    let json = serde_json::to_value(&declared).expect("the report serializes");
    assert_eq!(
        json["mcp_tool_roles"],
        serde_json::json!({
            "mcp:mail/read_inbox": ["private-read"],
            "mcp:mail/send_email": ["egress"],
        })
    );
    let undeclared = composed(MAIL, &pins(None, None));
    let json = serde_json::to_value(&undeclared).expect("the report serializes");
    assert!(json.get("mcp_tool_roles").is_none(), "{json}");
}

/// A child workflow that reads the inbox through the MCP tool.
const CHILD_READS_INBOX: &str = r#"nika: read-inbox
permits:
  tools: ["mcp:mail/read_inbox"]
tasks:
  inbox:
    invoke:
      tool: "mcp:mail/read_inbox"
      args: {}
outputs:
  text: { value: "${{ tasks.inbox.output }}", type: string }
"#;

/// The parent calls that child, then sends what it returned.
const PARENT_SENDS: &str = r#"nika: parent
permits:
  tools: ["mcp:mail/read_inbox", "mcp:mail/send_email"]
tasks:
  call:
    invoke: { workflow: "./child.nika" }
    returns: { object: { text: string } }
  send:
    with:
      mail: ${{ tasks.call.output.text }}
    invoke:
      tool: "mcp:mail/send_email"
      args:
        to: "${{ with.mail }}"
        body: "digest"
"#;

/// The composition wall does not hide a declared role: a child that calls
/// the private-read tool arms that leg for its caller, through the tool
/// grants the call holds.
#[test]
fn a_child_workflow_carries_the_declared_role_of_the_tool_it_calls() {
    let lockfile = pins(Some(r#"["private-read"]"#), Some(r#"["egress"]"#));
    let report = check_composed(&parsed(PARENT_SENDS), WORKFLOW, &mut |path| match path {
        ".nika/mcp_servers.json" => Ok(REGISTRY.to_owned()),
        ".nika/mcp_pins.json" => Ok(lockfile.clone()),
        "child.nika" => Ok(CHILD_READS_INBOX.to_owned()),
        other => Err(format!("No such file or directory: {other}")),
    });
    assert!(
        report.composition.is_empty(),
        "the call resolves: {:?}",
        report.composition
    );
    assert_eq!(sec_009(&report), vec!["send"], "{:#?}", report.findings);
    let finding = &report.trifecta_findings[0];
    assert_eq!(finding.source.as_deref(), Some("call"));
    assert!(
        finding
            .detail
            .contains("an MCP tool declared a private read on its pin (task `call`)"),
        "{}",
        finding.detail
    );
}

/// An agent whose whitelist admits the tools by a glob carries what the
/// glob covers: the read upstream and the agent's send complete the
/// trifecta.
#[test]
fn an_agent_whitelist_glob_carries_the_roles_it_covers() {
    const AGENT: &str = r#"nika: mail-agent
model: mock/echo
permits:
  tools: ["mcp:mail/*"]
tasks:
  inbox:
    invoke:
      tool: "mcp:mail/read_inbox"
      args: {}
  triage:
    with:
      mail: ${{ tasks.inbox.output }}
    agent:
      prompt: "Triage this mail: ${{ with.mail }}"
      tools: ["mcp:mail/send_email"]
      max_turns: 3
      max_tokens_total: 2000
"#;
    let report = composed(
        AGENT,
        &pins(Some(r#"["private-read"]"#), Some(r#"["egress"]"#)),
    );
    assert_eq!(sec_009(&report), vec!["triage"], "{:#?}", report.findings);
    assert!(
        report.trifecta_findings[0]
            .detail
            .contains("an MCP tool declared an egress on its pin (task `triage`)"),
        "{}",
        report.trifecta_findings[0].detail
    );
    // The same agent with the send declared inert is not an egress by role.
    let inert = composed(AGENT, &pins(Some(r#"["private-read"]"#), Some("[]")));
    assert!(sec_009(&inert).is_empty(), "{:#?}", inert.findings);
}
