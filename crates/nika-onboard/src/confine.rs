// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! Every file `nika init` writes stays inside the project it founds.
//!
//! A repository can hold links: a settings file that points at the user's
//! global one, a `.claude` folder that leads elsewhere, a dangling link
//! where init would create a file. Writing by path follows them. Here a
//! write never does:
//!
//! - a link at the file's own name is never written through, `--force`
//!   included;
//! - every link on the way is read one step at a time
//!   ([`nika_fs::resolve_beneath`]) and followed only while each place it
//!   leads through is inside the project, or one of the folders above it on
//!   the way down: a way that leaves the project stops the write before
//!   anything is made beyond it, even when it comes back in, since a link
//!   outside is its owner's to turn elsewhere between the look and the
//!   write;
//! - the bytes go to a new file in the real folder that walk reached,
//!   created under a random name that must not exist yet, then renamed over
//!   the name, so nothing outside is ever opened for writing and no link is
//!   followed again; a file it replaces keeps its mode (a shell script is
//!   always made executable).
//!
//! Reads ([`read_text`]) are bounded, never wait (a FIFO is refused, not
//! read) and never go through a link at the file's own name.
//!
//! One gap remains: the walk looks by path, then the new file is made in
//! the real folder by path. A process running beside `init` that swaps a
//! real folder inside the project for a link in between could still
//! redirect the write. Closing it needs every step held by descriptor (an
//! `openat` walk), which this module does not do yet.
//!
//! A refusal is an [`io::Error`] carrying a [`Refusal`]: every writer
//! already reports an I/O failure as its ✖ row and exit 3, so a refusal
//! lands in the same place, with its own remedy and no `--force` advice.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

/// Why a path is refused. The text is the row's remedy: `--force` changes
/// none of these, so none of them names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// The name itself is a link (dangling, leading outside, or met under
    /// `--force`).
    Link,
    /// A folder on the way resolves outside the project.
    Outside,
    /// A link on the way, at a folder or at the name, never ends: it leads
    /// back to itself, or through more links than a walk follows
    /// ([`nika_fs::MAX_LINKS`]).
    Loop,
    /// Something other than a regular file stands at the name.
    NotRegular,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Link => {
                "a link — init never writes through a link, --force included · remove the link, then run nika init again"
            }
            Self::Outside => {
                "its folder leads outside the project through a link — init writes only inside the project, --force included · make that folder a real one inside the project, then run nika init again"
            }
            Self::Loop => {
                "a link loop on its way — following it never ends, and init writes only through links that end, --force included · remove the loop, then run nika init again"
            }
            Self::NotRegular => {
                "not a regular file — init writes only regular files, --force included · remove it, then run nika init again"
            }
        })
    }
}

impl std::error::Error for Refusal {}

impl Refusal {
    /// The note of a row that keeps what stands at a name init has nothing
    /// to write to (a file it would only skip): no write is needed, so it
    /// is left as it is and never followed or read.
    pub(crate) const fn kept(self) -> &'static str {
        match self {
            Self::Link => "a link leading out of the project or to nothing · kept, never followed",
            Self::Outside => "its folder leads out of the project · kept, never followed",
            Self::Loop => "a link loop on its way · kept, never followed",
            Self::NotRegular => "not a regular file · kept as it is",
        }
    }
}

fn refused(why: Refusal) -> io::Error {
    io::Error::other(why)
}

/// The refusal an error carries, when it is one.
pub(crate) fn refusal(e: &io::Error) -> Option<Refusal> {
    e.get_ref()
        .and_then(|inner| inner.downcast_ref::<Refusal>())
        .copied()
}

/// The project folder as a path the checks can compare against (an empty
/// one is the working folder).
fn base(root: &Path) -> &Path {
    if root.as_os_str().is_empty() {
        Path::new(".")
    } else {
        root
    }
}

/// The part of `path` below `root`, made of plain names only.
fn below<'a>(root: &Path, path: &'a Path) -> io::Result<&'a Path> {
    let rel = path
        .strip_prefix(root)
        .or_else(|_| path.strip_prefix(base(root)))
        .map_err(|_| refused(Refusal::Outside))?;
    if rel
        .components()
        .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
    {
        Ok(rel)
    } else {
        Err(refused(Refusal::Outside))
    }
}

