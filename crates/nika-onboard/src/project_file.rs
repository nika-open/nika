// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! The starter `nika.yaml` — laid by default at founding (#1283 · the
//! team's shared ceiling and retention is THE team file, so a scaffold
//! that omits it is not a team scaffold). Never a toll: the wizard asks
//! (Enter lays, `n` skips), the scripted lane lays it with its receipt,
//! and the file itself says what it does — its examples ride commented,
//! so a laid starter governs NOTHING until the human uncomments it
//! (absence IS the defaults, ratchet-pinned in [`nika_vocab::project`]).
//!
//! The adds-only law is the `.gitignore` row's sibling: an existing
//! file is SKIPPED (`--force` overrides), the write creates parents
//! rather than fail on the missing, and the row rides the founding
//! report in both lanes (one law, two doors).

use std::path::Path;

use nika_vocab::project::{FILE_NAME, STARTER};

/// What one `ensure` came to — the founding register's vocabulary
/// (the [`crate`] `gitignore` precedent, project-file side).
#[non_exhaustive]
pub enum Outcome {
    /// The file was absent — created carrying the starter.
    Created,
    /// The file existed — nothing written (the skip law; `--force`
    /// is the only override).
    Skipped,
    /// Left as it is, `--force` included or not needed: a link to a file
    /// inside the project, never written through, or, at a file init only
    /// skips, a link or a folder leading out (never followed) or something
    /// not a regular file. The text is the row's note.
    Kept(String),
    /// A write failure — init's one environment error (exit 3).
    Failed(String),
}

/// Lay `dir/nika.yaml` when absent. `force` overwrites (the founding
/// law's one override); every other path is adds-only.
#[must_use]
pub fn ensure(dir: &str, force: bool) -> (String, Outcome) {
    let path = Path::new(dir)
        .join(FILE_NAME)
        .to_string_lossy()
        .into_owned();
    match ensure_inner(Path::new(dir), Path::new(&path), force) {
        Ok(outcome) => (path, outcome),
        Err(e) => (path, Outcome::Failed(e.to_string())),
    }
}

/// The io half — `?` propagation, the caller owns the error voice. The
/// same care as the briefs' write (`crate::confine`): the dir may not
/// exist yet on a bare `init new-dir`, and no write goes through a link. A
/// link inside the project is kept under `--force` too; what would only be
/// skipped is never refused, since nothing is written there.
fn ensure_inner(dir: &Path, path: &Path, force: bool) -> std::io::Result<Outcome> {
    use crate::confine;
    let occupied = match confine::occupied(dir, path) {
        Ok(occupied) => occupied,
        Err(e) if !force => {
            return confine::refusal(&e)
                .map_or(Err(e), |why| Ok(Outcome::Kept(why.kept().to_owned())));
        }
        Err(e) => return Err(e),
    };
    if confine::link_inside(dir, path) {
        return Ok(Outcome::Kept(crate::founding::KEPT_LINK.to_owned()));
    }
    if occupied && !force {
        return Ok(Outcome::Skipped);
    }
    confine::write(dir, path, STARTER.as_bytes())?;
    Ok(Outcome::Created)
}

/// The report row for one outcome — the founding register's ✔/·/✖
/// vocabulary, `path` caller-shaped (joined in the scripted lane ·
/// project-relative in the wizard); a created row says WHY (#1283).
#[must_use]
pub fn report(path: &str, outcome: &Outcome) -> (char, String) {
    match outcome {
        Outcome::Created => (
            '✔',
            format!("created {path} — {}", crate::briefs::purpose(FILE_NAME)),
        ),
        Outcome::Skipped => ('·', format!("skipped {path} (exists · --force)")),
        Outcome::Kept(note) => ('·', format!("skipped {path} ({note})")),
        Outcome::Failed(e) => ('✖', format!("{path}: {e}")),
    }
}

