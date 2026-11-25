// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! The MCP shelf on the wire: what `nika catalog --mcp --json` and the MCP
//! `nika_catalog` tool (with `mcp: true`) emit.
//!
//! The shelf (`data/mcp-servers.toml`) is a vendored census of third-party
//! MCP servers: each entry names how a server is reached (a local package,
//! a hosted remote, or both), the environment variables it reads (by NAME,
//! never a value) and the date its distribution last answered the refresh
//! probe. This module is its single machine surface, so an author or an
//! agent reads the same list the binary embeds.
//!
//! # Wire contract
//!
//! A separate envelope from [`crate::export`], whose two top-level keys are
//! a locked contract: `{ "catalog_version": 1, "mcp_servers": [...] }`.
//! Evolution is additive-only within a version. Every enum leaves as the
//! kebab-case word the TOML uses (`streamable-http`, `oauth`, `freemium`,
//! `tool-hub`), never a Rust variant name.

use serde::Serialize;

use crate::types::{
    AuthMode, EnvVarSpec, McpPackage, McpPricing, McpRemote, McpServer, PyRunner, RegistryType,
    Transport,
};

/// Version marker of the `nika catalog --mcp --json` envelope. Additive-only.
pub const MCP_SHELF_EXPORT_VERSION: u32 = 1;

/// The MCP shelf projection (`catalog_version` + every embedded server).
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct McpShelfExport {
    /// Envelope version (see [`MCP_SHELF_EXPORT_VERSION`]).
    pub catalog_version: u32,
    /// Every embedded MCP server, in shelf order.
    pub mcp_servers: Vec<McpServerExport>,
}

/// One MCP server of the shelf.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct McpServerExport {
    /// Canonical id (`"firecrawl"`).
    pub id: &'static str,
    /// Other names resolving to this entry.
    pub aliases: &'static [&'static str],
    /// Human-readable title.
    pub title: &'static str,
    /// One-paragraph description.
    pub description: &'static str,
    /// Display category (`"search"`, `"developer"`, …).
    pub category: &'static str,
    /// Editorial pricing hint: `free`, `freemium` or `paid`.
    pub pricing: &'static str,
    /// Tags, kebab-case. Exactly one of `read-only` / `destructive`; a tool
    /// hub (one server fronting many providers' tools) carries `tool-hub`.
    pub tags: Vec<&'static str>,
    /// Local-install packages (empty for a remote-only server).
    pub packages: Vec<McpPackageExport>,
    /// Hosted endpoints (empty for a local-only server).
    pub remotes: Vec<McpRemoteExport>,
    /// Environment variables the server reads, by name only.
    pub env_vars: Vec<McpEnvVarExport>,
    /// Vendor homepage, when recorded.
    pub homepage: Option<&'static str>,
    /// ISO date on which every package and remote of the entry last
    /// answered the refresh probe (`scripts/refresh-mcp-servers.sh`).
    pub last_verified: &'static str,
    /// `current`, `deprecated` (its vendor or registry deprecates the listed
    /// distribution) or `restricted` (its vendor limits which clients may
    /// connect).
    pub status: &'static str,
    /// Why a non-current entry is not current, in plain words; absent when
    /// current.
    pub status_note: Option<&'static str>,
}

/// One local-install package of a server.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct McpPackageExport {
    /// `npm`, `pypi`, `oci`, `cargo` or `mcpb`.
    pub registry_type: &'static str,
    /// Package identifier on that registry.
    pub identifier: &'static str,
    /// `stdio`, `streamable-http` or `sse`.
    pub transport: &'static str,
    /// `uvx` or `pipx` for a `PyPI` package, otherwise absent.
    pub runner: Option<&'static str>,
}

/// One hosted endpoint of a server.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct McpRemoteExport {
    /// `streamable-http` or `sse`.
    pub transport: &'static str,
    /// Endpoint URL.
    pub url: &'static str,
    /// `none`, `bearer` or `oauth`.
    pub auth: &'static str,
}

/// One environment variable a server reads. The NAME only: no surface
/// here ever reads or emits a value.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct McpEnvVarExport {
    /// Variable name (`"FIRECRAWL_API_KEY"`).
    pub name: &'static str,
    /// Whether the server needs it.
    pub required: bool,
    /// Whether it holds a secret.
    pub is_secret: bool,
}

