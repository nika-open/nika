// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! The recipe's ingredients — ONE implementation for every taking door.
//!
//! Init and Try stage a lesson's declared companion files through this owner.
//! Compile only returns source: it never materializes fixtures or reads files.

use std::path::{Path, PathBuf};

/// Relative `workflow: "./foo.nika"` children a body invokes, read from
/// its PARSED `invoke:` targets. `nika try 10-compose-pipeline` staged the
/// parent alone and check died on NIKA-COMP-001 (the child is an
/// ingredient, same class as a fixture file). A raw-text scan also took a
/// child a lesson only names in a comment (`15-compose-self-check`), which
/// init then wrote and audited. Only `./` static pack slugs — templated
/// targets cannot be staged; a body that does not parse stages no child
/// (its own check refuses it first).
#[must_use]
fn composition_siblings(body: &str) -> Vec<String> {
    use nika_schema::raw::{RawAction, RawInvokeTarget};
    let Ok(wf) = nika_schema::parse(
        body,
        nika_schema::FileId::new(0),
        nika_schema::ParseMode::Lenient,
    ) else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for task in &wf.tasks {
        let RawAction::Invoke(invoke) = &task.value.action else {
            continue;
        };
        let RawInvokeTarget::Workflow(target) = &invoke.target else {
            continue;
        };
        let Some(name) = target.value.strip_prefix("./") else {
            continue;
        };
        if nika_source::is_canonical_program_file_name(name) && !out.iter().any(|p| p == name) {
            out.push(name.to_owned());
        }
    }
    out
}

