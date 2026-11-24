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

    fn entry<'a>(wire: &'a serde_json::Value, id: &str) -> &'a serde_json::Value {
        wire["mcp_servers"]
            .as_array()
            .expect("mcp_servers array")
            .iter()
            .find(|s| s["id"] == id)
            .unwrap_or_else(|| panic!("`{id}` is on the shelf"))
    }

    #[test]
    fn the_envelope_is_two_keys_and_every_server() {
        let wire = wire();
        let mut keys: Vec<&str> = wire
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, ["catalog_version", "mcp_servers"]);
        assert_eq!(wire["catalog_version"], MCP_SHELF_EXPORT_VERSION);
        assert_eq!(
            wire["mcp_servers"].as_array().expect("array").len(),
            crate::all_mcp_servers().len()
        );
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
            assert_eq!(s.last_verified.len(), 10, "`{}` date", s.id);
        }
    }

    #[test]
    fn enums_leave_as_the_toml_words() {
        let wire = wire();
        let firecrawl = entry(&wire, "firecrawl");
        assert_eq!(firecrawl["category"], "search");
        assert_eq!(firecrawl["pricing"], "freemium");
        assert_eq!(firecrawl["packages"][0]["registry_type"], "npm");
        assert_eq!(firecrawl["packages"][0]["identifier"], "firecrawl-mcp");
        assert_eq!(firecrawl["packages"][0]["transport"], "stdio");
        assert!(firecrawl["packages"][0]["runner"].is_null());
        assert_eq!(firecrawl["remotes"][0]["transport"], "streamable-http");
        assert_eq!(
            firecrawl["remotes"][0]["url"],
            "https://mcp.firecrawl.dev/v2/mcp"
        );
        assert_eq!(firecrawl["remotes"][0]["auth"], "bearer");
        assert_eq!(firecrawl["remotes"][1]["auth"], "oauth");
        let qdrant = entry(&wire, "qdrant");
        assert_eq!(qdrant["packages"][0]["registry_type"], "pypi");
        assert_eq!(qdrant["packages"][0]["runner"], "uvx");
    }

    #[test]
    fn the_firecrawl_entry_names_alexandria_discovery() {
        let wire = wire();
        let description = entry(&wire, "firecrawl")["description"]
            .as_str()
            .expect("description");
        for word in ["search", "scrape", "map", "crawl", "Alexandria"] {
            assert!(
                description.contains(word),
                "missing `{word}`: {description}"
            );
        }
    }

    #[test]
    fn the_tool_hubs_are_marked_and_reached_by_their_hosted_endpoint() {
        let shelf = mcp_shelf_export();
        let mut hubs: Vec<&str> = shelf
            .mcp_servers
            .iter()
            .filter(|s| s.tags.contains(&"tool-hub"))
            .map(|s| s.id)
            .collect();
        hubs.sort_unstable();
        assert_eq!(hubs, ["composio", "pipedream", "treg", "zapier"]);
        for id in hubs {
            let s = shelf
                .mcp_servers
                .iter()
                .find(|s| s.id == id)
                .expect("listed above");
            assert!(s.remotes.iter().all(|r| r.url.starts_with("https://")));
            assert!(
                s.tags.contains(&"destructive"),
                "`{id}` runs third-party tools"
            );
        }
        let treg = shelf
            .mcp_servers
            .iter()
            .find(|s| s.id == "treg")
            .expect("treg");
        assert_eq!(treg.remotes.len(), 1);
        assert_eq!(
            (treg.remotes[0].url, treg.remotes[0].auth),
            ("https://treg.to/mcp/", "oauth")
        );
    }

    #[test]
    fn an_env_var_leaves_as_a_name_and_two_flags() {
        let wire = wire();
        let var = &entry(&wire, "firecrawl")["env_vars"][0];
        let mut keys: Vec<&str> = var
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, ["is_secret", "name", "required"]);
        assert_eq!(var["name"], "FIRECRAWL_API_KEY");
    }
}
