// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>
//! The Codex profile read back on a scripted bundled codex before any session opens. The script
//! answers `features list` and `mcp list` from the very `-c` overrides it receives, so the judge
//! reads what the adapter would run: a conversation must read Codex's approval route to Nika on,
//! the completion profile must read it off.
use super::*;
use crate::authoring::acp::codex;
use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt as _;

/// The measured npm layout (`<package>/dist/index.js`, its bundled
/// `<package>/node_modules/@openai/codex/bin/codex.js`) around a codex that lists each feature
/// as its `-c features.<name>=<bool>` override sets it (`true` when none does), the configuration's
/// own server `olympus` (on until an override disables it), and each server an override mounts.
/// `applies_route: false` is a codex whose route stays off whatever it is told. Each listing it
/// answers is logged, one line, in `codex.js.calls` beside it.
fn package(dir: &std::path::Path, applies_route: bool) -> String {
    let entry = dir.join("codex-acp/dist/index.js");
    std::fs::create_dir_all(entry.parent().expect("dist")).expect("dist");
    std::fs::write(&entry, "// the adapter's entry point").expect("entry");
    let script = dir.join("codex-acp/node_modules/@openai/codex/bin/codex.js");
    std::fs::create_dir_all(script.parent().expect("bin")).expect("bin");
    let features = crate::infer::tool_free::DISABLED_FEATURES.join(" ");
    let applies = if applies_route { "True" } else { "False" };
    std::fs::write(
        &script,
        format!(
            r#"#!/usr/bin/env python3
import json, sys
args = sys.argv[1:]
told = dict(args[i + 1].split("=", 1) for i in range(len(args) - 1) if args[i] == "-c")
if not {applies}:
    told["features.{route}"] = "false"
words = [a for i, a in enumerate(args) if a != "-c" and (i == 0 or args[i - 1] != "-c")]
open(__file__ + ".calls", "a").write(" ".join(words) + "\n")
if words[:2] == ["features", "list"]:
    for name in "{features}".split():
        print(name, "stable", told.get("features." + name, "true"))
elif words[:2] == ["mcp", "list"]:
    servers = {{"olympus": True}}
    for key, value in told.items():
        parts = key.split(".")
        if parts[0] == "mcp_servers" and len(parts) == 3 and parts[2] == "enabled":
            servers[parts[1]] = value == "true"
        elif parts[0] == "mcp_servers" and len(parts) == 3 and parts[2] == "url":
            servers.setdefault(parts[1], True)
    print(json.dumps([{{"name": name, "enabled": on}} for name, on in servers.items()]))
"#,
            route = codex::APPROVAL_FEATURE,
        ),
    )
    .expect("script");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("mode");
    entry.to_string_lossy().into_owned()
}

fn nika_server() -> Value {
    json!({"url": "http://127.0.0.1:4242/mcp", "bearer_token_env_var": codex::BEARER_ENV})
}

/// The adapter environment's `CODEX_CONFIG`, parsed.
fn config_of(env: &[(String, String)]) -> Value {
    let (_, config) = (env.iter())
        .find(|(name, _)| name == "CODEX_CONFIG")
        .expect("CODEX_CONFIG");
    serde_json::from_str(config).expect("one JSON object")
}

/// A conversation opens on a profile whose approval route to Nika reads back on, beside the
/// configuration's own server disabled and Nika's mounted; the completion profile reads it off.
#[tokio::test]
async fn a_conversation_reads_codexs_approval_route_back_on() {
    let dir = tempfile::tempdir().expect("dir");
    let row = HarnessAdapter::new("codex", package(dir.path(), true)).expect("id");
    let harness = SpawnedHarness::new(row);
    let env = (harness.codex_profile_env(Some(("nika", nika_server()))))
        .await
        .expect("the conversation profile holds");
    let config = config_of(&env);
    assert_eq!(config["features"][codex::APPROVAL_FEATURE], json!(true));
    assert_eq!(config["mcp_servers"]["olympus"], json!({"enabled": false}));
    assert_eq!(config["mcp_servers"]["nika"], nika_server());
    let completion = (harness.codex_profile_env(None))
        .await
        .expect("the completion profile holds");
    let config = config_of(&completion);
    assert_eq!(config["features"][codex::APPROVAL_FEATURE], json!(false));
}

/// A codex whose route stays off opens no conversation, and the refusal names the feature: on
/// such a profile every ask about Nika's tools ends cancelled without reaching Nika (as a live
/// conversation showed on 2026-10-11).
#[tokio::test]
async fn a_route_that_reads_off_opens_no_conversation() {
    let dir = tempfile::tempdir().expect("dir");
    let row = HarnessAdapter::new("codex", package(dir.path(), false)).expect("id");
    let harness = SpawnedHarness::new(row);
    let refused = (harness.codex_profile_env(Some(("nika", nika_server()))))
        .await
        .expect_err("the route reads off");
    let HarnessError::Refused { reason } = refused else {
        panic!("a refusal: {refused:?}");
    };
    assert!(
        reason.contains("tool_call_mcp_elicitation=false"),
        "{reason}"
    );
    (harness.codex_profile_env(None))
        .await
        .expect("the completion profile needs the route off");
}

