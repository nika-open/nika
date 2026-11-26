// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! What `init` writes, and where: links in a project never carry a write
//! outside it, and a kept hook wiring beside scripts init did not write is
//! named in its row.
//!
//! Each link layout puts the project three folders below a fake home whose
//! `.claude/` stands for the user's global settings, then founds the
//! project with and without `--force` and checks that every file outside
//! the project is exactly as it was.

use std::os::unix::fs::{PermissionsExt as _, symlink};
use std::path::{Path, PathBuf};

use super::*;
use crate::confine::Refusal;

/// `<top>/home/src/project` and the fake global `<top>/home/.claude`.
struct Layout {
    top: PathBuf,
    project: PathBuf,
    global: PathBuf,
}

impl Layout {
    fn new(tag: &str) -> Self {
        let top =
            std::env::temp_dir().join(format!("nika-init-links-{tag}-{}", std::process::id()));
        std::fs::remove_dir_all(&top).ok();
        std::fs::create_dir_all(top.join("home/src/project")).expect("project");
        std::fs::create_dir_all(top.join("home/.claude")).expect("global settings folder");
        std::fs::create_dir_all(top.join("outside")).expect("outside");
        let top = top.canonicalize().expect("canonical");
        Self {
            project: top.join("home/src/project"),
            global: top.join("home/.claude"),
            top,
        }
    }

    fn dir(&self) -> String {
        self.project.to_string_lossy().into_owned()
    }

    fn found(&self, force: bool) -> Outcome {
        scripted_run(
            &self.dir(),
            force,
            None,
            None,
            None,
            &[],
            &|path: &str| Outcome::ok(format!("  ✔ audited (stub) ← {path}")),
            &|_: &str, _: &str| Outcome::ok(String::new()),
        )
    }

    /// Every file under `<top>/home` and `<top>/outside` outside the
    /// project, with its bytes (links listed by their target).
    fn outside(&self) -> Vec<(PathBuf, String)> {
        let mut seen = Vec::new();
        let mut todo = vec![self.top.join("home"), self.top.join("outside")];
        while let Some(dir) = todo.pop() {
            for entry in std::fs::read_dir(&dir)
                .expect("list")
                .filter_map(Result::ok)
            {
                let path = entry.path();
                if path.starts_with(&self.project) {
                    continue;
                }
                let kind = std::fs::symlink_metadata(&path).expect("stat");
                if kind.file_type().is_symlink() {
                    let to = std::fs::read_link(&path).expect("link");
                    seen.push((path, format!("-> {}", to.display())));
                } else if kind.is_dir() {
                    todo.push(path);
                } else {
                    let bytes = std::fs::read(&path).expect("read");
                    seen.push((path, String::from_utf8_lossy(&bytes).into_owned()));
                }
            }
        }
        seen.sort();
        seen
    }

    fn row(&self, rel: &str, why: Refusal) -> String {
        format!("✖ {}: {why}", join(&self.dir(), rel))
    }
}

impl Drop for Layout {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.top).ok();
    }
}

fn has_line(text: &str, line: &str) -> bool {
    text.lines().any(|l| l == line)
}

/// No refusal row sends the reader to `--force`: it would not help.
fn no_force_advice(text: &str) {
    for line in text.lines().filter(|l| l.starts_with('✖')) {
        assert!(
            !line.contains("rerun with --force") && !line.contains("--force to overwrite"),
            "{line}"
        );
    }
}

