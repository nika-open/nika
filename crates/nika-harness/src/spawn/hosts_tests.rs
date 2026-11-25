// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>
//! The host switch at every place a seat starts a program. The adapter here is a stub named
//! like a host (`codex-acp`) that writes a marker for any call: with hosts off, the version
//! probe, the handshake probe, the session spawn and the bundled codex are each refused as a
//! missing binary is (NIKA-1803, final), and the marker is never written. The same stub with
//! hosts started is the witness that it would have run.
use super::*;
use crate::hosts::Hosts;
use nika_error::traits::NikaErrorCode as _;
use std::os::unix::fs::PermissionsExt as _;

/// An executable `name` in `dir` that writes `<name>.started` beside itself, then `body`.
fn stub(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
    let bin = dir.join(name);
    std::fs::write(&bin, format!("#!/bin/sh\n: > \"$0.started\"\n{body}\n")).expect("stub");
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).expect("mode");
    bin
}

/// The refusal of an absent host: final, NIKA-1803, in the switch's own words.
fn absent(error: &HarnessError, program: &std::path::Path) {
    assert!(!error.is_transient(), "no retry starts it: {error:?}");
    assert_eq!(error.nika_code().to_string(), "NIKA-1803");
    let HarnessError::Unavailable { reason } = error else {
        panic!("an absent host is unavailable, got {error:?}")
    };
    assert_eq!(
        reason,
        &format!(
            "adapter `codex`: `{}` is not started: NIKA_HARNESS_HOSTS=off keeps every host \
             program absent (set NIKA_HARNESS_HOSTS=on to start it)",
            program.display()
        )
    );
}

#[tokio::test]
async fn a_host_adapter_never_starts_while_hosts_are_off() {
    let dir = tempfile::tempdir().expect("dir");
    let bin = stub(dir.path(), "codex-acp", "printf '%s\\n' 'codex-acp 0.16.0'");
    let marker = bin.with_extension("started");
    let command = bin.to_string_lossy().into_owned();
    let pin = crate::probe::VersionPin::new((0, 16), 1);
    let versioned = HarnessAdapter::new("codex", command.clone())
        .expect("id")
        .with_version_pin(pin.clone());
    let handshake = HarnessAdapter::new("codex", command)
        .expect("id")
        .with_handshake_probe()
        .with_version_pin(pin);
    for row in [&versioned, &handshake] {
        let off = SpawnedHarness::new(row.clone()).with_hosts(Hosts::Off);
        let error = off.probe_version().await.expect_err("an absent host");
        absent(&error, &bin);
        let error = (off.spawn_child(dir.path(), true, &[])).expect_err("an absent host");
        absent(&error, &bin);
        let error = off
            .codex_profile_env(None)
            .await
            .expect_err("an absent host");
        absent(&error, &bin);
    }
    assert!(!marker.exists(), "no host started under the switch");
    let seen = (SpawnedHarness::new(versioned).with_hosts(Hosts::Start))
        .probe_version()
        .await
        .expect("the stub answers its version");
    assert_eq!(seen, Some((0, 16)));
    assert!(marker.exists(), "the same stub runs when hosts start");
}

/// The bundled codex is found from the adapter's own name, here a host's (`bin/codex-acp`, a
/// link to the package's entry point): with hosts off it is never looked for nor started.
#[tokio::test]
async fn the_bundled_codex_of_an_absent_host_never_starts() {
    let dir = tempfile::tempdir().expect("dir");
    let package = dir
        .path()
        .join("lib/node_modules/@agentclientprotocol/codex-acp");
    std::fs::create_dir_all(package.join("dist")).expect("dist");
    std::fs::write(
        package.join("dist/index.js"),
        "// the adapter's entry point",
    )
    .expect("entry");
    let codex = package.join("node_modules/@openai/codex/bin");
    std::fs::create_dir_all(&codex).expect("codex dir");
    let codex = stub(&codex, "codex.js", "exit 0");
    std::fs::create_dir_all(dir.path().join("bin")).expect("bin");
    let adapter = dir.path().join("bin/codex-acp");
    std::os::unix::fs::symlink(package.join("dist/index.js"), &adapter).expect("link");
    let row = HarnessAdapter::new("codex", adapter.to_string_lossy()).expect("id");
    let error = (SpawnedHarness::new(row.clone()).with_hosts(Hosts::Off))
        .codex_profile_env(None)
        .await
        .expect_err("an absent host");
    absent(&error, &adapter);
    let marker = codex.with_extension("js.started");
    assert!(
        !marker.exists(),
        "no bundled codex started under the switch"
    );
    let refused = (SpawnedHarness::new(row).with_hosts(Hosts::Start))
        .codex_profile_env(None)
        .await
        .expect_err("the stub lists no server");
    assert!(
        matches!(refused, HarnessError::Refused { .. }),
        "{refused:?}"
    );
    assert!(
        marker.exists(),
        "the same bundled codex runs when hosts start"
    );
}
