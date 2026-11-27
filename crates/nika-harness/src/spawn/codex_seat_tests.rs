// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>
//! The Codex seat's sessions against keyless scripted adapters: every session kind is judged
//! where its process starts, a scratch included, and one that would see a project's `.codex`
//! configuration starts no adapter; a seat is recognised by what it runs, not by its id; the
//! `initialize`-only probe is not a session and is not judged; every session of the seat
//! carries prompt suggestions off in its environment, where another seat's carries nothing of
//! the kind.
use super::*;
use crate::authoring::acp::{Completion, codex};
use crate::conversation::{ConversationSetup, ToolOffer};
use futures_core::Stream as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// A scripted adapter under the seat `id`: each start appends one line to `starts.log` in
/// `dir`, the value its environment gives the suggestions variable (`<absent>` without one);
/// then it exits without a word.
fn seat(dir: &Path, id: &str) -> (SpawnedHarness, PathBuf) {
    let starts = dir.join("starts.log");
    let script = dir.join(format!("{id}-adapter.py"));
    std::fs::write(
        &script,
        format!(
            r#"import os
value = os.environ.get({name:?}, "<absent>")
open({starts:?}, "a").write(value + "\n")
"#,
            name = codex::SUGGESTIONS_ENV.0,
            starts = starts.to_string_lossy(),
        ),
    )
    .expect("script");
    let row = HarnessAdapter::new(id, "python3")
        .expect("id is fine")
        .with_args(vec![script.to_string_lossy().into_owned()]);
    (SpawnedHarness::new(row), starts)
}

/// The measured npm layout of the `codex` seat around a scripted bundled codex whose profile
/// reads back as it should ([`super::codex_profile_tests::package`]), with an entry point that
/// logs each start (the suggestions variable's value, `<absent>` without one) to `starts.log`
/// in `dir`, answers one `initialize` as codex-acp 1.13.1 and exits. A handshake row: its
/// identity is that answer, and a conversation probes it once before its session.
fn bundled(dir: &Path) -> (SpawnedHarness, PathBuf) {
    let entry = super::codex_profile_tests::package(dir, true);
    let starts = dir.join("starts.log");
    std::fs::write(
        &entry,
        format!(
            r#"#!/usr/bin/env python3
import json, os, sys
open({starts:?}, "a").write(os.environ.get({name:?}, "<absent>") + "\n")
line = sys.stdin.readline()
if line:
    asked = json.loads(line)
    info = {{"name": {agent:?}, "version": {version:?}}}
    result = {{"protocolVersion": 1, "agentInfo": info, "agentCapabilities": {{}}}}
    print(json.dumps({{"jsonrpc": "2.0", "id": asked["id"], "result": result}}), flush=True)
"#,
            starts = starts.to_string_lossy(),
            name = codex::SUGGESTIONS_ENV.0,
            agent = codex::NAME,
            version = codex::VERSION,
        ),
    )
    .expect("entry point");
    std::fs::set_permissions(&entry, std::fs::Permissions::from_mode(0o755)).expect("mode");
    let row = (HarnessAdapter::new("codex", &entry).expect("id"))
        .with_handshake_probe()
        .with_identities(vec![codex::NAME.to_owned()]);
    (SpawnedHarness::new(row), starts)
}

/// What each start of a scripted adapter recorded, in order.
fn starts(log: &Path) -> Vec<String> {
    std::fs::read_to_string(log).map_or_else(
        |_| Vec::new(),
        |text| text.lines().map(str::to_owned).collect(),
    )
}

/// A project marked as a repository root, holding `.codex/config.toml`; returns the project and
/// that file.
fn configured_project(tree: &Path) -> (PathBuf, PathBuf) {
    let project = tree.join("project");
    std::fs::create_dir_all(project.join(".git")).expect("repository");
    std::fs::create_dir_all(project.join(".codex")).expect(".codex");
    let config = project.join(".codex/config.toml");
    std::fs::write(&config, "[mcp_servers.project]\ncommand = \"x\"\n").expect("config");
    (project, config)
}