/// (a) A dangling `.claude/settings.json` aimed at the global settings
/// file: init would have created the global file through it. Without
/// `--force` nothing is written there, so the link is kept (exit 0); under
/// `--force` the write it would need is refused.
#[test]
fn a_dangling_settings_link_never_creates_the_global_file() {
    let room = Layout::new("dangling");
    let link = room.project.join(".claude/settings.json");
    std::fs::create_dir_all(link.parent().expect("folder")).expect("mkdir");
    symlink("../../../.claude/settings.json", &link).expect("link");
    let before = room.outside();
    for force in [false, true] {
        let out = room.found(force);
        no_force_advice(&out.text);
        if force {
            assert_eq!(out.code, codes::ENV, "force={force}\n{}", out.text);
            assert!(
                has_line(&out.text, &room.row(".claude/settings.json", Refusal::Link)),
                "force={force}\n{}",
                out.text
            );
        } else {
            assert_eq!(out.code, codes::OK, "force={force}\n{}", out.text);
            let kept = format!(
                "· skipped {} ({})",
                join(&room.dir(), ".claude/settings.json"),
                Refusal::Link.kept()
            );
            assert!(has_line(&out.text, &kept), "{}", out.text);
        }
        assert!(!room.global.join("settings.json").exists(), "force={force}");
        assert_eq!(
            room.outside(),
            before,
            "force={force}: nothing outside changed"
        );
        assert!(std::fs::symlink_metadata(&link).is_ok_and(|m| m.file_type().is_symlink()));
    }
}

/// (b) The same link with the global file present, plus a decoy hook
/// script: `--force` used to overwrite the global settings.
#[test]
fn force_never_overwrites_the_global_settings_through_a_link() {
    let room = Layout::new("global");
    std::fs::write(room.global.join("settings.json"), "{\"the\": \"user's\"}\n").expect("global");
    let link = room.project.join(".claude/settings.json");
    let decoy = room.project.join(".claude/hooks-nika/check-on-edit.sh");
    std::fs::create_dir_all(decoy.parent().expect("folder")).expect("mkdir");
    std::fs::write(&decoy, "#!/bin/sh\necho decoy\n").expect("decoy");
    symlink("../../../.claude/settings.json", &link).expect("link");
    let before = room.outside();
    for force in [false, true] {
        let out = room.found(force);
        no_force_advice(&out.text);
        if force {
            assert_eq!(out.code, codes::ENV, "force={force}\n{}", out.text);
            assert!(
                has_line(&out.text, &room.row(".claude/settings.json", Refusal::Link)),
                "force={force}\n{}",
                out.text
            );
        } else {
            // Kept, never followed, and named beside the decoy its hooks
            // may run: information only.
            assert_eq!(out.code, codes::OK, "force={force}\n{}", out.text);
            let dir = room.dir();
            let row = format!(
                "· {}",
                kept_wiring_line(
                    &join(&dir, ".claude/settings.json"),
                    &[join(&dir, ".claude/hooks-nika/check-on-edit.sh")],
                    Kept::Aside(Refusal::Link.kept())
                )
            );
            assert!(has_line(&out.text, &row), "{}", out.text);
        }
        assert_eq!(
            room.outside(),
            before,
            "force={force}: the global settings are untouched"
        );
    }
}

/// (c) `.claude` itself leads to the global folder: nothing is made or
/// rewritten in it, whatever file init wanted under it.
#[test]
fn a_linked_claude_folder_is_never_written_into() {
    let room = Layout::new("folder");
    std::fs::write(room.global.join("settings.json"), "{\"the\": \"user's\"}\n").expect("global");
    symlink("../../.claude", room.project.join(".claude")).expect("folder link");
    let before = room.outside();
    for force in [false, true] {
        let out = room.found(force);
        assert_eq!(out.code, codes::ENV, "force={force}\n{}", out.text);
        no_force_advice(&out.text);
        // A script init must write there is refused; the settings file it
        // would only skip without `--force` is kept, never followed.
        let refused: &[&str] = if force {
            &[
                ".claude/settings.json",
                ".claude/hooks-nika/check-on-edit.sh",
            ]
        } else {
            &[".claude/hooks-nika/check-on-edit.sh"]
        };
        for rel in refused {
            assert!(
                has_line(&out.text, &room.row(rel, Refusal::Outside)),
                "force={force} {rel}\n{}",
                out.text
            );
        }
        if !force {
            let kept = format!(
                "· skipped {} ({}) · its hooks may run ",
                join(&room.dir(), ".claude/settings.json"),
                Refusal::Outside.kept()
            );
            assert!(
                out.text.lines().any(|l| l.starts_with(&kept)),
                "{}",
                out.text
            );
        }
        assert_eq!(
            room.outside(),
            before,
            "force={force}: the global folder is untouched"
        );
        assert!(
            room.project.join(".cursor/hooks.json").is_file(),
            "the other client is wired"
        );
    }
}

