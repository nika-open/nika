// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! The project copies of the kit's hook scripts: the two stamps `init`
//! applies to them, and which scripts each hook-wiring file runs. The
//! scripts' bytes themselves are briefs (`crate::briefs::targets`).

use std::borrow::Cow;

/// The session hook's provenance slot, and the stamp a project copy carries.
const VERSION_SLOT: &str = "scaffold_version=\"\"";
const VERSION_STAMP: &str = concat!("scaffold_version=\"", env!("CARGO_PKG_VERSION"), "\"");

/// The edit hook's one line naming the resolver beside it. Inside the plugin
/// that resolver is the kit's own; beside a project copy the same relative
/// path is a folder the repository holds, so the copy carries the line
/// emptied and applies the binary order itself.
const RESOLVER_SLOT: &str = "resolver=\"../bin/nika-kit\"";
const RESOLVER_OFF: &str = "resolver=\"\"";

/// The bytes `init` writes for one scaffold target: the kit's bytes, with the
/// two project-copy stamps applied (the session hook's producer version, the
/// edit hook's resolver emptied). Everything else is written verbatim. This
/// is also what an existing hook script is compared against before `init`
/// wires a client to it.
pub(crate) fn scaffold_body(path: &str, body: &'static str) -> Cow<'static, str> {
    if path.ends_with("hooks-nika/session-context.sh") {
        Cow::Owned(body.replacen(VERSION_SLOT, VERSION_STAMP, 1))
    } else if path.ends_with("hooks-nika/check-on-edit.sh") {
        Cow::Owned(body.replacen(RESOLVER_SLOT, RESOLVER_OFF, 1))
    } else {
        Cow::Borrowed(body)
    }
}

/// Each project hook-wiring file and the scaffolded scripts its commands run.
/// `init` writes a wiring file only when every script it names holds exactly
/// the bytes `init` writes for it: a hook never runs a file `init` did not
/// write. A test walks each wiring body's commands back to this table.
pub(crate) const HOOK_WIRING: [(&str, [&str; 3]); 2] = [
    (
        ".claude/settings.json",
        [
            ".claude/hooks-nika/session-context.sh",
            ".claude/hooks-nika/check-on-edit.sh",
            ".claude/hooks-nika/guard-run.sh",
        ],
    ),
    (
        ".cursor/hooks.json",
        [
            ".cursor/hooks-nika/session-context.sh",
            ".cursor/hooks-nika/check-on-edit.sh",
            ".cursor/hooks-nika/guard-run.sh",
        ],
    ),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::briefs::targets;

    /// The kit's own bytes for one scaffold target.
    fn kit(rel: &str) -> &'static str {
        targets()
            .into_iter()
            .find(|(r, _)| *r == rel)
            .map(|(_, body)| body)
            .expect("a scaffold target")
    }

    /// Each project-copy stamp rewrites exactly one line of the kit's script,
    /// and the plugin's own copy keeps its resolver line: a renamed or
    /// duplicated slot would make the stamp miss and the copy ask the
    /// repository's folder for its judge.
    #[test]
    fn each_stamp_rewrites_exactly_its_one_line() {
        for (rel, kit, slot, stamp) in [
            (
                ".claude/hooks-nika/check-on-edit.sh",
                kit(".claude/hooks-nika/check-on-edit.sh"),
                RESOLVER_SLOT,
                RESOLVER_OFF,
            ),
            (
                ".cursor/hooks-nika/session-context.sh",
                kit(".cursor/hooks-nika/session-context.sh"),
                VERSION_SLOT,
                VERSION_STAMP,
            ),
        ] {
            assert_eq!(
                kit.lines().filter(|l| *l == slot).count(),
                1,
                "{rel}: the kit carries its slot once, as a whole line"
            );
            let copy = scaffold_body(rel, kit);
            let changed: Vec<(&str, &str)> = kit
                .lines()
                .zip(copy.lines())
                .filter(|(a, b)| a != b)
                .collect();
            assert_eq!(changed, [(slot, stamp)], "{rel}");
            assert_eq!(kit.lines().count(), copy.lines().count(), "{rel}");
        }
        assert_eq!(
            scaffold_body(
                ".cursor/hooks-nika/guard-run.sh",
                kit(".cursor/hooks-nika/guard-run.sh")
            ),
            kit(".cursor/hooks-nika/guard-run.sh"),
            "the run guard is copied verbatim"
        );
    }

    /// `HOOK_WIRING` names exactly the scripts each wiring body's commands
    /// run, and every target that wires hooks is in it: `init` checks a
    /// script before wiring it only if the table knows the wiring calls it.
    /// A target that names a `hooks-nika/` script outside prose counts as a
    /// wiring whatever its keys, and must be JSON the walk below can read.
    #[test]
    fn the_hook_wiring_table_matches_the_wiring_bodies() {
        fn commands(value: &serde_json::Value, out: &mut Vec<String>) {
            match value {
                serde_json::Value::Object(map) => {
                    for (key, v) in map {
                        match (key.as_str(), v.as_str()) {
                            ("command", Some(cmd)) => out.push(cmd.to_owned()),
                            _ => commands(v, out),
                        }
                    }
                }
                serde_json::Value::Array(items) => {
                    for v in items {
                        commands(v, out);
                    }
                }
                _ => {}
            }
        }
        let mut wiring_targets = Vec::new();
        for (rel, body) in targets() {
            let prose = [".md", ".mdc"].iter().any(|e| rel.ends_with(e));
            let names_a_script =
                !prose && !rel.contains("hooks-nika/") && body.contains("hooks-nika/");
            let parsed = serde_json::from_str::<serde_json::Value>(body);
            assert!(
                parsed.is_ok() || !names_a_script,
                "{rel} names a hooks-nika/ script but is not JSON"
            );
            let Ok(value) = parsed else {
                continue;
            };
            if value.get("hooks").is_none() && !names_a_script {
                continue;
            }
            wiring_targets.push(rel);
            let mut run = Vec::new();
            commands(&value, &mut run);
            let mut run: Vec<String> = run
                .iter()
                .map(|cmd| {
                    cmd.strip_prefix("/bin/bash -p ")
                        .unwrap_or_else(|| panic!("{rel}: {cmd} is not started by /bin/bash -p"))
                        .trim_start_matches("\"$CLAUDE_PROJECT_DIR\"/")
                        .trim_start_matches("./")
                        .to_owned()
                })
                .collect();
            run.sort();
            let (_, table) = HOOK_WIRING
                .iter()
                .find(|(w, _)| *w == rel)
                .expect("every hook wiring is in HOOK_WIRING");
            let mut table: Vec<String> = table.iter().map(|s| (*s).to_owned()).collect();
            table.sort();
            assert_eq!(run, table, "{rel}");
        }
        assert_eq!(
            wiring_targets,
            HOOK_WIRING.iter().map(|(w, _)| *w).collect::<Vec<_>>()
        );
    }
}
