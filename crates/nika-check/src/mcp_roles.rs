// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! The role an operator declares for an approved MCP tool.
//!
//! An MCP tool reads and sends on its server, where no `permits:` field
//! sees it. The lethal-trifecta lane (NEP-0002 · `NIKA-SEC-009`) reads its
//! private-read and external-egress legs off `permits.fs` and `permits.net`,
//! so a workflow that reads a private inbox through one MCP tool and sends
//! mail through another armed the ingress leg only, and passed.
//!
//! The operator already reviews every tool definition at
//! `nika mcp approve <server>`. That review is where the role is declared,
//! and the pin lockfile (`.nika/mcp_pins.json`) carries it beside the pin:
//!
//! - `private-read` · the tool returns data the operator holds private.
//! - `egress` · the tool can send data to a destination outside the machine.
//! - an empty list · the operator declared the tool does neither.
//! - no `roles` key · nobody declared anything: the role is UNKNOWN.
//!
//! A declared role only ADDS a leg. An unknown role arms nothing more than
//! before, and the lane says so in a `mcp-roles` hint: the verdict then
//! covers the declared boundary and not what that tool reads or sends.
//!
//! This crate reads no file. The composed check injects the reader, and the
//! roles it found ride [`crate::CheckReport::mcp_tool_roles`] so a gate that
//! reads no file (the run's trust gate) judges the same legs.

use std::collections::{BTreeMap, BTreeSet};

use nika_schema::raw::{RawAction, RawWorkflow};

use crate::tools::{MCP_PINS_PATH, mcp_call_sites};

/// The roles declared for every approved tool a workflow can reach, keyed
/// by the full tool id (`mcp:<server>/<tool>`). A tool with no entry has no
/// declared role.
pub type McpRoleMap = BTreeMap<String, McpToolRoles>;

/// What the operator declared one approved MCP tool does, in the trifecta's
/// own terms. `Default` is the declared-inert tool (neither role).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
#[non_exhaustive]
pub struct McpToolRoles {
    /// The tool returns data the operator holds private (leg ①).
    pub private_read: bool,
    /// The tool can send data outside the machine (leg ③).
    pub egress: bool,
}

impl McpToolRoles {
    /// The lockfile token of the private-read role.
    pub const PRIVATE_READ: &'static str = "private-read";
    /// The lockfile token of the egress role.
    pub const EGRESS: &'static str = "egress";

    /// Construct (INV-019 · `new()` on every `#[non_exhaustive]` struct).
    #[must_use]
    pub const fn new(private_read: bool, egress: bool) -> Self {
        Self {
            private_read,
            egress,
        }
    }

    /// Read the lockfile's token list.
    ///
    /// # Errors
    ///
    /// The first token this engine does not know, verbatim: a role written
    /// by a newer engine must never be read as "no role".
    pub fn from_tokens<I, S>(tokens: I) -> Result<Self, String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut roles = Self::default();
        for token in tokens {
            match token.as_ref() {
                Self::PRIVATE_READ => roles.private_read = true,
                Self::EGRESS => roles.egress = true,
                other => return Err(other.to_owned()),
            }
        }
        Ok(roles)
    }

    /// The lockfile's token list, in a fixed order.
    #[must_use]
    pub fn tokens(self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.private_read {
            out.push(Self::PRIVATE_READ);
        }
        if self.egress {
            out.push(Self::EGRESS);
        }
        out
    }

    /// Both declarations joined: a task that can call two tools carries
    /// what either can do.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self::new(
            self.private_read || other.private_read,
            self.egress || other.egress,
        )
    }
}

impl serde::Serialize for McpToolRoles {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.tokens())
    }
}

impl<'de> serde::Deserialize<'de> for McpToolRoles {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let tokens = Vec::<String>::deserialize(deserializer)?;
        Self::from_tokens(&tokens).map_err(|unknown| {
            serde::de::Error::custom(format!(
                "unknown MCP tool role `{unknown}` (this engine knows `{}` and `{}`)",
                Self::PRIVATE_READ,
                Self::EGRESS
            ))
        })
    }
}