/// (d) `--force` over a hook script that is a link to a file outside: the
/// file stays as it was, and the wiring that would run it is refused.
#[test]
fn force_never_writes_a_linked_hook_script() {
    let room = Layout::new("script");
    let victim = room.top.join("outside/victim.sh");
    std::fs::write(&victim, "#!/bin/sh\necho theirs\n").expect("victim");
    let script = room.project.join(".claude/hooks-nika/check-on-edit.sh");
    std::fs::create_dir_all(script.parent().expect("folder")).expect("mkdir");
    symlink(&victim, &script).expect("link");
    let before = room.outside();
    for force in [true, false] {
        let out = room.found(force);
        assert_eq!(out.code, codes::ENV, "force={force}\n{}", out.text);
        no_force_advice(&out.text);
        let dir = room.dir();
        // Under `--force` the script's write is refused; without it the link
        // is kept, never followed. Either way the wiring is refused.
        let script_row = if force {
            room.row(".claude/hooks-nika/check-on-edit.sh", Refusal::Link)
        } else {
            format!(
                "· skipped {} ({})",
                join(&dir, ".claude/hooks-nika/check-on-edit.sh"),
                Refusal::Link.kept()
            )
        };
        assert!(
            has_line(&out.text, &script_row),
            "force={force}\n{}",
            out.text
        );
        let wiring = format!(
            "✖ {}",
            refused_line(
                &join(&dir, ".claude/settings.json"),
                &[join(&dir, ".claude/hooks-nika/check-on-edit.sh")],
                if force {
                    Blocker::Unwritten
                } else {
                    Blocker::Link
                }
            )
        );
        assert!(has_line(&out.text, &wiring), "force={force}\n{}", out.text);
        assert!(!room.project.join(".claude/settings.json").exists());
        assert_eq!(
            room.outside(),
            before,
            "force={force}: the linked file is untouched"
        );
    }
}

/// A link that stays inside the project (`CLAUDE.md` pointing at
/// `AGENTS.md`, a `nika.yaml` pointing at a file of the project) is kept as
/// it is, with a row that does not send the reader to `--force`, and the
/// founding goes on (exit 0); under `--force` too, never written through.
#[test]
fn a_link_inside_the_project_is_kept_and_never_written_through() {
    let room = Layout::new("inside");
    let shared = room.project.join("config/nika.yaml");
    std::fs::create_dir_all(shared.parent().expect("folder")).expect("mkdir");
    std::fs::write(&shared, "nika: the team's\n").expect("shared project file");
    std::fs::write(room.project.join("AGENTS.md"), "the human's guide\n").expect("agents");
    symlink("AGENTS.md", room.project.join("CLAUDE.md")).expect("link");
    symlink("config/nika.yaml", room.project.join("nika.yaml")).expect("project file link");
    for force in [false, true] {
        let out = room.found(force);
        assert_eq!(out.code, codes::OK, "force={force}\n{}", out.text);
        for rel in ["CLAUDE.md", "nika.yaml"] {
            let kept = format!("· skipped {} ({KEPT_LINK})", join(&room.dir(), rel));
            assert!(
                has_line(&out.text, &kept),
                "force={force} {rel}\n{}",
                out.text
            );
            assert!(
                std::fs::symlink_metadata(room.project.join(rel))
                    .is_ok_and(|m| m.file_type().is_symlink()),
                "force={force}: {rel} is still a link"
            );
        }
        assert_eq!(
            std::fs::read_to_string(&shared).expect("shared"),
            "nika: the team's\n",
            "force={force}: never written through"
        );
    }
    assert_eq!(
        std::fs::read_link(room.project.join("CLAUDE.md")).expect("link"),
        Path::new("AGENTS.md")
    );
}