/// A directory created as a scratch would be, inside `parent`.
fn scratch_in(parent: &Path) -> PathBuf {
    let scratch = parent.join(".tmpQ7rX2a");
    std::fs::create_dir_all(&scratch).expect("scratch");
    scratch
}

/// The refusal a session that would see `entry` meets, as it reaches the caller.
fn refusal_naming(entry: &Path) -> String {
    format!(
        "Codex would apply the project configuration `{}` to this session: codex-acp trusts \
         the session's directory, and Codex reads the `.codex` entry of that directory and of \
         each parent up to the repository root; no session was opened; no fallback (move the \
         entry out of the project, or run the task from a directory that has none)",
        entry.display()
    )
}

/// The refusal `result` carries; a test failure for any other outcome.
fn refused<T>(result: Result<T, HarnessError>) -> String {
    match result {
        Err(HarnessError::Refused { reason }) => reason,
        Err(other) => panic!("a typed refusal, got {other:?}"),
        Ok(_) => panic!("a typed refusal, got a session"),
    }
}

/// Nika's tool server as a conversation offers it.
fn offer() -> ToolOffer {
    let tools = vec!["candidate_read".to_owned()];
    ToolOffer::new("nika", tools).with_http("http://127.0.0.1:4242/mcp", "5e".repeat(32))
}

/// Read the session's stream to its end (the scripted adapter exits at once), bounded.
async fn drain(mut stream: HarnessEventStream) {
    let read = async {
        while std::future::poll_fn(|cx| std::pin::Pin::new(&mut stream).poll_next(cx))
            .await
            .is_some()
        {}
    };
    tokio::time::timeout(Duration::from_secs(20), read)
        .await
        .expect("the scripted adapter exits at once");
}

/// An agent task whose project, or a parent up to the repository root, holds a `.codex`
/// configuration is refused where its process would start, the file named.
#[tokio::test]
async fn an_agent_session_that_would_see_project_configuration_starts_nothing() {
    let tree = tempfile::tempdir().expect("tree");
    let (seat, log) = seat(tree.path(), "codex");
    let (project, config) = configured_project(tree.path());
    for cwd in [project.clone(), project.join("docs/guides")] {
        std::fs::create_dir_all(&cwd).expect("session directory");
        let reason =
            refused((seat.run_agent(HarnessRequest::new("summarize the ledger", &cwd))).await);
        assert_eq!(reason, refusal_naming(&config), "{}", cwd.display());
    }
    assert_eq!(starts(&log), [] as [String; 0], "no adapter started");
}

/// A scratch the engine created inside a repository whose root holds a `.codex` is judged like
/// any session directory: a one-shot's spawn, an offer and a conversation are refused, naming
/// the parent's file, and no session process starts. The conversation's `initialize`-only
/// probe is not a session: it ran once, without the suggestions variable, before the refusal.
#[tokio::test]
async fn a_scratch_inside_a_configured_repository_refuses_every_kind() {
    let tree = tempfile::tempdir().expect("tree");
    let (project, config) = configured_project(tree.path());
    let temporary = project.join("target/tmp");
    let scratch = scratch_in(&temporary);

    let (python, log) = seat(tree.path(), "codex");
    let one_shot = (python.clone().for_completion(Completion::Infer)).expect("audited");
    let profile = [("CODEX_CONFIG".to_owned(), "{}".to_owned())];
    let reason = refused(one_shot.spawn_child(&scratch, true, &profile));
    assert_eq!(reason, refusal_naming(&config), "one-shot");
    let reason = refused(
        python
            .offer_in(&scratch, None, Duration::from_secs(10))
            .await,
    );
    assert_eq!(reason, refusal_naming(&config), "offer");
    assert_eq!(
        starts(&log),
        [] as [String; 0],
        "no one-shot or offer adapter"
    );

    let package = tree.path().join("package");
    std::fs::create_dir_all(&package).expect("package");
    let (bundled, bundled_log) = bundled(&package);
    let opened = bundled
        .converse_in(Some(&temporary), ConversationSetup::new(), offer())
        .await;
    assert_eq!(refused(opened), refusal_naming(&config), "conversation");
    assert_eq!(
        starts(&bundled_log),
        ["<absent>"],
        "the initialize probe alone, which carries no session environment"
    );
}

