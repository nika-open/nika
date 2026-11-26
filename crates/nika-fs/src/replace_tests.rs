// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! Every staged write lands under a new, random temporary name, never
//! through an entry planted beside the destination.
//!
//! Each case runs in a private temporary directory holding the folder that
//! is written and an `outside/` file that stands for one the account owns.
//! Links are planted at every name the earlier writers used (pid and a
//! counter, and the counter alone); each write must leave the outside file
//! byte-identical with its mode, the planted links as they were, the
//! destination a regular file with the new bytes, and no temporary behind.
//!
//! Not observed here: a folder swapped for a link between the caller's
//! check and the rename (the module docs name that gap).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::os::unix::fs::{PermissionsExt as _, symlink};
use std::path::{Path, PathBuf};

use nika_kernel::fs::FsWriteDyn;

use crate::{Durability, OwnedDir, ReplaceMode, TokioFs, replace_file};

const VICTIM: &[u8] = b"export KEEP=1\n";

struct Room {
    _keep: tempfile::TempDir,
    folder: PathBuf,
    victim: PathBuf,
}

impl Room {
    fn new() -> Self {
        let keep = tempfile::tempdir().expect("room");
        let top = keep.path().canonicalize().expect("canonical");
        let folder = top.join("project");
        std::fs::create_dir_all(&folder).expect("folder");
        std::fs::create_dir_all(top.join("outside")).expect("outside");
        let victim = top.join("outside/rc");
        std::fs::write(&victim, VICTIM).expect("victim");
        std::fs::set_permissions(&victim, std::fs::Permissions::from_mode(0o644)).expect("mode");
        Self {
            _keep: keep,
            folder,
            victim,
        }
    }

    /// Links to the outside file at the names a pid-and-counter writer
    /// would pick next (from the counter's current value on), and at the
    /// counter-free names earlier writers used, for `destination`.
    fn plant(&self, destination: &str) -> Vec<PathBuf> {
        let pid = std::process::id();
        let next = crate::TMP_COUNTER.load(std::sync::atomic::Ordering::Relaxed);
        let mut names: Vec<String> = Vec::new();
        for n in next..next + 64 {
            names.push(format!(".nika-tmp.{pid}.{n}"));
            names.push(format!("..nika-tmp.{pid}.{n}"));
        }
        names.push(format!(".{destination}.tmp-{pid}"));
        names.push(format!(".{destination}.{pid}.tmp"));
        names.push(format!(".nika-fix-tmp.{destination}"));
        names.push(format!("{destination}.tmp-{pid}"));
        names.push(format!(".nika-tmp-{pid}"));
        names
            .into_iter()
            .map(|name| {
                let link = self.folder.join(name);
                symlink(&self.victim, &link).expect("plant");
                link
            })
            .collect()
    }

    /// The outside file and every planted link are as they were, and no
    /// entry but the destination and the planted links is left.
    fn untouched(&self, planted: &[PathBuf], destination: &Path) {
        assert_eq!(std::fs::read(&self.victim).expect("victim"), VICTIM);
        let mode = std::fs::metadata(&self.victim)
            .expect("stat")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o644, "the outside file keeps its mode");
        for link in planted {
            assert_eq!(
                std::fs::read_link(link).expect("still planted"),
                self.victim
            );
        }
        let mut left: Vec<PathBuf> = std::fs::read_dir(&self.folder)
            .expect("list")
            .map(|entry| entry.expect("entry").path())
            .filter(|path| !planted.contains(path))
            .collect();
        left.sort();
        assert_eq!(
            left,
            [destination.to_path_buf()],
            "no temporary is left behind"
        );
        let meta = std::fs::symlink_metadata(destination).expect("lstat");
        assert!(
            meta.file_type().is_file(),
            "the destination is a regular file"
        );
    }
}

fn mode_of(path: &Path) -> u32 {
    std::fs::metadata(path).expect("stat").permissions().mode() & 0o777
}

#[test]
fn a_replace_never_writes_through_a_planted_temporary_name() {
    let room = Room::new();
    let planted = room.plant("flow.nika");
    let destination = room.folder.join("flow.nika");
    std::fs::write(&destination, b"old\n").expect("seed");
    for round in [b"first\n".as_slice(), b"second\n".as_slice()] {
        replace_file(
            &destination,
            round,
            ReplaceMode::KeepOr(0o644),
            Durability::All,
        )
        .expect("replace");
        assert_eq!(std::fs::read(&destination).expect("read"), round);
        room.untouched(&planted, &destination);
    }
}

