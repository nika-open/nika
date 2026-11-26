// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! A project config is written only where the project owns it: never
//! through a spare file name the repository planted beside it, and through
//! a link (at its own name or at a folder on the way) only when the link
//! leads to something that is there, inside the project and outside its
//! `.git` folder; the config's own link only to a regular file, which is
//! then updated where it is while the link stays. Every layout runs in a
//! throwaway room holding the `project`, a stand-in `home`, and an
//! `outside` folder that plays the account's own files.

use std::os::unix::fs::{PermissionsExt as _, symlink};

use super::*;

/// What the account's outside file holds, byte for byte.
const VICTIM: &str = "export KEEP=1\n";

/// A project config of the repository's own, with one server of its own.
const SEEDED: &str = "{\"servers\":{\"other\":{\"command\":\"x\"}}}\n";

struct Room {
    _keep: tempfile::TempDir,
    project: PathBuf,
    home: PathBuf,
    outside: PathBuf,
}

impl Room {
    fn new() -> Self {
        let keep = tempfile::Builder::new()
            .prefix("nika-wire-links-")
            .tempdir()
            .expect("room");
        let top = keep.path().to_path_buf();
        let room = Self {
            project: top.join("project"),
            home: top.join("home"),
            outside: top.join("outside"),
            _keep: keep,
        };
        for folder in [&room.project, &room.home, &room.outside] {
            std::fs::create_dir_all(folder).expect("folder");
        }
        room
    }

    /// An outside file the account owns: its bytes, and a mode of its own.
    fn victim(&self) -> PathBuf {
        let victim = self.outside.join("rc");
        std::fs::write(&victim, VICTIM).expect("victim");
        std::fs::set_permissions(&victim, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        victim
    }

    fn path(&self, target: WireTarget) -> PathBuf {
        config_path(target, Some(&self.home), &self.project).expect("path")
    }

    fn wire(&self, target: WireTarget, dry_run: bool) -> VerbOutput {
        run_in(
            target,
            Some(&self.home),
            self.project.to_str().expect("utf8"),
            WireOptions {
                dry_run,
                ..WireOptions::default()
            },
        )
    }

    fn real(path: &Path) -> String {
        std::fs::canonicalize(path)
            .expect("real")
            .display()
            .to_string()
    }
}

/// The outside file holds what it held, with the mode it had.
fn untouched(victim: &Path) {
    assert_eq!(
        std::fs::read_to_string(victim).expect("read"),
        VICTIM,
        "the outside file is byte-identical"
    );
    let mode = std::fs::metadata(victim)
        .expect("stat")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o644, "the outside file keeps its mode");
}

/// The entries in `folder` named like a temporary of nika's, any release.
fn spares(folder: &Path) -> Vec<String> {
    std::fs::read_dir(folder)
        .expect("list")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter(|name| name.starts_with('.') && name.contains("nika-tmp"))
        .collect()
}

/// The one refused row a run printed for `target`, by preview and by run.
fn refused_row(room: &Room, target: WireTarget, row: &str) {
    for dry_run in [true, false] {
        let out = room.wire(target, dry_run);
        assert_eq!(
            out.code,
            crate::output::exit::ENV,
            "{target:?} (dry-run {dry_run}): {out:?}"
        );
        assert!(
            out.text.lines().any(|line| line == row),
            "{target:?} (dry-run {dry_run}): wanted {row:?} in {out:?}"
        );
    }
}