/// The trace cover and the project file follow the same law: a link
/// leading out is never followed or read. Where init would only skip (the
/// project file without `--force`, the cover always, being adds-only) it is
/// kept with a row and the founding goes on; the project file's write under
/// `--force` is refused. The cover's row says to replace the link, which
/// git does not read as a `.gitignore`.
#[test]
fn the_trace_cover_and_the_project_file_never_follow_a_link_out() {
    let room = Layout::new("files");
    std::fs::write(room.top.join("outside/nika.yaml"), "nika: theirs\n").expect("theirs");
    std::fs::write(room.top.join("outside/gitignore"), "target/\n").expect("their rules");
    symlink(
        "../../../outside/gitignore",
        room.project.join(".gitignore"),
    )
    .expect("cover");
    symlink(
        room.top.join("outside/nika.yaml"),
        room.project.join("nika.yaml"),
    )
    .expect("file");
    let before = room.outside();
    let dir = room.dir();
    let cover = format!(
        "· skipped {} (a link · init never writes through a link, and git does not read a .gitignore that is a link) · replace it with a regular file holding .nika/traces/, .nika/serve.token and .nika/serve/",
        join(&dir, ".gitignore")
    );
    for force in [false, true] {
        let out = room.found(force);
        no_force_advice(&out.text);
        assert!(has_line(&out.text, &cover), "force={force}\n{}", out.text);
        if force {
            assert_eq!(out.code, codes::ENV, "force={force}\n{}", out.text);
            assert!(
                has_line(&out.text, &room.row("nika.yaml", Refusal::Link)),
                "{}",
                out.text
            );
        } else {
            assert_eq!(out.code, codes::OK, "force={force}\n{}", out.text);
            let kept = format!(
                "· skipped {} ({})",
                join(&dir, "nika.yaml"),
                Refusal::Link.kept()
            );
            assert!(has_line(&out.text, &kept), "{}", out.text);
        }
        assert_eq!(room.outside(), before, "force={force}");
    }
}

/// A hook script holding the bytes init writes but not executable is not
/// init's copy (a host could not start it as init leaves it): an existing
/// wiring beside it is kept and names it, and a missing one is refused
/// with the `--force` remedy, which rewrites the script `0755`.
#[test]
fn a_hook_script_without_its_exec_bit_is_not_inits_copy() {
    let room = Layout::new("exec-bit");
    let dir = room.dir();
    assert_eq!(room.found(false).code, codes::OK);
    let script = room.project.join(".claude/hooks-nika/guard-run.sh");
    let settings = room.project.join(".claude/settings.json");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o644)).expect("chmod");
    let out = room.found(false);
    assert_eq!(out.code, codes::OK, "{}", out.text);
    let row = format!(
        "· {}",
        kept_wiring_line(
            &join(&dir, ".claude/settings.json"),
            &[join(&dir, ".claude/hooks-nika/guard-run.sh")],
            Kept::File
        )
    );
    assert!(has_line(&out.text, &row), "{}", out.text);
    std::fs::remove_file(&settings).expect("remove the wiring");
    let out = room.found(false);
    assert_eq!(out.code, codes::ENV, "{}", out.text);
    let refused = format!(
        "✖ {}",
        refused_line(
            &join(&dir, ".claude/settings.json"),
            &[join(&dir, ".claude/hooks-nika/guard-run.sh")],
            Blocker::Foreign
        )
    );
    assert!(has_line(&out.text, &refused), "{}", out.text);
    assert!(
        !settings.exists(),
        "no wiring beside a script a host cannot start"
    );
    let forced = room.found(true);
    assert_eq!(forced.code, codes::OK, "{}", forced.text);
    assert_eq!(
        std::fs::metadata(&script)
            .expect("stat")
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
    assert!(settings.is_file());
}