/// The `examples/fixtures/<tail>` references a body reads, each tail
/// cut at its first glob star (a `photos/**` read pulls the whole
/// `photos/` dir) and stripped of a trailing `/`. A star inside a name
/// cuts back to the directory holding it: `quotes/quote-*.txt` pulls
/// `quotes/`, since the bare `quotes/quote-` names no path.
#[must_use]
pub fn fixture_prefixes(body: &str) -> Vec<String> {
    const MARK: &str = "examples/fixtures/";
    let mut out: Vec<String> = Vec::new();
    for (i, _) in body.match_indices(MARK) {
        let tail: String = body[i + MARK.len()..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/' | '*'))
            .collect();
        let cut = match tail.split_once('*') {
            Some((before, _)) if !before.is_empty() && !before.ends_with('/') => {
                before.rsplit_once('/').map_or("", |(dir, _)| dir)
            }
            Some((before, _)) => before,
            None => tail.as_str(),
        };
        let cut = cut.trim_end_matches('/');
        if !cut.is_empty() && !out.iter().any(|p| p == cut) {
            out.push(cut.to_owned());
        }
    }
    out
}

/// Write the pack fixtures a body reads beside the taken file — under
/// `<dest dir>/examples/fixtures/…`, the exact relative path the yaml
/// names (moving the fixture without the path would relocate the lie,
/// not kill it). Files already yours are never clobbered. Returns
/// `(written, kept)` for the door's own note.
///
/// # Errors
/// An I/O failure creating a fixture directory or writing a fixture
/// file — the door reports it and refuses to pretend the take is whole.
pub fn materialize(body: &str, dest: &Path) -> std::io::Result<(usize, usize)> {
    let root = dest.parent().unwrap_or_else(|| Path::new(""));
    let mut written = Vec::new();
    let kept = stage(body, dest, root, &mut written)?;
    Ok((written.len(), kept))
}

/// The same ingredients for a lesson kept apart from its fixture root:
/// `nika init --example` lays the lesson under `workflows/`, while its runs
/// resolve `examples/fixtures/…` from the project root. Composition children
/// land beside `dest`, fixtures under `fixture_root`. Returns every file
/// written, for the door's receipt rows.
///
/// # Errors
/// The same I/O failures as [`materialize`].
pub(crate) fn materialize_into(
    body: &str,
    dest: &Path,
    fixture_root: &Path,
) -> std::io::Result<Vec<PathBuf>> {
    let mut written = Vec::new();
    stage(body, dest, fixture_root, &mut written)?;
    Ok(written)
}

/// One staging pass: composition children beside `dest`, fixtures under
/// `fixture_root`, each written path pushed onto `written`. Returns how
/// many were already there (kept, never clobbered).
fn stage(
    body: &str,
    dest: &Path,
    fixture_root: &Path,
    written: &mut Vec<PathBuf>,
) -> std::io::Result<usize> {
    let prefixes = fixture_prefixes(body);
    let base = dest.parent().unwrap_or_else(|| Path::new(""));
    let mut kept = 0;
    for name in &composition_siblings(body) {
        let Some(slug) = nika_source::program_stem(name) else {
            continue;
        };
        let Some(child) = nika_pack::example(slug) else {
            continue;
        };
        let target = base.join(name);
        if target.exists() {
            kept += 1;
            continue;
        }
        std::fs::write(&target, child)?;
        written.push(target.clone());
        kept += stage(child, &target, fixture_root, written)?;
    }
    if prefixes.is_empty() {
        return Ok(kept);
    }
    for (tail, bytes) in nika_pack::example_fixture_files() {
        let wanted = prefixes
            .iter()
            .any(|p| tail == p || tail.starts_with(&format!("{p}/")));
        if !wanted {
            continue;
        }
        let target = fixture_root.join("examples").join("fixtures").join(tail);
        if target.exists() {
            kept += 1;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&target, bytes)?;
        written.push(target);
    }
    Ok(kept)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// THE LAW (RAMS-3 · the sequence law): EVERY catalog entry that
    /// reads a fixture path, taken into an empty directory, has every
    /// fixture it names materialized beside it — a loop over the WHOLE
    /// corpus, so the next added job inherits the pin instead of
    /// re-breaking in silence (Rosa's rage-quit class).
    #[test]
    fn every_fixture_reading_entry_takes_its_ingredients_along() {
        let mut covered = 0;
        for slug in nika_pack::example_slugs() {
            let body = nika_pack::example(&slug).expect("catalog body");
            let prefixes = fixture_prefixes(body);
            if prefixes.is_empty() {
                continue;
            }
            covered += 1;
            let dir = std::env::temp_dir().join(format!(
                "nika-fixtures-law-{}-{}",
                std::process::id(),
                slug.replace('/', "-")
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("tmpdir");
            let dest = dir.join("taken.nika");
            std::fs::write(&dest, body).expect("take the body");
            let (written, _kept) = materialize(body, &dest).expect("ingredients follow");
            assert!(written > 0, "{slug}: reads {prefixes:?}, wrote nothing");
            for p in &prefixes {
                let on_disk = dir.join("examples").join("fixtures").join(p);
                assert!(
                    on_disk.exists(),
                    "{slug}: `{p}` named by the body is missing on disk"
                );
            }
            let _ = std::fs::remove_dir_all(&dir);
        }
        assert!(
            covered >= 3,
            "the corpus carries fixture-reading jobs; the law must bite: {covered}"
        );
    }

    /// A star cuts the tail where it stands when it opens a segment, and
    /// back to the holding directory when it sits inside a name: the
    /// result is always a path the pack can hold, never a name fragment.
    #[test]
    fn a_star_inside_a_name_cuts_back_to_its_directory() {
        let cases: [(&str, &[&str]); 7] = [
            ("examples/fixtures/quotes/quote-*.txt", &["quotes"]),
            ("examples/fixtures/photos/**", &["photos"]),
            ("examples/fixtures/ci/logs/*.log", &["ci/logs"]),
            ("examples/fixtures/a/b-*/c.txt", &["a"]),
            ("examples/fixtures/rfq.yaml", &["rfq.yaml"]),
            // At the fixture root itself there is no directory to cut to.
            ("examples/fixtures/top-*.csv", &[]),
            (
                "examples/fixtures/q/rfq.yaml examples/fixtures/q/quote-*.txt examples/fixtures/q",
                &["q/rfq.yaml", "q"],
            ),
        ];
        for (body, want) in cases {
            assert_eq!(fixture_prefixes(body), want, "{body}");
        }
    }

    /// `quote-compare-decide` reads its quotes through `quote-*.txt`: the
    /// take stages the folder, so the three quotes land with the pack's
    /// exact bytes beside the request for quotation.
    #[test]
    fn the_quote_recipe_takes_its_three_quotes_along() {
        let body = nika_pack::example("quote-compare-decide").expect("pack");
        assert_eq!(
            fixture_prefixes(body),
            ["quote-compare-decide/rfq.yaml", "quote-compare-decide"],
        );
        let dir = std::env::temp_dir().join(format!("nika-fixtures-quotes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("tmpdir");
        let dest = dir.join("quote-compare-decide.nika");
        std::fs::write(&dest, body).expect("take the body");
        let (written, kept) = materialize(body, &dest).expect("ingredients follow");
        assert_eq!((written, kept), (4, 0), "three quotes and the request");
        let pack = nika_pack::example_fixture_files();
        for name in ["quote-a.txt", "quote-b.txt", "quote-c.txt", "rfq.yaml"] {
            let tail = format!("quote-compare-decide/{name}");
            let (_, bytes) = pack
                .iter()
                .find(|(t, _)| *t == tail)
                .expect("the pack carries the file");
            let on_disk = dir.join("examples").join("fixtures").join(&tail);
            assert_eq!(
                std::fs::read(&on_disk).expect("staged"),
                *bytes,
                "{tail} lands byte for byte"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `nika try 10-compose-pipeline` must stage the child it invokes
    /// (`./10-compose-child.nika`) or check dies NIKA-COMP-001 in
    /// the rehearsal room (measured 2026-08-22 · e2e S2 17/18).
    #[test]
    fn a_compose_parent_takes_its_child_along() {
        let body = nika_pack::example("10-compose-pipeline").expect("pack");
        let siblings = composition_siblings(body);
        assert!(
            siblings
                .iter()
                .any(|s| nika_source::typed_stem(s) == "10-compose-child"
                    || s.starts_with("10-compose-child.")),
            "the parent names the child: {siblings:?}"
        );
        let dir = std::env::temp_dir().join(format!("nika-compose-sib-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("tmpdir");
        let dest = dir.join("10-compose-pipeline.nika");
        std::fs::write(&dest, body).expect("parent");
        let (written, _) = materialize(body, &dest).expect("child follows");
        assert!(written >= 1, "wrote the child");
        assert!(
            dir.join("10-compose-child.nika").exists(),
            "the rehearsal room has the sibling"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A child named only in a comment is not an ingredient: the
    /// `15-compose-self-check` header cites `./10-compose-child.nika`, but
    /// its tasks invoke no workflow (its draft's children are never
    /// resolved), so no door stages, writes or audits that child.
    #[test]
    fn a_child_named_only_in_a_comment_is_not_an_ingredient() {
        let body = nika_pack::example("15-compose-self-check").expect("pack");
        assert!(
            body.contains("./10-compose-child.nika"),
            "the header still names the child, so this test still bites"
        );
        assert_eq!(composition_siblings(body), Vec::<String>::new());
    }

    /// A body with no fixture reads materializes nothing — and a
    /// fixture already yours is counted kept, never clobbered.
    #[test]
    fn no_reads_no_writes_and_yours_stays_yours() {
        let dir = std::env::temp_dir().join(format!("nika-fixtures-kept-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("tmpdir");
        let dest = dir.join("t.nika");
        assert_eq!(
            materialize("nika: v1\n", &dest).expect("clean"),
            (0, 0),
            "no fixture references → untouched disk"
        );
        // Pick a real fixture tail and pre-claim it with OUR bytes.
        let (tail, _) = nika_pack::example_fixture_files()
            .first()
            .copied()
            .expect("the pack carries fixtures");
        let mine = dir.join("examples").join("fixtures").join(tail);
        std::fs::create_dir_all(mine.parent().expect("parent")).expect("mkdir");
        std::fs::write(&mine, b"mine, not the pack's").expect("pre-claim");
        let body = format!("# reads examples/fixtures/{tail}\n");
        let (written, kept) = materialize(&body, &dest).expect("kept");
        assert_eq!(
            (written, kept),
            (0, 1),
            "already yours → kept, not clobbered"
        );
        assert_eq!(
            std::fs::read(&mine).expect("still mine"),
            b"mine, not the pack's",
            "the pre-claimed bytes survive"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