/// 0.125.1 named its spare file `.nika-tmp-<pid>` and opened it with a
/// call that follows a link: a repository that planted that name as a
/// link to an account file had the config written there, the file set to
/// 0600, and the link renamed onto the config. Every such name stays the
/// repository's link, never opened: the outside file is byte-identical
/// with its mode, the config is a regular file with nika's entry and the
/// repository's own server, and no spare of nika's is left beside it.
#[test]
fn a_planted_spare_file_link_is_never_written_through() {
    let room = Room::new();
    let victim = room.victim();
    let path = room.path(WireTarget::Vscode);
    let folder = path.parent().expect("folder").to_path_buf();
    std::fs::create_dir_all(&folder).expect("folder");
    std::fs::write(&path, SEEDED).expect("seed");
    let pid = std::process::id();
    let planted: Vec<PathBuf> = (pid.saturating_sub(64)..=pid.saturating_add(64))
        .map(|n| folder.join(format!(".nika-tmp-{n}")))
        .collect();
    for link in &planted {
        symlink(&victim, link).expect("plant");
    }

    for round in ["first", "again"] {
        let out = room.wire(WireTarget::Vscode, false);
        let line = format!("vscode: {}", path.display());
        let want = if round == "first" {
            format!("✔ updated {line}")
        } else {
            format!("· current {line}")
        };
        assert_eq!(out.text, want, "{round}: {out:?}");
        assert_eq!(out.code, crate::output::exit::OK, "{round}");
        untouched(&victim);
        for link in &planted {
            assert_eq!(std::fs::read_link(link).expect("still planted"), victim);
        }
        let meta = std::fs::symlink_metadata(&path).expect("lstat");
        assert!(meta.file_type().is_file(), "{round}: the config is a file");
        let body = read_json(&path).expect("json");
        assert_eq!(body["servers"]["other"]["command"], "x", "{round}");
        assert_eq!(body["servers"]["nika"]["command"], "nika", "{round}");
        assert_eq!(
            spares(&folder).len(),
            planted.len(),
            "{round}: every spare name there is a planted one"
        );
    }
}

/// A project config whose own name links to a regular file inside the
/// project (`opencode.json -> config/opencode.json`) is followed: the file
/// it reaches is updated where it is, keeping the repository's server, the
/// link stays the link, the second run finds it current, and no spare is
/// left in either folder.
#[test]
fn a_project_config_linked_inside_the_project_is_written_at_its_target() {
    for target in [WireTarget::Vscode, WireTarget::Opencode, WireTarget::Junie] {
        let room = Room::new();
        let path = room.path(target);
        let folder = path.parent().expect("folder").to_path_buf();
        std::fs::create_dir_all(&folder).expect("folder");
        let inner = room.project.join("config").join("settings.json");
        std::fs::create_dir_all(inner.parent().expect("folder")).expect("folder");
        std::fs::write(&inner, SEEDED).expect("inner");
        std::fs::set_permissions(&inner, std::fs::Permissions::from_mode(0o640)).expect("chmod");
        symlink(&inner, &path).expect("link in");
        for round in ["first", "again"] {
            let out = room.wire(target, false);
            let line = format!("{}: {}", target.id(), path.display());
            let want = if round == "first" {
                format!("✔ updated {line}")
            } else {
                format!("· current {line}")
            };
            assert_eq!(out.text, want, "{target:?} {round}: {out:?}");
            assert_eq!(out.code, crate::output::exit::OK, "{target:?} {round}");
            assert_eq!(std::fs::read_link(&path).expect("still the link"), inner);
            let body = std::fs::read_to_string(&inner).expect("inner");
            assert!(body.contains("\"other\""), "{target:?} {round}: {body}");
            assert!(body.contains("\"nika\""), "{target:?} {round}: {body}");
            let mode = std::fs::metadata(&inner)
                .expect("stat")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o640, "{target:?} {round}: the target keeps its mode");
            for place in [folder.as_path(), inner.parent().expect("folder")] {
                assert_eq!(spares(place), Vec::<String>::new(), "{target:?} {round}");
            }
        }
    }
}

/// A project config whose own name links to an account file outside the
/// project is refused, the row naming where it leads: the link stays the
/// link, and the outside file is as it was.
#[test]
fn a_project_config_link_leading_out_is_never_written_through() {
    for target in [WireTarget::Vscode, WireTarget::Opencode, WireTarget::Junie] {
        let room = Room::new();
        let victim = room.victim();
        let path = room.path(target);
        std::fs::create_dir_all(path.parent().expect("folder")).expect("folder");
        symlink(&victim, &path).expect("link out");
        let row = format!(
            "✖ skipped {}: {}",
            target.id(),
            ProjectLink::Outside(path.clone())
        );
        refused_row(&room, target, &row);
        untouched(&victim);
        assert_eq!(std::fs::read_link(&path).expect("still the link"), victim);
    }
}

