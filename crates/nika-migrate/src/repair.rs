// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! The byte-level repair mechanics every codemod applier shares —
//! whole-word unique-site splicing and the atomic publish. Descended out
//! of `nika-cli`'s fix verb at the C2 wall (the 15k prod-LOC budget —
//! the splice engine IS migration machinery, the migrations crate is
//! its home · the `w1`/`w2`/`esplit` family precedent).
//!
//! SAFETY over reach — a repair applies ONLY when the old token occurs
//! EXACTLY ONCE in the source as a whole word (ambiguity or a second
//! occurrence skips it honestly; a blind splice could rewrite the wrong
//! site), and the file publishes atomically (a crash mid-write never
//! truncates a workflow).

/// Byte offsets where `needle` occurs in `hay` bounded by non-word
/// characters (or the ends) — `inpit` matches `inpit:` but never
/// `originpit`. Word chars: `[A-Za-z0-9_]` (the identifier alphabet every
/// spliceable token — field · arg key · `nika:` tool id — draws from;
/// `:` in a tool id is a non-word char, so the FULL `nika:raed` needle
/// still boundary-checks correctly at both ends).
#[must_use]
pub fn word_sites(hay: &str, needle: &str) -> Vec<usize> {
    let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut sites = Vec::new();
    let mut from = 0;
    while let Some(pos) = hay[from..].find(needle) {
        let at = from + pos;
        let before_ok = at == 0 || !is_word(hay.as_bytes()[at - 1]);
        let end = at + needle.len();
        let after_ok = end >= hay.len() || !is_word(hay.as_bytes()[end]);
        if before_ok && after_ok {
            sites.push(at);
        }
        from = at + needle.len().max(1);
    }
    sites
}

/// Whether `token` has the SHAPE of something a splice may write into a
/// workflow: a bare identifier · a `nika:` tool id · a dotted reference —
/// never prose. A rename target that carries whitespace, a backtick or a
/// `#` is a sentence that leaked out of a teaching, and splicing it makes
/// a YAML key out of a diagnostic (measured 2026-08-18: `check --fix`
/// wrote "the fields here: nika · model · …:" as a mapping key and the
/// file stopped parsing; the shipped 0.108.0 did the same with the
/// modeline teaching). The typed door upstream is the real fix — this is
/// the byte-surgery site's own refusal, so no future caller can repeat it.
#[must_use]
pub fn is_splice_token(token: &str) -> bool {
    !token.is_empty()
        && !token
            .chars()
            .any(|c| c.is_whitespace() || c == '`' || c == '#')
}

/// Splice `old` → `new` in `source` ONLY when `old` occurs EXACTLY ONCE
/// as a whole word ([`word_sites`] — the unique-site gate) AND `new` has
/// the shape of a token ([`is_splice_token`] — never prose). Returns
/// whether it applied; an ambiguous or absent token, or a prose target,
/// leaves the source untouched.
pub fn splice_unique(source: &mut String, old: &str, new: &str) -> bool {
    if !is_splice_token(new) {
        return false;
    }
    if let [at] = word_sites(source, old)[..] {
        source.replace_range(at..at + old.len(), new);
        true
    } else {
        false
    }
}

