// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>
//! A task's declared turn limit on an agent session: sent as the Claude Code session's
//! `maxTurns` beside its closed options, or refused before `session/new`. Duplex ACP peers; no
//! installed harness, credential or model is consulted.
use super::*;
use nika_types::access::{HarnessBoundary, HarnessRuntime};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, ReadHalf, WriteHalf};

async fn read(r: &mut BufReader<ReadHalf<DuplexStream>>) -> Value {
    let mut line = String::new();
    r.read_line(&mut line).await.unwrap();
    serde_json::from_str(&line).unwrap()
}

async fn write(w: &mut WriteHalf<DuplexStream>, value: Value) {
    w.write_all(format!("{value}\n").as_bytes()).await.unwrap();
}

/// A Claude Code agent session's options with `maxTurns` set, every isolation key at its exact
/// value.
fn limited(max_turns: u32) -> Value {
    json!({"claudeCode": {"options": {
        "settingSources": [], "strictMcpConfig": true, "mcpServers": {},
        "plugins": [], "skills": [], "agents": {},
        "allowDangerouslySkipPermissions": false,
        "env": {"CLAUDE_CODE_DISABLE_AUTO_MEMORY": "1",
                "ENABLE_CLAUDEAI_MCP_SERVERS": "false"},
        "thinking": {"type": "adaptive", "display": "summarized"},
        "maxTurns": max_turns}}})
}

fn answering(name: &str) -> Value {
    json!({"protocolVersion":1,"agentInfo":{"name":name,"version":"1.0.0"}})
}

/// The client's refusal of a declared turn limit no session option carries.
const REFUSED: &str = "this session cannot carry the task's `max_turns`: its options hold no \
     turn limit (only a Claude Code agent session's do); no session was opened; drop \
     `max_turns:` or run the task as an `agent:` on the `claude-code` seat, or on an API or \
     local model";

/// A refusal's reason, `None` for any other error.
fn refusal_reason(error: &HarnessError) -> Option<&str> {
    match error {
        HarnessError::Refused { reason } => Some(reason.as_str()),
        _ => None,
    }
}

/// Only the `claude-code` seat is sent a declared turn limit: every other shipped seat, a local
/// row of another id and the retired ACP speaker names are not.
#[test]
fn only_the_claude_code_seat_is_sent_a_declared_turn_limit() {
    for runtime in HarnessRuntime::ALL {
        assert_eq!(
            applies_max_turns(runtime.id),
            runtime.id == "claude-code",
            "{}",
            runtime.id
        );
    }
    for other in [
        "my-claude",
        "claude-agent-acp",
        "codex-acp",
        "",
        "Claude-Code",
    ] {
        assert!(!applies_max_turns(other), "{other:?}");
    }
}

/// The limit is the one option added: the closed surface keeps every key and its value, and a
/// task without a limit sends the options exactly as before, with no `maxTurns`.
#[test]
fn a_declared_limit_rides_the_claude_code_agent_options_as_max_turns() {
    let carried = carry_max_turns(agent_meta("claude-code"), Some(7), false).unwrap();
    assert_eq!(carried, Some(limited(7)));
    assert_eq!(
        carried.unwrap().to_string(),
        concat!(
            r#"{"claudeCode":{"options":{"agents":{},"allowDangerouslySkipPermissions":false,"#,
            r#""env":{"CLAUDE_CODE_DISABLE_AUTO_MEMORY":"1","#,
            r#""ENABLE_CLAUDEAI_MCP_SERVERS":"false"},"maxTurns":7,"mcpServers":{},"#,
            r#""plugins":[],"settingSources":[],"skills":[],"strictMcpConfig":true,"#,
            r#""thinking":{"display":"summarized","type":"adaptive"}}}}"#
        )
    );
    let unlimited = carry_max_turns(agent_meta("claude-code"), None, false).unwrap();
    assert_eq!(unlimited, agent_meta("claude-code"));
    let options = unlimited.unwrap();
    assert!(options["claudeCode"]["options"].get("maxTurns").is_none());
    assert_eq!(carry_max_turns(None, None, false).unwrap(), None);
    // A one-shot without a declared limit keeps its audited profile (`maxTurns: 1`) as is.
    assert_eq!(
        carry_max_turns(Some(profile()), None, true).unwrap(),
        Some(profile())
    );
}

/// A limit no session option can carry is refused, never dropped: no options (Codex, an
/// adapter Nika could not place), options of another shape, and a one-shot, whose audited
/// profile is never rewritten.
#[test]
fn a_declared_limit_with_no_option_to_carry_it_is_refused() {
    for (meta, one_shot) in [
        (None, false),
        (Some(json!({"other": {"options": {}}})), false),
        (Some(json!({"claudeCode": {}})), false),
        (agent_meta("claude-code"), true),
        (Some(profile()), true),
    ] {
        let error = carry_max_turns(meta.clone(), Some(7), one_shot).unwrap_err();
        assert_eq!(refusal_reason(&error), Some(REFUSED), "{meta:?} {one_shot}");
    }
}