/// The shelf projection of every embedded MCP server, in shelf order.
#[must_use]
pub fn mcp_shelf_export() -> McpShelfExport {
    McpShelfExport {
        catalog_version: MCP_SHELF_EXPORT_VERSION,
        mcp_servers: crate::all_mcp_servers().iter().map(server_export).collect(),
    }
}

fn server_export(s: &McpServer) -> McpServerExport {
    McpServerExport {
        id: s.id,
        aliases: s.aliases,
        title: s.title,
        description: s.description,
        category: s.category.as_str(),
        pricing: pricing_word(s.pricing),
        tags: s.tags.iter().map(|t| t.as_str()).collect(),
        packages: s.packages.iter().map(package_export).collect(),
        remotes: s.remotes.iter().map(remote_export).collect(),
        env_vars: s.env_vars.iter().map(env_export).collect(),
        homepage: s.homepage,
        last_verified: s.last_verified,
        status: s.status.as_str(),
        status_note: s.status_note,
    }
}

fn package_export(p: &McpPackage) -> McpPackageExport {
    McpPackageExport {
        registry_type: registry_word(p.registry_type),
        identifier: p.identifier,
        transport: transport_word(p.transport),
        runner: p.runner.map(runner_word),
    }
}

fn remote_export(r: &McpRemote) -> McpRemoteExport {
    McpRemoteExport {
        transport: transport_word(r.transport),
        url: r.url,
        auth: auth_word(r.auth),
    }
}

const fn env_export(v: &EnvVarSpec) -> McpEnvVarExport {
    McpEnvVarExport {
        name: v.name,
        required: v.required,
        is_secret: v.is_secret,
    }
}

const fn pricing_word(p: McpPricing) -> &'static str {
    match p {
        McpPricing::Free => "free",
        McpPricing::Freemium => "freemium",
        McpPricing::Paid => "paid",
    }
}

const fn registry_word(r: RegistryType) -> &'static str {
    match r {
        RegistryType::Npm => "npm",
        RegistryType::Pypi => "pypi",
        RegistryType::Oci => "oci",
        RegistryType::Cargo => "cargo",
        RegistryType::Mcpb => "mcpb",
    }
}

const fn transport_word(t: Transport) -> &'static str {
    match t {
        Transport::Stdio => "stdio",
        Transport::StreamableHttp => "streamable-http",
        Transport::Sse => "sse",
    }
}

const fn auth_word(a: AuthMode) -> &'static str {
    match a {
        AuthMode::None => "none",
        AuthMode::Bearer => "bearer",
        AuthMode::OAuth => "oauth",
    }
}

const fn runner_word(r: PyRunner) -> &'static str {
    match r {
        PyRunner::Uvx => "uvx",
        PyRunner::Pipx => "pipx",
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::expect_used)]
mod tests {
    use super::*;

    fn wire() -> serde_json::Value {
        serde_json::to_value(mcp_shelf_export()).expect("the shelf serializes")
    }

    fn servers(wire: &serde_json::Value) -> &Vec<serde_json::Value> {
        wire["mcp_servers"].as_array().expect("mcp_servers array")
    }

    fn sorted_keys(v: &serde_json::Value) -> Vec<&str> {
        let mut keys: Vec<&str> = v
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        keys
    }

    #[test]
    fn the_envelope_is_two_keys_and_every_server() {
        let wire = wire();
        assert_eq!(sorted_keys(&wire), ["catalog_version", "mcp_servers"]);
        assert_eq!(wire["catalog_version"], MCP_SHELF_EXPORT_VERSION);
        assert_eq!(servers(&wire).len(), crate::all_mcp_servers().len());
    }

    #[test]
    fn every_server_carries_the_same_keys() {
        let wire = wire();
        for s in servers(&wire) {
            assert_eq!(
                sorted_keys(s),
                [
                    "aliases",
                    "category",
                    "description",
                    "env_vars",
                    "homepage",
                    "id",
                    "last_verified",
                    "packages",
                    "pricing",
                    "remotes",
                    "status",
                    "status_note",
                    "tags",
                    "title",
                ],
                "{}",
                s["id"]
            );
            for p in s["packages"].as_array().expect("packages") {
                assert_eq!(
                    sorted_keys(p),
                    ["identifier", "registry_type", "runner", "transport"]
                );
            }
            for r in s["remotes"].as_array().expect("remotes") {
                assert_eq!(sorted_keys(r), ["auth", "transport", "url"]);
            }
            for v in s["env_vars"].as_array().expect("env_vars") {
                assert_eq!(sorted_keys(v), ["is_secret", "name", "required"]);
                assert!(v["name"].as_str().is_some_and(|n| !n.is_empty()));
            }
        }
    }