/// The founding workflow and its index follow it too: a `workflows`
/// folder leading out stops the founding with a refusal, nothing written.
#[test]
fn a_workflows_folder_leading_out_is_never_written_into() {
    let room = Layout::new("workflows");
    let away: &Path = &room.top.join("outside/workflows");
    std::fs::create_dir_all(away).expect("away");
    symlink(away, room.project.join("workflows")).expect("folder link");
    let out = room.found(false);
    assert_eq!(out.code, codes::ENV, "{}", out.text);
    let row = format!("✖ workflows/{DEFAULT_EXAMPLE}.nika: {}", Refusal::Outside);
    assert!(has_line(&out.text, &row), "{}", out.text);
    assert_eq!(
        std::fs::read_dir(away).expect("list").count(),
        0,
        "nothing written there"
    );
}

/// An existing hook wiring is the repository's and stays as it is; beside
/// a script init did not write, its row says that the file's hooks may run
/// it. Information only: the founding goes on.
#[test]
fn a_kept_wiring_beside_a_foreign_script_is_named() {
    let room = Layout::new("kept-wiring");
    let settings = room.project.join(".claude/settings.json");
    let script = room.project.join(".claude/hooks-nika/guard-run.sh");
    std::fs::create_dir_all(script.parent().expect("folder")).expect("mkdir");
    std::fs::write(&settings, "{\"hooks\": {}}\n").expect("their settings");
    std::fs::write(&script, "#!/bin/sh\necho theirs\n").expect("their script");
    let out = room.found(false);
    assert_eq!(out.code, codes::OK, "{}", out.text);
    let dir = room.dir();
    let row = format!(
        "· {}",
        kept_wiring_line(
            &join(&dir, ".claude/settings.json"),
            &[join(&dir, ".claude/hooks-nika/guard-run.sh")],
            Kept::File
        )
    );
    assert!(has_line(&out.text, &row), "{}", out.text);
    assert_eq!(
        row,
        format!(
            "· skipped {} (exists · --force to overwrite) · its hooks may run {}, which init did not write: review it before trusting it",
            join(&dir, ".claude/settings.json"),
            join(&dir, ".claude/hooks-nika/guard-run.sh")
        )
    );
    assert_eq!(
        std::fs::read_to_string(&settings).expect("kept"),
        "{\"hooks\": {}}\n"
    );
    assert_eq!(
        std::fs::read_to_string(&script).expect("kept"),
        "#!/bin/sh\necho theirs\n"
    );
}

// The hook wiring: written only beside scripts init wrote.

/// A fresh temp project for one hook-wiring case.
/// W2: a wiring kept because it is a link to a file inside the project
/// still names the scripts it may run, which init did not write.
#[test]
fn a_kept_linked_wiring_beside_a_foreign_script_is_named() {
    let room = Layout::new("kept-linked-wiring");
    let theirs = room.project.join("config/claude-settings.json");
    let settings = room.project.join(".claude/settings.json");
    let script = room.project.join(".claude/hooks-nika/guard-run.sh");
    std::fs::create_dir_all(theirs.parent().expect("folder")).expect("mkdir");
    std::fs::create_dir_all(script.parent().expect("folder")).expect("mkdir");
    std::fs::write(&theirs, "{\"hooks\": {}}\n").expect("their settings");
    symlink("../config/claude-settings.json", &settings).expect("link");
    std::fs::write(&script, "#!/bin/sh\necho theirs\n").expect("their script");
    let before = room.outside();
    let out = room.found(false);
    assert_eq!(out.code, codes::OK, "{}", out.text);
    let dir = room.dir();
    let row = format!(
        "· skipped {} (a link · init never writes through a link) · its hooks may run {}, which init did not write: review it before trusting it",
        join(&dir, ".claude/settings.json"),
        join(&dir, ".claude/hooks-nika/guard-run.sh")
    );
    assert!(has_line(&out.text, &row), "{}", out.text);
    assert!(
        !out.text.contains(&format!(
            "skipped {} (a link · init never writes through a link)\n",
            join(&dir, ".claude/settings.json")
        )),
        "the kept link's plain row is replaced by the warning\n{}",
        out.text
    );
    assert_eq!(
        std::fs::read_to_string(&theirs).expect("kept"),
        "{\"hooks\": {}}\n"
    );
    assert!(std::fs::symlink_metadata(&settings).is_ok_and(|m| m.file_type().is_symlink()));
    assert_eq!(room.outside(), before, "nothing outside changed");
}