/// A dangling link at a project config's name is refused: no file
/// appears where it points, inside the project or out, and the link stays
/// as the repository left it. One that points outside is refused at its
/// first step out, before what it points at is looked for, so its row
/// says it leads outside.
#[test]
fn a_dangling_project_config_link_creates_nothing() {
    let room = Room::new();
    let path = room.path(WireTarget::Vscode);
    std::fs::create_dir_all(path.parent().expect("folder")).expect("folder");
    for (gone, why) in [
        (
            room.outside.join("gone.json"),
            ProjectLink::Outside(path.clone()),
        ),
        (
            room.project.join("gone.json"),
            ProjectLink::Dangling(path.clone()),
        ),
    ] {
        symlink(&gone, &path).expect("link");
        let row = format!("✖ skipped vscode: {why}");
        refused_row(&room, WireTarget::Vscode, &row);
        assert!(
            std::fs::symlink_metadata(&gone).is_err(),
            "nothing created where it points"
        );
        assert_eq!(std::fs::read_link(&path).expect("still the link"), gone);
        std::fs::remove_file(&path).expect("unlink");
    }
}

/// A project config whose own name links into the project's `.git`
/// folder is refused (any spelling of `.git`): the file there keeps its
/// bytes and the link stays.
#[test]
fn a_project_config_link_into_git_is_never_written_through() {
    for git in [".git", ".GIT"] {
        let room = Room::new();
        let path = room.path(WireTarget::Opencode);
        let config = room.project.join(git).join("config");
        std::fs::create_dir_all(config.parent().expect("folder")).expect("folder");
        std::fs::write(&config, SEEDED).expect("git config");
        symlink(&config, &path).expect("link into .git");
        let row = format!("✖ skipped opencode: {}", ProjectLink::IntoGit(path.clone()));
        refused_row(&room, WireTarget::Opencode, &row);
        assert_eq!(std::fs::read_to_string(&config).expect("read"), SEEDED);
        assert_eq!(std::fs::read_link(&path).expect("still the link"), config);
        assert_eq!(
            spares(config.parent().expect("folder")),
            Vec::<String>::new()
        );
    }
}

/// A project config whose own name links to a folder of the project is
/// refused: nothing is written in that folder, and the link stays.
#[test]
fn a_project_config_link_to_a_folder_is_never_written_through() {
    let room = Room::new();
    let path = room.path(WireTarget::Opencode);
    let folder = room.project.join("config");
    std::fs::create_dir_all(&folder).expect("folder");
    symlink(&folder, &path).expect("link to a folder");
    let row = format!(
        "✖ skipped opencode: {}",
        ProjectLink::NotAFile(path.clone())
    );
    refused_row(&room, WireTarget::Opencode, &row);
    assert_eq!(std::fs::read_dir(&folder).expect("list").count(), 0);
    assert_eq!(std::fs::read_link(&path).expect("still the link"), folder);
}

/// `.vscode` as a link out of the project, to a folder that is there or
/// to one that is not, as a link to nothing inside it, or into the
/// project's `.git` folder: refused, and nothing appears where it leads.
/// A link to nothing outside is refused at its first step out, before what
/// it points at is looked for, so its row says it leads outside.
#[test]
fn a_config_folder_leading_out_is_never_written_into() {
    let room = Room::new();
    let path = room.path(WireTarget::Vscode);
    let folder = room.project.join(".vscode");
    symlink(&room.outside, &folder).expect("link");
    let row = format!("✖ skipped vscode: {}", ProjectLink::Outside(folder.clone()));
    assert!(path.starts_with(&folder), "the config sits under the link");
    refused_row(&room, WireTarget::Vscode, &row);
    assert_eq!(
        std::fs::read_dir(&room.outside).expect("list").count(),
        0,
        "nothing created outside"
    );

    for (gone, why) in [
        (
            room.outside.join("vscode"),
            ProjectLink::Outside(folder.clone()),
        ),
        (
            room.project.join("vscode"),
            ProjectLink::Dangling(folder.clone()),
        ),
    ] {
        std::fs::remove_file(&folder).expect("unlink");
        symlink(&gone, &folder).expect("dangling link");
        let row = format!("✖ skipped vscode: {why}");
        refused_row(&room, WireTarget::Vscode, &row);
        assert!(
            std::fs::symlink_metadata(&gone).is_err(),
            "nothing created where it points"
        );
        assert_eq!(std::fs::read_link(&folder).expect("still the link"), gone);
    }
    assert_eq!(
        std::fs::read_dir(&room.outside).expect("list").count(),
        0,
        "nothing created outside"
    );

    std::fs::remove_file(&folder).expect("unlink");
    let hooks = room.project.join(".git").join("hooks");
    std::fs::create_dir_all(&hooks).expect("git hooks");
    symlink(&hooks, &folder).expect("link into .git");
    let row = format!("✖ skipped vscode: {}", ProjectLink::IntoGit(folder.clone()));
    refused_row(&room, WireTarget::Vscode, &row);
    assert_eq!(
        std::fs::read_dir(&hooks).expect("list").count(),
        0,
        "nothing created in .git"
    );
}

