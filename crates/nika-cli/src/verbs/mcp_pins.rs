// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! `nika mcp approve <server>` — the operator surface of the MCP
//! tool-pinning defence: review a server's CURRENT tool definitions, pin
//! them, print the pin set as the receipt. The server resolves from
//! `.nika/mcp_servers.json` against the workspace CWD; every decision is
//! the library's ([`nika_mcp::client`] · [`nika_mcp::pin`]) — this module
//! maps the outcome to text + the spec §4 exit codes. A run reaches ONLY
//! what was approved here ([`nika_mcp::dispatch`] · NIKA-MCP-006).

/// Operator-facing honesty card for `nika mcp --help` (C02 · issue 1303): a
/// read-only oracle, `nika run` executes · names ⊆ the nine the oracle serves.
pub const OPERATOR_HELP: &str = "\
This MCP server is a read-only authoring oracle.
Tools: nika_check · nika_inspect · nika_explain · nika_schema · nika_examples · nika_template · nika_canon · nika_catalog · nika_tools
It never writes a file and never runs a workflow. To execute: nika run.";

/// The MCP subcommand surface — `approve` (the tool-pinning remediation)
/// or serve (stdio · streamable HTTP) · descended from the bin 2026-07-21.
#[derive(Clone, clap::Subcommand)]
pub enum McpAction {
    /// Re-pin the server's CURRENT tool definitions after human review
    /// (the remediation a drift refusal names), printing the new pin set.
    Approve {
        /// The server name from `.nika/mcp_servers.json`.
        server: String,
        /// Declare that TOOL returns data you hold private (an inbox, a
        /// database, internal files). `nika check` then counts a call to it
        /// as the private read of the lethal trifecta (NIKA-SEC-009).
        #[arg(long = "reads-private", value_name = "TOOL")]
        reads_private: Vec<String>,
        /// Declare that TOOL can send data outside this machine (an email,
        /// a message, a web request). `nika check` then counts a call to it
        /// as the external egress of the lethal trifecta (NIKA-SEC-009).
        #[arg(long = "sends-out", value_name = "TOOL")]
        sends_out: Vec<String>,
        /// Declare that TOOL neither reads private data nor sends anything
        /// out. Without any of the three flags a tool's role stays unknown
        /// and `nika check` says it does not judge that tool.
        #[arg(long = "no-role", value_name = "TOOL")]
        no_role: Vec<String>,
    },
}

/// The roles the three `approve` flags declare, by tool name.
///
/// # Errors
///
/// The tools named both by `--no-role` and by a role flag: the two
/// declarations contradict each other.
pub(crate) fn declared_roles(
    reads_private: &[String],
    sends_out: &[String],
    no_role: &[String],
) -> Result<RoleDeclarations, Vec<String>> {
    let mut declared = RoleDeclarations::new();
    for tool in reads_private {
        let roles = declared.entry(tool.clone()).or_default();
        *roles = roles.union(McpToolRoles::new(true, false));
    }
    for tool in sends_out {
        let roles = declared.entry(tool.clone()).or_default();
        *roles = roles.union(McpToolRoles::new(false, true));
    }
    let contradicted: Vec<String> = no_role
        .iter()
        .filter(|tool| declared.contains_key(*tool))
        .cloned()
        .collect();
    if !contradicted.is_empty() {
        return Err(contradicted);
    }
    for tool in no_role {
        declared.insert(tool.clone(), McpToolRoles::default());
    }
    Ok(declared)
}

/// The MCP wire (the clap arm of `--transport`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum McpTransportArg {
    /// Newline-delimited JSON-RPC over stdin/stdout.
    Stdio,
    /// Streamable HTTP (POST JSON-RPC · origin-gated · loopback default).
    Http,
}