/// Protect local run traces beside an explicitly materialized workflow.
/// Uses the same adds-only ignore policy as project bootstrap. The traces
/// are protected only when the entry is in a `.gitignore` git reads. git
/// does not read a `.gitignore` that is a link, whatever the file it
/// points at holds (its 2.32 release notes:
/// <https://github.com/git/git/blob/master/Documentation/RelNotes/2.32.0.adoc>),
/// and nika never writes through one; something that is not a regular
/// file is never read. Either is no protection, so the caller refuses
/// rather than publish beside it, and the refusal names the way out.
/// # Errors
/// Returns the filesystem failure, or the `.gitignore` left as it is, to
/// the materializing caller.
pub fn protect_traces(dir: &std::path::Path) -> std::io::Result<()> {
    use crate::gitignore::Outcome as Ignore;
    let (path, outcome) = crate::gitignore::ensure(&dir.to_string_lossy());
    match outcome {
        Ignore::Created | Ignore::Appended | Ignore::Covered => Ok(()),
        Ignore::Failed(message) => Err(std::io::Error::other(message)),
        Ignore::KeptLink => Err(std::io::Error::other(format!(
            "{path} is a link, and git does not read a .gitignore that is a link, so the traces would not be ignored · replace the link with a regular file holding .nika/traces/, then retry"
        ))),
        Ignore::KeptAside(note) => Err(std::io::Error::other(format!(
            "{path}: {note}, so the traces would not be ignored · make it a regular file inside the project, then retry"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trace protection holds only with the entry in a file git reads: a
    /// `.gitignore` created, appended or already covering passes; a folder
    /// in its place, or a link with or without the entry in the file it
    /// reaches, is refused, named, and left as it is.
    #[cfg(unix)]
    #[test]
    fn protect_traces_refuses_an_ignore_file_it_leaves_as_it_is() {
        let fresh = fresh_dir("protect-fresh");
        protect_traces(&fresh).expect("created");
        let laid = std::fs::read_to_string(fresh.join(".gitignore")).expect("read");
        assert!(laid.contains(".nika/traces/"), "{laid}");
        protect_traces(&fresh).expect("covered");

        let folder = fresh_dir("protect-folder");
        std::fs::create_dir(folder.join(".gitignore")).expect("a folder in its place");
        let error = protect_traces(&folder).expect_err("a folder protects nothing");
        let text = error.to_string();
        assert!(text.ends_with(
            ".gitignore: not a regular file · kept as it is, so the traces would not be ignored · make it a regular file inside the project, then retry"
        ), "{text}");
        assert!(folder.join(".gitignore").is_dir(), "left as it is");

        let link = fresh_dir("protect-link");
        std::fs::write(link.join("rules"), "target/\n").expect("seed");
        std::os::unix::fs::symlink("rules", link.join(".gitignore")).expect("link");
        let error = protect_traces(&link).expect_err("a link without the entry protects nothing");
        assert!(error.to_string().ends_with(LINKED), "{error}");
        assert_eq!(
            std::fs::read_to_string(link.join("rules")).expect("read"),
            "target/\n",
            "the file it points at keeps every byte"
        );
        std::fs::write(link.join("rules"), "target/\n.nika/traces/\n").expect("cover");
        let error = protect_traces(&link).expect_err("git reads no linked .gitignore");
        assert!(error.to_string().ends_with(LINKED), "{error}");
        assert_eq!(
            std::fs::read_to_string(link.join("rules")).expect("read"),
            "target/\n.nika/traces/\n",
            "the file it points at keeps every byte"
        );
        for dir in [fresh, folder, link] {
            std::fs::remove_dir_all(dir).ok();
        }
    }

    /// The refusal's last words for a `.gitignore` that is a link.
    const LINKED: &str = ".gitignore is a link, and git does not read a .gitignore that is a link, so the traces would not be ignored · replace the link with a regular file holding .nika/traces/, then retry";

    /// git's own reading, on a workflow folder whose `.gitignore` links to
    /// the project's, which holds the entry (`workflows/.gitignore ->
    /// ../.gitignore`): git does not read the link, so the traces there are
    /// not ignored, and `protect_traces` refuses. Once the link is replaced
    /// by a regular file holding the entry, as the refusal says, git ignores
    /// them and `protect_traces` passes. The environment git reads its own
    /// settings from is the scratch repository's, never the caller's.
    // disallowed_types: the witness's whole job is asking git itself; the
    // ban guards production effects, not a test arranging the surface
    // under test (the same carve-out as nika-cli-host's git fixture).
    #[allow(clippy::disallowed_types)]
    #[cfg(unix)]
    #[test]
    fn git_reads_no_linked_gitignore_and_protect_traces_agrees() {
        let repo = fresh_dir("protect-git");
        let git = |args: &[&str]| {
            let mut command = std::process::Command::new("git");
            for (key, _) in std::env::vars_os() {
                if key.to_string_lossy().starts_with("GIT_") {
                    command.env_remove(&key);
                }
            }
            command
                .args(args)
                .current_dir(&repo)
                .env("HOME", &repo)
                .env("XDG_CONFIG_HOME", &repo)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .expect("git is on PATH (CI runners ship it)")
                .code()
        };
        assert_eq!(git(&["init", "-q"]), Some(0));
        let workflows = repo.join("workflows");
        std::fs::create_dir(&workflows).expect("mkdir");
        std::fs::write(repo.join(".gitignore"), ".nika/traces/\n").expect("the project's");
        std::os::unix::fs::symlink("../.gitignore", workflows.join(".gitignore")).expect("link");
        let traces = "workflows/.nika/traces/run.ndjson";
        assert_eq!(git(&["check-ignore", "-q", traces]), Some(1), "not ignored");
        let error = protect_traces(&workflows).expect_err("git reads no linked .gitignore");
        assert!(error.to_string().ends_with(LINKED), "{error}");
        assert_eq!(
            std::fs::read_link(workflows.join(".gitignore")).expect("still the link"),
            std::path::Path::new("../.gitignore")
        );

        std::fs::remove_file(workflows.join(".gitignore")).expect("unlink");
        std::fs::write(workflows.join(".gitignore"), ".nika/traces/\n").expect("regular");
        assert_eq!(git(&["check-ignore", "-q", traces]), Some(0), "ignored");
        protect_traces(&workflows).expect("a regular file holding the entry protects");
        std::fs::remove_dir_all(repo).ok();
    }

    fn fresh_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("nika-projfile-{tag}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    /// The skip/overwrite law, pinned: absent → created with the
    /// starter · existing → skipped, byte-untouched · `--force` →
    /// overwritten with the starter.
    #[test]
    fn the_skip_overwrite_law() {
        let dir = fresh_dir("law");
        let d = dir.to_str().expect("utf8");

        let (path, outcome) = ensure(d, false);
        assert!(matches!(outcome, Outcome::Created));
        assert!(path.ends_with("nika.yaml"), "{path}");
        let laid = std::fs::read_to_string(dir.join("nika.yaml")).expect("written");
        assert_eq!(laid, STARTER, "the grammar's own starter, verbatim");

        // A hand-written file is never touched without --force.
        std::fs::write(dir.join("nika.yaml"), "nika: proj\nceiling: 9.99\n").expect("seed");
        let (_, outcome) = ensure(d, false);
        assert!(matches!(outcome, Outcome::Skipped), "existing = skip");
        assert_eq!(
            std::fs::read_to_string(dir.join("nika.yaml")).expect("read"),
            "nika: proj\nceiling: 9.99\n",
            "the human's bytes survive"
        );

        let (_, outcome) = ensure(d, true);
        assert!(matches!(outcome, Outcome::Created), "--force overrides");
        assert_eq!(
            std::fs::read_to_string(dir.join("nika.yaml")).expect("read"),
            STARTER
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The laid starter PARSES — and to the defaults (the offer can
    /// never lay a broken or a silently-governing file).
    #[test]
    fn the_laid_starter_parses_to_the_defaults() {
        let dir = fresh_dir("parses");
        let d = dir.to_str().expect("utf8");
        let _ = ensure(d, false);
        let parsed = nika_vocab::project::discover(&dir)
            .expect("the laid file parses")
            .map(|(_path, project)| project);
        let parsed = parsed.expect("the laid file is discovered");
        assert_eq!(
            parsed.name.as_deref(),
            Some("my-project"),
            "the starter lays a NAME — the version rides the $schema line"
        );
        // `Project` is `#[non_exhaustive]` (FCI-016), so a sibling crate
        // compares the knobs rather than rebuilding the struct.
        assert!(
            parsed.ceiling.is_none()
                && parsed.traces.is_none()
                && parsed.registry.is_none()
                && parsed.arm().is_empty(),
            "every other starter line is a commented example: {parsed:?}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The report rows speak the founding register.
    #[test]
    fn report_rows_speak_the_founding_register() {
        let (mark, msg) = report("nika.yaml", &Outcome::Created);
        assert_eq!(mark, '✔');
        assert!(
            msg.starts_with("created nika.yaml — team defaults"),
            "the why rides the row: {msg}"
        );
        let (mark, msg) = report("nika.yaml", &Outcome::Skipped);
        assert_eq!(mark, '·');
        assert!(msg.contains("--force"), "{msg}");
        let (mark, msg) = report("nika.yaml", &Outcome::Failed("denied".to_owned()));
        assert_eq!(mark, '✖');
        assert!(msg.contains("denied"), "{msg}");
    }
}