/// A folder link that stays inside the project is followed: the config is
/// created through it, the link stays a link, and no spare is left.
#[test]
fn a_config_folder_linked_inside_the_project_is_followed() {
    let room = Room::new();
    let real = room.project.join("tools").join("junie");
    std::fs::create_dir_all(&real).expect("folder");
    let folder = room.project.join(".junie");
    symlink(&real, &folder).expect("link");
    let path = room.path(WireTarget::Junie);

    let out = room.wire(WireTarget::Junie, false);
    assert_eq!(out.text, format!("✔ created junie: {}", path.display()));
    assert_eq!(out.code, crate::output::exit::OK);
    assert!(
        real.join("mcp").join("mcp.json").is_file(),
        "written inside"
    );
    assert_eq!(std::fs::read_link(&folder).expect("still the link"), real);
    assert_eq!(spares(&real.join("mcp")), Vec::<String>::new());
}

/// The plain case: a new project config is created owner-only (0600), an
/// existing one keeps its mode, the written file holds the body, and no
/// spare is left beside it.
#[test]
fn a_plain_project_config_is_written_with_its_mode_and_no_spare() {
    let room = Room::new();
    let path = room.path(WireTarget::Vscode);
    let folder = path.parent().expect("folder").to_path_buf();
    let mode = |p: &Path| std::fs::metadata(p).expect("stat").permissions().mode() & 0o777;

    let out = room.wire(WireTarget::Vscode, false);
    assert_eq!(out.text, format!("✔ created vscode: {}", path.display()));
    assert_eq!(out.code, crate::output::exit::OK);
    assert!(std::fs::symlink_metadata(&path).expect("lstat").is_file());
    assert_eq!(mode(&path), 0o600, "a new config is owner-only");
    assert_eq!(spares(&folder), Vec::<String>::new());

    std::fs::write(&path, SEEDED).expect("seed");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).expect("chmod");
    let out = room.wire(WireTarget::Vscode, false);
    assert_eq!(out.text, format!("✔ updated vscode: {}", path.display()));
    assert_eq!(mode(&path), 0o640, "an existing config keeps its mode");
    assert_eq!(spares(&folder), Vec::<String>::new());

    let body = "{\"a\":1}\n";
    assert_eq!(write_atomic(&path, body), Ok(()));
    assert_eq!(std::fs::read_to_string(&path).expect("read"), body);
    assert_eq!(mode(&path), 0o640);
    assert_eq!(spares(&folder), Vec::<String>::new());
}

/// An account file outside the project holding JSON a writer would parse
/// and extend, with a mode of its own: the place a redirected write lands.
fn json_victim(room: &Room) -> PathBuf {
    let victim = room.outside.join("settings.json");
    std::fs::write(&victim, SEEDED).expect("victim");
    std::fs::set_permissions(&victim, std::fs::Permissions::from_mode(0o644)).expect("chmod");
    victim
}

/// The outside JSON file holds what it held, with the mode it had.
fn json_untouched(victim: &Path) {
    assert_eq!(std::fs::read_to_string(victim).expect("read"), SEEDED);
    let mode = std::fs::metadata(victim)
        .expect("stat")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o644, "the outside file keeps its mode");
}