/// The scripted peer of one agent session: answers `initialize` with `answers`, then returns
/// the `session/new` it reads (after ending the one prompt), or `None` when the client wrote
/// nothing more after `initialize`.
fn peer(theirs: DuplexStream, answers: Value) -> tokio::task::JoinHandle<Option<Value>> {
    let (r2, mut w2) = tokio::io::split(theirs);
    tokio::spawn(async move {
        let mut r2 = BufReader::new(r2);
        let init = read(&mut r2).await;
        write(
            &mut w2,
            json!({"jsonrpc":"2.0","id":init["id"],"result":answers}),
        )
        .await;
        let mut line = String::new();
        if r2.read_line(&mut line).await.unwrap() == 0 {
            return None;
        }
        let new: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(new["method"], "session/new");
        write(
            &mut w2,
            json!({"jsonrpc":"2.0","id":new["id"],"result":{"sessionId":"s"}}),
        )
        .await;
        let prompt = read(&mut r2).await;
        assert_eq!(prompt["method"], "session/prompt");
        write(
            &mut w2,
            json!({"jsonrpc":"2.0","id":prompt["id"],"result":{"stopReason":"end_turn"}}),
        )
        .await;
        Some(new)
    })
}

/// The first event of the session, past its progress.
async fn first_outcome(
    stream: &mut nika_kernel::ai::harness::HarnessEventStream,
) -> Result<HarnessEvent, HarnessError> {
    loop {
        let event = std::future::poll_fn(|cx| Pin::new(&mut *stream).poll_next(cx))
            .await
            .expect("the session reports an outcome");
        match event {
            Ok(HarnessEvent::Completed { .. }) | Err(_) => return event,
            Ok(_) => {}
        }
    }
}

/// On both meta paths the declared limit reaches `session/new` as `maxTurns` beside the exact
/// isolation keys: the options the spawner chose for the `claude-code` seat, and the options
/// the client follows from the Claude Code adapter that answered `initialize` on a row of
/// another id. Without a declared limit the same sessions send no `maxTurns`.
#[tokio::test]
async fn the_limit_reaches_session_new_on_both_meta_paths() {
    let claude = answering(HarnessBoundary::CLAUDE_AGENT_ACP.agents[0]);
    for (path, meta) in [
        ("spawner", agent_meta("claude-code")),
        ("follow", agent_meta("my-claude")),
    ] {
        for max_turns in [Some(7), None] {
            let (ours, theirs) = tokio::io::duplex(65536);
            let (r, w) = tokio::io::split(ours);
            let peer = peer(theirs, claude.clone());
            let request = HarnessRequest::new("go", "/tmp").with_max_turns(max_turns);
            let mut stream = crate::client::drive_profile(
                r,
                w,
                request,
                Duration::from_secs(1),
                None,
                None,
                meta.clone(),
                None,
            );
            let outcome = first_outcome(&mut stream).await;
            assert!(
                matches!(outcome, Ok(HarnessEvent::Completed { .. })),
                "{path} {max_turns:?}: {outcome:?}"
            );
            let new = tokio::time::timeout(Duration::from_secs(1), peer)
                .await
                .unwrap()
                .unwrap()
                .expect("session/new was sent");
            let expected = match max_turns {
                Some(limit) => limited(limit),
                None => agent_meta("claude-code").unwrap(),
            };
            assert_eq!(new["params"]["_meta"], expected, "{path} {max_turns:?}");
        }
    }
}

/// A declared limit on a session whose options cannot carry it is refused after `initialize`
/// and before `session/new`, the parameter named: a Codex adapter, an adapter that names
/// itself after no measured row, and a one-shot. The peer reads no `session/new`.
#[tokio::test]
async fn a_limit_no_session_option_carries_stops_before_session_new() {
    let codex = answering(HarnessBoundary::CODEX_ACP.agents[0]);
    let unnamed = json!({"protocolVersion":1});
    let claude = || OneShot {
        role: Completion::Authoring,
        profile: Profile::ClaudeCode,
    };
    let one_shot_identity =
        json!({"protocolVersion":1,"agentInfo":{"name":NAME,"version":VERSION}});
    for (case, answers, completion, meta) in [
        ("codex", codex, None, agent_meta("codex")),
        ("unnamed", unnamed, None, None),
        (
            "one-shot",
            one_shot_identity,
            Some(claude()),
            claude().session_meta(),
        ),
    ] {
        let (ours, theirs) = tokio::io::duplex(65536);
        let (r, w) = tokio::io::split(ours);
        let peer = peer(theirs, answers);
        let request = HarnessRequest::new("go", "/tmp").with_max_turns(Some(7));
        let mut stream = crate::client::drive_profile(
            r,
            w,
            request,
            Duration::from_secs(1),
            completion,
            None,
            meta,
            None,
        );
        let outcome = first_outcome(&mut stream).await;
        let error = outcome.expect_err("refused before session/new");
        assert_eq!(refusal_reason(&error), Some(REFUSED), "{case}");
        drop(stream);
        let after_initialize = tokio::time::timeout(Duration::from_secs(1), peer)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after_initialize, None, "{case}: no session/new was written");
    }
}