/// Publish `contents` at `path` through a new temporary sibling and one
/// `rename` (POSIX-atomic within a filesystem — the same contract the
/// `nika-fs` effect crate documents for `nika:write`). A crash or ENOSPC
/// mid-write leaves the ORIGINAL file untouched and at most a
/// `.nika-fix-tmp.*` sibling to sweep — never a truncated workflow. The
/// temp lands in the target's own directory (rename across filesystems
/// is not atomic); any failure removes it best-effort at this one site.
///
/// The temporary is always a NEW file, created with `create_new`
/// (`O_CREAT | O_EXCL`): it never opens an entry already in the folder and
/// never follows a link at that name, so nothing planted beside the
/// workflow receives its bytes. That open is the guarantee; the name (16
/// hex digits of a randomly keyed hash, never the target's name or the
/// pid) is hard to guess, but that is not promised. A replaced regular
/// file keeps its permission bits (no set-id bit); a new one gets `0o666`
/// under the umask, as `std::fs::write` gave it. No sync is added, as
/// before. This is `nika_fs::replace_file` with `Durability::Unsynced`,
/// restated in std alone because this L0 crate may not depend on the L1
/// `nika-fs`.
///
/// # Errors
///
/// `InvalidInput` when `path` names no file; otherwise the create, write,
/// mode change or rename failure (a missing parent directory, a
/// permissions refusal, a folder at the workflow's name); the temp
/// sibling is swept before the error returns.
pub fn write_atomic(path: &str, contents: &str) -> std::io::Result<()> {
    let target = std::path::Path::new(path);
    if target.file_name().is_none() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "write path has no file name",
        ));
    }
    let kept = std::fs::symlink_metadata(target)
        .ok()
        .filter(std::fs::Metadata::is_file)
        .map(|meta| meta.permissions());
    let (tmp, mut file) = create_spare(target, kept.is_some())?;
    let publish = lay(&mut file, contents.as_bytes(), kept).and_then(|()| {
        drop(file);
        std::fs::rename(&tmp, target)
    });
    if publish.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    publish
}

/// A new file at a fresh random `.nika-fix-tmp.<hex>` name beside
/// `target`. Only a name this call created is handed back, so the sweep
/// never removes an entry somebody else made.
fn create_spare(
    target: &std::path::Path,
    keeps: bool,
) -> std::io::Result<(std::path::PathBuf, std::fs::File)> {
    use std::hash::BuildHasher as _;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = target.parent().filter(|p| !p.as_os_str().is_empty());
    let mut last = None;
    for _ in 0..8 {
        let random = std::hash::RandomState::new().hash_one(NEXT.fetch_add(1, Ordering::Relaxed));
        let name = format!(".nika-fix-tmp.{random:016x}");
        let tmp = dir.map_or_else(|| std::path::PathBuf::from(&name), |d| d.join(&name));
        match open_spare(&tmp, keeps) {
            Ok(file) => return Ok((tmp, file)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => last = Some(e),
            Err(e) => return Err(e),
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::from(std::io::ErrorKind::AlreadyExists)))
}

/// Open `tmp` only as a file this call creates (`O_CREAT | O_EXCL`): an
/// entry already at that name, a link included (dangling or not), is
/// never opened or followed and is refused with `AlreadyExists`.
fn open_spare(tmp: &std::path::Path, keeps: bool) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        // Owner-only until a kept mode is set through the handle; a new
        // workflow gets what `std::fs::write` gave it.
        options.mode(if keeps { 0o600 } else { 0o666 });
    }
    #[cfg(not(unix))]
    let _ = keeps;
    options.open(tmp)
}