/// What the check settled is where the write lands: a config swapped for
/// a link to an outside file after the check (while the writer reads it)
/// is refused just before the write, naming both places, and the outside
/// file is as it was. Once the hold ends, a new run refuses the link as
/// any other.
#[test]
fn a_config_swapped_after_the_check_is_never_written() {
    let room = Room::new();
    let victim = json_victim(&room);
    let path = room.path(WireTarget::Vscode);
    std::fs::create_dir_all(path.parent().expect("folder")).expect("folder");
    std::fs::write(&path, SEEDED).expect("seed");
    let (settled, lands) =
        settled_path(WireTarget::Vscode, Some(&room.home), &room.project).expect("settled");
    assert_eq!(settled, path);
    assert_eq!(lands, std::fs::canonicalize(&path).expect("real"));

    let held = Settled::hold(&path, lands.clone());
    std::fs::remove_file(&path).expect("unlink");
    symlink(&victim, &path).expect("swap for a link out");
    let refused = wire_at(WireTarget::Vscode, &path, false).expect_err("refused");
    assert_eq!(
        refused,
        format!(
            "vscode: {}: changed since nika wire checked it: a write would now land at {}, \
             not {}; nothing written · run nika wire again",
            path.display(),
            Room::real(&victim),
            lands.display()
        )
    );
    json_untouched(&victim);
    assert_eq!(std::fs::read_link(&path).expect("still the link"), victim);
    assert_eq!(spares(path.parent().expect("folder")), Vec::<String>::new());
    assert_eq!(spares(&room.outside), Vec::<String>::new());
    drop(held);

    assert_eq!(
        room.wire(WireTarget::Vscode, false).code,
        crate::output::exit::ENV
    );
    json_untouched(&victim);
}

/// The write goes to the real place the check compared, never to the name
/// resolved once more: a config not there yet under a folder link that
/// stays inside lands at its name in the real folder, the place the hold
/// names, not at the path through the link; the write puts it there.
#[test]
fn the_write_goes_to_the_place_the_check_compared() {
    let room = Room::new();
    let real = room.project.join("config");
    std::fs::create_dir_all(&real).expect("real folder");
    symlink(&real, room.project.join(".vscode")).expect("folder link in");
    let path = room.path(WireTarget::Vscode);
    let (settled, lands) =
        settled_path(WireTarget::Vscode, Some(&room.home), &room.project).expect("settled");
    assert_eq!(settled, path);
    let want = std::fs::canonicalize(&real).expect("real").join("mcp.json");
    assert_eq!(lands, want);
    let held = Settled::hold(&path, lands);
    assert_eq!(destination(&path), Ok(want.clone()));
    assert_ne!(
        want, path,
        "the destination is not the path through the link"
    );
    write_atomic(&path, "{}\n").expect("write");
    assert_eq!(std::fs::read_to_string(&want).expect("read"), "{}\n");
    assert!(
        std::fs::symlink_metadata(room.project.join(".vscode"))
            .expect("lstat")
            .file_type()
            .is_symlink(),
        "the folder link stays"
    );
    assert_eq!(spares(&real), Vec::<String>::new());
    drop(held);
}

/// A folder link out whose far side links back into the project: the
/// config's real place is inside, yet the way leaves the project, so it
/// is refused at the folder that leads out, and the file it would reach
/// keeps its bytes.
#[test]
fn a_folder_way_out_and_back_is_refused_where_it_leaves() {
    let room = Room::new();
    let inner = room.project.join("config").join("mcp.json");
    std::fs::create_dir_all(inner.parent().expect("folder")).expect("folder");
    std::fs::write(&inner, SEEDED).expect("inner");
    let far = room.outside.join("vscode");
    std::fs::create_dir_all(&far).expect("far folder");
    symlink(&inner, far.join("mcp.json")).expect("link back in");
    let folder = room.project.join(".vscode");
    symlink(&far, &folder).expect("link out");
    let path = room.path(WireTarget::Vscode);
    assert!(
        std::fs::canonicalize(&path)
            .expect("real")
            .starts_with(std::fs::canonicalize(&room.project).expect("real project")),
        "the chain ends inside the project"
    );
    let row = format!("✖ skipped vscode: {}", ProjectLink::Outside(folder.clone()));
    refused_row(&room, WireTarget::Vscode, &row);
    assert_eq!(std::fs::read_to_string(&inner).expect("read"), SEEDED);
    assert_eq!(std::fs::read_link(&folder).expect("still the link"), far);
    assert_eq!(spares(&far), Vec::<String>::new());
}