/// Where the link at `at` (a real place inside `home`, the project as it
/// is) leads, found one step at a time: a real place inside the project,
/// reached only through places inside it or above it. A way that leaves
/// and a link to nothing are both `why`; a loop is [`Refusal::Loop`].
fn followed(root: &Path, home: &Path, at: &Path, why: Refusal) -> io::Result<PathBuf> {
    let rest = at.strip_prefix(home).map_err(|_| refused(why))?;
    match nika_fs::resolve_beneath(base(root), rest) {
        Ok(nika_fs::Beneath::There(real)) => Ok(real),
        Err(nika_fs::Escape::Io(_, e)) => Err(e),
        Err(nika_fs::Escape::Loop(_)) => Err(refused(Refusal::Loop)),
        // A way out (and back), a link to nothing.
        Ok(_) | Err(_) => Err(refused(why)),
    }
}

/// Walk the folders from `root` down to `folder`, one name at a time from
/// the project as it really is (`home`): a real folder is entered, a link
/// is followed only as [`followed`] allows. With `make`, a missing folder is
/// created in the real folder above it (one level at a time); without it
/// the walk stops at the first missing one, below which nothing exists yet
/// (`None`). What comes back is the real folder `folder` is.
fn folders(root: &Path, home: &Path, folder: &Path, make: bool) -> io::Result<Option<PathBuf>> {
    let mut real = home.to_path_buf();
    for part in below(root, folder)?.components() {
        let Component::Normal(part) = part else {
            continue;
        };
        let next = real.join(part);
        let meta = match fs::symlink_metadata(&next) {
            Err(e) if e.kind() == io::ErrorKind::NotFound && make => match fs::create_dir(&next) {
                Ok(()) => {
                    real = next;
                    continue;
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => fs::symlink_metadata(&next)?,
                Err(e) => return Err(e),
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            other => other?,
        };
        real = if meta.is_dir() {
            next
        } else if meta.file_type().is_symlink() {
            followed(root, home, &next, Refusal::Outside)?
        } else {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                format!("{} is a file where init needs a folder", next.display()),
            ));
        };
    }
    Ok(Some(real))
}

