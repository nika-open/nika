// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! The ONE client-detection table `probe` and `wire` share (ADR-110
//! split-residue cleanup · 2026-07-31). Before this module, `probe`'s
//! `client_probes()` and `wire`'s `detected_targets_in()` each carried
//! their own per-client path list — and only probe's was gated on the
//! vendored matrix, so a client dropped from the matrix stayed
//! wire-visible but doctor-invisible (two truths). Now both derive from
//! [`sights`]: one row per wire target, one registry gate, one stale
//! predicate, one Hermes recognition predicate.
//!
//! No path is written here. A row looks where its wire target writes
//! ([`crate::wire::config_path`]), and every wire target is a row: this
//! module once listed six clients' paths beside a `wire` that wrote
//! twenty-one, and a machine with only the other fifteen answered « no
//! MCP client detected ».

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::clients_registry;
use crate::wire::WireTarget;

/// How a row's config is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reading {
    /// JSON — the `nika` server entry lives at this key path
    /// (`["mcpServers", "nika"]` for the cursor-like family).
    Json([&'static str; 2]),
    /// YAML (Hermes) — no parser round-trips user YAML, the
    /// [`hermes_recognized`] substring predicate carries recognition.
    Yaml,
    /// The client's own writer reads it ([`crate::wire::reads`]): TOML,
    /// a dotted key, a shape of its own. No second description of the
    /// format lives here to drift from the one that writes it.
    Writer,
}

/// One client the detection engine knows how to SEE: its id, the config
/// paths this machine may show, and how the `nika` entry is read there.
#[derive(Debug, Clone)]
pub(crate) struct ClientSight {
    /// The registry / probe id (`cursor` · `claude` · `hermes` …).
    pub(crate) id: String,
    /// The wire target the row answers.
    pub(crate) target: WireTarget,
    /// How the config is read — the probe path differs per reading.
    pub(crate) reading: Reading,
    /// Every path the config may live at, most-wired first: the one
    /// `wire` writes, then any other the host also reads.
    pub(crate) paths: Vec<PathBuf>,
    /// A standing row is reported whether or not its host is on the
    /// machine (the six historical rows — findings and receipts ride
    /// them). Any other row is reported once the machine shows it.
    pub(crate) standing: bool,
}

impl ClientSight {
    /// The machine shows this client: one of its config files exists.
    /// Presence only, whatever the file holds.
    pub(crate) fn present(&self) -> bool {
        self.paths.iter().any(|path| path.exists())
    }
}

/// A standing row: how the historical probe reads the client, and the
/// workspace-relative paths its host reads beyond the one `wire` writes.
struct Standing {
    target: WireTarget,
    reading: Reading,
    also: &'static [&'static [&'static str]],
}

/// The standing rows, in the historical probe order (findings and
/// receipts ride it — the order is load-bearing, and
/// [`clients_registry::PROBE_MECHANISMS`] opens with it).
const STANDING: &[Standing] = &[
    // Cursor reads a project `.cursor/mcp.json` beside the home one.
    Standing {
        target: WireTarget::Cursor,
        reading: Reading::Json(["mcpServers", "nika"]),
        also: &[&[".cursor", "mcp.json"]],
    },
    Standing {
        target: WireTarget::Windsurf,
        reading: Reading::Json(["mcpServers", "nika"]),
        also: &[],
    },
    Standing {
        target: WireTarget::Claude,
        reading: Reading::Json(["mcpServers", "nika"]),
        also: &[],
    },
    // Zed: the MCP entry lives under `context_servers`
    // (zed.dev/docs/ai/mcp). The JSON probe is best-effort on Zed's JSONC
    // settings: a commented file parses as not-wired → the fix line says
    // `nika wire zed`, which itself degrades to the ✋ manual snippet.
    // Honest chain.
    Standing {
        target: WireTarget::Zed,
        reading: Reading::Json(["context_servers", "nika"]),
        also: &[],
    },
    // Hermes: YAML, the JSON probe cannot see it (H2) — recognition is
    // the `hermes_recognized` substring predicate, never a parse.
    Standing {
        target: WireTarget::Hermes,
        reading: Reading::Yaml,
        also: &[],
    },
    Standing {
        target: WireTarget::Vscode,
        reading: Reading::Json(["servers", "nika"]),
        also: &[],
    },
];

/// Every row, in probe order: the standing rows as the historical probe
/// reads them, then every other client `nika wire` can write, in its
/// declaration order. The second half is derived from the wire targets
/// themselves — a client added to `wire` is a row with no line to
/// remember. All twenty-one are seen today; one that must not be would
/// be filtered out here, with its reason written beside the filter.
fn rows() -> Vec<(WireTarget, Option<&'static Standing>)> {
    let standing = STANDING.iter().map(|row| (row.target, Some(row)));
    let appended = crate::wire::expand_target(WireTarget::All)
        .into_iter()
        .filter(|target| STANDING.iter().all(|row| row.target != *target))
        .map(|target| (target, None));
    standing.chain(appended).collect()
}

/// Resolve the sight rows for THIS machine: every row gated on the
/// vendored matrix (a client is SEEN only while the matrix claims its
/// wire target — the H6 law `probe` and `wire` now share). A row's first
/// path is the one its wire target writes (dropped when that needs a
/// `home` this machine does not have); the workspace-anchored extras
/// follow, resolved against `work`. A row with no resolvable path is no
/// sight at all.
pub(crate) fn sights(home: Option<&Path>, work: &Path) -> Vec<ClientSight> {
    let join = |base: &Path, segs: &[&str]| segs.iter().fold(base.to_path_buf(), |p, s| p.join(s));
    rows()
        .into_iter()
        .filter_map(|(target, standing)| {
            let id = target.id();
            if !clients_registry::registry_wires(&id) {
                return None;
            }
            let mut paths: Vec<PathBuf> = crate::wire::config_path(target, home, work)
                .into_iter()
                .collect();
            let also = standing.map_or(&[][..], |row| row.also);
            paths.extend(also.iter().map(|segs| join(work, segs)));
            (!paths.is_empty()).then_some(ClientSight {
                id,
                target,
                reading: standing.map_or(Reading::Writer, |row| row.reading),
                paths,
                standing: standing.is_some(),
            })
        })
        .collect()
}

/// The stale argv form (`mcp serve --stdio` — the pre-current wire) —
/// THE one stale predicate: `probe` flags the drift, `wire` migrates it.
pub(crate) fn is_stale_mcp_server(value: &Value) -> bool {
    let Some(args) = value.get("args").and_then(Value::as_array) else {
        return false;
    };
    args.len() == 3
        && args[0].as_str() == Some("mcp")
        && args[1].as_str() == Some("serve")
        && args[2].as_str() == Some("--stdio")
}

/// The Hermes recognition predicate — a `nika:` server whose command
/// line names the binary. Substring match BY DESIGN: user YAML carries
/// comments and anchors no parser round-trips; presence-only, never a
/// guess. `probe` reads it for detection, `wire` for the Current arm.
pub(crate) fn hermes_recognized(body: &str) -> bool {
    body.contains("nika:") && body.contains("command: nika")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rows mirror the registry's probe mechanisms exactly — one
    /// list, one order (findings and receipts ride probe order).
    #[test]
    fn the_table_mirrors_the_registry_probe_mechanisms() {
        let ids: Vec<String> = rows().iter().map(|(target, _)| target.id()).collect();
        assert_eq!(
            ids,
            clients_registry::PROBE_MECHANISMS,
            "the detect rows drifted from the registry's probe mechanisms"
        );
    }

    /// Every client `nika wire` can write is a row, once: a wire target
    /// added without one fails here, before a machine that only has that
    /// client answers « no MCP client detected ».
    #[test]
    fn every_wire_target_is_a_row() {
        let seen: Vec<WireTarget> = rows().iter().map(|(target, _)| *target).collect();
        for target in crate::wire::expand_target(WireTarget::All) {
            assert_eq!(
                seen.iter().filter(|row| **row == target).count(),
                1,
                "{} can be wired and has no detection row (or two)",
                target.id()
            );
        }
    }

    /// The six rows the probe has always reported keep their place and
    /// their order: receipts and findings ride it, the rest is appended.
    #[test]
    fn the_standing_rows_lead_in_the_historical_order() {
        let rows = sights(
            Some(Path::new("/nika-detect-test-home")),
            Path::new("/nika-detect-test-work"),
        );
        let standing: Vec<&str> = rows
            .iter()
            .take_while(|row| row.standing)
            .map(|row| row.id.as_str())
            .collect();
        assert_eq!(
            standing,
            ["cursor", "windsurf", "claude", "zed", "hermes", "vscode"]
        );
        assert!(
            rows.iter().skip(standing.len()).all(|row| !row.standing),
            "a standing row sits after an appended one"
        );
        assert_eq!(rows.len(), 21, "the row pin moved — deliberate?");
    }

    /// Every sight row answers a wire target the matrix claims today
    /// (the degrade-open law keeps the rows when the vendored snapshot
    /// cannot parse — never a silent coverage loss).
    #[test]
    fn every_row_is_matrix_claimed() {
        for (target, _) in rows() {
            assert!(
                clients_registry::registry_wires(&target.id()),
                "{} has no matrix wire row",
                target.id()
            );
        }
    }

    /// A row whose paths all need a HOME the machine does not have is no
    /// sight at all — the workspace-anchored rows survive.
    #[test]
    fn no_home_keeps_only_worktree_anchored_rows() {
        let work = Path::new("/tmp/nika-detect-test-work");
        let rows = sights(None, work);
        let ids: Vec<&str> = rows.iter().map(|row| row.id.as_str()).collect();
        assert_eq!(
            ids,
            ["cursor", "vscode", "opencode", "junie"],
            "got {ids:?}"
        );
        for row in &rows {
            assert!(
                row.paths.iter().all(|p| p.starts_with(work)),
                "{} anchored outside the workspace: {:?}",
                row.id,
                row.paths
            );
        }
    }

    /// The stale predicate pins the exact `mcp serve --stdio` argv and
    /// nothing else.
    #[test]
    fn stale_predicate_matches_only_the_stale_argv() {
        let stale = serde_json::json!({"command": "nika", "args": ["mcp", "serve", "--stdio"]});
        let current = serde_json::json!({"command": "nika", "args": ["mcp"]});
        assert!(is_stale_mcp_server(&stale));
        assert!(!is_stale_mcp_server(&current));
        assert!(!is_stale_mcp_server(&serde_json::json!({})));
        assert!(!is_stale_mcp_server(
            &serde_json::json!({"args": ["mcp", "serve"]})
        ));
    }

    /// The Hermes predicate needs both halves — a `nika:` server AND the
    /// binary's command line (presence-only, never a guess).
    #[test]
    fn hermes_predicate_needs_both_halves() {
        assert!(hermes_recognized(
            "mcp_servers:\n  nika:\n    command: nika\n    args: [mcp]\n"
        ));
        assert!(!hermes_recognized(
            "mcp_servers:\n  other:\n    command: nika\n"
        ));
        assert!(!hermes_recognized(
            "mcp_servers:\n  nika:\n    command: other\n"
        ));
    }
}