/// The config's own name leads out of the project to a link that leads back
/// in (`.vscode/mcp.json -> <outside>/hop.json`, `hop.json ->
/// .vscode/real.json`). Resolved in one call the chain ends inside, yet the
/// hop outside belongs to whoever owns that folder, who can turn it to an
/// account file between wire's look and its write. Refused at the config's
/// link, by preview and by run: the file the chain reaches keeps its bytes.
#[test]
fn a_config_way_out_and_back_in_is_refused_at_its_link() {
    let room = Room::new();
    let path = room.path(WireTarget::Vscode);
    let folder = path.parent().expect("folder").to_path_buf();
    std::fs::create_dir_all(&folder).expect("folder");
    let real = folder.join("real.json");
    std::fs::write(&real, SEEDED).expect("real");
    let hop = room.outside.join("hop.json");
    symlink(&real, &hop).expect("hop back in");
    symlink(&hop, &path).expect("out");
    assert!(
        std::fs::canonicalize(&path)
            .expect("one call")
            .starts_with(std::fs::canonicalize(&room.project).expect("real project")),
        "resolved in one call, the chain ends inside"
    );
    let row = format!("✖ skipped vscode: {}", ProjectLink::Outside(path.clone()));
    refused_row(&room, WireTarget::Vscode, &row);
    assert_eq!(std::fs::read_to_string(&real).expect("read"), SEEDED);
    assert_eq!(std::fs::read_link(&path).expect("still the link"), hop);
    assert_eq!(spares(&folder), Vec::<String>::new());
    assert_eq!(spares(&room.outside), Vec::<String>::new());
}

/// A folder on the way that cannot be searched: the run refuses the config
/// on the step walk, which names the entry it could not read with the
/// reason the system gives, and writes nothing. A process that reads every
/// folder whatever its mode (root) cannot hold this layout: there the test
/// asserts nothing.
#[test]
fn an_unsearchable_folder_on_the_way_is_refused_and_named() {
    let room = Room::new();
    let path = room.path(WireTarget::Vscode);
    let folder = path.parent().expect("folder").to_path_buf();
    std::fs::create_dir_all(&folder).expect("folder");
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o000)).expect("chmod");
    let restore = || {
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).expect("restore");
    };
    if std::fs::read_dir(&folder).is_ok() {
        // This process reads a folder of mode 000 (root): the layout cannot
        // be held, and there is nothing to refuse.
        restore();
        return;
    }
    let walked = project_way(&room.project, &path);
    let lstat = std::fs::symlink_metadata(&path).expect_err("unsearchable");
    let row = format!(
        "✖ skipped vscode: {}",
        ProjectLink::Unreadable(path.clone(), lstat.to_string())
    );
    let outs: Vec<_> = [true, false]
        .into_iter()
        .map(|dry_run| room.wire(WireTarget::Vscode, dry_run))
        .collect();
    restore();
    assert_eq!(
        walked,
        Err(ProjectLink::Unreadable(path.clone(), lstat.to_string()))
    );
    for out in outs {
        assert_eq!(out.code, crate::output::exit::ENV, "{out:?}");
        assert!(
            out.text.lines().any(|line| line == row),
            "wanted {row:?} in {out:?}"
        );
    }
    assert_eq!(std::fs::read_dir(&folder).expect("list").count(), 0);
}

/// A relative link out (`opencode.json -> ../outside/rc`) is refused as an
/// absolute one is, and the link stays as the repository wrote it.
#[test]
fn a_relative_project_config_link_leading_out_is_never_written_through() {
    let room = Room::new();
    let victim = room.victim();
    let path = room.path(WireTarget::Opencode);
    symlink("../outside/rc", &path).expect("relative link out");
    let row = format!("✖ skipped opencode: {}", ProjectLink::Outside(path.clone()));
    refused_row(&room, WireTarget::Opencode, &row);
    untouched(&victim);
    assert_eq!(
        std::fs::read_link(&path).expect("still the link"),
        Path::new("../outside/rc")
    );
}