/// W3: a `.gitignore` that is a link to a file inside the project is kept
/// without `--force`, exit 0, and its row says to replace it with a regular
/// file; once the file it reaches holds the entries, the row stays the
/// same: git does not read a `.gitignore` that is a link.
#[test]
fn a_gitignore_link_inside_the_project_is_kept_with_what_to_add() {
    let room = Layout::new("gitignore-link");
    let rules = room.project.join("config/ignore");
    let ignore = room.project.join(".gitignore");
    std::fs::create_dir_all(rules.parent().expect("folder")).expect("mkdir");
    std::fs::write(&rules, "target/\n").expect("their rules");
    symlink("config/ignore", &ignore).expect("link");
    let dir = room.dir();
    let out = room.found(false);
    assert_eq!(out.code, codes::OK, "{}", out.text);
    let row = format!(
        "· skipped {} (a link · init never writes through a link, and git does not read a .gitignore that is a link) · replace it with a regular file holding .nika/traces/, .nika/serve.token and .nika/serve/",
        join(&dir, ".gitignore")
    );
    assert!(has_line(&out.text, &row), "{}", out.text);
    assert_eq!(std::fs::read_to_string(&rules).expect("kept"), "target/\n");
    assert!(std::fs::symlink_metadata(&ignore).is_ok_and(|m| m.file_type().is_symlink()));
    std::fs::write(&rules, "target/\n.nika/\n").expect("covered by hand");
    let out = room.found(false);
    assert_eq!(out.code, codes::OK, "{}", out.text);
    assert!(has_line(&out.text, &row), "{}", out.text);
    assert!(!out.text.contains("already ignored"), "{}", out.text);
    assert_eq!(
        std::fs::read_to_string(&rules).expect("kept"),
        "target/\n.nika/\n"
    );
}

fn hook_room(tag: &str) -> (std::path::PathBuf, String) {
    let tmp = std::env::temp_dir().join(format!("nika-init-wiring-{tag}-{}", std::process::id()));
    std::fs::remove_dir_all(&tmp).ok();
    std::fs::create_dir_all(&tmp).expect("temp project");
    let dir = tmp.to_string_lossy().into_owned();
    (tmp, dir)
}

/// The bytes init writes for one scaffold target, stamps included.
fn written(rel: &str) -> String {
    let (_, body) = briefs::targets()
        .into_iter()
        .find(|(r, _)| *r == rel)
        .expect("a scaffold target");
    hook_copies::scaffold_body(rel, body).into_owned()
}

fn scripted(dir: &str, force: bool) -> Outcome {
    scripted_run(
        dir,
        force,
        None,
        None,
        None,
        &[],
        &|path: &str| Outcome::ok(format!("  ✔ audited (stub) ← {path}")),
        &|_: &str, _: &str| Outcome::ok(String::new()),
    )
}