/// The names are random, so no test can plant one in advance: this one
/// hands the open itself a name that is taken (a link to the outside file,
/// a dangling link, a plain file) in every mode.
#[test]
fn a_temporary_name_already_taken_is_never_opened_or_followed() {
    let room = Room::new();
    let link = room.folder.join(".nika-tmp.link");
    symlink(&room.victim, &link).expect("plant");
    let gone = room.folder.join("gone");
    let dangling = room.folder.join(".nika-tmp.dangling");
    symlink(&gone, &dangling).expect("plant");
    let plain = room.folder.join(".nika-tmp.plain");
    std::fs::write(&plain, b"kept").expect("plain");
    let modes = [
        ReplaceMode::New(0o600),
        ReplaceMode::KeepOr(0o644),
        ReplaceMode::Exactly(0o755),
    ];
    for taken in [&link, &dangling, &plain] {
        for mode in modes {
            for keeps in [false, true] {
                let refused = crate::replace::open_spare(taken, mode, keeps)
                    .expect_err("a taken name is refused");
                assert_eq!(
                    refused.kind(),
                    std::io::ErrorKind::AlreadyExists,
                    "{} {mode:?}",
                    taken.display()
                );
            }
        }
    }
    assert_eq!(std::fs::read(&room.victim).expect("victim"), VICTIM);
    assert_eq!(
        mode_of(&room.victim),
        0o644,
        "the outside file keeps its mode"
    );
    assert!(
        std::fs::symlink_metadata(&gone).is_err(),
        "a dangling link's target is never created"
    );
    assert_eq!(std::fs::read(&plain).expect("plain"), b"kept");
    assert_eq!(std::fs::read_link(&link).expect("link"), room.victim);
}

#[test]
fn a_link_at_the_destination_is_replaced_never_written_through() {
    let room = Room::new();
    let destination = room.folder.join("opencode.json");
    symlink(&room.victim, &destination).expect("link");
    replace_file(
        &destination,
        b"{}\n",
        ReplaceMode::KeepOr(0o600),
        Durability::Unsynced,
    )
    .expect("replace");
    assert_eq!(std::fs::read(&destination).expect("read"), b"{}\n");
    room.untouched(&[], &destination);
}

#[test]
fn the_three_modes_end_where_they_say() {
    let room = Room::new();
    let kept = room.folder.join("kept");
    std::fs::write(&kept, b"x").expect("seed");
    std::fs::set_permissions(&kept, std::fs::Permissions::from_mode(0o640)).expect("mode");
    replace_file(
        &kept,
        b"y",
        ReplaceMode::KeepOr(0o600),
        Durability::Unsynced,
    )
    .expect("replace");
    assert_eq!(mode_of(&kept), 0o640, "a replaced file keeps its mode");

    let fresh = room.folder.join("fresh");
    replace_file(
        &fresh,
        b"y",
        ReplaceMode::KeepOr(0o600),
        Durability::Unsynced,
    )
    .expect("create");
    assert_eq!(mode_of(&fresh), 0o600, "a new file gets the given bits");

    let script = room.folder.join("hook.sh");
    std::fs::write(&script, b"x").expect("seed");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o600)).expect("mode");
    replace_file(
        &script,
        b"#!/bin/sh\n",
        ReplaceMode::Exactly(0o755),
        Durability::Unsynced,
    )
    .expect("replace");
    assert_eq!(mode_of(&script), 0o755, "exact bits, whatever was there");

    let reset = room.folder.join("reset");
    std::fs::write(&reset, b"x").expect("seed");
    std::fs::set_permissions(&reset, std::fs::Permissions::from_mode(0o640)).expect("mode");
    replace_file(&reset, b"y", ReplaceMode::New(0o600), Durability::Unsynced).expect("replace");
    assert_eq!(
        mode_of(&reset),
        0o600,
        "New does not keep the replaced mode"
    );

    let setid = room.folder.join("setid");
    std::fs::write(&setid, b"x").expect("seed");
    std::fs::set_permissions(&setid, std::fs::Permissions::from_mode(0o4750)).expect("mode");
    replace_file(
        &setid,
        b"y",
        ReplaceMode::KeepOr(0o600),
        Durability::Unsynced,
    )
    .expect("replace");
    assert_eq!(
        std::fs::metadata(&setid)
            .expect("stat")
            .permissions()
            .mode()
            & 0o7777,
        0o750,
        "a kept mode never carries a set-id bit"
    );
}