/// A project reached through a link (the folder handed to `wire` is a
/// link to it) is judged by where it really is: a config linked to a file
/// of the project is updated there, and a folder link out is still refused.
#[test]
fn a_project_reached_through_a_link_is_judged_where_it_is() {
    let room = Room::new();
    let alias = room.project.parent().expect("top").join("alias");
    symlink(&room.project, &alias).expect("alias");
    let inner = room.project.join("config").join("settings.json");
    std::fs::create_dir_all(inner.parent().expect("folder")).expect("folder");
    std::fs::write(&inner, SEEDED).expect("inner");
    std::fs::create_dir_all(room.project.join(".vscode")).expect("folder");
    symlink(
        "../config/settings.json",
        room.project.join(".vscode/mcp.json"),
    )
    .expect("link in");
    let wire = |target: WireTarget| {
        run_in(
            target,
            Some(&room.home),
            alias.to_str().expect("utf8"),
            WireOptions::default(),
        )
    };
    let path = alias.join(".vscode").join("mcp.json");
    let out = wire(WireTarget::Vscode);
    assert_eq!(out.text, format!("✔ updated vscode: {}", path.display()));
    assert_eq!(out.code, crate::output::exit::OK);
    let body = read_json(&inner).expect("json");
    assert_eq!(body["servers"]["other"]["command"], "x");
    assert_eq!(body["servers"]["nika"]["command"], "nika");
    assert!(
        std::fs::symlink_metadata(room.project.join(".vscode/mcp.json"))
            .expect("lstat")
            .file_type()
            .is_symlink(),
        "the link stays"
    );

    let victim = room.victim();
    symlink(&victim, room.project.join("opencode.json")).expect("link out");
    let out = wire(WireTarget::Opencode);
    assert_eq!(out.code, crate::output::exit::ENV, "{out:?}");
    let row = format!(
        "✖ skipped opencode: {}",
        ProjectLink::Outside(alias.join("opencode.json"))
    );
    assert!(
        out.text.lines().any(|line| line == row),
        "wanted {row:?} in {out:?}"
    );
    untouched(&victim);
}

/// A project config that is a hard link to an outside file is not written
/// into that file: the new config takes the name, the outside file keeps
/// its bytes, its mode and no other name, and the project's file is its
/// own.
#[test]
fn a_hard_linked_project_config_never_writes_the_outside_file() {
    use std::os::unix::fs::MetadataExt as _;
    let room = Room::new();
    let victim = json_victim(&room);
    let path = room.path(WireTarget::Vscode);
    std::fs::create_dir_all(path.parent().expect("folder")).expect("folder");
    std::fs::hard_link(&victim, &path).expect("hard link");
    assert_eq!(std::fs::metadata(&victim).expect("stat").nlink(), 2);

    let out = room.wire(WireTarget::Vscode, false);
    assert_eq!(out.text, format!("✔ updated vscode: {}", path.display()));
    assert_eq!(out.code, crate::output::exit::OK);
    json_untouched(&victim);
    assert_eq!(std::fs::metadata(&victim).expect("stat").nlink(), 1);
    let body = read_json(&path).expect("json");
    assert_eq!(body["servers"]["nika"]["command"], "nika");
    assert_eq!(body["servers"]["other"]["command"], "x");
    assert_ne!(
        std::fs::metadata(&path).expect("stat").ino(),
        std::fs::metadata(&victim).expect("stat").ino()
    );
}

