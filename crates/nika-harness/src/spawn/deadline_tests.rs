// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>
//! The probe deadline: a probe that does not answer inside it is unanswered, NIKA-1804 and
//! transient (a loaded machine), never the absent adapter a retry cannot heal; a program that
//! cannot be found or started stays NIKA-1803 and final. Each slow probe is a real program that
//! sleeps; the clock is paused, so the 30 s deadline passes at once and is read exactly.
use super::*;
use nika_error::traits::NikaErrorCode as _;
use std::os::unix::fs::PermissionsExt as _;

/// The measured npm layout (`<package>/dist/index.js` beside its bundled
/// `<package>/node_modules/@openai/codex/bin/codex.js`) around a codex whose `mcp list` never
/// answers, with the bundled program's `mode`.
fn sleeping_codex(dir: &std::path::Path, mode: u32) -> String {
    let entry = dir.join("codex-acp/dist/index.js");
    std::fs::create_dir_all(entry.parent().expect("dist")).expect("dist");
    std::fs::write(&entry, "// the adapter's entry point").expect("entry");
    let codex = dir.join("codex-acp/node_modules/@openai/codex/bin/codex.js");
    std::fs::create_dir_all(codex.parent().expect("bin")).expect("bin");
    let script =
        "#!/bin/sh\ncase \" $* \" in\n  *\" mcp list \"*) exec sleep 600 ;;\nesac\nexit 0\n";
    std::fs::write(&codex, script).expect("codex");
    std::fs::set_permissions(&codex, std::fs::Permissions::from_mode(mode)).expect("mode");
    entry.to_string_lossy().into_owned()
}

/// The deadline's own class, code and words, and that it passed exactly at the deadline.
fn unanswered(error: &HarnessError, started: tokio::time::Instant) -> String {
    let waited = started.elapsed();
    assert!(
        waited >= crate::probe::deadline()
            && waited < crate::probe::deadline() + std::time::Duration::from_secs(1),
        "cut at the 30 s probe deadline: {waited:?}"
    );
    assert!(error.is_transient(), "a plain retry: replays it: {error:?}");
    assert_eq!(error.nika_code().to_string(), "NIKA-1804");
    let HarnessError::Session { reason } = error else {
        panic!("an unanswered probe, got {error:?}")
    };
    reason.clone()
}

/// The absent or unstartable program's class: final, NIKA-1803.
fn unavailable(error: &HarnessError) -> String {
    assert!(!error.is_transient(), "no retry heals it: {error:?}");
    assert_eq!(error.nika_code().to_string(), "NIKA-1803");
    let HarnessError::Unavailable { reason } = error else {
        panic!("an unavailable adapter, got {error:?}")
    };
    reason.clone()
}

/// The Codex profile listing that misses the deadline on a loaded machine is unanswered.
#[tokio::test(start_paused = true)]
async fn a_codex_profile_listing_past_the_deadline_is_unanswered_and_transient() {
    let dir = tempfile::tempdir().expect("dir");
    let entry = sleeping_codex(dir.path(), 0o755);
    let harness = SpawnedHarness::new(HarnessAdapter::new("codex", entry).expect("id"));
    let started = tokio::time::Instant::now();
    let error = (harness.codex_profile_env(None))
        .await
        .expect_err("the listing never answers");
    let reason = unanswered(&error, started);
    assert!(
        reason.starts_with("adapter `codex`: codex mcp list `"),
        "{reason}"
    );
    assert!(
        reason
            .ends_with("codex.js -c features.plugins=false mcp list --json` did not answer in 30s"),
        "{reason}"
    );
}

