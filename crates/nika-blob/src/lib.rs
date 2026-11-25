// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! `nika-blob` — the production content-addressed blob store.
//!
//! This crate sits at **L1** (effect crate): it implements the L0.5
//! `nika_kernel::blob::BlobStore` trait with **blake3** content
//! addressing over a sharded filesystem layout. Crates that store media
//! / artifacts inject the kernel trait and receive [`DiskBlobStore`] in
//! production, `MockBlob` in tests (Invariant #27).
//!
//! [`DiskBlobStore`] implements the `BlobStoreDyn` trait-variant
//! companion (`Send` futures — the base `BlobStore` arrives via the
//! `trait_variant` blanket impl), same pattern as `nika-fs`/`nika-http`.
//!
//! # Storage layout
//!
//! ```text
//! {root}/
//! ├── ab/                 ← first 2 hex chars of the blake3 hash (sharding)
//! │   ├── cdef0123...      ← remaining hash chars · the blob bytes
//! │   └── cdef0123....mime ← sidecar · the caller's declared mime type
//! ```
//!
//! # Content addressing & dedup
//!
//! The key is `blake3:<hex>` — identical bytes always hash identically,
//! so `put` of already-stored content is a no-op that returns the
//! existing hash (dedup). A partial write lands at a temp path nobody
//! addresses, so a dropped `put` can never corrupt an existing blob
//! (the kernel CANCEL SAFETY contract).
//!
//! # The sidecar mime (Diamond fix vs brouillon)
//!
//! The blob bytes are pure content; the mime type is **not** part of the
//! identity. Brouillon's `put(mime)` discarded the mime and `stat`
//! re-sniffed it from magic bytes — so `put("x", "text/plain")` then
//! `stat` returned `application/octet-stream` (a silent round-trip
//! divergence). Diamond records the declared mime in a `.mime` sidecar
//! written atomically beside the blob, so `put`/`stat` agree. `stat`
//! falls back to `application/octet-stream` when a sidecar is absent
//! (a blob written by an older/foreign writer) — it records, it never
//! guesses (content sniffing is a higher-layer concern).

mod filesystem;
pub use filesystem::FsBlobStore;

use std::path::{Path, PathBuf};

use bytes::Bytes;
use nika_kernel::blob::BlobStoreDyn;
use nika_kernel::{BlobError, BlobMetadata};

/// Default maximum blob size: 500 MiB.
const DEFAULT_MAX_SIZE: u64 = 500 * 1024 * 1024;
/// Hash scheme prefix for Nika CAS keys.
const HASH_PREFIX: &str = "blake3:";
/// Default mime when no sidecar records one.
const DEFAULT_MIME: &str = "application/octet-stream";

/// Content-addressable blob storage backed by the filesystem.
///
/// Cheap to clone (`PathBuf` + `u64`). Blobs are stored under their
/// blake3 hash, sharded by the first two hex chars.
#[derive(Debug, Clone)]
pub struct DiskBlobStore {
    root: PathBuf,
    max_size: u64,
}