/// Every session kind of the Codex seat starts its adapter with prompt suggestions off: an
/// agent task, an offer and a one-shot spawn on a scripted seat, and a conversation on the
/// bundled layout, whose `initialize`-only probe runs first without it. Another seat in a
/// project that holds a `.codex` is not judged on it, and its environment holds no such
/// variable.
#[tokio::test]
async fn every_codex_session_kind_starts_with_suggestions_off() {
    let tree = tempfile::tempdir().expect("tree");
    let clean = tree.path().join("clean");
    std::fs::create_dir_all(clean.join(".git")).expect("repository");
    let (codex, codex_log) = seat(tree.path(), "codex");
    let stream = (codex.run_agent(HarnessRequest::new("p", &clean)))
        .await
        .expect("the agent session starts");
    drain(stream).await;
    assert_eq!(starts(&codex_log), ["false"], "agent");
    let offered = codex.offer_in(&clean, None, Duration::from_secs(10)).await;
    assert!(
        !matches!(&offered, Err(HarnessError::Refused { .. })),
        "{offered:?}"
    );
    assert_eq!(starts(&codex_log), ["false", "false"], "offer");
    let one_shot = (codex.clone().for_completion(Completion::Infer)).expect("audited");
    let profile = [("CODEX_CONFIG".to_owned(), "{}".to_owned())];
    let mut child = (one_shot.spawn_child(&clean, true, &profile)).expect("the spawn");
    let _ = tokio::time::timeout(Duration::from_secs(20), child.wait()).await;
    assert_eq!(starts(&codex_log), ["false", "false", "false"], "one-shot");

    let package = tree.path().join("package");
    std::fs::create_dir_all(&package).expect("package");
    let (bundled, bundled_log) = bundled(&package);
    let temporary = clean.join("tmp");
    std::fs::create_dir_all(&temporary).expect("temporary");
    let opened = bundled
        .converse_in(Some(&temporary), ConversationSetup::new(), offer())
        .await;
    assert!(
        !matches!(&opened, Err(HarnessError::Refused { .. })),
        "past the spawn: {:?}",
        opened.as_ref().err()
    );
    assert_eq!(
        starts(&bundled_log),
        ["<absent>", "false"],
        "the probe, then the conversation's session"
    );

    let other = tree.path().join("other");
    std::fs::create_dir_all(&other).expect("other seat's tree");
    let (claude, claude_log) = seat(&other, "claude-code");
    let (project, _) = configured_project(&other);
    let stream = (claude.run_agent(HarnessRequest::new("p", &project)))
        .await
        .expect("another seat is not judged on Codex's configuration");
    drain(stream).await;
    assert_eq!(starts(&claude_log), ["<absent>"]);
}

/// A row declared under another id is the Codex seat when it runs codex-acp: by its program's
/// file name (no process starts here: the project refusal comes first, then the host switch
/// keeps the program absent), or by a name it declares its adapter reports (a scripted adapter
/// then starts, with suggestions off, in a clean project).
#[tokio::test]
async fn codex_acp_declared_under_another_id_is_the_codex_seat() {
    let tree = tempfile::tempdir().expect("tree");
    let (project, config) = configured_project(tree.path());
    let program = tree.path().join("bin/codex-acp");
    let by_program = SpawnedHarness::new(
        HarnessAdapter::new("codex-dev", program.to_string_lossy()).expect("id"),
    )
    .with_hosts(crate::hosts::Hosts::Off);
    let reason = refused((by_program.run_agent(HarnessRequest::new("p", &project))).await);
    assert_eq!(reason, refusal_naming(&config), "by its program");
    let clean = tree.path().join("clean");
    std::fs::create_dir_all(clean.join(".git")).expect("repository");
    let absent = (by_program.run_agent(HarnessRequest::new("p", &clean))).await;
    let Err(HarnessError::Unavailable { reason }) = absent else {
        panic!("the host switch keeps it absent: {:?}", absent.err());
    };
    assert_eq!(
        reason,
        format!(
            "adapter `codex-dev`: `{}` is not started: NIKA_HARNESS_HOSTS=off keeps every host \
             program absent (set NIKA_HARNESS_HOSTS=on to start it)",
            program.display()
        )
    );
    let env = by_program.session_env(&BTreeMap::new(), &[]);
    assert_eq!(
        env.get(codex::SUGGESTIONS_ENV.0).map(String::as_str),
        Some("false")
    );

    let scripted = tree.path().join("scripted");
    std::fs::create_dir_all(&scripted).expect("scripted");
    let (python, log) = seat(&scripted, "acp-local");
    let by_name =
        SpawnedHarness::new((python.adapter.clone()).with_identities(vec!["codex-acp".to_owned()]));
    let reason = refused((by_name.run_agent(HarnessRequest::new("p", &project))).await);
    assert_eq!(reason, refusal_naming(&config), "by its reported name");
    let stream = (by_name.run_agent(HarnessRequest::new("p", &clean)))
        .await
        .expect("a clean project starts it");
    drain(stream).await;
    assert_eq!(starts(&log), ["false"]);
    let other = SpawnedHarness::new(
        (python.adapter.clone()).with_identities(vec!["Claude Agent".to_owned()]),
    );
    assert_eq!(
        other
            .session_env(&BTreeMap::new(), &[])
            .get(codex::SUGGESTIONS_ENV.0),
        None
    );
}

