// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! Where a path below a folder really is, found one name at a time.
//!
//! `canonicalize` says where a chain of links ends, in one call. A chain can
//! leave a folder and come back (`a -> /tmp/hop`, `/tmp/hop -> folder/b`):
//! judged by its end it stays inside, yet the hop outside is a link that
//! whoever can write there may change between the look and the use. Here
//! every link is read with `read_link` and its target walked name by name,
//! so each place the way passes is seen, the links on it included, and a
//! way that passes anywhere outside the folder stops there.
//!
//! The folders above the folder are part of every way down to it (`/var`
//! is a link to `/private/var` on macOS): the walk may pass through them,
//! as they really are or as the caller spelled the folder, and through
//! nothing else outside. What it returns is the real place, with no link
//! left in it, so a caller that writes there follows no link again.
//!
//! This is a mechanism: the caller decides what an [`Escape`] means. It
//! looks by path, so a folder inside that is swapped for a link after the
//! walk is not seen; only a walk held by descriptors would see it.

use std::ffi::OsString;
use std::io;
use std::path::{Component, Path, PathBuf};

/// The most links one walk follows. Past it the way is a loop, or a chain
/// longer than the system itself follows (32 links on macOS).
pub const MAX_LINKS: usize = 32;

/// Where [`resolve_beneath`] found a path below its folder.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Beneath {
    /// Every name is there: the real path, inside the folder, with no link
    /// left in it.
    There(PathBuf),
    /// A name of the path itself is not there (no link leads to it).
    Missing {
        /// The real folder, inside the folder walked, that holds the first
        /// missing name: creating the names there creates the path.
        folder: PathBuf,
        /// The missing names, the first missing one first.
        rest: PathBuf,
    },
}

/// Why [`resolve_beneath`] stopped. Each place is where the walk stood: the
/// real folder it was in, joined with the name it met.
#[derive(Debug)]
#[non_exhaustive]
pub enum Escape {
    /// The way leaves the folder, through the link at this place (or, when
    /// no link is involved, at the path itself: a `..` that climbs out).
    Outside(PathBuf),
    /// The link at this place leads to nothing: its target, or a folder on
    /// the way to it, is not there.
    Dangling(PathBuf),
    /// More than [`MAX_LINKS`] links; the last one met is at this place.
    Loop(PathBuf),
    /// This place could not be read.
    Io(PathBuf, io::Error),
}

/// One move of the walk.
enum Step {
    /// The root an absolute link target starts from.
    Root(PathBuf),
    /// `..`: the folder above the real place the walk stands in.
    Up,
    /// The next name.
    Name(OsString),
    /// The end of the target of the innermost link being walked.
    Close,
}

/// Walk `path`, made of plain names, from `folder` down, the way the
/// system resolves it but one name at a time: each link is read
/// (`read_link`) and its target walked in turn, at most [`MAX_LINKS`] of
/// them. The walk may stand inside `folder` (as it really is) or in one of
/// the folders above it, really or as `folder` is spelled when that is an
/// absolute path; a link or a name anywhere else ends it. A link whose
/// target is not there is never followed to a creation.
///
/// # Errors
/// [`Escape::Outside`] for a way that leaves `folder` (or ends above it),
/// [`Escape::Dangling`], [`Escape::Loop`], or [`Escape::Io`] for an entry
/// that could not be read (`folder` itself included).
pub fn resolve_beneath(folder: &Path, path: &Path) -> Result<Beneath, Escape> {
    let home = folder
        .canonicalize()
        .map_err(|e| Escape::Io(folder.to_path_buf(), e))?;
    let named = folder.is_absolute().then_some(folder);
    let mut todo = Vec::new();
    for part in path.components().rev() {
        match part {
            Component::Normal(name) => todo.push(Step::Name(name.to_owned())),
            Component::CurDir => {}
            _ => return Err(Escape::Outside(folder.join(path))),
        }
    }
    let passable = |at: &Path| {
        at.starts_with(&home) || home.starts_with(at) || named.is_some_and(|n| n.starts_with(at))
    };
    let mut real = home.clone();
    // The links whose targets are being walked, innermost last, and the
    // last link followed.
    let mut open: Vec<PathBuf> = Vec::new();
    let mut last: Option<PathBuf> = None;
    let mut links = 0usize;
    while let Some(step) = todo.pop() {
        let name = match step {
            Step::Close => {
                open.pop();
                continue;
            }
            Step::Root(root) => {
                real = root;
                continue;
            }
            Step::Up => {
                real.pop();
                continue;
            }
            Step::Name(name) => name,
        };
        let next = real.join(&name);
        if !passable(&next) {
            return Err(Escape::Outside(
                open.last().or(last.as_ref()).cloned().unwrap_or(next),
            ));
        }
        let meta = match std::fs::symlink_metadata(&next) {
            Ok(meta) => meta,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                if let Some(link) = open.last() {
                    return Err(Escape::Dangling(link.clone()));
                }
                // No link is being walked: only the path's own names are left.
                let mut rest = PathBuf::from(name);
                rest.extend(todo.into_iter().rev().filter_map(|step| match step {
                    Step::Name(name) => Some(name),
                    Step::Root(_) | Step::Up | Step::Close => None,
                }));
                return Ok(Beneath::Missing { folder: real, rest });
            }
            Err(e) => return Err(Escape::Io(next, e)),
        };
        if !meta.file_type().is_symlink() {
            real = next;
            continue;
        }
        links += 1;
        if links > MAX_LINKS {
            return Err(Escape::Loop(next));
        }
        let target = std::fs::read_link(&next).map_err(|e| Escape::Io(next.clone(), e))?;
        todo.push(Step::Close);
        todo.extend(steps(&target).into_iter().rev());
        open.push(next.clone());
        last = Some(next);
    }
    if real.starts_with(&home) {
        Ok(Beneath::There(real))
    } else {
        Err(Escape::Outside(last.unwrap_or_else(|| folder.join(path))))
    }
}

/// The moves a link target makes, in order: an absolute target starts
/// again from its root.
fn steps(target: &Path) -> Vec<Step> {
    let mut root = PathBuf::new();
    let mut steps = Vec::new();
    for part in target.components() {
        match part {
            Component::Prefix(_) | Component::RootDir => root.push(part.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => steps.push(Step::Up),
            Component::Normal(name) => steps.push(Step::Name(name.to_owned())),
        }
    }
    if !root.as_os_str().is_empty() {
        steps.insert(0, Step::Root(root));
    }
    steps
}
