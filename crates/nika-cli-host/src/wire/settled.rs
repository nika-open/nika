// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! Where a config write lands, found at the check and looked at again just
//! before the write: a config whose landing changed in between is refused.

use std::path::{Path, PathBuf};

/// Where a write to `path` lands, as a real path: the file it reaches,
/// through any link, when that is there; else the real folder it would be
/// created in, with the names below it (a link at the name that leads
/// nowhere is replaced there, never followed).
pub(super) fn landing(path: &Path) -> std::io::Result<PathBuf> {
    let mut below = Vec::new();
    let mut at = path;
    loop {
        match std::fs::canonicalize(at) {
            Ok(real) => return Ok(below.iter().rev().fold(real, |real, name| real.join(name))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let (Some(name), Some(up)) = (at.file_name(), at.parent()) else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("{}: no folder of it is there", path.display()),
            ));
        };
        below.push(name.to_owned());
        at = if up.as_os_str().is_empty() {
            Path::new(".")
        } else {
            up
        };
    }
}

thread_local! {
    /// The config a run is writing, with where [`super::settled_path`] found a
    /// write to it lands, while [`Settled`] holds it.
    static SETTLED: std::cell::RefCell<Option<(PathBuf, PathBuf)>> =
        const { std::cell::RefCell::new(None) };
}

/// While alive, a write to the config [`super::settled_path`] checked lands only
/// where it found it would: [`super::write_atomic`] looks again just before the
/// write and refuses any other place. Dropped, the hold ends.
pub(super) struct Settled;

impl Settled {
    pub(super) fn hold(path: &Path, lands: PathBuf) -> Self {
        SETTLED.with_borrow_mut(|held| *held = Some((path.to_path_buf(), lands)));
        Self
    }

    /// Where the held config was found to land, when `path` is the one held.
    pub(super) fn lands(path: &Path) -> Option<PathBuf> {
        SETTLED.with_borrow(|held| {
            held.as_ref()
                .filter(|(at, _)| at == path)
                .map(|(_, lands)| lands.clone())
        })
    }
}

impl Drop for Settled {
    fn drop(&mut self) {
        SETTLED.with_borrow_mut(|held| *held = None);
    }
}