    #[test]
    fn every_server_is_reachable_and_carries_one_effect_class() {
        for s in mcp_shelf_export().mcp_servers {
            assert!(
                !s.packages.is_empty() || !s.remotes.is_empty(),
                "`{}` has neither a package nor a remote",
                s.id
            );
            let classes = s
                .tags
                .iter()
                .filter(|t| matches!(**t, "read-only" | "destructive"))
                .count();
            assert_eq!(classes, 1, "`{}` tags {:?}", s.id, s.tags);
            assert!(
                matches!(s.pricing, "free" | "freemium" | "paid"),
                "`{}` pricing {}",
                s.id,
                s.pricing
            );
            let d = s.last_verified.as_bytes();
            assert!(
                d.len() == 10
                    && d[4] == b'-'
                    && d[7] == b'-'
                    && d.iter()
                        .enumerate()
                        .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit()),
                "`{}` date {}",
                s.id,
                s.last_verified
            );
        }
    }

    #[test]
    fn enums_leave_as_the_toml_words() {
        for s in mcp_shelf_export().mcp_servers {
            for p in &s.packages {
                assert!(
                    matches!(p.registry_type, "npm" | "pypi" | "oci" | "cargo" | "mcpb"),
                    "`{}` registry {}",
                    s.id,
                    p.registry_type
                );
                assert!(matches!(p.transport, "stdio" | "streamable-http" | "sse"));
                assert_eq!(
                    p.runner.is_some(),
                    p.registry_type == "pypi",
                    "`{}`: a runner exactly on a PyPI package",
                    s.id
                );
                assert!(p.runner.is_none_or(|r| matches!(r, "uvx" | "pipx")));
            }
            for r in &s.remotes {
                assert!(matches!(r.transport, "streamable-http" | "sse"));
                assert!(matches!(r.auth, "none" | "bearer" | "oauth"));
                assert!(r.url.starts_with("https://"), "`{}` {}", s.id, r.url);
            }
        }
    }

    #[test]
    fn a_note_says_why_exactly_when_an_entry_is_not_current() {
        let shelf = mcp_shelf_export();
        for s in &shelf.mcp_servers {
            assert!(
                matches!(s.status, "current" | "deprecated" | "restricted"),
                "`{}` status {}",
                s.id,
                s.status
            );
            assert_eq!(
                s.status_note.is_some_and(|n| !n.trim().is_empty()),
                s.status != "current",
                "`{}` status {} note {:?}",
                s.id,
                s.status,
                s.status_note
            );
        }
        let wire = wire();
        let current = servers(&wire)
            .iter()
            .find(|s| s["status"] == "current")
            .expect("a current entry");
        assert!(current["status_note"].is_null(), "absent, never empty");
    }

    #[test]
    fn printed_texts_carry_no_control_character() {
        for s in mcp_shelf_export().mcp_servers {
            let texts = [s.title, s.description, s.status_note.unwrap_or("")]
                .into_iter()
                .chain(s.packages.iter().map(|p| p.identifier))
                .chain(s.remotes.iter().map(|r| r.url));
            for text in texts {
                assert!(!text.chars().any(char::is_control), "`{}`: {text:?}", s.id);
            }
        }
    }

    #[test]
    fn the_tool_hubs_are_reached_by_a_hosted_endpoint_and_run_third_party_tools() {
        let shelf = mcp_shelf_export();
        let hubs: Vec<&McpServerExport> = shelf
            .mcp_servers
            .iter()
            .filter(|s| s.tags.contains(&"tool-hub"))
            .collect();
        assert!(!hubs.is_empty(), "the shelf marks its tool hubs");
        for s in hubs {
            assert!(!s.remotes.is_empty(), "`{}` has a hosted endpoint", s.id);
            assert!(
                s.tags.contains(&"destructive"),
                "`{}` runs third-party tools",
                s.id
            );
        }
    }
}