/// Every hook script init writes is the kit's script with its stamps,
/// and none can run a resolver from the folder beside it: in a project
/// that folder is the repository's (`.claude/bin/`, `.cursor/bin/`).
#[test]
fn scaffolded_hook_scripts_never_consult_a_resolver_beside_them() {
    let (tmp, dir) = hook_room("no-resolver");
    let rows = apply_briefs(&dir, false, None);
    assert!(
        rows.iter().all(|(_, o)| matches!(o, BriefOutcome::Created)),
        "a fresh project gets every brief"
    );
    let scripts: Vec<&str> = briefs::targets()
        .iter()
        .map(|(rel, _)| *rel)
        .filter(|rel| Path::new(rel).extension().is_some_and(|e| e == "sh"))
        .collect();
    assert_eq!(scripts.len(), 6, "three seatbelts for each of two clients");
    for rel in scripts {
        let body = std::fs::read_to_string(tmp.join(rel)).expect("scaffolded script");
        assert_eq!(body, written(rel), "{rel}: the bytes init writes");
        assert!(
            !body.contains("nika-kit") && !body.contains("../bin/"),
            "{rel} names a resolver beside it"
        );
    }
    for rel in [
        ".claude/hooks-nika/check-on-edit.sh",
        ".cursor/hooks-nika/check-on-edit.sh",
    ] {
        let body = std::fs::read_to_string(tmp.join(rel)).expect("edit hook");
        assert_eq!(
            body.lines()
                .filter(|l| l.starts_with("resolver="))
                .collect::<Vec<_>>(),
            ["resolver=\"\""],
            "{rel} carries the one resolver line, emptied"
        );
    }
    std::fs::remove_dir_all(tmp).expect("remove temp project");
}

/// A hook script that exists with bytes init does not write (a
/// repository's own, the plugin's unstamped copy, or an older init's copy)
/// refuses the wiring file that would run it: no wiring, one report row
/// naming both files, exit 3, and every other brief still written.
#[test]
fn a_differing_hook_script_refuses_the_wiring_that_runs_it() {
    let plugin_copy = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../.agents/plugins/nika/scripts/check-on-edit.sh"
    ));
    for (wiring, scripts) in hook_copies::HOOK_WIRING {
        let other = hook_copies::HOOK_WIRING
            .iter()
            .find(|(w, _)| *w != wiring)
            .map(|(w, _)| *w)
            .expect("the other client");
        let older = written(scripts[0]).replace(
            concat!("scaffold_version=\"", env!("CARGO_PKG_VERSION"), "\""),
            "scaffold_version=\"0.99.0\"",
        );
        for (script, bytes) in [
            (scripts[1], "#!/bin/sh\necho planted >>marker\n".to_owned()),
            (scripts[1], plugin_copy.to_owned()),
            (scripts[0], older),
        ] {
            let (tmp, dir) = hook_room("refused");
            let planted = tmp.join(script);
            std::fs::create_dir_all(planted.parent().expect("hooks folder")).expect("mkdir");
            std::fs::write(&planted, &bytes).expect("planted script");

            let out = scripted(&dir, false);
            assert_eq!(out.code, codes::ENV, "{}", out.text);
            let line = format!(
                "✖ refused {} — its hooks would run {}, which init did not write · remove it and run nika init again, or rerun with --force, which rewrites every file init writes, AGENTS.md, CLAUDE.md, .mcp.json and settings included",
                join(&dir, wiring),
                join(&dir, script)
            );
            assert_eq!(
                out.text
                    .lines()
                    .filter(|l| l.starts_with('✖'))
                    .collect::<Vec<_>>(),
                [line.as_str()],
                "{}",
                out.text
            );
            assert!(
                out.text.contains(&format!(
                    "· skipped {} (exists · --force to overwrite)",
                    join(&dir, script)
                )),
                "{}",
                out.text
            );
            assert!(!tmp.join(wiring).exists(), "{wiring} must not be written");
            assert_eq!(
                std::fs::read_to_string(&planted).expect("planted"),
                bytes,
                "the existing script is left as it was"
            );
            assert!(tmp.join(other).is_file(), "the other client is still wired");
            for rel in scripts.iter().filter(|rel| **rel != script) {
                assert_eq!(
                    std::fs::read_to_string(tmp.join(rel)).expect("sibling script"),
                    written(rel)
                );
            }
            assert!(tmp.join("AGENTS.md").is_file() && tmp.join("nika.yaml").is_file());
            assert!(!tmp.join("marker").exists(), "init runs no hook");
            std::fs::remove_dir_all(&tmp).expect("remove temp project");
        }
    }
}