#[test]
fn a_path_that_names_no_file_is_refused_and_nothing_is_made() {
    let room = Room::new();
    let error = replace_file(
        Path::new("/"),
        b"x",
        ReplaceMode::New(0o644),
        Durability::Unsynced,
    )
    .expect_err("no name");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput, "{error}");
    let missing = room.folder.join("absent/file");
    let error = replace_file(
        &missing,
        b"x",
        ReplaceMode::New(0o644),
        Durability::Unsynced,
    )
    .expect_err("no folder");
    assert_eq!(error.kind(), std::io::ErrorKind::NotFound, "{error}");
    assert_eq!(std::fs::read_dir(&room.folder).expect("list").count(), 0);
}

/// The rename is the last step that can fail: over a folder that holds a
/// file it does, and the temporary written for it is removed.
#[test]
fn a_failed_rename_leaves_no_temporary_behind() {
    let room = Room::new();
    let destination = room.folder.join("flow.nika");
    std::fs::create_dir(&destination).expect("a folder at the name");
    std::fs::write(destination.join("inside"), b"kept").expect("a file in it");
    for durability in [Durability::Unsynced, Durability::Data, Durability::All] {
        let error = replace_file(&destination, b"x", ReplaceMode::KeepOr(0o644), durability)
            .expect_err("a file cannot replace a folder that holds one");
        assert_ne!(error.kind(), std::io::ErrorKind::InvalidInput, "{error}");
        let left: Vec<_> = std::fs::read_dir(&room.folder)
            .expect("list")
            .map(|entry| entry.expect("entry").file_name())
            .collect();
        assert_eq!(left, ["flow.nika"], "{durability:?}: no temporary is left");
        assert_eq!(
            std::fs::read(destination.join("inside")).expect("inside"),
            b"kept"
        );
    }
}

/// `New(0o666)` and `KeepOr(0o666)` create what `std::fs::write` creates:
/// `0666` narrowed by the umask, whatever the umask is.
#[test]
fn a_new_file_gets_the_mode_std_write_gives_it() {
    let room = Room::new();
    let reference = room.folder.join("reference");
    std::fs::write(&reference, b"x").expect("reference");
    for (name, mode) in [
        ("new", ReplaceMode::New(0o666)),
        ("kept", ReplaceMode::KeepOr(0o666)),
    ] {
        let made = room.folder.join(name);
        replace_file(&made, b"x", mode, Durability::Unsynced).expect("create");
        assert_eq!(mode_of(&made), mode_of(&reference), "{name}");
    }
}

#[test]
fn temporary_names_carry_no_pid_and_never_repeat() {
    let pid = std::process::id().to_string();
    let names: Vec<String> = (0..64)
        .map(|_| {
            crate::tmp_sibling(Path::new("d/flow.nika"))
                .1
                .file_name()
                .expect("name")
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    for name in &names {
        let random = name.strip_prefix(".nika-tmp.").expect("the hidden prefix");
        assert_eq!(random.len(), 16, "{name}");
        assert!(random.chars().all(|c| c.is_ascii_hexdigit()), "{name}");
        assert!(!name.contains(&format!(".{pid}.")), "{name}");
    }
    let mut unique = names.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), names.len());
}

#[tokio::test]
async fn the_host_write_never_writes_through_a_planted_temporary_name() {
    let room = Room::new();
    let planted = room.plant("out.txt");
    let destination = room.folder.join("out.txt");
    for round in [b"first\n".as_slice(), b"second\n".as_slice()] {
        TokioFs.write(&destination, round).await.expect("write");
        assert_eq!(std::fs::read(&destination).expect("read"), round);
        room.untouched(&planted, &destination);
    }
}

#[test]
fn an_owned_directory_write_is_never_blocked_by_a_planted_name() {
    let room = Room::new();
    let planted = room.plant("state.json");
    let destination = room.folder.join("state.json");
    let dir = OwnedDir::open(&room.folder).expect("held");
    for round in ["{\"a\":1}\n", "{\"a\":2}\n"] {
        dir.write_atomic("state.json", round).expect("write");
        assert_eq!(std::fs::read_to_string(&destination).expect("read"), round);
        room.untouched(&planted, &destination);
    }
}