/// The declared roles of every approved tool `wf` can reach, read from the
/// pin lockfile through the injected reader. Empty when the workflow names
/// no MCP tool, when the lockfile is absent or unreadable, or when nothing
/// was declared.
///
/// A `roles` value this engine cannot read (not a list, or a token it does
/// not know) declares BOTH roles: an unreadable declaration never loosens
/// the lane.
pub(crate) fn read_declared(
    wf: &RawWorkflow,
    read: &mut dyn FnMut(&str) -> Result<String, String>,
) -> McpRoleMap {
    let reachable = reachable_grants(wf);
    if reachable.is_empty() {
        return McpRoleMap::new();
    }
    let pins: serde_json::Value = read(MCP_PINS_PATH)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(serde_json::Value::Null);
    let Some(servers) = pins.get("servers").and_then(serde_json::Value::as_object) else {
        return McpRoleMap::new();
    };
    let mut out = McpRoleMap::new();
    for (server, entry) in servers {
        let Some(tools) = entry.get("tools").and_then(serde_json::Value::as_object) else {
            continue;
        };
        for (name, pin) in tools {
            let Some(declared) = pin.get("roles") else {
                continue;
            };
            let id = format!("mcp:{server}/{name}");
            if !reachable.iter().any(|g| nika_cap::glob_matches(g, &id)) {
                continue;
            }
            out.insert(id, roles_of_value(declared));
        }
    }
    out
}

/// One lockfile `roles` value, fail-closed (see [`read_declared`]).
fn roles_of_value(declared: &serde_json::Value) -> McpToolRoles {
    declared
        .as_array()
        .and_then(|items| {
            let tokens: Option<Vec<&str>> = items.iter().map(serde_json::Value::as_str).collect();
            McpToolRoles::from_tokens(tokens?).ok()
        })
        .unwrap_or(McpToolRoles::new(true, true))
}

/// Every `mcp:` tool id or glob the workflow can reach: its `permits.tools`
/// grants (a child workflow runs inside them), each `invoke:` target and
/// each `agent:` whitelist entry. Negated entries admit nothing.
fn reachable_grants(wf: &RawWorkflow) -> BTreeSet<String> {
    let granted = wf
        .permits
        .iter()
        .filter_map(|p| p.value.tools.as_ref())
        .flatten()
        .map(String::as_str);
    let named = wf
        .tasks
        .iter()
        .flat_map(|task| named_tools(&task.value.action));
    granted
        .chain(named)
        .filter(|entry| entry.starts_with("mcp:"))
        .map(str::to_owned)
        .collect()
}

/// The tool names one action carries: an `invoke:` target, an `agent:`
/// whitelist. `exec:` and `infer:` name no tool.
fn named_tools(action: &RawAction) -> Vec<&str> {
    match action {
        RawAction::Invoke(invoke) => invoke.tool().iter().map(|t| t.value.as_str()).collect(),
        RawAction::Agent(agent) => agent.tools.iter().map(|t| t.value.as_str()).collect(),
        _ => Vec::new(),
    }
}

/// What the tools one task can call were declared to do: the union over
/// the `mcp:` ids and globs it names.
pub(crate) fn of_grants<'a>(
    roles: &McpRoleMap,
    grants: impl IntoIterator<Item = &'a str>,
) -> McpToolRoles {
    let mut out = McpToolRoles::default();
    for grant in grants {
        if !grant.starts_with("mcp:") {
            continue;
        }
        for (id, declared) in roles {
            if nika_cap::glob_matches(grant, id) {
                out = out.union(*declared);
            }
        }
    }
    out
}