/// The project as it really is, or `None` while it is still to be made.
fn home(root: &Path) -> io::Result<Option<PathBuf>> {
    match base(root).canonicalize() {
        Ok(home) => Ok(Some(home)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// The real regular file standing at `path` under `root`, or `None` when
/// nothing does. A link at the name counts only when it leads, by places
/// inside the project or above it, to a regular file inside the project.
fn standing(root: &Path, path: &Path) -> io::Result<Option<PathBuf>> {
    // A project folder still to be made holds nothing yet.
    let Some(home) = home(root)? else {
        return Ok(None);
    };
    let (Some(folder), Some(name)) = (path.parent(), path.file_name()) else {
        return Err(refused(Refusal::Outside));
    };
    let Some(real) = folders(root, &home, folder, false)? else {
        return Ok(None);
    };
    let at = real.join(name);
    match fs::symlink_metadata(&at) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
        Ok(m) if m.file_type().is_symlink() => {
            let file = followed(root, &home, &at, Refusal::Link)?;
            match fs::symlink_metadata(&file) {
                Ok(m) if m.is_file() => Ok(Some(file)),
                _ => Err(refused(Refusal::Link)),
            }
        }
        Ok(m) if m.is_file() => Ok(Some(at)),
        Ok(_) => Err(refused(Refusal::NotRegular)),
    }
}

/// What stands at `path`, a file init may write under `root`: `false` when
/// nothing does, `true` for a regular file inside the project (reached
/// directly, or through a link that stays inside, which init keeps but
/// never writes through). Anything else is a [`Refusal`]: a dangling link,
/// a link or a folder whose way leaves the project, something not a
/// regular file.
///
/// # Errors
/// The refusal, or the I/O failure that kept the path from being read.
pub(crate) fn occupied(root: &Path, path: &Path) -> io::Result<bool> {
    standing(root, path).map(|file| file.is_some())
}

/// Whether `path` is a link to a regular file inside the project
/// (`CLAUDE.md` pointing at `AGENTS.md`): init keeps it as it is and never
/// writes through it, `--force` included.
#[must_use]
pub(crate) fn link_inside(root: &Path, path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
        && occupied(root, path).unwrap_or(false)
}

/// Whether `path` is a regular file inside the project, reached without a
/// link at its own name: the only kind of file init counts as its own.
#[must_use]
pub(crate) fn plain_file(root: &Path, path: &Path) -> bool {
    occupied(root, path).unwrap_or(false)
        && fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_file())
}

/// The most [`read_text`] reads from one file the project holds.
pub(crate) const READ_CAP: u64 = 4 * 1024 * 1024;

/// The text of the regular file `path` inside `root`, at most `cap` bytes of
/// UTF-8 (a larger file is an error, never cut), or `None` when nothing
/// stands there. The open never waits (a FIFO swapped in is refused, not
/// read) and never follows a link at the file's own name: a link that
/// [`occupied`] accepts (a file inside the project) is read at the file it
/// reaches.
///
/// # Errors
/// A [`Refusal`] from [`occupied`], or a failed, oversized or non-UTF-8 read.
pub(crate) fn read_text(root: &Path, path: &Path, cap: u64) -> io::Result<Option<String>> {
    match standing(root, path)? {
        Some(file) => nika_fs::read_owned(&file, cap),
        None => Ok(None),
    }
}

/// Write `body` to `path` inside `root`, creating the folders on the way.
/// A file it replaces keeps its mode; shell scripts (`.sh`, the scaffolded
/// hooks) are always made executable on unix (Cursor spawns them
/// directly), and a new file of any other kind gets the mode
/// `std::fs::write` gave it (`0666` under the umask). No sync is added, as
/// before.
///
/// # Errors
/// A [`Refusal`] (a link at the name, a folder leading outside, something
/// not a regular file there), or the I/O failure of the write.
pub(crate) fn write(root: &Path, path: &Path, body: &[u8]) -> io::Result<()> {
    // The project folder is the caller's own choice, links included.
    fs::create_dir_all(base(root))?;
    let home = base(root).canonicalize()?;
    let (Some(folder), Some(name)) = (path.parent(), path.file_name()) else {
        return Err(refused(Refusal::Outside));
    };
    let real = folders(root, &home, folder, true)?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("{}: no folder", folder.display()),
        )
    })?;
    // The real folder the walk reached: no link on the way is followed
    // again by the write.
    let target = real.join(name);
    match fs::symlink_metadata(&target) {
        Ok(m) if m.file_type().is_symlink() => return Err(refused(Refusal::Link)),
        Ok(m) if !m.is_file() => return Err(refused(Refusal::NotRegular)),
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        // A regular file to replace, or nothing there yet.
        Ok(_) | Err(_) => {}
    }
    // A new file under a random name (`O_EXCL`, which never follows a
    // link), renamed over the name: a name planted in advance cannot catch
    // the bytes, and the temporary is removed again if anything fails. The
    // kept mode carries the permission bits only, never a set-id bit.
    nika_fs::replace_file(&target, body, mode(path), nika_fs::Durability::Unsynced)
}

