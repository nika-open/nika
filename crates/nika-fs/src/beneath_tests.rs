// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! A walk below a folder sees every place its way passes: a link is
//! followed only while each place it leads through is inside the folder,
//! or one of the folders above it on the way down, and what comes back is
//! the real place. Each case runs in a private temporary directory holding
//! the `project` folder walked and an `outside` folder beside it.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use crate::{Beneath, Escape, MAX_LINKS, resolve_beneath};

struct Room {
    _keep: tempfile::TempDir,
    /// The project as the temporary directory spells it (on macOS through
    /// the `/var` link), the way a caller names it.
    named: PathBuf,
    /// The project as it really is.
    project: PathBuf,
    outside: PathBuf,
}

impl Room {
    fn new() -> Self {
        let keep = tempfile::tempdir().expect("room");
        let named = keep.path().join("project");
        std::fs::create_dir_all(&named).expect("project");
        std::fs::create_dir_all(keep.path().join("outside")).expect("outside");
        let top = keep.path().canonicalize().expect("canonical");
        Self {
            named,
            project: top.join("project"),
            outside: top.join("outside"),
            _keep: keep,
        }
    }

    fn walk(&self, path: &str) -> Result<Beneath, Escape> {
        resolve_beneath(&self.named, Path::new(path))
    }
}

fn there(result: Result<Beneath, Escape>) -> PathBuf {
    match result {
        Ok(Beneath::There(real)) => real,
        other => panic!("expected a real place, got {other:?}"),
    }
}

fn outside_at(result: Result<Beneath, Escape>) -> PathBuf {
    match result {
        Err(Escape::Outside(at)) => at,
        other => panic!("expected the way out, got {other:?}"),
    }
}

#[test]
fn a_plain_path_is_its_real_place() {
    let room = Room::new();
    std::fs::create_dir_all(room.project.join(".claude")).expect("folder");
    std::fs::write(room.project.join(".claude/settings.json"), "{}").expect("file");
    assert_eq!(
        there(room.walk(".claude/settings.json")),
        room.project.join(".claude/settings.json")
    );
    assert_eq!(there(room.walk("")), room.project);
    assert_eq!(there(room.walk("./.claude")), room.project.join(".claude"));
}

#[test]
fn a_missing_name_gives_the_real_folder_and_the_names_below_it() {
    let room = Room::new();
    std::fs::create_dir_all(room.project.join("config")).expect("real");
    symlink("config", room.project.join(".cursor")).expect("link in");
    let walked = room.walk(".cursor/hooks-nika/guard-run.sh").expect("walk");
    assert_eq!(
        walked,
        Beneath::Missing {
            folder: room.project.join("config"),
            rest: PathBuf::from("hooks-nika/guard-run.sh"),
        }
    );
}

#[test]
fn links_that_stay_inside_are_followed_to_the_real_place() {
    let room = Room::new();
    std::fs::create_dir_all(room.project.join("shared/claude")).expect("real");
    std::fs::write(room.project.join("AGENTS.md"), "x").expect("file");
    // Relative, two hops, a `..` from a real folder, and an absolute
    // target spelled the way the project is named.
    symlink("AGENTS.md", room.project.join("CLAUDE.md")).expect("relative");
    symlink("hop", room.project.join(".claude")).expect("first hop");
    symlink("shared/claude", room.project.join("hop")).expect("second hop");
    std::fs::create_dir_all(room.project.join("docs")).expect("docs");
    symlink("../AGENTS.md", room.project.join("docs/AGENTS.md")).expect("up");
    symlink(room.named.join("AGENTS.md"), room.project.join("abs.md")).expect("absolute");
    let agents = room.project.join("AGENTS.md");
    assert_eq!(there(room.walk("CLAUDE.md")), agents);
    assert_eq!(
        there(room.walk(".claude")),
        room.project.join("shared/claude")
    );
    assert_eq!(there(room.walk("docs/AGENTS.md")), agents);
    assert_eq!(there(room.walk("abs.md")), agents);
}

/// The way the system resolves in one call ends inside, yet it passes
/// through a link outside the project: the walk stops at the project's
/// link that leads out, at a folder and at a file's own name.
#[test]
fn a_way_out_and_back_in_is_refused_where_it_leaves() {
    let room = Room::new();
    std::fs::create_dir_all(room.project.join("real")).expect("real");
    std::fs::write(room.project.join("real/settings.json"), "{}").expect("file");
    symlink(room.project.join("real"), room.outside.join("hop")).expect("hop back in");
    symlink(room.outside.join("hop"), room.project.join(".claude")).expect("out");
    symlink(
        room.outside.join("hop/settings.json"),
        room.project.join("mcp.json"),
    )
    .expect("out, by file");
    for (path, link) in [
        (".claude/settings.json", ".claude"),
        ("mcp.json", "mcp.json"),
    ] {
        let through = room.named.join(path);
        assert!(
            through
                .canonicalize()
                .expect("one call")
                .starts_with(&room.project),
            "{path}: resolved in one call, the way ends inside"
        );
        assert_eq!(
            outside_at(room.walk(path)),
            room.project.join(link),
            "{path}"
        );
    }
}