/// The `nika mcp` routing: `approve` dispatches to the pins layer; the
/// server surfaces bind + serve forever. Banner + errors ride stderr.
///
/// # Errors
///
/// `verbs::exit::OK` on a clean serve; `1` when the server itself fails.
#[allow(clippy::disallowed_macros, clippy::print_stderr)]
#[must_use]
pub fn mcp_verb(
    action: Option<McpAction>,
    transport: McpTransportArg,
    port: u16,
    bind: &str,
) -> u8 {
    match action {
        Some(McpAction::Approve {
            server,
            reads_private,
            sends_out,
            no_role,
        }) => {
            return emit(
                &match declared_roles(&reads_private, &sends_out, &no_role) {
                    Ok(declared) => approve(&server, &declared),
                    Err(contradicted) => VerbOutput::env(format!(
                        "`--no-role` and a role flag both name {}: a tool either has a role or \
                     has none — nothing was pinned",
                        contradicted
                            .iter()
                            .map(|t| format!("`{t}`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )),
                },
            );
        }
        None => {}
    }
    let served = match transport {
        McpTransportArg::Stdio => nika_mcp::run_stdio(),
        McpTransportArg::Http => match nika_mcp::HttpServer::bind(bind, port) {
            Ok(server) => {
                // The sanctioned env boundary (same seam as config_from_env):
                // the token is operator config crossing into a server hold.
                #[allow(clippy::disallowed_methods)]
                let token = std::env::var("NIKA_MCP_TOKEN").ok();
                // #890: a public bind with no bearer refuses to START — the
                // refusal names both fixes (set the token · bind loopback).
                if let Err(err) = server.guard_bind_auth(token.as_deref()) {
                    eprintln!("nika mcp: {err}");
                    return 1;
                }
                let addr = server
                    .addr()
                    .map_or_else(|_| format!("{bind}:{port}"), |a| a.to_string());
                eprintln!(
                    "nika mcp · http://{addr}/mcp · POST JSON-RPC (MCP 2025-11-25) · origin-gated · {} · production TLS belongs to a reverse proxy",
                    if token.is_some() {
                        "bearer auth ON (NIKA_MCP_TOKEN)"
                    } else {
                        "no auth (set NIKA_MCP_TOKEN to require a bearer)"
                    }
                );
                server.serve(token.as_deref())
            }
            Err(err) => Err(err),
        },
    };
    match served {
        Ok(()) => crate::verbs::exit::OK,
        Err(err) => {
            eprintln!("nika mcp: {err}");
            1
        }
    }
}

/// Print a verb's text on the right stream and return its exit code
/// (the bin's `emit` — findings and successes go to stdout, only
/// environment errors to stderr · the MCP surface owns its streams).
#[allow(clippy::disallowed_macros, clippy::print_stdout, clippy::print_stderr)]
fn emit(out: &VerbOutput) -> u8 {
    if out.code == crate::verbs::exit::ENV {
        eprintln!("nika: {}", out.text);
    } else if !out.text.is_empty() {
        println!("{}", out.text.trim_end());
    }
    out.code
}
use std::path::Path;

use nika_mcp::client::{
    ApproveError, McpServerConfig, StdioMcpClient, ToolsListDyn, approve_server_declaring,
    load_server_configs,
};
use nika_mcp::pin::{McpToolRoles, PINS_PATH, PinError, RoleDeclarations};

use super::VerbOutput;

/// `nika mcp approve <server>` — re-pin after human review, production client,
/// recording the roles the operator declares for the tools they reviewed.
#[must_use]
pub fn approve(server: &str, declared: &RoleDeclarations) -> VerbOutput {
    let dir = Path::new(".");
    match resolve_config(server, dir) {
        Ok(config) => approve_with(
            &config,
            &StdioMcpClient::new(&config),
            dir,
            declared,
            now_epoch(),
        ),
        Err(out) => out,
    }
}

/// One receipt cell: what `nika check` will read for the tool.
fn role_cell(roles: Option<McpToolRoles>) -> String {
    match roles {
        None => "role: not declared".to_owned(),
        Some(roles) if roles.tokens().is_empty() => "role: none (declared)".to_owned(),
        Some(roles) => format!("role: {}", roles.tokens().join(" + ")),
    }
}

/// The injected-seam core of [`approve`] — generic over the client so
/// tests drive it with a mock (no subprocess · no network).
pub(crate) fn approve_with<C: ToolsListDyn + ?Sized>(
    config: &McpServerConfig,
    client: &C,
    dir: &Path,
    declared: &RoleDeclarations,
    now: u64,
) -> VerbOutput {
    match approve_server_declaring(config, client, dir, declared, now) {
        Ok(report) => {
            use std::fmt::Write as _;
            let mut text = format!(
                "{}approved {} tool(s) from MCP server `{}` — the lockfile now pins the CURRENT definitions\n  \
                 lockfile: {PINS_PATH} · pinned_at {}",
                report
                    .confinement
                    .as_deref()
                    .map_or_else(String::new, |note| format!(
                        "mcp `{}`: {note}\n",
                        report.server
                    )),
                report.pins.len(),
                report.server,
                report.pinned_at
            );
            for (name, pin) in &report.pins {
                let role = role_cell(report.roles.get(name).copied().flatten());
                let _ = write!(text, "\n  {name}  {pin}  {role}");
            }
            if report.pins.is_empty() {
                text.push_str("\n  (the server exposes no tools — nothing to pin)");
            }
            if !report.dropped_roles.is_empty() {
                let _ = write!(
                    text,
                    "\n  role dropped (the definition changed since it was declared): {} — \
                     declare it again after reading the new definition",
                    report.dropped_roles.join(", ")
                );
            }
            if report.roles.values().any(Option::is_none) {
                text.push_str(
                    "\n  a tool with no declared role is not judged by the TRIFECTA row of \
                     `nika check`: declare each with `--reads-private <tool>`, `--sends-out \
                     <tool>` or `--no-role <tool>`",
                );
            }
            VerbOutput::ok(text)
        }
        Err(ApproveError::Pin(err)) => pin_error_output(&err),
        Err(err) => VerbOutput::env(format!("{err}")),
    }
}

/// Resolve one configured server — a teaching env error (exit 3) when the registry lacks it.
fn resolve_config(server: &str, dir: &Path) -> Result<McpServerConfig, VerbOutput> {
    let configs = load_server_configs(dir).map_err(|e| pin_error_output(&e))?;
    configs
        .into_iter()
        .find(|c| c.name == server)
        .ok_or_else(|| {
            VerbOutput::env(format!(
                "MCP server `{server}` is not configured — declare it in .nika/mcp_servers.json first\n  \
                 form: {{\"mcp_servers_format\": 1, \"servers\": {{\"{server}\": {{\"command\": \"…\", \"args\": […]}}}}}}"
            ))
        })
}

/// Pin failures are environment-class (spec §4 · exit 3) · one voice: the error's own Display.
fn pin_error_output(err: &PinError) -> VerbOutput {
    VerbOutput::env(format!("{err}"))
}

/// Wall-clock seconds for `pinned_at`, read at the L4 boundary (INV-027).
fn now_epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use std::path::PathBuf;

    use nika_mcp::pin::McpToolDef;
    use serde_json::json;

    use super::*;
    use crate::verbs::exit;

    struct Mock {
        tools: Vec<McpToolDef>,
    }

    impl ToolsListDyn for Mock {
        fn tools_list(&self) -> Result<Vec<McpToolDef>, PinError> {
            Ok(self.tools.clone())
        }
    }

    fn config() -> McpServerConfig {
        McpServerConfig::stdio("postgres", "honest-srv", Vec::new())
    }

    fn tools() -> Vec<McpToolDef> {
        vec![McpToolDef::new(
            "query",
            "Run a SQL query",
            json!({"type": "object", "properties": {"sql": {"type": "string"}}}),
        )]
    }

    /// No role declared: the pre-existing approve behaviour.
    fn none() -> RoleDeclarations {
        RoleDeclarations::new()
    }

    fn mail_tools() -> Vec<McpToolDef> {
        vec![
            McpToolDef::new("read_inbox", "Read the unread emails", json!({})),
            McpToolDef::new("send_email", "Send an email", json!({})),
            McpToolDef::new("add", "Add two numbers", json!({})),
        ]
    }

    /// The roles the lockfile records for server `postgres`, by tool.
    fn recorded_roles(dir: &Path) -> serde_json::Value {
        let text = std::fs::read_to_string(dir.join(PINS_PATH)).expect("lockfile");
        let file: serde_json::Value = serde_json::from_str(&text).expect("lockfile is JSON");
        let tools = file["servers"]["postgres"]["tools"]
            .as_object()
            .expect("tools")
            .clone();
        tools
            .into_iter()
            .map(|(name, pin)| (name, pin.get("roles").cloned().unwrap_or(json!("absent"))))
            .collect::<serde_json::Map<_, _>>()
            .into()
    }

    fn tmp(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("nika-cli-mcppin-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("tmp dir");
        dir
    }

    #[test]
    fn approve_repins_and_prints_the_pin_set() {
        let dir = tmp("approve");
        approve_with(&config(), &Mock { tools: tools() }, &dir, &none(), 1);
        let upgraded = Mock {
            tools: vec![
                McpToolDef::new(
                    "query",
                    "Run a SQL query — v2",
                    json!({"type": "object", "properties": {"sql": {"type": "string"}}}),
                ),
                McpToolDef::new("explain", "Plan a query", json!({})),
            ],
        };
        let out = approve_with(&config(), &upgraded, &dir, &none(), 1_700_000_000);
        assert_eq!(out.code, exit::OK, "{}", out.text);
        assert!(out.text.contains("approved 2 tool(s)"), "{}", out.text);
        assert!(out.text.contains("explain  blake3:"), "{}", out.text);
        assert!(out.text.contains("query  blake3:"), "{}", out.text);
        // The re-pin is real: the lockfile now matches the NEW surface.
        let text = std::fs::read_to_string(dir.join(PINS_PATH)).expect("lockfile");
        assert!(text.contains("Run a SQL query — v2"), "{text}");
        assert!(text.contains("2023-11-14T22:13:20Z"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// C02 · issue 1303 · the mcp verb's honesty card names the read-only
    /// oracle, at least three served tools, and `nika run` as the execute
    /// door — and does not advertise a write/run tool.
    #[test]
    fn operator_help_names_the_read_only_oracle_and_the_run_door() {
        assert!(OPERATOR_HELP.contains("read-only"), "C02: {OPERATOR_HELP}");
        let named = [
            "nika_check",
            "nika_inspect",
            "nika_explain",
            "nika_schema",
            "nika_examples",
            "nika_template",
            "nika_canon",
            "nika_catalog",
            "nika_tools",
        ]
        .iter()
        .filter(|n| OPERATOR_HELP.contains(*n))
        .count();
        assert!(
            named >= 3,
            "C02 must name at least 3 of the 9 tools, named {named}: {OPERATOR_HELP}"
        );
        assert!(OPERATOR_HELP.contains("nika run"), "C02: {OPERATOR_HELP}");
        for forbidden in ["nika_run", "nika_write", "nika_exec"] {
            assert!(
                !OPERATOR_HELP.contains(forbidden),
                "C02 WALL: help must not advertise `{forbidden}`: {OPERATOR_HELP}"
            );
        }
    }

    #[test]
    fn an_unconfigured_server_is_a_teaching_env_error() {
        let dir = tmp("unconfigured");
        let out = resolve_config("ghost", &dir).expect_err("unknown server");
        assert_eq!(out.code, exit::ENV, "{}", out.text);
        assert!(out.text.contains("not configured"), "{}", out.text);
        assert!(out.text.contains(".nika/mcp_servers.json"), "{}", out.text);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The receipt names the confinement the server ran under FIRST — the
    /// production client reports it; a mock has none and the receipt
    /// starts at the approval line.
    #[test]
    fn the_receipt_names_the_confinement_first() {
        struct Confined(Mock);
        impl ToolsListDyn for Confined {
            fn tools_list(&self) -> Result<Vec<McpToolDef>, PinError> {
                self.0.tools_list()
            }
            fn confinement(&self) -> Option<String> {
                Some("sandboxed (seatbelt · net deny)".to_owned())
            }
        }
        let dir = tmp("confined");
        let out = approve_with(
            &config(),
            &Confined(Mock { tools: tools() }),
            &dir,
            &none(),
            1,
        );
        assert_eq!(out.code, exit::OK, "{}", out.text);
        assert!(
            out.text
                .starts_with("mcp `postgres`: sandboxed (seatbelt · net deny)\napproved 1 tool(s)"),
            "{}",
            out.text
        );
        let bare = approve_with(&config(), &Mock { tools: tools() }, &dir, &none(), 1);
        assert!(bare.text.starts_with("approved 1 tool(s)"), "{}", bare.text);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_zero_tool_server_says_nothing_to_pin() {
        let dir = tmp("zero");
        let out = approve_with(&config(), &Mock { tools: Vec::new() }, &dir, &none(), 1);
        assert_eq!(out.code, exit::OK, "{}", out.text);
        assert!(out.text.contains("approved 0 tool(s)"), "{}", out.text);
        assert!(out.text.contains("nothing to pin"), "{}", out.text);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The three flags become the lockfile's `roles`, and the receipt says
    /// what `nika check` will read for each tool.
    #[test]
    fn approve_records_the_declared_roles_and_prints_them() {
        let dir = tmp("roles");
        let declared = declared_roles(
            &["read_inbox".to_owned()],
            &["send_email".to_owned()],
            &["add".to_owned()],
        )
        .expect("no contradiction");
        let server = Mock {
            tools: mail_tools(),
        };
        let out = approve_with(&config(), &server, &dir, &declared, 1);
        assert_eq!(out.code, exit::OK, "{}", out.text);
        assert_eq!(
            recorded_roles(&dir),
            json!({"add": [], "read_inbox": ["private-read"], "send_email": ["egress"]})
        );
        for line in [
            "  role: none (declared)",
            "  role: private-read",
            "  role: egress",
        ] {
            assert!(out.text.contains(line), "`{line}` in:\n{}", out.text);
        }
        assert!(
            !out.text.contains("not judged by the TRIFECTA row"),
            "every tool is declared, so nothing is left unjudged: {}",
            out.text
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A tool nobody declared stays unknown: no `roles` key, and the
    /// receipt says the check does not judge it.
    #[test]
    fn an_undeclared_tool_stays_unknown_and_the_receipt_says_so() {
        let dir = tmp("undeclared");
        let declared =
            declared_roles(&[], &["send_email".to_owned()], &[]).expect("no contradiction");
        let server = Mock {
            tools: mail_tools(),
        };
        let out = approve_with(&config(), &server, &dir, &declared, 1);
        assert_eq!(out.code, exit::OK, "{}", out.text);
        assert_eq!(
            recorded_roles(&dir),
            json!({"add": "absent", "read_inbox": "absent", "send_email": ["egress"]})
        );
        assert!(out.text.contains("role: not declared"), "{}", out.text);
        assert!(
            out.text
                .contains("a tool with no declared role is not judged by the TRIFECTA row"),
            "{}",
            out.text
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Approving again without flags keeps a role while the definition is
    /// the same, and drops it, loudly, once the definition changed.
    #[test]
    fn a_role_survives_reapproval_only_while_the_definition_is_unchanged() {
        let dir = tmp("carry");
        let declared = declared_roles(&["read_inbox".to_owned()], &["send_email".to_owned()], &[])
            .expect("no contradiction");
        let server = Mock {
            tools: mail_tools(),
        };
        approve_with(&config(), &server, &dir, &declared, 1);
        // Same definitions, no flag: both roles stay.
        let out = approve_with(&config(), &server, &dir, &none(), 2);
        assert_eq!(out.code, exit::OK, "{}", out.text);
        assert_eq!(
            recorded_roles(&dir),
            json!({"add": "absent", "read_inbox": ["private-read"], "send_email": ["egress"]})
        );
        assert!(!out.text.contains("role dropped"), "{}", out.text);
        // `send_email` changes its definition: its role is not carried over.
        let mut changed = mail_tools();
        changed[1] = McpToolDef::new("send_email", "Send an email to anyone", json!({}));
        let out = approve_with(&config(), &Mock { tools: changed }, &dir, &none(), 3);
        assert_eq!(out.code, exit::OK, "{}", out.text);
        assert_eq!(
            recorded_roles(&dir),
            json!({"add": "absent", "read_inbox": ["private-read"], "send_email": "absent"})
        );
        assert!(
            out.text.contains(
                "role dropped (the definition changed since it was declared): send_email"
            ),
            "{}",
            out.text
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A role declared for a tool the server does not serve pins nothing:
    /// a typo must not leave the intended tool undeclared.
    #[test]
    fn a_role_for_a_tool_the_server_does_not_serve_pins_nothing() {
        let dir = tmp("typo");
        let declared =
            declared_roles(&[], &["send_mail".to_owned()], &[]).expect("no contradiction");
        let server = Mock {
            tools: mail_tools(),
        };
        let out = approve_with(&config(), &server, &dir, &declared, 1);
        assert_eq!(out.code, exit::ENV, "{}", out.text);
        assert_eq!(
            out.text,
            "MCP server `postgres` serves no tool named `send_mail` — nothing was pinned \
             (it serves: `add`, `read_inbox`, `send_email`)"
        );
        assert!(!dir.join(PINS_PATH).exists(), "no lockfile was written");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// One tool under two role flags carries both; `--no-role` on a tool
    /// that also has a role flag is a contradiction the verb refuses.
    #[test]
    fn the_flags_combine_and_a_contradiction_is_named() {
        let both = declared_roles(&["sync".to_owned()], &["sync".to_owned()], &[])
            .expect("no contradiction");
        assert_eq!(both.get("sync"), Some(&McpToolRoles::new(true, true)));
        assert_eq!(
            declared_roles(&["a".to_owned()], &["b".to_owned()], &["b".to_owned()]),
            Err(vec!["b".to_owned()])
        );
    }
}