/// The kept mode and the bytes, through the create's handle.
fn lay(
    file: &mut std::fs::File,
    body: &[u8],
    kept: Option<std::fs::Permissions>,
) -> std::io::Result<()> {
    use std::io::Write as _;
    if let Some(kept) = kept {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            file.set_permissions(std::fs::Permissions::from_mode(kept.mode() & 0o777))?;
        }
        #[cfg(not(unix))]
        file.set_permissions(kept)?;
    }
    file.write_all(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_sites_respects_boundaries_and_counts() {
        // whole-word only — substrings inside identifiers never match
        assert_eq!(word_sites("inpit: 1", "inpit"), vec![0]);
        assert_eq!(word_sites("originpit: 1", "inpit"), Vec::<usize>::new());
        assert_eq!(word_sites("a inpit b inpit", "inpit").len(), 2);
        // the full `nika:` tool id boundary-checks at both ends
        assert_eq!(word_sites("tool: \"nika:raed\"", "nika:raed"), vec![7]);
        // …and the `raed` HALF alone still bounds on the `:` (by design:
        // the splice always receives the full typed token, never a half)
        assert_eq!(word_sites("tool: \"nika:raed\"", "raed").len(), 1);
    }

    #[test]
    fn splice_unique_applies_unique_and_skips_ambiguous() {
        let mut s = "invoke: { tool: \"nika:jq\", args: { inpit: 1 } }".to_owned();
        assert!(splice_unique(&mut s, "inpit", "input"));
        assert!(s.contains("input: 1") && !s.contains("inpit"));
        // ambiguous: two sites → untouched
        let mut s2 = "a: { promt: 1 }\nb: { promt: 2 }".to_owned();
        assert!(!splice_unique(&mut s2, "promt", "prompt"));
        assert!(s2.contains("promt: 1"), "ambiguous stays untouched");
    }

    #[test]
    fn splice_unique_refuses_a_prose_target() {
        // The corruption class: a teaching sentence offered as a rename.
        // The old token is unique — the splice would apply — and the
        // ONLY thing standing between the sentence and the file is the
        // token-shape gate. Removing it makes this test fail (the source
        // would then carry the sentence as a key).
        let body = "nika: v1\nworkflow:\n  id: hello\ntasks:\n  say:\n    exec: {}\n";
        let mut s = body.to_owned();
        let prose = "the fields here: nika · model · inputs · const · secrets · permits · run · tasks · outputs";
        assert!(!splice_unique(&mut s, "workflow", prose));
        assert_eq!(s, body, "a prose target never touches the source");
        let mut s2 = body.to_owned();
        assert!(!splice_unique(
            &mut s2,
            "workflow",
            "restore the `# ` prefix"
        ));
        assert_eq!(s2, body);
        // The shapes a repair legitimately writes all pass.
        assert!(is_splice_token("prompt"));
        assert!(is_splice_token("nika:write"));
        assert!(is_splice_token("tasks.build"));
        assert!(!is_splice_token(""));
        assert!(!is_splice_token("two words"));
    }

    #[test]
    fn write_atomic_error_path_cleans_its_temp() {
        // The failure half of the atomic contract: renaming onto a path
        // whose parent DIRECTORY vanished fails — the original (absent)
        // target stays absent and the temp is swept, never leaked.
        let dir = std::env::temp_dir().join(format!("nika-repair-atomic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("tmpdir");
        let gone = dir.join("gone");
        std::fs::create_dir_all(&gone).expect("subdir");
        let target = gone.join("wf.nika");
        let target_str = target.to_str().expect("utf8").to_owned();
        std::fs::remove_dir_all(&gone).expect("vanish parent");
        assert!(
            write_atomic(&target_str, "nika: v1\n").is_err(),
            "no parent = publish fails"
        );
        // the temp would have lived in `gone/` — the whole dir is absent,
        // and the sweep must not have recreated anything under `dir`.
        let residue: Vec<_> = std::fs::read_dir(&dir)
            .expect("readdir")
            .filter_map(Result::ok)
            .collect();
        assert!(residue.is_empty(), "no residue: {residue:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A link planted at the fixed temporary name of earlier releases (and
    /// at pid-shaped ones) never receives the bytes: the outside file it
    /// points at keeps its bytes, the links stay, the workflow is replaced
    /// with its own mode, and nothing else is left.
    #[cfg(unix)]
    #[test]
    fn write_atomic_never_writes_through_a_planted_temporary_name() {
        use std::os::unix::fs::PermissionsExt as _;
        let top = std::env::temp_dir().join(format!("nika-repair-planted-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&top);
        let project = top.join("project");
        std::fs::create_dir_all(&project).expect("project");
        let outside = top.join("rc");
        std::fs::write(&outside, "export KEEP=1\n").expect("outside");
        let target = project.join("wf.nika");
        std::fs::write(&target, "nika: v0\n").expect("workflow");
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o640)).expect("mode");
        let pid = std::process::id();
        let names = [
            ".nika-fix-tmp.wf.nika".to_owned(),
            format!(".nika-fix-tmp.wf.nika.{pid}"),
            format!(".wf.nika.{pid}.tmp"),
        ];
        for name in &names {
            std::os::unix::fs::symlink(&outside, project.join(name)).expect("plant");
        }
        let path = target.to_str().expect("utf8");
        write_atomic(path, "nika: v1\n").expect("first");
        write_atomic(path, "nika: v2\n").expect("second");
        assert_eq!(
            std::fs::read_to_string(&outside).expect("outside"),
            "export KEEP=1\n"
        );
        assert_eq!(
            std::fs::read_to_string(&target).expect("workflow"),
            "nika: v2\n"
        );
        let meta = std::fs::symlink_metadata(&target).expect("lstat");
        assert!(meta.file_type().is_file());
        assert_eq!(meta.permissions().mode() & 0o7777, 0o640);
        let mut left: Vec<String> = std::fs::read_dir(&project)
            .expect("list")
            .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        let mut expected: Vec<String> = names.to_vec();
        expected.push("wf.nika".to_owned());
        expected.sort();
        assert_eq!(left, expected, "no temporary left behind");
        for name in &names {
            assert_eq!(
                std::fs::read_link(project.join(name)).expect("still a link"),
                outside
            );
        }
        let _ = std::fs::remove_dir_all(&top);
    }

    /// A scratch folder of this test's own, removed when it ends.
    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("nika-repair-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("scratch");
            Self(dir)
        }

        fn names(&self) -> Vec<String> {
            let mut names: Vec<String> = std::fs::read_dir(&self.0)
                .expect("list")
                .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// The rename is the last step that can fail: over a folder that holds
    /// a file it does, the error comes back and the temporary is swept.
    #[test]
    fn a_failed_rename_leaves_no_temporary_behind() {
        let scratch = Scratch::new("rename");
        let target = scratch.0.join("wf.nika");
        std::fs::create_dir(&target).expect("a folder at the name");
        std::fs::write(target.join("inside"), "kept").expect("a file in it");
        let error = write_atomic(target.to_str().expect("utf8"), "nika: v1\n")
            .expect_err("a file cannot replace a folder that holds one");
        assert_ne!(error.kind(), std::io::ErrorKind::InvalidInput, "{error}");
        assert_eq!(scratch.names(), ["wf.nika"], "no temporary is left");
        assert_eq!(
            std::fs::read_to_string(target.join("inside")).expect("inside"),
            "kept"
        );
    }

    /// The open itself, at a name already taken: a link to a file outside,
    /// a dangling link and a plain file are each refused, never opened,
    /// followed or created through, in both modes of the open.
    #[cfg(unix)]
    #[test]
    fn a_temporary_name_already_taken_is_never_opened_or_followed() {
        let scratch = Scratch::new("taken");
        let victim = scratch.0.join("rc");
        std::fs::write(&victim, "export KEEP=1\n").expect("victim");
        let gone = scratch.0.join("gone");
        let to_victim = scratch.0.join(".nika-fix-tmp.a");
        let dangling = scratch.0.join(".nika-fix-tmp.b");
        let plain = scratch.0.join(".nika-fix-tmp.c");
        std::os::unix::fs::symlink(&victim, &to_victim).expect("link");
        std::os::unix::fs::symlink(&gone, &dangling).expect("dangling");
        std::fs::write(&plain, "someone's").expect("plain");
        for taken in [&to_victim, &dangling, &plain] {
            for keeps in [false, true] {
                let error = open_spare(taken, keeps).expect_err("a taken name is refused");
                assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists, "{error}");
            }
        }
        assert_eq!(
            std::fs::read_to_string(&victim).expect("victim"),
            "export KEEP=1\n"
        );
        assert!(std::fs::symlink_metadata(&gone).is_err(), "never created");
        assert_eq!(std::fs::read_to_string(&plain).expect("plain"), "someone's");
        assert_eq!(std::fs::read_link(&to_victim).expect("link"), victim);
    }

    /// A new workflow gets the mode `std::fs::write` gives a new file:
    /// `0o666` under the umask, whatever the umask is.
    #[cfg(unix)]
    #[test]
    fn a_new_workflow_gets_the_mode_std_write_gives_it() {
        use std::os::unix::fs::PermissionsExt as _;
        let scratch = Scratch::new("mode");
        let reference = scratch.0.join("reference");
        std::fs::write(&reference, "x").expect("reference");
        let workflow = scratch.0.join("wf.nika");
        write_atomic(workflow.to_str().expect("utf8"), "nika: v1\n").expect("create");
        let mode =
            |p: &std::path::Path| std::fs::metadata(p).expect("stat").permissions().mode() & 0o7777;
        assert_eq!(mode(&workflow), mode(&reference));
    }
}