/// A link loop that stays inside the project, at the config's own name
/// (`.vscode/mcp.json -> a.json -> mcp.json`) or at its folder (`.vscode ->
/// loop -> .vscode`): the step walk, which comes before any look that
/// resolves the whole path, names it a link loop with its way out, never a
/// way out of the project nor the system's own reason, by preview and by
/// run, and creates nothing: the links stay as they are.
#[test]
fn a_link_loop_inside_the_project_is_named_as_one() {
    let room = Room::new();
    let path = room.path(WireTarget::Vscode);
    let folder = path.parent().expect("folder").to_path_buf();
    std::fs::create_dir_all(&folder).expect("folder");
    let other = folder.join("a.json");
    symlink(&other, &path).expect("to a.json");
    symlink(&path, &other).expect("back to mcp.json");
    assert_eq!(
        project_way(&room.project, &path),
        Err(ProjectLink::Loop(path.clone()))
    );
    refused_row(
        &room,
        WireTarget::Vscode,
        &format!("✖ skipped vscode: {}", ProjectLink::Loop(path.clone())),
    );
    assert_eq!(std::fs::read_link(&path).expect("still the link"), other);
    assert_eq!(std::fs::read_link(&other).expect("still the link"), path);
    assert_eq!(spares(&folder), Vec::<String>::new());

    let room = Room::new();
    let folder = room.project.join(".vscode");
    let hop = room.project.join("loop");
    symlink(&hop, &folder).expect("to loop");
    symlink(&folder, &hop).expect("back to .vscode");
    let path = room.path(WireTarget::Vscode);
    let looped = ProjectLink::Loop(folder.clone());
    assert_eq!(project_way(&room.project, &path), Err(looped.clone()));
    assert_eq!(
        looped.to_string(),
        format!(
            "{} is a link loop: following it never ends; nika wire follows a link only to a \
             place it reaches: point it at a file inside the project, or remove it",
            folder.display()
        )
    );
    refused_row(
        &room,
        WireTarget::Vscode,
        &format!("✖ skipped vscode: {looped}"),
    );
    assert_eq!(std::fs::read_link(&folder).expect("still the link"), hop);
    assert_eq!(std::fs::read_link(&hop).expect("still the link"), folder);
    assert_eq!(
        std::fs::read_dir(&room.project).expect("list").count(),
        2,
        "nothing created in the project"
    );
}

/// Where a write lands comes from a look of its own, after the two that
/// judged the path, and is judged before the writer holds it: a look that
/// then finds a place outside the folder that owns the config (the project
/// for a project config, the home for every other), below it by a name
/// that is not plain (`..`), at that folder itself, or in a `.git` folder
/// refuses the config; a place inside is held as the look found it.
#[test]
fn a_held_landing_is_judged_before_it_is_held() {
    let room = Room::new();
    let project = std::fs::canonicalize(&room.project).expect("real project");
    let home = std::fs::canonicalize(&room.home).expect("real home");
    let outside = std::fs::canonicalize(&room.outside)
        .expect("real outside")
        .join("rc");
    let leaves = "nika wire leaves it as it is · run nika wire again";
    let out = |inside: &Path| format!("outside {}; {leaves}", inside.display());
    let not_plain = |inside: &Path| {
        format!(
            "not a file named plainly inside {}; {leaves}",
            inside.display()
        )
    };
    let in_git =
        || "in a .git folder; nika wire never writes there · run nika wire again".to_owned();
    // Under the owner by its prefix, yet out of it by `..`, or the owner
    // folder itself: neither is a file named plainly inside it.
    for (target, lands, why) in [
        (
            WireTarget::Vscode,
            project.join(".vscode/../../rc"),
            not_plain(&project),
        ),
        (WireTarget::Vscode, project.clone(), not_plain(&project)),
        (WireTarget::Claude, home.join("../rc"), not_plain(&home)),
        (WireTarget::Vscode, outside.clone(), out(&project)),
        (
            WireTarget::Vscode,
            project.join(".git").join("mcp.json"),
            in_git(),
        ),
        (WireTarget::Claude, outside.clone(), out(&home)),
        (
            WireTarget::Claude,
            home.join(".GIT").join("config"),
            in_git(),
        ),
    ] {
        let path = room.path(target);
        let got = settled_with(target, Some(&room.home), &room.project, &|_| {
            Ok(lands.clone())
        });
        let why = format!(
            "{}: a write would land at {}, {why}",
            path.display(),
            lands.display()
        );
        assert_eq!(got, Err(format!("{}: {why}", target.id())), "{path:?}");
    }
    let vscode = room.path(WireTarget::Vscode);
    let lands = project.join(".vscode").join("mcp.json");
    let found = lands.clone();
    assert_eq!(
        settled_with(WireTarget::Vscode, Some(&room.home), &room.project, &|_| {
            Ok(found.clone())
        }),
        Ok((vscode.clone(), lands))
    );
    assert_eq!(
        settled_path(WireTarget::Vscode, Some(&room.home), &room.project),
        Ok((vscode, project.join(".vscode").join("mcp.json"))),
        "the real look, unchanged, is held as it finds the place"
    );
}