/// The mode the written file ends with: a shell script's is `0755`, a
/// replaced file keeps its own, a new one is `0666` under the umask.
fn mode(path: &Path) -> nika_fs::ReplaceMode {
    let script = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("sh"));
    if script {
        nika_fs::ReplaceMode::Exactly(0o755)
    } else {
        nika_fs::ReplaceMode::KeepOr(0o666)
    }
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    /// A project folder and an outside folder beside it, both canonical.
    fn rooms(tag: &str) -> (PathBuf, PathBuf) {
        let top = std::env::temp_dir().join(format!("nika-confine-{tag}-{}", std::process::id()));
        fs::remove_dir_all(&top).ok();
        fs::create_dir_all(top.join("project")).expect("project");
        fs::create_dir_all(top.join("outside")).expect("outside");
        let top = top.canonicalize().expect("canonical");
        (top.join("project"), top.join("outside"))
    }

    fn why(result: io::Result<impl fmt::Debug>) -> Option<Refusal> {
        match result {
            Ok(v) => panic!("expected a refusal, got {v:?}"),
            Err(e) => refusal(&e),
        }
    }

    #[test]
    fn a_plain_write_creates_its_folders_and_the_file() {
        let (project, _) = rooms("plain");
        let target = project.join(".claude/hooks-nika/check.sh");
        assert!(!occupied(&project, &target).expect("probe"));
        write(&project, &target, b"#!/bin/sh\n").expect("write");
        assert_eq!(fs::read(&target).expect("read"), b"#!/bin/sh\n");
        let mode = fs::metadata(&target).expect("meta").permissions().mode() & 0o777;
        assert_eq!(mode, 0o755, "a hook script is executable");
        assert!(occupied(&project, &target).expect("probe"));
        assert!(plain_file(&project, &target));
        let left: Vec<_> = fs::read_dir(target.parent().expect("folder"))
            .expect("list")
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(left, ["check.sh"], "no spare file is left behind");
    }

    #[test]
    fn a_dangling_link_is_refused_and_its_target_never_made() {
        let (project, outside) = rooms("dangling");
        fs::create_dir_all(project.join(".claude")).expect("folder");
        let link = project.join(".claude/settings.json");
        symlink(outside.join("settings.json"), &link).expect("link");
        assert_eq!(why(occupied(&project, &link)), Some(Refusal::Link));
        assert_eq!(why(write(&project, &link, b"{}")), Some(Refusal::Link));
        assert!(
            !outside.join("settings.json").exists(),
            "nothing written outside"
        );
        assert!(!plain_file(&project, &link));
    }

    #[test]
    fn a_link_to_an_outside_file_is_refused_and_the_file_untouched() {
        let (project, outside) = rooms("outlink");
        fs::write(outside.join("global.json"), b"theirs").expect("global");
        let link = project.join("settings.json");
        symlink(outside.join("global.json"), &link).expect("link");
        assert_eq!(why(occupied(&project, &link)), Some(Refusal::Link));
        assert_eq!(why(write(&project, &link, b"ours")), Some(Refusal::Link));
        assert_eq!(
            fs::read(outside.join("global.json")).expect("read"),
            b"theirs"
        );
    }

    #[test]
    fn a_link_inside_the_project_is_kept_but_never_written_through() {
        let (project, _) = rooms("inlink");
        fs::write(project.join("AGENTS.md"), b"the human's").expect("agents");
        let link = project.join("CLAUDE.md");
        symlink("AGENTS.md", &link).expect("link");
        assert!(occupied(&project, &link).expect("an inside link counts as there"));
        assert!(
            !plain_file(&project, &link),
            "a link is never init's own file"
        );
        assert_eq!(why(write(&project, &link, b"ours")), Some(Refusal::Link));
        assert_eq!(
            fs::read(project.join("AGENTS.md")).expect("read"),
            b"the human's"
        );
    }

    #[test]
    fn a_folder_leading_outside_is_refused_before_anything_is_made_there() {
        let (project, outside) = rooms("folder");
        symlink(&outside, project.join(".claude")).expect("link");
        let target = project.join(".claude/hooks-nika/check.sh");
        assert_eq!(why(occupied(&project, &target)), Some(Refusal::Outside));
        assert_eq!(why(write(&project, &target, b"x")), Some(Refusal::Outside));
        assert!(
            !outside.join("hooks-nika").exists(),
            "no folder made outside"
        );
        fs::write(outside.join("settings.json"), b"global").expect("global");
        let settings = project.join(".claude/settings.json");
        assert_eq!(why(occupied(&project, &settings)), Some(Refusal::Outside));
        assert_eq!(
            why(write(&project, &settings, b"x")),
            Some(Refusal::Outside)
        );
        assert_eq!(
            fs::read(outside.join("settings.json")).expect("read"),
            b"global"
        );
    }

    #[test]
    fn a_folder_link_that_stays_inside_is_followed() {
        let (project, _) = rooms("infolder");
        fs::create_dir_all(project.join("config/cursor")).expect("real");
        symlink("config/cursor", project.join(".cursor")).expect("link");
        let target = project.join(".cursor/hooks.json");
        write(&project, &target, b"{}").expect("an inside folder link is fine");
        assert_eq!(
            fs::read(project.join("config/cursor/hooks.json")).expect("read"),
            b"{}"
        );
    }

    /// A folder link that points inside the project, at a link that leads
    /// back out, is judged where it ends: outside.
    #[test]
    fn a_folder_link_inside_that_leads_back_out_is_refused() {
        let (project, outside) = rooms("backout");
        symlink(&outside, project.join("shared")).expect("inner link out");
        symlink("shared", project.join(".claude")).expect("link to the inner link");
        let target = project.join(".claude/settings.json");
        assert_eq!(why(occupied(&project, &target)), Some(Refusal::Outside));
        assert_eq!(why(write(&project, &target, b"{}")), Some(Refusal::Outside));
        let deeper = project.join(".claude/hooks-nika/guard-run.sh");
        assert_eq!(why(write(&project, &deeper, b"x")), Some(Refusal::Outside));
        assert_eq!(
            fs::read_dir(&outside).expect("list").count(),
            0,
            "nothing made outside"
        );
    }

    /// A way that leaves the project and comes back (`.claude ->
    /// <outside>/hop`, `hop -> <project>/real`) ends inside when resolved in
    /// one call, yet the hop outside is a link its owner can turn elsewhere
    /// between the look and the write. It is refused where it leaves, at a
    /// folder and at a file's own name, and nothing is written in what it
    /// reaches.
    #[test]
    fn a_way_out_and_back_in_is_refused_where_it_leaves() {
        let (project, outside) = rooms("hop");
        fs::create_dir_all(project.join("real")).expect("real");
        symlink(project.join("real"), outside.join("hop")).expect("hop back in");
        symlink(outside.join("hop"), project.join(".claude")).expect("out");
        let target = project.join(".claude/settings.json");
        assert!(
            project
                .join(".claude")
                .canonicalize()
                .expect("one call")
                .starts_with(&project),
            "resolved in one call, the way ends inside"
        );
        assert_eq!(why(occupied(&project, &target)), Some(Refusal::Outside));
        assert_eq!(why(write(&project, &target, b"{}")), Some(Refusal::Outside));
        let deeper = project.join(".claude/hooks-nika/guard-run.sh");
        assert_eq!(why(write(&project, &deeper, b"x")), Some(Refusal::Outside));
        assert_eq!(
            fs::read_dir(project.join("real")).expect("list").count(),
            0,
            "nothing written where the way comes back"
        );

        fs::write(project.join("AGENTS.md"), b"the human's").expect("agents");
        symlink(project.join("AGENTS.md"), outside.join("hop.md")).expect("hop back in");
        let name = project.join("CLAUDE.md");
        symlink(outside.join("hop.md"), &name).expect("out, by file");
        assert_eq!(why(occupied(&project, &name)), Some(Refusal::Link));
        assert!(
            !link_inside(&project, &name),
            "never kept as an inside link"
        );
        assert_eq!(
            why(read_text(&project, &name, READ_CAP)),
            Some(Refusal::Link)
        );
        assert_eq!(why(write(&project, &name, b"ours")), Some(Refusal::Link));
        assert_eq!(
            fs::read(project.join("AGENTS.md")).expect("read"),
            b"the human's"
        );
    }

    /// Links that stay inside, however many, are followed, and the write
    /// lands in the real folder they reach.
    #[test]
    fn a_chain_of_links_inside_is_followed_to_the_real_folder() {
        let (project, _) = rooms("chain");
        fs::create_dir_all(project.join("shared/claude")).expect("real");
        symlink("hop", project.join(".claude")).expect("first hop");
        symlink(project.join("shared/claude"), project.join("hop")).expect("second hop");
        let target = project.join(".claude/hooks-nika/guard-run.sh");
        write(&project, &target, b"#!/bin/sh\n").expect("inside all the way");
        assert_eq!(
            fs::read(project.join("shared/claude/hooks-nika/guard-run.sh")).expect("read"),
            b"#!/bin/sh\n"
        );
        assert!(
            plain_file(&project, &target),
            "reached through inside links"
        );
    }

    #[test]
    fn a_dangling_folder_link_is_refused() {
        let (project, outside) = rooms("deadfolder");
        symlink(outside.join("gone"), project.join(".claude")).expect("link");
        let target = project.join(".claude/settings.json");
        assert_eq!(why(write(&project, &target, b"x")), Some(Refusal::Outside));
        assert!(!outside.join("gone").exists());
    }

    #[test]
    fn a_hard_link_is_replaced_not_written_through() {
        let (project, outside) = rooms("hard");
        fs::write(outside.join("shared"), b"theirs").expect("shared");
        let name = project.join("nika.yaml");
        fs::hard_link(outside.join("shared"), &name).expect("hard link");
        assert!(occupied(&project, &name).expect("a regular file"));
        write(&project, &name, b"ours").expect("rewrite");
        assert_eq!(fs::read(&name).expect("read"), b"ours");
        assert_eq!(fs::read(outside.join("shared")).expect("read"), b"theirs");
    }

    #[test]
    fn something_not_a_regular_file_is_refused() {
        let (project, _) = rooms("notfile");
        fs::create_dir_all(project.join("AGENTS.md")).expect("a folder named like a file");
        let name = project.join("AGENTS.md");
        assert_eq!(why(occupied(&project, &name)), Some(Refusal::NotRegular));
        assert_eq!(why(write(&project, &name, b"x")), Some(Refusal::NotRegular));
    }

    #[test]
    fn a_path_climbing_out_of_the_project_is_refused() {
        let (project, outside) = rooms("climb");
        let target = project.join("../outside/x");
        assert_eq!(why(write(&project, &target, b"x")), Some(Refusal::Outside));
        assert!(!outside.join("x").exists());
    }

    #[test]
    fn a_project_still_to_be_made_is_made_and_written() {
        let (project, _) = rooms("fresh");
        let fresh = project.join("new/dir");
        let target = fresh.join(".gitignore");
        assert!(!occupied(&fresh, &target).expect("nothing yet"));
        write(&fresh, &target, b"x").expect("write");
        assert_eq!(fs::read(&target).expect("read"), b"x");
    }

    /// A link loop that stays inside the project, at a folder (`.claude ->
    /// loop -> .claude`) or at a file's own name (`CLAUDE.md -> a.md ->
    /// CLAUDE.md`), is refused as a loop, never as a way out of the
    /// project, wherever it is followed (the probe, the read, a write
    /// through the folder), and nothing is made.
    #[test]
    fn a_link_loop_inside_the_project_is_refused_as_one() {
        let (project, outside) = rooms("loop");
        symlink("loop", project.join(".claude")).expect("to loop");
        symlink(".claude", project.join("loop")).expect("back to .claude");
        let target = project.join(".claude/settings.json");
        assert_eq!(why(occupied(&project, &target)), Some(Refusal::Loop));
        assert_eq!(
            why(read_text(&project, &target, READ_CAP)),
            Some(Refusal::Loop)
        );
        assert_eq!(why(write(&project, &target, b"{}")), Some(Refusal::Loop));
        let deeper = project.join(".claude/hooks-nika/guard-run.sh");
        assert_eq!(why(write(&project, &deeper, b"x")), Some(Refusal::Loop));

        let name = project.join("CLAUDE.md");
        symlink("a.md", &name).expect("to a.md");
        symlink("CLAUDE.md", project.join("a.md")).expect("back to CLAUDE.md");
        assert_eq!(why(occupied(&project, &name)), Some(Refusal::Loop));
        assert!(
            !link_inside(&project, &name),
            "never kept as an inside link"
        );
        // The write refuses a link at the name before following it at all.
        assert_eq!(why(write(&project, &name, b"ours")), Some(Refusal::Link));

        let mut left: Vec<_> = fs::read_dir(&project)
            .expect("list")
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(
            left,
            [".claude", "CLAUDE.md", "a.md", "loop"],
            "nothing made"
        );
        assert_eq!(fs::read_dir(&outside).expect("list").count(), 0);
        let text = Refusal::Loop.to_string();
        assert!(text.starts_with("a link loop"), "{text}");
        assert!(!text.contains("outside"), "{text}");
        assert_eq!(
            Refusal::Loop.kept(),
            "a link loop on its way · kept, never followed"
        );
    }

    #[test]
    fn the_refusals_never_advise_force() {
        for why in [
            Refusal::Link,
            Refusal::Outside,
            Refusal::Loop,
            Refusal::NotRegular,
        ] {
            let text = why.to_string();
            assert!(text.contains("--force included"), "{text}");
            assert!(!text.contains("rerun with --force"), "{text}");
            assert!(text.contains("run nika init again"), "{text}");
        }
    }

    fn mode_of(path: &Path) -> u32 {
        fs::metadata(path).expect("meta").permissions().mode() & 0o7777
    }

    #[test]
    fn a_replaced_file_keeps_its_mode_and_a_script_stays_executable() {
        let (project, _) = rooms("modes");
        let ignore = project.join(".gitignore");
        fs::write(&ignore, b"target/\n").expect("seed");
        fs::set_permissions(&ignore, fs::Permissions::from_mode(0o640)).expect("chmod");
        write(&project, &ignore, b"target/\n.nika/traces/\n").expect("write");
        assert_eq!(
            fs::read(&ignore).expect("read"),
            b"target/\n.nika/traces/\n"
        );
        assert_eq!(
            mode_of(&ignore),
            0o640,
            "a rewritten .gitignore keeps its mode"
        );
        let fresh = project.join("nika.yaml");
        write(&project, &fresh, b"x").expect("write");
        // The mode `std::fs::write` gives a new file (0666 under the umask).
        let reference = project.join("reference");
        fs::write(&reference, b"x").expect("reference");
        assert_eq!(mode_of(&fresh), mode_of(&reference));
        let script = project.join("hook.sh");
        fs::write(&script, b"old").expect("seed");
        fs::set_permissions(&script, fs::Permissions::from_mode(0o600)).expect("chmod");
        write(&project, &script, b"#!/bin/sh\n").expect("write");
        assert_eq!(
            mode_of(&script),
            0o755,
            "a rewritten script is executable again"
        );
    }

    #[test]
    fn a_planted_spare_name_never_catches_the_bytes() {
        let (project, outside) = rooms("spare");
        let victim = outside.join("victim");
        fs::write(&victim, b"theirs").expect("victim");
        fs::set_permissions(&victim, fs::Permissions::from_mode(0o644)).expect("chmod");
        let folder = project.join(".claude/hooks-nika");
        fs::create_dir_all(&folder).expect("mkdir");
        // The pid-shaped names earlier writers used (`.<name>.nika-init-<pid>-<n>`,
        // `.nika-tmp.<pid>.<n>`), and a dangling one beside them.
        for n in 0..64 {
            let pid = std::process::id();
            symlink(
                &victim,
                folder.join(format!(".check.sh.nika-init-{pid}-{n}")),
            )
            .expect("plant");
            symlink(&victim, folder.join(format!(".nika-tmp.{pid}.{n}"))).expect("plant");
        }
        symlink(outside.join("made"), folder.join(".check.sh.nika-init-")).expect("plant");
        let target = folder.join("check.sh");
        write(&project, &target, b"#!/bin/sh\n").expect("write");
        write(&project, &target, b"#!/bin/sh\necho\n").expect("rewrite");
        assert_eq!(fs::read(&target).expect("read"), b"#!/bin/sh\necho\n");
        assert_eq!(fs::read(&victim).expect("victim"), b"theirs");
        assert_eq!(mode_of(&victim), 0o644);
        assert!(
            !outside.join("made").exists(),
            "nothing is made where a planted name leads"
        );
        let left = fs::read_dir(&folder)
            .expect("list")
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(left, ["check.sh"], "no spare file is left behind");
    }

    #[test]
    fn reads_are_bounded_and_never_open_anything_but_a_regular_file() {
        let (project, outside) = rooms("reads");
        let plain = project.join(".gitignore");
        assert_eq!(read_text(&project, &plain, READ_CAP).expect("absent"), None);
        fs::write(&plain, "target/\n").expect("seed");
        assert_eq!(
            read_text(&project, &plain, READ_CAP)
                .expect("read")
                .as_deref(),
            Some("target/\n")
        );
        let over = read_text(&project, &plain, 3).expect_err("over the cap");
        assert_eq!(over.kind(), io::ErrorKind::InvalidData, "{over}");
        // A link inside the project is read at the file it reaches.
        let inner = project.join("ignore-rules");
        fs::write(&inner, ".nika/\n").expect("inner");
        let linked = project.join("linked/.gitignore");
        fs::create_dir_all(linked.parent().expect("folder")).expect("mkdir");
        symlink(&inner, &linked).expect("link");
        assert_eq!(
            read_text(&project, &linked, READ_CAP)
                .expect("read")
                .as_deref(),
            Some(".nika/\n")
        );
        // A link leading out and a node that is not a regular file are
        // refused before any open.
        fs::write(outside.join("secret"), "theirs").expect("secret");
        let away = project.join("away");
        symlink(outside.join("secret"), &away).expect("link");
        assert_eq!(
            why(read_text(&project, &away, READ_CAP)),
            Some(Refusal::Link)
        );
        let folder = project.join("nika.yaml");
        fs::create_dir_all(&folder).expect("a folder at a file's name");
        assert_eq!(
            why(read_text(&project, &folder, READ_CAP)),
            Some(Refusal::NotRegular)
        );
    }
}