/// A version probe and the standalone `initialize` handshake that never answer are unanswered
/// too, in their own words.
#[tokio::test(start_paused = true)]
async fn a_version_probe_and_a_handshake_past_the_deadline_are_unanswered() {
    let silent = || vec!["-c".to_owned(), "exec sleep 600".to_owned()];
    let pin = crate::probe::VersionPin::new((0, 16), 1);
    let version = HarnessAdapter::new("slow", "/bin/sh")
        .expect("id")
        .with_version_args(silent())
        .with_version_pin(pin.clone());
    let started = tokio::time::Instant::now();
    let error = (SpawnedHarness::new(version).probe_version())
        .await
        .expect_err("--version never answers");
    assert_eq!(
        unanswered(&error, started),
        "adapter `slow`: version probe `/bin/sh -c exec sleep 600` did not answer in 30s"
    );
    let handshake = HarnessAdapter::new("slow", "/bin/sh")
        .expect("id")
        .with_args(silent())
        .with_handshake_probe()
        .with_version_pin(pin);
    let started = tokio::time::Instant::now();
    let error = (SpawnedHarness::new(handshake).probe_version())
        .await
        .expect_err("initialize never answers");
    assert_eq!(
        unanswered(&error, started),
        "adapter `slow`: the initialize handshake did not answer in 30s"
    );
}

/// What cannot be found or started stays unavailable and final: an adapter that is not there,
/// a bundled codex that cannot be executed, a version probe whose program is absent.
#[tokio::test]
async fn a_program_that_cannot_be_found_or_started_stays_unavailable() {
    let absent = HarnessAdapter::new("codex", "/nonexistent/nika-codex-acp-2026").expect("id");
    let error = (SpawnedHarness::new(absent).codex_profile_env(None))
        .await
        .expect_err("no adapter");
    assert_eq!(
        unavailable(&error),
        "`/nonexistent/nika-codex-acp-2026` is not on the adapter PATH"
    );
    let dir = tempfile::tempdir().expect("dir");
    let entry = sleeping_codex(dir.path(), 0o644);
    let error = (SpawnedHarness::new(HarnessAdapter::new("codex", entry).expect("id")))
        .codex_profile_env(None)
        .await
        .expect_err("the bundled codex cannot start");
    let reason = unavailable(&error);
    assert!(
        reason.starts_with("adapter `codex`: cannot start codex mcp list `"),
        "{reason}"
    );
    let version = HarnessAdapter::new("ghost", "/nonexistent/nika-ghost-2026")
        .expect("id")
        .with_version_pin(crate::probe::VersionPin::new((0, 16), 1));
    let error = (SpawnedHarness::new(version).probe_version())
        .await
        .expect_err("no binary");
    let reason = unavailable(&error);
    assert!(
        reason.starts_with(
            "adapter `ghost`: cannot start version probe `/nonexistent/nika-ghost-2026 --version`"
        ),
        "{reason}"
    );
}

/// NIK-385 · a seat lives as long as its run and proves its adapter once: an unanswered proof
/// is not kept (the next session proves again), the first good proof is kept for every later
/// session, by the seat and its clones. The probe logs each answer it gives.
#[tokio::test(start_paused = true)]
async fn a_good_proof_is_kept_for_the_run_and_an_unanswered_one_is_not() {
    let dir = tempfile::tempdir().expect("dir");
    let (log, ready) = (dir.path().join("answers.log"), dir.path().join("ready"));
    let script = format!(
        "if [ -f '{ready}' ]; then echo answered >> '{log}'; echo 'codex-acp 0.16.2'; \
         else exec sleep 600; fi",
        ready = ready.display(),
        log = log.display()
    );
    let seat = SpawnedHarness::new(
        HarnessAdapter::new("codex", "/bin/sh")
            .expect("id")
            .with_version_args(vec!["-c".to_owned(), script])
            .with_version_pin(crate::probe::VersionPin::new((0, 16), 1)),
    );
    let answers = || std::fs::read_to_string(&log).map_or(0, |text| text.lines().count());
    let started = tokio::time::Instant::now();
    let error = seat
        .prove()
        .await
        .map(|_| ())
        .expect_err("the first probe never answers");
    unanswered(&error, started);
    // The machine answers now, in real time.
    tokio::time::resume();
    std::fs::write(&ready, "").expect("ready");
    let (start, env) = seat
        .prove()
        .await
        .expect("the unanswered proof was not kept");
    assert!(start.is_none() && env.is_empty());
    assert_eq!(answers(), 1);
    let clone = seat.clone();
    for kept in [&seat, &clone, &seat] {
        kept.prove().await.expect("kept");
    }
    assert_eq!(
        answers(),
        1,
        "one good proof serves every later session of the run"
    );
}