/// An existing script holding exactly what init writes, with the mode init
/// gives it, is init's own: the wiring is written and the script row is a
/// calm skip.
#[test]
fn an_identical_hook_script_is_wired() {
    for (wiring, scripts) in hook_copies::HOOK_WIRING {
        let (tmp, dir) = hook_room("identical");
        for rel in scripts {
            let path = tmp.join(rel);
            std::fs::create_dir_all(path.parent().expect("hooks folder")).expect("mkdir");
            std::fs::write(&path, written(rel)).expect("identical script");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .expect("the mode init gives it");
        }
        let out = scripted(&dir, false);
        assert_eq!(out.code, codes::OK, "{}", out.text);
        assert!(
            out.text.contains(&format!(
                "✔ created {} — connect session, edit and execution hooks",
                join(&dir, wiring)
            )),
            "{}",
            out.text
        );
        assert!(!out.text.contains("refused"), "{}", out.text);
        std::fs::remove_dir_all(&tmp).expect("remove temp project");
    }
}

/// `--force` keeps its meaning: it rewrites the scripts with init's bytes,
/// so the wiring that runs them is written too.
#[test]
fn force_rewrites_a_differing_hook_script_and_wires_it() {
    let (tmp, dir) = hook_room("force");
    let planted = tmp.join(".claude/hooks-nika/check-on-edit.sh");
    std::fs::create_dir_all(planted.parent().expect("hooks folder")).expect("mkdir");
    std::fs::write(&planted, "#!/bin/sh\necho planted\n").expect("planted script");
    let out = scripted(&dir, true);
    assert_eq!(out.code, codes::OK, "{}", out.text);
    assert_eq!(
        std::fs::read_to_string(&planted).expect("rewritten"),
        written(".claude/hooks-nika/check-on-edit.sh")
    );
    let settings: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(tmp.join(".claude/settings.json")).expect("wired"),
    )
    .expect("settings JSON");
    assert!(settings["hooks"]["PostToolUse"].is_array(), "{settings}");
    std::fs::remove_dir_all(&tmp).expect("remove temp project");
}

/// A link in place of a script is not a file init wrote, whatever it
/// points at: the wiring is refused.
#[cfg(unix)]
#[test]
fn a_linked_hook_script_refuses_its_wiring() {
    let (tmp, dir) = hook_room("linked");
    let rel = ".cursor/hooks-nika/guard-run.sh";
    std::fs::write(tmp.join("elsewhere.sh"), written(rel)).expect("identical bytes");
    std::fs::create_dir_all(tmp.join(".cursor/hooks-nika")).expect("mkdir");
    std::os::unix::fs::symlink("../../elsewhere.sh", tmp.join(rel)).expect("link");
    let out = scripted(&dir, false);
    assert_eq!(out.code, codes::ENV, "{}", out.text);
    assert!(
        out.text.contains(&format!(
            "✖ refused {} — its hooks would run {},",
            join(&dir, ".cursor/hooks.json"),
            join(&dir, rel)
        )),
        "{}",
        out.text
    );
    assert!(!tmp.join(".cursor/hooks.json").exists());
    assert!(tmp.join(".claude/settings.json").is_file());
    std::fs::remove_dir_all(&tmp).expect("remove temp project");
}