/// The advisory beside the trifecta row: every concrete `mcp:` tool the
/// workflow calls that was approved (its pin exists) with no declared role.
/// The lane's verdict does not cover what such a tool reads or sends, so
/// the check says which tool and how to declare it. Advice, never a
/// refusal. A tool that is not approved yet is the `mcp-pins` hint's.
pub(crate) fn scan_undeclared(
    wf: &RawWorkflow,
    read: &mut dyn FnMut(&str) -> Result<String, String>,
) -> Vec<crate::hints::Hint> {
    let calls = mcp_call_sites(wf);
    if calls.is_empty() {
        return Vec::new();
    }
    let pins: serde_json::Value = read(MCP_PINS_PATH)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(serde_json::Value::Null);
    calls
        .into_iter()
        .filter_map(|(task, tool)| {
            let (server, name) = tool.strip_prefix("mcp:")?.split_once('/')?;
            let pin = pins["servers"][server]["tools"].get(name)?;
            if pin.get("roles").is_some() {
                return None;
            }
            Some(crate::hints::hint(
                "mcp-roles",
                &task,
                format!(
                    "`{tool}` is approved with no declared role, so the TRIFECTA row does not \
                     judge what it reads or where it sends (a tool's effects are server-side, \
                     outside `permits:`) · declare it once: `nika mcp approve {server} \
                     --reads-private <tool>` · `--sends-out <tool>` · or `--no-role <tool>` \
                     when it does neither"
                ),
            ))
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use nika_schema::parser::{ParseMode, parse};
    use nika_schema::source::FileId;

    use super::*;

    const MAIL: &str = r#"
nika: mail
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
"#;

    fn workflow(yaml: &str) -> RawWorkflow {
        parse(yaml, FileId::new(0), ParseMode::Strict).expect("the fixture parses")
    }

    /// A reader over one lockfile text (every other path is absent).
    fn lockfile(text: &str) -> impl FnMut(&str) -> Result<String, String> + '_ {
        move |path: &str| {
            if path == MCP_PINS_PATH {
                Ok(text.to_owned())
            } else {
                Err(format!("{path}: absent"))
            }
        }
    }

    fn pins(read_inbox: &str, send_email: &str) -> String {
        format!(
            r#"{{"mcp_pins_format":1,"servers":{{"mail":{{"tools":{{
                "read_inbox":{{"pin":"blake3:00"{read_inbox}}},
                "send_email":{{"pin":"blake3:01"{send_email}}},
                "unused":{{"pin":"blake3:02","roles":["egress"]}}}}}},
              "other":{{"tools":{{"x":{{"pin":"blake3:03","roles":["egress"]}}}}}}}}}}"#
        )
    }

    #[test]
    fn tokens_round_trip_and_an_unknown_token_is_an_error() {
        let both = McpToolRoles::from_tokens(["egress", "private-read"]).expect("known tokens");
        assert_eq!(both, McpToolRoles::new(true, true));
        assert_eq!(both.tokens(), vec!["private-read", "egress"]);
        assert_eq!(
            McpToolRoles::from_tokens(Vec::<&str>::new()),
            Ok(McpToolRoles::default()),
            "an empty list is the declared-inert tool"
        );
        assert_eq!(
            McpToolRoles::from_tokens(["egress", "admin"]),
            Err("admin".to_owned())
        );
        let json = serde_json::to_string(&both).expect("serializes");
        assert_eq!(json, r#"["private-read","egress"]"#);
        assert_eq!(
            serde_json::from_str::<McpToolRoles>(&json).expect("deserializes"),
            both
        );
        let refused = serde_json::from_str::<McpToolRoles>(r#"["admin"]"#)
            .expect_err("an unknown role is refused");
        assert!(
            refused
                .to_string()
                .contains("unknown MCP tool role `admin`"),
            "{refused}"
        );
    }

    #[test]
    fn declared_roles_are_read_for_the_tools_the_workflow_can_reach() {
        let text = pins(r#","roles":["private-read"]"#, r#","roles":["egress"]"#);
        let roles = read_declared(&workflow(MAIL), &mut lockfile(&text));
        assert_eq!(
            roles,
            McpRoleMap::from([
                (
                    "mcp:mail/read_inbox".to_owned(),
                    McpToolRoles::new(true, false)
                ),
                (
                    "mcp:mail/send_email".to_owned(),
                    McpToolRoles::new(false, true)
                ),
            ]),
            "a tool the workflow cannot reach (`unused`, server `other`) is left out"
        );
    }

    #[test]
    fn an_undeclared_tool_has_no_entry_and_an_inert_one_has_an_empty_entry() {
        let text = pins("", r#","roles":[]"#);
        let roles = read_declared(&workflow(MAIL), &mut lockfile(&text));
        assert_eq!(
            roles,
            McpRoleMap::from([("mcp:mail/send_email".to_owned(), McpToolRoles::default())])
        );
    }

    #[test]
    fn an_unreadable_declaration_declares_both_roles() {
        for unreadable in [
            r#","roles":["admin"]"#,
            r#","roles":"egress""#,
            r#","roles":[1]"#,
        ] {
            let text = pins(unreadable, "");
            let roles = read_declared(&workflow(MAIL), &mut lockfile(&text));
            assert_eq!(
                roles.get("mcp:mail/read_inbox"),
                Some(&McpToolRoles::new(true, true)),
                "`{unreadable}` must never read as no role"
            );
        }
    }

    #[test]
    fn a_missing_or_malformed_lockfile_declares_nothing() {
        let wf = workflow(MAIL);
        assert!(read_declared(&wf, &mut |path: &str| Err(format!("{path}: absent"))).is_empty());
        assert!(read_declared(&wf, &mut lockfile("not json")).is_empty());
    }

    #[test]
    fn a_glob_grant_carries_the_union_of_what_it_covers() {
        let roles = McpRoleMap::from([
            (
                "mcp:mail/read_inbox".to_owned(),
                McpToolRoles::new(true, false),
            ),
            (
                "mcp:mail/send_email".to_owned(),
                McpToolRoles::new(false, true),
            ),
            ("mcp:calc/add".to_owned(), McpToolRoles::default()),
        ]);
        assert_eq!(
            of_grants(&roles, ["mcp:mail/*"]),
            McpToolRoles::new(true, true)
        );
        assert_eq!(
            of_grants(&roles, ["mcp:mail/read_inbox", "nika:fetch"]),
            McpToolRoles::new(true, false)
        );
        assert_eq!(of_grants(&roles, ["mcp:calc/*"]), McpToolRoles::default());
        assert_eq!(of_grants(&roles, ["mcp:*"]), McpToolRoles::new(true, true));
    }

    #[test]
    fn the_hint_names_each_approved_tool_with_no_declared_role() {
        let text = pins("", r#","roles":["egress"]"#);
        let hints = scan_undeclared(&workflow(MAIL), &mut lockfile(&text));
        assert_eq!(hints.len(), 1, "{hints:?}");
        assert_eq!(hints[0].kind, "mcp-roles");
        assert_eq!(hints[0].task, "inbox");
        assert!(
            hints[0].advice.starts_with(
                "`mcp:mail/read_inbox` is approved with no declared role, so the TRIFECTA row \
                 does not judge what it reads or where it sends"
            ),
            "{}",
            hints[0].advice
        );
        assert!(
            hints[0]
                .advice
                .contains("`nika mcp approve mail --reads-private <tool>`"),
            "{}",
            hints[0].advice
        );
        // A tool with no pin at all is the `mcp-pins` hint's, not this one's.
        let unpinned = r#"{"mcp_pins_format":1,"servers":{}}"#;
        assert!(scan_undeclared(&workflow(MAIL), &mut lockfile(unpinned)).is_empty());
    }
}