impl DiskBlobStore {
    /// Create a blob store at `root` with the default 500 MiB cap.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            max_size: DEFAULT_MAX_SIZE,
        }
    }

    /// Create a blob store with an explicit size cap (bytes).
    pub fn with_max_size(root: impl Into<PathBuf>, max_size: u64) -> Self {
        Self {
            root: root.into(),
            max_size,
        }
    }

    /// The root directory of this store.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The configured maximum blob size in bytes (default 500 MiB).
    #[must_use]
    pub fn max_size(&self) -> u64 {
        self.max_size
    }

    /// Validate + canonicalize a caller-supplied hash.
    ///
    /// Returns the 64-char lowercase blake3 hex (NO prefix) when the
    /// input is a well-formed key (`blake3:` prefix optional, case-
    /// insensitive), else `None`. A malformed hash is definitionally
    /// not-found — callers map `None` to [`BlobError::NotFound`] (or
    /// `false` for `exists`). This is also what keeps the path helpers
    /// panic-free: only validated 64-hex-ASCII strings reach the
    /// byte-slice, so `raw[..2]` can never split a UTF-8 codepoint
    /// (an attacker-supplied `"aé…"` is rejected here, not sliced).
    fn canonical_raw(hash: &str) -> Option<String> {
        // Strip the `blake3:` scheme prefix case-insensitively (schemes
        // are case-insensitive). `get(..N)` never panics on a non-char
        // boundary, unlike a bare slice.
        let raw = match hash.get(..HASH_PREFIX.len()) {
            Some(p) if p.eq_ignore_ascii_case(HASH_PREFIX) => &hash[HASH_PREFIX.len()..],
            _ => hash,
        };
        if raw.len() == blake3::OUT_LEN * 2 && raw.bytes().all(|b| b.is_ascii_hexdigit()) {
            Some(raw.to_ascii_lowercase())
        } else {
            None
        }
    }

    /// Filesystem path for a blob's bytes: `{root}/ab/cdef...`.
    /// `raw` MUST be a validated 64-char lowercase hex (from
    /// [`Self::canonical_raw`] or the self-generated put hash).
    fn blob_path(&self, raw: &str) -> PathBuf {
        self.root.join(&raw[..2]).join(&raw[2..])
    }

    /// Filesystem path for a blob's mime sidecar: `{root}/ab/cdef....mime`.
    /// `raw` MUST be a validated 64-char lowercase hex.
    fn sidecar_path(&self, raw: &str) -> PathBuf {
        self.root
            .join(&raw[..2])
            .join(format!("{}.mime", &raw[2..]))
    }

    /// Atomically write `contents` to `path` (temp-in-same-dir + rename).
    /// The temporary is a new file under a random name
    /// (`nika_fs::replace_file`): two concurrent writers (dedup races on
    /// identical content) never collide, and no entry planted in the shard
    /// folder catches the bytes. Any failure removes the temporary. The
    /// mode is what `tokio::fs::write` gave a new file, and no sync is
    /// added, as before.
    async fn atomic_write(path: &Path, contents: &[u8]) -> std::io::Result<()> {
        let path = path.to_path_buf();
        let contents = contents.to_vec();
        tokio::task::spawn_blocking(move || {
            nika_fs::replace_file(
                &path,
                &contents,
                nika_fs::ReplaceMode::New(0o666),
                nika_fs::Durability::Unsynced,
            )
        })
        .await
        .map_err(std::io::Error::other)?
    }
}

/// Map a raw `io::Error` onto the kernel `BlobError`, treating a missing
/// path as `NotFound` and everything else as `Io`.
fn io_to_blob_error(e: &std::io::Error, hash: &str, op: &str) -> BlobError {
    if e.kind() == std::io::ErrorKind::NotFound {
        BlobError::NotFound {
            hash: hash.to_string(),
        }
    } else {
        BlobError::Io {
            reason: format!("failed to {op} blob: {e}"),
        }
    }
}