#[test]
fn a_link_out_or_up_is_refused() {
    let room = Room::new();
    std::fs::write(room.outside.join("rc"), "x").expect("victim");
    symlink(room.outside.join("rc"), room.project.join("abs")).expect("absolute out");
    symlink("../outside/rc", room.project.join("rel")).expect("relative out");
    symlink("..", room.project.join("up")).expect("the folder above");
    symlink("/", room.project.join("root")).expect("the root");
    for name in ["abs", "rel", "up", "root"] {
        assert_eq!(
            outside_at(room.walk(name)),
            room.project.join(name),
            "{name}"
        );
    }
    // Back down from above, into the project again, is still inside.
    let back = format!(
        "up/{}",
        room.project.file_name().expect("name").to_string_lossy()
    );
    assert_eq!(there(room.walk(&back)), room.project);
}

#[test]
fn a_path_that_climbs_is_refused_before_any_look() {
    let room = Room::new();
    assert_eq!(
        outside_at(room.walk("../outside")),
        room.named.join("../outside")
    );
    // An absolute path replaces the folder it is joined to: it is named as
    // it was given.
    assert_eq!(outside_at(room.walk("/etc")), PathBuf::from("/etc"));
}

#[test]
fn a_dangling_link_is_never_a_missing_name() {
    let room = Room::new();
    symlink("gone", room.project.join("a")).expect("dangling");
    symlink("real/gone", room.project.join("b")).expect("under a missing folder");
    symlink("a", room.project.join("c")).expect("to a dangling link");
    for (name, link) in [("a", "a"), ("b/x", "b"), ("c", "a")] {
        match room.walk(name) {
            Err(Escape::Dangling(at)) => assert_eq!(at, room.project.join(link), "{name}"),
            other => panic!("{name}: expected a dangling link, got {other:?}"),
        }
    }
}

#[test]
fn a_loop_ends_the_walk() {
    let room = Room::new();
    symlink("b", room.project.join("a")).expect("a");
    symlink("a", room.project.join("b")).expect("b");
    assert!(
        matches!(room.walk("a"), Err(Escape::Loop(_))),
        "a loop is refused, never followed forever"
    );
    // A chain as long as the limit is followed.
    std::fs::write(room.project.join("end"), "x").expect("end");
    let mut previous = String::from("end");
    for n in 0..MAX_LINKS {
        let name = format!("l{n}");
        symlink(&previous, room.project.join(&name)).expect("link");
        previous = name;
    }
    assert_eq!(there(room.walk(&previous)), room.project.join("end"));
    // One more is past the limit: the walk ends at the link it met last.
    let over = format!("l{MAX_LINKS}");
    symlink(&previous, room.project.join(&over)).expect("link");
    match room.walk(&over) {
        Err(Escape::Loop(at)) => assert_eq!(at, room.project.join("l0")),
        other => panic!("a chain of {} links is a loop: {other:?}", MAX_LINKS + 1),
    }
}

/// A project reached through a link of its own (the folder handed in is a
/// link) is walked where it really is, and a link spelled through that
/// alias is inside.
#[test]
fn a_project_named_through_a_link_is_walked_where_it_is() {
    let room = Room::new();
    let alias = room.outside.join("alias");
    symlink(&room.project, &alias).expect("alias");
    std::fs::write(room.project.join("AGENTS.md"), "x").expect("file");
    symlink(alias.join("AGENTS.md"), room.project.join("CLAUDE.md")).expect("through alias");
    assert_eq!(
        there(resolve_beneath(&alias, Path::new("CLAUDE.md"))),
        room.project.join("AGENTS.md")
    );
    // The alias is not the project from the project's own spelling.
    assert_eq!(
        outside_at(room.walk("CLAUDE.md")),
        room.project.join("CLAUDE.md")
    );
}

#[test]
fn a_folder_that_is_not_there_is_an_io_error() {
    let room = Room::new();
    let gone = room.named.join("gone");
    match resolve_beneath(&gone, Path::new("x")) {
        Err(Escape::Io(at, e)) => {
            assert_eq!(at, gone);
            assert_eq!(e.kind(), std::io::ErrorKind::NotFound);
        }
        other => panic!("expected the folder's own error, got {other:?}"),
    }
}