/// Every session kind of the Codex seat starts its adapter with prompt suggestions off, over
/// the parent's own value, which a row that passes the variable through would otherwise hand
/// over, and beside what its kind adds (nothing for an agent task, the proven profile for a
/// one-shot, the profile and Nika's bearer for a conversation); another seat's environment is
/// the composed one and its profile's, nothing more.
#[test]
fn every_codex_session_kind_carries_prompt_suggestions_off() {
    let parent: BTreeMap<String, String> = [
        ("PATH", "/usr/bin"),
        ("HOME", "/home/u"),
        ("CODEX_HOME", "/home/u/.codex"),
        (codex::SUGGESTIONS_ENV.0, "true"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect();
    let pair = |name: &str, value: &str| (name.to_owned(), value.to_owned());
    let one_shot = vec![
        pair("CODEX_CONFIG", "{}"),
        pair("CODEX_PATH", "/opt/codex.js"),
    ];
    let mut conversation = one_shot.clone();
    conversation.push(pair(codex::BEARER_ENV, "bearer-of-this-conversation"));
    let rows = crate::registry_with(&|_| None).expect("registry");
    let shipped = |id: &str| {
        let row = rows
            .iter()
            .find(|row| row.adapter.id == id)
            .expect("a shipped row");
        SpawnedHarness::new(row.adapter.clone())
    };
    let passing = (shipped("codex").adapter.clone()).with_passthrough_env(vec![
        "CODEX_HOME".to_owned(),
        codex::SUGGESTIONS_ENV.0.to_owned(),
    ]);
    for codex_seat in [shipped("codex"), SpawnedHarness::new(passing)] {
        for (kind, profile) in [
            ("agent", Vec::new()),
            ("one-shot", one_shot.clone()),
            ("conversation", conversation.clone()),
        ] {
            let env = codex_seat.session_env(&parent, &profile);
            let mut expected: BTreeMap<String, String> = [
                pair("CODEX_HOME", "/home/u/.codex"),
                pair(codex::SUGGESTIONS_ENV.0, codex::SUGGESTIONS_ENV.1),
                pair("HOME", "/home/u"),
                pair("PATH", "/usr/bin"),
            ]
            .into_iter()
            .collect();
            expected.extend(profile);
            let passes = &codex_seat.adapter.passthrough_env;
            assert_eq!(env, expected, "{kind} · passthrough {passes:?}");
            assert_eq!(
                env.get("CODEX_ACP_PROMPT_SUGGESTIONS").map(String::as_str),
                Some("false"),
                "{kind}"
            );
        }
    }
    let claude = shipped("claude-code");
    let env = claude.session_env(&parent, &one_shot);
    assert_eq!(
        env.keys().map(String::as_str).collect::<Vec<_>>(),
        ["CODEX_CONFIG", "CODEX_PATH", "HOME", "PATH"]
    );
}
