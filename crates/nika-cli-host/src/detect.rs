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
//! Every wire target is a row, and a row's first path is the one its
//! wire target writes ([`crate::wire::config_path`]): this module once
//! listed six clients' paths beside a `wire` that wrote twenty-one, and
//! a machine with only the other fifteen answered « no MCP client
//! detected ». The one path named here is the project `.cursor/mcp.json`
//! Cursor also reads: `doctor` reports that sighting, `nika wire
//! detected` does not act on it (it wires a host only when the file its
//! writer patches is there).

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
    /// Read beside the client's own writer ([`crate::wire::reads`]):
    /// TOML, a dotted key, a shape of its own. No description of the
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
    /// Every path the config may live at: the one `wire` writes, then
    /// any other the host also reads (`doctor` reports those, `nika wire
    /// detected` counts only the first).
    pub(crate) paths: Vec<PathBuf>,
    /// A standing row is reported whether or not its host is on the
    /// machine (the six historical rows — findings and receipts ride
    /// them). Any other row is reported once the machine shows it.
    pub(crate) standing: bool,
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

/// The most a host's settings file may weigh. Real ones are a few
/// kilobytes, the largest a few hundred: past this, the path is not a
/// settings file.
const MAX_CONFIG_BYTES: usize = 4 * 1024 * 1024;

/// The ONE reader of a host's settings file: `wire` and `probe` read
/// through nothing else. A settings path is not ours to trust — `doctor`
/// reads `./opencode.json` from whatever repository it is run in — and
/// it may be a FIFO (opening one blocks forever) or a link to
/// `/dev/zero` (reading one ends when memory does). So a link is
/// followed, what it leads to must be a regular file, checked BEFORE it
/// is opened, and at most 4 MiB are read, through a bounded reader and
/// not on the size the metadata reports. Anything else is an error that
/// says why.
pub(crate) fn read_config(path: &Path) -> Result<String, String> {
    use std::io::Read as _;
    let refuse = |why: &dyn std::fmt::Display| format!("{}: {why}", path.display());
    let meta = std::fs::metadata(path).map_err(|e| refuse(&e))?;
    if !meta.is_file() {
        return Err(refuse(&"not a regular file"));
    }
    let file = std::fs::File::open(path).map_err(|e| refuse(&e))?;
    let limit = u64::try_from(MAX_CONFIG_BYTES).unwrap_or(u64::MAX);
    let mut bytes = Vec::new();
    file.take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|e| refuse(&e))?;
    if bytes.len() > MAX_CONFIG_BYTES {
        return Err(refuse(&"larger than 4 MiB"));
    }
    String::from_utf8(bytes).map_err(|_| refuse(&"not UTF-8 text"))
}

/// What a settings path can be besides a settings file — the fixtures
/// the `wire` and `probe` tests share with this module's own.
#[cfg(test)]
pub(crate) mod hazards {
    use std::path::Path;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum Hazard {
        Directory,
        Empty,
        Unreadable,
        NotText,
        DevZeroLink,
        Fifo,
        Oversized,
    }

    impl Hazard {
        /// Put the hazard at `path` and return the reason a JSON host's
        /// writer must give for refusing it, or `None` when this machine
        /// cannot stage it (root reads through a `000` mode).
        pub(crate) fn plant(self, path: &Path) -> Option<&'static str> {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
            match self {
                Self::Directory => std::fs::create_dir(path).expect("directory"),
                Self::Empty => std::fs::write(path, "").expect("empty"),
                Self::Unreadable => {
                    std::fs::write(path, "{}").expect("seed");
                    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o000))
                        .expect("chmod");
                    if std::fs::File::open(path).is_ok() {
                        return None;
                    }
                }
                Self::NotText => std::fs::write(path, [0xff, 0xfe]).expect("bytes"),
                Self::DevZeroLink => std::os::unix::fs::symlink("/dev/zero", path).expect("link"),
                Self::Fifo => {
                    nix::unistd::mkfifo(path, nix::sys::stat::Mode::S_IRWXU).expect("fifo");
                }
                // Sparse: 4 MiB and one byte on the meter, nothing on disk.
                Self::Oversized => std::fs::File::create(path)
                    .and_then(|file| file.set_len(4 * 1024 * 1024 + 1))
                    .expect("oversized"),
            }
            Some(match self {
                Self::Directory | Self::DevZeroLink | Self::Fifo => "not a regular file",
                Self::Empty => "malformed JSON",
                Self::Unreadable => "Permission denied",
                Self::NotText => "not UTF-8 text",
                Self::Oversized => "larger than 4 MiB",
            })
        }
    }

    /// Run `work` on a thread and fail the test when it has not returned
    /// in twenty seconds: a read that blocks (a FIFO) or never ends
    /// (`/dev/zero`) must fail a test, not hang the suite.
    #[allow(clippy::disallowed_methods)] // a watchdog beside a read that may block, tests only
    pub(crate) fn promptly<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
        let (done, wait) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = done.send(work());
        });
        wait.recv_timeout(std::time::Duration::from_secs(20))
            .expect("the read returned promptly")
    }
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

    /// The one reader, value by value: a link is followed, a regular
    /// file of at most 4 MiB is read, and everything else comes back as
    /// an error with its reason — promptly, FIFO and `/dev/zero` included.
    #[test]
    fn the_reader_takes_only_a_bounded_regular_file() {
        use super::hazards::{Hazard, promptly};
        let dir = tempfile::tempdir().expect("temp dir");
        let read = |path: &Path| {
            let path = path.to_path_buf();
            promptly(move || read_config(&path))
        };

        let plain = dir.path().join("plain.json");
        std::fs::write(&plain, "{\"a\":1}\n").expect("seed");
        assert_eq!(read(&plain), Ok("{\"a\":1}\n".to_owned()));
        let link = dir.path().join("link.json");
        std::os::unix::fs::symlink(&plain, &link).expect("link");
        assert_eq!(
            read(&link),
            Ok("{\"a\":1}\n".to_owned()),
            "a link is followed"
        );
        let full = dir.path().join("full.json");
        std::fs::write(&full, " ".repeat(MAX_CONFIG_BYTES)).expect("seed");
        assert_eq!(read(&full).map(|text| text.len()), Ok(MAX_CONFIG_BYTES));
        for hazard in [
            Hazard::Directory,
            Hazard::Empty,
            Hazard::Unreadable,
            Hazard::NotText,
            Hazard::DevZeroLink,
            Hazard::Fifo,
            Hazard::Oversized,
        ] {
            let path = dir.path().join(format!("{hazard:?}")).join("settings.json");
            let Some(reason) = hazard.plant(&path) else {
                continue; // root reads through a 000 mode: nothing to see
            };
            let got = read(&path);
            if hazard == Hazard::Empty {
                assert_eq!(got, Ok(String::new()), "an empty file is a file");
                continue;
            }
            let why = got.expect_err("refused");
            assert!(
                why.starts_with(&format!("{}: ", path.display())) && why.contains(reason),
                "{hazard:?}: {why}"
            );
        }
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