/// NIK-385 review · a kept Codex proof holds only while the configuration it was proven against
/// is unchanged: the profile disables the MCP servers that configuration defined, so a
/// configuration written since, or another Codex home, proves the profile again (its listings
/// run again) and a server defined during the run is disabled like the others.
#[tokio::test]
async fn a_kept_codex_proof_is_proven_again_once_its_configuration_changes() {
    let dir = tempfile::tempdir().expect("dir");
    let entry = package(dir.path(), true);
    let calls = dir
        .path()
        .join("codex-acp/node_modules/@openai/codex/bin/codex.js.calls");
    let listings = || {
        std::fs::read_to_string(&calls).map_or(0, |log| {
            log.lines()
                .filter(|line| line.starts_with("mcp list"))
                .count()
        })
    };
    let seat = SpawnedHarness::new(HarnessAdapter::new("codex", entry).expect("id"))
        .for_completion(crate::authoring::acp::Completion::Infer)
        .expect("audited");
    let stamp = |home: &std::path::Path| {
        let home = home.to_string_lossy().into_owned();
        Some(codex::config_stamp(&BTreeMap::from([(
            "CODEX_HOME".to_owned(),
            home,
        )])))
    };
    let (home, other) = (dir.path().join("codex-home"), dir.path().join("other-home"));
    std::fs::create_dir_all(&home).expect("home");
    std::fs::create_dir_all(&other).expect("other");
    seat.prove_against(stamp(&home), None)
        .await
        .expect("proven");
    assert_eq!(listings(), 2, "the defined servers, then the read-back");
    seat.prove_against(stamp(&home), None).await.expect("kept");
    assert_eq!(listings(), 2, "an unchanged configuration keeps the proof");
    std::fs::write(
        home.join("config.toml"),
        "[mcp_servers.added]\ncommand = \"x\"\n",
    )
    .expect("config");
    seat.prove_against(stamp(&home), None)
        .await
        .expect("proven again");
    assert_eq!(
        listings(),
        4,
        "a configuration written since is proven again"
    );
    seat.prove_against(stamp(&home), None).await.expect("kept");
    assert_eq!(listings(), 4, "and that proof is kept");
    seat.prove_against(stamp(&other), None)
        .await
        .expect("proven again");
    assert_eq!(listings(), 6, "another Codex home is proven again");
}

/// The stamp reads the home the adapter's codex resolves: `CODEX_HOME`, else `$HOME/.codex`.
#[test]
fn the_stamp_reads_codex_home_before_the_home_default() {
    let dir = tempfile::tempdir().expect("dir");
    let (home, codex_home) = (dir.path().join("home"), dir.path().join("codex"));
    std::fs::create_dir_all(home.join(".codex")).expect("default");
    std::fs::create_dir_all(&codex_home).expect("codex home");
    std::fs::write(home.join(".codex/config.toml"), "model = \"a\"\n").expect("default config");
    let text = |path: &std::path::Path| path.to_string_lossy().into_owned();
    let by_home = BTreeMap::from([("HOME".to_owned(), text(&home))]);
    let mut by_codex_home = by_home.clone();
    by_codex_home.insert("CODEX_HOME".to_owned(), text(&codex_home));
    let (default, chosen) = (
        codex::config_stamp(&by_home),
        codex::config_stamp(&by_codex_home),
    );
    assert_ne!(default, chosen, "CODEX_HOME wins over $HOME/.codex");
    std::fs::write(codex_home.join("config.toml"), "model = \"b\"\n").expect("codex config");
    assert_ne!(
        chosen,
        codex::config_stamp(&by_codex_home),
        "its config.toml is read"
    );
    assert_eq!(
        default,
        codex::config_stamp(&by_home),
        "an untouched one reads the same"
    );
}

/// NIK-385 review · every machine layer that can add or enable an MCP server is stamped, so a
/// kept proof is proven again once one is written: the system `config.toml`, the managed
/// defaults `managed_config.toml`, `requirements.toml` (its `mcp_servers` allowlist enables
/// servers), and the `com.openai.codex` managed preferences, device-wide and per user. The
/// machine roots are temporary directories standing in for `/etc/codex` and macOS's managed
/// preferences.
#[test]
fn every_layer_that_can_add_a_server_is_stamped() {
    let dir = tempfile::tempdir().expect("dir");
    let (home, system, preferences) = (
        dir.path().join("codex-home"),
        dir.path().join("etc-codex"),
        dir.path().join("managed-preferences"),
    );
    for made in [&home, &system, &preferences] {
        std::fs::create_dir_all(made).expect("root");
    }
    let env = BTreeMap::from([("CODEX_HOME".to_owned(), home.to_string_lossy().into_owned())]);
    let stamp = || codex::config_stamp_in(&env, &system, Some(&preferences));
    let server = "[mcp_servers.added]\ncommand = \"x\"\n";
    let user = preferences.join("someone");
    std::fs::create_dir_all(&user).expect("user preferences");
    for layer in [
        system.join("config.toml"),
        system.join("managed_config.toml"),
        system.join("requirements.toml"),
        preferences.join("com.openai.codex.plist"),
        user.join("com.openai.codex.plist"),
    ] {
        let before = stamp();
        std::fs::write(&layer, server).expect("layer");
        assert_ne!(before, stamp(), "{} is stamped", layer.display());
    }
    let settled = stamp();
    std::fs::write(home.join("work.config.toml"), server).expect("profile file");
    assert_eq!(
        settled,
        stamp(),
        "a profile file loads only under --profile, which the adapter never passes"
    );
}
