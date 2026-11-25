// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! Publish a file by writing a temporary beside it, then renaming it over
//! the name: the one way the engine stages a whole file.
//!
//! The guarantee is the open: the temporary is always a NEW file, created
//! with `O_CREAT | O_EXCL` (`create_new`), which never opens an entry that
//! is already there and never follows a link at that name. An entry
//! planted at a temporary name ahead of time can therefore neither
//! receive the bytes nor be renamed over the destination; at worst it
//! makes this write try another name, and fail after a few. The name
//! itself ([`crate::tmp_sibling`]) is hard to guess, which keeps such a
//! collision rare, but that is not promised and nothing here relies on it.
//!
//! The mode is set when the file is created (or through the open file,
//! before any byte is written), the bytes are written through that same
//! handle, synced when the caller asks for it ([`Durability`]), and on any
//! failure the temporary is removed.
//!
//! `rename` replaces whatever stands at the destination, a link
//! included, and never writes through it. Whether a link at the
//! destination, or a linked folder on the way to it, may be followed at
//! all is the caller's decision, taken before it calls; the folder is
//! still named by path here, so a folder swapped for a link between that
//! decision and the rename is not stopped.

use std::fs::{File, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

/// How many fresh names a stage tries before it gives up: a collision
/// on a random name means something is racing for the folder.
const ATTEMPTS: usize = 8;

/// The mode the file that takes the destination's name ends with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ReplaceMode {
    /// These permission bits, narrowed by the process umask, as a new
    /// file gets them; a replaced file's own mode is not kept.
    New(u32),
    /// The replaced regular file's permission bits; with nothing to
    /// replace, [`ReplaceMode::New`] with these bits.
    KeepOr(u32),
    /// Exactly these permission bits, whatever was there.
    Exactly(u32),
}

/// What the stage asks of the disk before the rename publishes the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Durability {
    /// No sync: the rename publishes what the page cache holds, and a
    /// crash right after it can leave the new name empty.
    Unsynced,
    /// `sync_data` on the temporary before the rename.
    Data,
    /// `sync_all` on the temporary before the rename.
    All,
}

/// Write `body` to `path` through a new temporary file beside it, then
/// rename it over `path`.
///
/// The folder must exist. A link at `path` is replaced, never written
/// through; see the module docs for what the caller decides first.
///
/// # Errors
/// `InvalidInput` when `path` names no file; otherwise the I/O failure of
/// the create, mode change, write, sync or rename. The temporary is
/// removed before any error returns.
pub fn replace_file(
    path: &Path,
    body: &[u8],
    mode: ReplaceMode,
    durability: Durability,
) -> io::Result<()> {
    if path.file_name().is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{}: the path names no file", path.display()),
        ));
    }
    let kept = match mode {
        ReplaceMode::KeepOr(_) => std::fs::symlink_metadata(path)
            .ok()
            .filter(std::fs::Metadata::is_file)
            .map(|meta| meta.permissions()),
        ReplaceMode::New(_) | ReplaceMode::Exactly(_) => None,
    };
    let (spare, mut file) = create_spare(path, mode, kept.is_some())?;
    let laid = lay(&mut file, body, mode, kept, durability).and_then(|()| {
        drop(file);
        std::fs::rename(&spare, path)
    });
    if laid.is_err() {
        let _ = std::fs::remove_file(&spare);
    }
    laid
}

/// A new file at a fresh temporary name beside `path`. Only a name this
/// call created is ever handed back, so a cleanup never removes an entry
/// somebody else made.
fn create_spare(path: &Path, mode: ReplaceMode, keeps: bool) -> io::Result<(PathBuf, File)> {
    let mut last = None;
    for _ in 0..ATTEMPTS {
        let (_, spare) = crate::tmp_sibling(path);
        match open_spare(&spare, mode, keeps) {
            Ok(file) => return Ok((spare, file)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => last = Some(e),
            Err(e) => return Err(e),
        }
    }
    Err(last.unwrap_or_else(|| io::Error::from(io::ErrorKind::AlreadyExists)))
}

/// Open `spare` only as a file this call creates (`O_CREAT | O_EXCL`): an
/// entry already at that name, a link included (dangling or not), is never
/// opened or followed and is refused with `AlreadyExists`.
pub(crate) fn open_spare(spare: &Path, mode: ReplaceMode, keeps: bool) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        // Owner-only until the final mode is set through the handle; a new
        // file of `New`/`KeepOr` gets its bits at creation.
        options.mode(match mode {
            ReplaceMode::New(bits) => bits & 0o777,
            ReplaceMode::KeepOr(bits) if !keeps => bits & 0o777,
            ReplaceMode::KeepOr(_) | ReplaceMode::Exactly(_) => 0o600,
        });
    }
    #[cfg(not(unix))]
    let _ = (mode, keeps);
    options.open(spare)
}

/// The final mode, the bytes and the sync the caller asked for, all
/// through the one handle the create returned.
fn lay(
    file: &mut File,
    body: &[u8],
    mode: ReplaceMode,
    kept: Option<std::fs::Permissions>,
    durability: Durability,
) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let exact = match (mode, kept) {
            (ReplaceMode::Exactly(bits), _) => Some(bits),
            (ReplaceMode::KeepOr(_), Some(kept)) => Some(kept.mode()),
            (ReplaceMode::KeepOr(_) | ReplaceMode::New(_), _) => None,
        };
        if let Some(bits) = exact {
            // The permission bits only: a set-id bit is never carried
            // onto bytes this write produced.
            file.set_permissions(std::fs::Permissions::from_mode(bits & 0o777))?;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = mode;
        if let Some(kept) = kept {
            file.set_permissions(kept)?;
        }
    }
    file.write_all(body)?;
    match durability {
        Durability::Unsynced => Ok(()),
        Durability::Data => file.sync_data(),
        Durability::All => file.sync_all(),
    }
}