impl BlobStoreDyn for DiskBlobStore {
    /// Store bytes, returning metadata with the `blake3:` content hash.
    ///
    /// Empty blobs are rejected ([`BlobError::Io`]); blobs over the cap
    /// raise [`BlobError::TooLarge`]. Already-stored content is a no-op
    /// (dedup) returning the existing hash. The mime is recorded in a
    /// `.mime` sidecar so `stat` round-trips it.
    ///
    /// CANCEL SAFETY: cancel-safe via CAS — a dropped write leaves at
    /// most a temp file nobody addresses; existing blobs are untouched.
    async fn put(&self, data: Bytes, mime_type: &str) -> Result<BlobMetadata, BlobError> {
        if data.is_empty() {
            return Err(BlobError::Io {
                reason: "cannot store an empty blob".to_string(),
            });
        }
        if mime_type.trim().is_empty() {
            return Err(BlobError::Io {
                reason: "mime_type must not be blank".to_string(),
            });
        }
        let size = data.len() as u64;
        if size > self.max_size {
            return Err(BlobError::TooLarge {
                size,
                max: self.max_size,
            });
        }

        // Self-generated → already canonical 64-char lowercase hex.
        let raw = blake3::hash(&data).to_hex().to_string();
        let blob_path = self.blob_path(&raw);
        let sidecar_path = self.sidecar_path(&raw);

        // Dedup: identical content is already stored. Refresh the sidecar
        // so a re-put with a new declared mime updates the record.
        let already = tokio::fs::try_exists(&blob_path).await.unwrap_or(false);

        if let Some(parent) = blob_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| BlobError::Io {
                    reason: format!("failed to create shard directory: {e}"),
                })?;
        }

        if !already {
            Self::atomic_write(&blob_path, &data)
                .await
                .map_err(|e| BlobError::Io {
                    reason: format!("failed to write blob: {e}"),
                })?;
        }
        Self::atomic_write(&sidecar_path, mime_type.as_bytes())
            .await
            .map_err(|e| BlobError::Io {
                reason: format!("failed to write mime sidecar: {e}"),
            })?;

        Ok(BlobMetadata::new(
            format!("{HASH_PREFIX}{raw}"),
            mime_type,
            size,
        ))
    }

    /// Retrieve bytes by content hash. The `blake3:` prefix is optional
    /// and the hex is case-insensitive; a malformed hash is `NotFound`.
    ///
    /// CANCEL SAFETY: cancel-safe — read-only.
    async fn get(&self, hash: &str) -> Result<Bytes, BlobError> {
        let raw = Self::canonical_raw(hash).ok_or_else(|| BlobError::NotFound {
            hash: hash.to_string(),
        })?;
        let data = tokio::fs::read(self.blob_path(&raw))
            .await
            .map_err(|e| io_to_blob_error(&e, &raw, "read"))?;
        Ok(Bytes::from(data))
    }

    /// Whether a blob exists by hash. A malformed hash and I/O errors
    /// both report `false`.
    ///
    /// CANCEL SAFETY: cancel-safe — read-only.
    async fn exists(&self, hash: &str) -> bool {
        match Self::canonical_raw(hash) {
            Some(raw) => tokio::fs::try_exists(self.blob_path(&raw))
                .await
                .unwrap_or(false),
            None => false,
        }
    }

    /// Metadata for a stored blob: size from the bytes, mime from the
    /// sidecar (falling back to `application/octet-stream` when the
    /// sidecar is absent OR blank). A malformed hash is `NotFound`.
    ///
    /// CANCEL SAFETY: cancel-safe — read-only.
    async fn stat(&self, hash: &str) -> Result<BlobMetadata, BlobError> {
        let raw = Self::canonical_raw(hash).ok_or_else(|| BlobError::NotFound {
            hash: hash.to_string(),
        })?;
        let meta = tokio::fs::metadata(self.blob_path(&raw))
            .await
            .map_err(|e| io_to_blob_error(&e, &raw, "stat"))?;
        // Only a MISSING sidecar (or a blank one) degrades to the default
        // — any other read error (permissions, I/O) is surfaced, not
        // masked into a bogus mime.
        let mime = match tokio::fs::read_to_string(self.sidecar_path(&raw)).await {
            Ok(s) if !s.trim().is_empty() => s,
            Ok(_) => DEFAULT_MIME.to_string(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => DEFAULT_MIME.to_string(),
            Err(e) => {
                return Err(BlobError::Io {
                    reason: format!("failed to read mime sidecar: {e}"),
                });
            }
        };
        Ok(BlobMetadata::new(
            format!("{HASH_PREFIX}{raw}"),
            mime,
            meta.len(),
        ))
    }

    /// Delete a blob and its sidecar. Returns [`BlobError::NotFound`]
    /// when the blob is already absent (NOT a silent no-op — the
    /// codebase-wide missing-blob contract). A malformed hash is
    /// likewise `NotFound`.
    ///
    /// CANCEL SAFETY: cancel-safe — each unlink is atomic.
    async fn delete(&self, hash: &str) -> Result<(), BlobError> {
        let raw = Self::canonical_raw(hash).ok_or_else(|| BlobError::NotFound {
            hash: hash.to_string(),
        })?;
        tokio::fs::remove_file(self.blob_path(&raw))
            .await
            .map_err(|e| io_to_blob_error(&e, &raw, "delete"))?;
        // Sidecar removal is best-effort: a blob put by a foreign writer
        // may have none, and the blob itself is already gone.
        let _ = tokio::fs::remove_file(self.sidecar_path(&raw)).await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Public-type + behaviour assertions live in
    // `tests/blob_contract.rs` — here only crate-private invariants.

    /// A real 64-char lowercase blake3 hex (blake3("hello")).
    const HELLO: &str = "ea8f163db38682925e4491c5e58d4bb3506ef8c14eb78a86e908c5624a67200f";

    #[test]
    fn canonical_raw_validates_and_lowercases() {
        // prefix optional · case-insensitive · returns the bare lower hex
        assert_eq!(DiskBlobStore::canonical_raw(HELLO).as_deref(), Some(HELLO));
        assert_eq!(
            DiskBlobStore::canonical_raw(&format!("blake3:{HELLO}")).as_deref(),
            Some(HELLO)
        );
        assert_eq!(
            DiskBlobStore::canonical_raw(&HELLO.to_uppercase()).as_deref(),
            Some(HELLO),
            "uppercase hex normalizes to the canonical lower form"
        );
    }

    #[test]
    fn canonical_raw_rejects_malformed() {
        assert_eq!(DiskBlobStore::canonical_raw("abcd"), None, "too short");
        assert_eq!(DiskBlobStore::canonical_raw(""), None, "empty");
        assert_eq!(
            DiskBlobStore::canonical_raw(&"z".repeat(64)),
            None,
            "non-hex chars"
        );
        // The panic vector rust-pro flagged: a multi-byte UTF-8 char that
        // would split mid-codepoint at byte index 2 — rejected, not sliced.
        assert_eq!(DiskBlobStore::canonical_raw("aé00"), None);
    }

    #[test]
    fn blob_and_sidecar_paths_share_the_shard_dir() {
        let s = DiskBlobStore::new("/store");
        let blob = s.blob_path(HELLO);
        let side = s.sidecar_path(HELLO);
        assert_eq!(blob.parent(), side.parent(), "blob + sidecar in one shard");
        assert_eq!(blob, Path::new("/store/ea").join(&HELLO[2..]));
        assert_eq!(
            side,
            Path::new("/store/ea").join(format!("{}.mime", &HELLO[2..]))
        );
    }

    #[test]
    fn default_max_size_is_500_mib() {
        // Pins the 500 * 1024 * 1024 arithmetic exactly.
        assert_eq!(DiskBlobStore::new("/x").max_size(), 524_288_000);
        assert_eq!(DEFAULT_MAX_SIZE, 524_288_000);
    }

    #[test]
    fn with_max_size_overrides_the_default() {
        assert_eq!(DiskBlobStore::with_max_size("/x", 4096).max_size(), 4096);
    }

    /// A blob lands through a new temporary file: links waiting at the
    /// pid-and-counter temporary names of earlier releases keep pointing
    /// at their outside file, which keeps its bytes.
    #[cfg(unix)]
    #[tokio::test]
    #[allow(clippy::expect_used)]
    async fn a_blob_write_never_writes_through_a_link_at_an_old_temporary_name() {
        let shard = tempfile::tempdir().expect("shard");
        let outside = tempfile::tempdir().expect("outside");
        let victim = outside.path().join("rc");
        std::fs::write(&victim, "export KEEP=1\n").expect("outside file");
        let pid = std::process::id();
        let planted: Vec<PathBuf> = (0..64)
            .map(|n| {
                let link = shard.path().join(format!(".nika-tmp.{pid}.{n}"));
                std::os::unix::fs::symlink(&victim, &link).expect("planted link");
                link
            })
            .collect();
        let blob = shard.path().join("abcd");
        for body in [b"first".as_slice(), b"second".as_slice()] {
            DiskBlobStore::atomic_write(&blob, body).await.expect("put");
            assert_eq!(std::fs::read(&blob).expect("blob"), body);
        }
        assert_eq!(
            std::fs::read_to_string(&victim).expect("outside file"),
            "export KEEP=1\n"
        );
        for link in &planted {
            assert_eq!(std::fs::read_link(link).expect("still a link"), victim);
        }
        assert!(
            std::fs::symlink_metadata(&blob)
                .expect("blob")
                .file_type()
                .is_file()
        );
        assert_eq!(std::fs::read_dir(shard.path()).expect("list").count(), 65);
    }
}
