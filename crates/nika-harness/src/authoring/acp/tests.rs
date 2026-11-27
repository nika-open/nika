// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>
//! Duplex ACP peers; no installed harness, credential or model is consulted.
use super::*;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, ReadHalf, WriteHalf};

/// The authoring role under the Claude Code profile.
fn claude() -> OneShot {
    OneShot {
        role: Completion::Authoring,
        profile: Profile::ClaudeCode,
    }
}

fn identity() -> Value {
    json!({"protocolVersion":1,"agentInfo":{"name":NAME,"version":VERSION}})
}
async fn read(r: &mut BufReader<ReadHalf<DuplexStream>>) -> Value {
    let mut line = String::new();
    r.read_line(&mut line).await.unwrap();
    serde_json::from_str(&line).unwrap()
}
async fn write(w: &mut WriteHalf<DuplexStream>, value: Value) {
    w.write_all(format!("{value}\n").as_bytes()).await.unwrap();
}

#[test]
fn the_profile_disables_builtins_disk_settings_and_other_mcp_before_query() {
    let meta = profile();
    let options = &meta["claudeCode"]["options"];
    for key in ["tools", "settingSources", "plugins", "skills"] {
        assert_eq!(options[key], json!([]));
    }
    assert_eq!(options["mcpServers"], json!({}));
    assert_eq!(options["strictMcpConfig"], true);
    assert_eq!(options["maxTurns"], 1);
    assert_eq!(options["allowDangerouslySkipPermissions"], false);
    assert_eq!(options["persistSession"], false);
    assert!(admit(&identity(), claude()).is_ok());
    for value in [
        json!({}),
        json!({"protocolVersion":1,"agentInfo":{"name":NAME,"version":"0.23.1"}}),
        json!({"protocolVersion":1,"agentInfo":{"name":"Codex","version":VERSION}}),
    ] {
        assert!(admit(&value, claude()).is_err());
    }
}

/// The exact bytes the two closed profiles send (a one-shot, a conversation): building them
/// another way must not move one of them. Object keys serialize sorted, since the engine never
/// enables `serde_json`'s `preserve_order`.
#[test]
fn the_closed_profiles_send_these_exact_bytes() {
    assert_eq!(
        profile().to_string(),
        concat!(
            r#"{"claudeCode":{"options":{"agents":{},"allowDangerouslySkipPermissions":false,"#,
            r#""maxTurns":1,"mcpServers":{},"persistSession":false,"plugins":[],"#,
            r#""settingSources":[],"skills":[],"strictMcpConfig":true,"#,
            r#""thinking":{"display":"summarized","type":"adaptive"},"tools":[]}}}"#
        )
    );
    assert_eq!(
        conversation_profile().to_string(),
        concat!(
            r#"{"claudeCode":{"options":{"agents":{},"allowDangerouslySkipPermissions":false,"#,
            r#""mcpServers":{},"persistSession":false,"plugins":[],"#,
            r#""settingSources":[],"skills":[],"strictMcpConfig":true,"#,
            r#""thinking":{"display":"summarized","type":"adaptive"},"tools":[]}}}"#
        )
    );
}

/// What an agent session on the Claude Code seat sends with `session/new`: the shared closed
/// surface (no settings file, strict MCP naming no server, no plugin, skill or custom agent,
/// the bypass mode withheld), auto memory and claude.ai connectors turned off in the session's
/// environment, and its thinking shown. It keeps its built-in tools and its own loop: no
/// `tools`, no `maxTurns`, and no `persistSession`, which this session does not decide.
#[test]
fn a_claude_code_agent_session_loads_nothing_of_the_users_configuration() {
    let meta = agent_meta("claude-code").expect("a Claude Code agent session sends options");
    assert_eq!(
        meta,
        json!({"claudeCode": {"options": {
            "settingSources": [], "strictMcpConfig": true, "mcpServers": {},
            "plugins": [], "skills": [], "agents": {},
            "allowDangerouslySkipPermissions": false,
            "env": {"CLAUDE_CODE_DISABLE_AUTO_MEMORY": "1",
                    "ENABLE_CLAUDEAI_MCP_SERVERS": "false"},
            "thinking": {"type": "adaptive", "display": "summarized"}}}})
    );
    assert_eq!(
        meta.to_string(),
        concat!(
            r#"{"claudeCode":{"options":{"agents":{},"allowDangerouslySkipPermissions":false,"#,
            r#""env":{"CLAUDE_CODE_DISABLE_AUTO_MEMORY":"1","#,
            r#""ENABLE_CLAUDEAI_MCP_SERVERS":"false"},"mcpServers":{},"plugins":[],"#,
            r#""settingSources":[],"skills":[],"strictMcpConfig":true,"#,
            r#""thinking":{"display":"summarized","type":"adaptive"}}}}"#
        )
    );
    let options = meta["claudeCode"]["options"]
        .as_object()
        .expect("an object of options");
    for kept_to_the_agent in ["tools", "maxTurns", "persistSession"] {
        assert!(
            !options.contains_key(kept_to_the_agent),
            "{kept_to_the_agent} must not be sent to an agent session"
        );
    }
    // The three profiles share every key of the closed surface, with the same value.
    for other in [profile(), conversation_profile()] {
        for key in [
            "settingSources",
            "strictMcpConfig",
            "mcpServers",
            "plugins",
            "skills",
            "agents",
            "allowDangerouslySkipPermissions",
        ] {
            assert_eq!(other["claudeCode"]["options"][key], options[key], "{key}");
        }
        // The two switches belong to the agent session alone: the closed profiles are unchanged.
        assert!(other["claudeCode"]["options"].get("env").is_none());
    }
}

/// Every Claude Code session asks to see its thinking, whatever its role: the audited one-shot
/// profile and an agent session both carry summarized adaptive thinking beside their own
/// options. A thinking option already named is kept as given, a meta of another shape is left
/// as it is, and Codex or any other adapter sends no options at all.
#[test]
fn every_claude_code_session_asks_to_see_its_thinking() {
    let shown = json!({"type": "adaptive", "display": "summarized"});
    assert_eq!(profile()["claudeCode"]["options"]["thinking"], shown);
    assert_eq!(claude().session_meta(), Some(profile()));
    let agent = agent_meta("claude-code").expect("a Claude Code agent session sends options");
    assert_eq!(agent["claudeCode"]["options"]["thinking"], shown);
    let codex = OneShot {
        role: Completion::Infer,
        profile: Profile::Codex,
    };
    assert_eq!(codex.session_meta(), None);
    for adapter in ["codex", "gemini", "kimi-code", "qwen-code", "claude"] {
        assert_eq!(agent_meta(adapter), None, "{adapter}");
    }
    let own = json!({"claudeCode": {"options": {"thinking": {"type": "disabled"}, "maxTurns": 1}}});
    assert_eq!(
        show_thinking(own.clone()),
        own,
        "the caller's own thinking is kept"
    );
    for shape in [
        json!({"claudeCode": "x"}),
        json!({"claudeCode": {"options": []}}),
        json!(7),
    ] {
        assert_eq!(show_thinking(shape.clone()), shape);
    }
    let beside = show_thinking(json!({"vendor": {"k": 1}}));
    assert_eq!(
        beside,
        json!({"vendor": {"k": 1}, "claudeCode": {"options": {"thinking": shown}}})
    );
}

#[test]
fn text_only_and_configured_identity_never_become_served_or_free() {
    for tag in ["tool_call", "tool_call_update", "unexpected"] {
        assert!(judge_update(&json!({"sessionUpdate":tag}), claude()).is_err());
    }
    assert!(judge_update(&json!({"sessionUpdate":"agent_message_chunk","content":{"type":"image","data":"ignored"}}), claude()).is_err());
    assert!(
        judge_update(
            &json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"whole"}}),
            claude()
        )
        .is_ok()
    );
    let d = descriptor(
        Profile::ClaudeCode,
        "claude-code",
        Some("claude-code/test"),
        "claude-code/test",
        &[],
    );
    assert!(d["served_model"].is_null());
    assert!(d["billed_cost_usd"].is_null());
    assert_eq!(d["numeric_usage_reported"], false);
    assert!(
        d["tools_exposed"]
            .as_str()
            .is_some_and(|t| t.starts_with("none"))
    );
    // The Codex receipt names its residue: never « none », never empty tools.
    let codex = descriptor(Profile::Codex, "codex", None, "session", &[]);
    let tools = codex["tools_exposed"].as_str().expect("text");
    assert!(
        tools.starts_with("apply_patch only, confined to the per-call scratch")
            && tools.contains("not an empty-tools profile")
            && tools.contains("any tool beat refuses the answer"),
        "{tools}"
    );
    assert_eq!(codex["adapter_version"], "1.13.1");
}

#[tokio::test]
async fn unsupported_identity_stops_before_session_or_prompt() {
    let (ours, theirs) = tokio::io::duplex(65536);
    let (r, w) = tokio::io::split(ours);
    let (r2, mut w2) = tokio::io::split(theirs);
    let peer = tokio::spawn(async move {
        let mut r2 = BufReader::new(r2);
        let init = read(&mut r2).await;
        write(&mut w2,json!({"jsonrpc":"2.0","id":init["id"],"result":{"protocolVersion":1,"agentInfo":{"name":NAME,"version":"0.23.1"}}})).await;
        let mut line = String::new();
        assert_eq!(
            r2.read_line(&mut line).await.unwrap(),
            0,
            "no session/new or model prompt"
        );
    });
    let mut stream = crate::client::drive_profile(
        r,
        w,
        HarnessRequest::new("private prompt", "/tmp"),
        Duration::from_secs(1),
        Some(claude()),
        None,
        claude().session_meta(),
        None,
    );
    let first = std::future::poll_fn(|cx| Pin::new(&mut stream).poll_next(cx)).await;
    assert!(matches!(first, Some(Err(HarnessError::Refused { .. }))));
    tokio::time::timeout(Duration::from_secs(1), peer)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn admitted_wire_carries_strict_options_and_keeps_the_whole_answer() {
    let (ours, theirs) = tokio::io::duplex(65536);
    let (r, w) = tokio::io::split(ours);
    let (r2, mut w2) = tokio::io::split(theirs);
    let peer = tokio::spawn(async move {
        let mut r2 = BufReader::new(r2);
        let init = read(&mut r2).await;
        write(
            &mut w2,
            json!({"jsonrpc":"2.0","id":init["id"],"result":identity()}),
        )
        .await;
        let new = read(&mut r2).await;
        assert_eq!(new["method"], "session/new");
        assert_eq!(new["params"]["_meta"], profile());
        let thinking = &new["params"]["_meta"]["claudeCode"]["options"]["thinking"];
        assert_eq!(
            *thinking,
            json!({"type": "adaptive", "display": "summarized"})
        );
        assert_eq!(new["params"]["mcpServers"], json!([]));
        write(&mut w2,json!({"jsonrpc":"2.0","id":new["id"],"result":{"sessionId":"s","models":{"currentModelId":"served-config","availableModels":[]}}})).await;
        let prompt = read(&mut r2).await;
        assert_eq!(prompt["method"], "session/prompt");
        for text in ["prefix ", "{\"candidate\":true}", " suffix"] {
            write(&mut w2,json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":text}}}})).await;
        }
        write(
            &mut w2,
            json!({"jsonrpc":"2.0","id":prompt["id"],"result":{"stopReason":"end_turn"}}),
        )
        .await;
    });
    let mut stream = crate::client::drive_profile(
        r,
        w,
        HarnessRequest::new("private prompt", "/tmp"),
        Duration::from_secs(1),
        Some(claude()),
        None,
        claude().session_meta(),
        None,
    );
    let mut answer = None;
    while let Some(event) = std::future::poll_fn(|cx| Pin::new(&mut stream).poll_next(cx)).await {
        if let HarnessEvent::Completed { outcome } = event.unwrap() {
            answer = Some(outcome);
            break;
        }
    }
    let answer = answer.unwrap();
    assert_eq!(answer.output, "prefix {\"candidate\":true} suffix");
    assert_eq!(answer.observed_model.as_deref(), Some("served-config"));
    assert_eq!(
        answer.observed_model_source,
        Some(nika_kernel::ai::harness::ModelProvenance::SessionConfig)
    );
    tokio::time::timeout(Duration::from_secs(1), peer)
        .await
        .unwrap()
        .unwrap();
}

/// On the wire, an agent session (no one-shot profile) carries its seat's options in
/// `session/new`'s `_meta`: on the Claude Code seat, the closed surface that loads nothing of
/// the user's configuration, with no `tools`, turn limit or persistence choice. A row declared
/// under another id opens the same closed session when the adapter that answers `initialize`
/// is Claude Code's, the name its boundary row is found by. A session whose row and adapter
/// are both another's sends no `_meta` key at all.
#[tokio::test]
async fn an_agent_session_opens_with_its_seats_options_on_the_wire() {
    use nika_types::access::HarnessBoundary;
    let named =
        |name: &str| json!({"protocolVersion":1,"agentInfo":{"name":name,"version":"1.0.0"}});
    let claude = named(HarnessBoundary::CLAUDE_AGENT_ACP.agents[0]);
    let codex = named(HarnessBoundary::CODEX_ACP.agents[0]);
    let unnamed = json!({"protocolVersion":1});
    for (seat, answers, closed) in [
        ("claude-code", claude.clone(), true),
        ("claude-code", unnamed.clone(), true),
        ("my-claude", claude, true),
        ("codex", codex, false),
        ("my-claude", unnamed, false),
    ] {
        let meta = agent_meta(seat);
        let (ours, theirs) = tokio::io::duplex(65536);
        let (r, w) = tokio::io::split(ours);
        let (r2, mut w2) = tokio::io::split(theirs);
        let peer = tokio::spawn(async move {
            let mut r2 = BufReader::new(r2);
            let init = read(&mut r2).await;
            write(
                &mut w2,
                json!({"jsonrpc":"2.0","id":init["id"],"result":answers}),
            )
            .await;
            let new = read(&mut r2).await;
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
            new
        });
        let mut stream = crate::client::drive_profile(
            r,
            w,
            HarnessRequest::new("go", "/tmp"),
            Duration::from_secs(1),
            None,
            None,
            meta,
            None,
        );
        let mut completed = false;
        while let Some(event) = std::future::poll_fn(|cx| Pin::new(&mut stream).poll_next(cx)).await
        {
            if matches!(event.unwrap(), HarnessEvent::Completed { .. }) {
                completed = true;
                break;
            }
        }
        assert!(completed, "{seat}: the turn completes");
        let new = tokio::time::timeout(Duration::from_secs(1), peer)
            .await
            .unwrap()
            .unwrap();
        let sent = new["params"].get("_meta");
        if closed {
            assert_eq!(
                sent,
                Some(&json!({"claudeCode": {"options": {
                    "settingSources": [], "strictMcpConfig": true, "mcpServers": {},
                    "plugins": [], "skills": [], "agents": {},
                    "allowDangerouslySkipPermissions": false,
                    "env": {"CLAUDE_CODE_DISABLE_AUTO_MEMORY": "1",
                            "ENABLE_CLAUDEAI_MCP_SERVERS": "false"},
                    "thinking": {"type": "adaptive", "display": "summarized"}}}})),
                "{seat}"
            );
        } else {
            assert_eq!(sent, None, "{seat} sends no options");
        }
    }
}

async fn refuses_after_prompt(mode: &str) {
    let mode = mode.to_owned();
    let (ours, theirs) = tokio::io::duplex(65536);
    let (r, w) = tokio::io::split(ours);
    let (r2, mut w2) = tokio::io::split(theirs);
    let peer = tokio::spawn(async move {
        let mut r2 = BufReader::new(r2);
        let init = read(&mut r2).await;
        write(
            &mut w2,
            json!({"jsonrpc":"2.0","id":init["id"],"result":identity()}),
        )
        .await;
        let new = read(&mut r2).await;
        write(
            &mut w2,
            json!({"jsonrpc":"2.0","id":new["id"],"result":{"sessionId":"s"}}),
        )
        .await;
        let prompt = read(&mut r2).await;
        assert_eq!(prompt["method"], "session/prompt");
        match mode.as_str() {
            "eof" => {}
            "permission" => {
                write(&mut w2,json!({"jsonrpc":"2.0","id":90,"method":"session/request_permission","params":{"sessionId":"s","toolCall":{"title":"forbidden"},"options":[]}})).await;
                let denied = read(&mut r2).await;
                assert_eq!(denied["result"]["outcome"]["outcome"], "cancelled");
            }
            reason => {
                write(
                    &mut w2,
                    json!({"jsonrpc":"2.0","id":prompt["id"],"result":{"stopReason":reason}}),
                )
                .await;
            }
        }
    });
    let mut stream = crate::client::drive_profile(
        r,
        w,
        HarnessRequest::new("private prompt", "/tmp"),
        Duration::from_secs(1),
        Some(claude()),
        None,
        claude().session_meta(),
        None,
    );
    let first = std::future::poll_fn(|cx| Pin::new(&mut stream).poll_next(cx)).await;
    assert!(
        matches!(first, Some(Err(_))),
        "no completed candidate: {first:?}"
    );
    tokio::time::timeout(Duration::from_secs(1), peer)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn truncation_turn_limit_refusal_and_eof_never_become_completed() {
    for reason in [
        "max_tokens",
        "max_turn_requests",
        "refusal",
        "cancelled",
        "eof",
    ] {
        refuses_after_prompt(reason).await;
    }
}
#[tokio::test]
async fn an_unexpected_permission_is_denied_and_the_whole_answer_refused() {
    refuses_after_prompt("permission").await;
}
/// A lost sign-in is told as one, in our own words: the human signs in again in the app.
#[test]
fn a_lost_sign_in_is_told_as_one() {
    let why = safe_error(&HarnessError::Unavailable {
        reason: crate::client::SIGN_IN_EXPIRED.into(),
    });
    assert!(why.contains("sign-in has expired or was revoked"), "{why}");
    assert!(why.contains("sign in again"), "{why}");
    let other = safe_error(&HarnessError::Unavailable {
        reason: "OAuth token for someone@example.invalid".into(),
    });
    assert!(!other.contains("someone@example.invalid"), "{other}");
}
#[test]
fn transport_errors_do_not_publish_peer_secrets() {
    let why = safe_error(&HarnessError::Refused {
        reason: "private-key-test-marker and private prompt".into(),
    });
    assert!(!why.contains("private-key-test-marker"));
    assert!(!why.contains("private prompt"));
    assert!(why.contains("no answer accepted"));
}

/// Nika's own selection refusal reaches the author verbatim (the option,
/// the value and the discovered offer) while adapter text stays withheld.
#[test]
fn a_selection_refusal_is_shown_in_nika_own_words() {
    let reason = "the harness offers reasoning effort low · medium · high — `ultra` is not one \
                  (no prompt was sent)";
    let why = safe_error(&HarnessError::Selection {
        reason: reason.into(),
    });
    assert_eq!(
        why,
        format!("ACP authoring refused: {reason}; no answer accepted")
    );
}

/// The audit's loss hazard, ACP half: an explicit authoring effort rides the
/// session request verbatim (the client then applies it through the session's
/// own reasoning option and reads it back, or refuses before the prompt).
#[test]
fn an_explicit_authoring_effort_rides_the_session_request() {
    let native = crate::HarnessInferRequest::new("draft", "claude-code/opus")
        .with_system(Some("card".into()))
        .with_effort(Some("high".into()));
    let request = session_request(
        native,
        Some("claude-code/opus"),
        std::path::Path::new("/tmp"),
    );
    assert_eq!(request.requested_effort.as_deref(), Some("high"));
    assert_eq!(request.requested_model.as_deref(), Some("claude-code/opus"));
    assert_eq!(request.system.as_deref(), Some("card"));
    let plain = session_request(
        crate::HarnessInferRequest::new("draft", "session"),
        None,
        std::path::Path::new("/tmp"),
    );
    assert_eq!(
        (plain.requested_effort, plain.requested_model),
        (None, None)
    );
}

/// The authoring consumer holds an exact selection to its word: an answer during which the agent
/// moved the effort the client applied is refused, never accepted as the chosen one, and the move
/// is named; a model move nobody asked for is accepted and rides the record.
#[test]
fn a_mid_turn_move_of_an_applied_selection_refuses_the_answer_and_an_unasked_one_is_recorded() {
    let mut moved = nika_kernel::ai::harness::HarnessOutcome::new("answer");
    moved.selection.transmitted_effort = Some("high".into());
    moved.selection.changed_mid_turn = vec!["effort=low".into()];
    let refused = completed(moved, "test");
    assert!(
        matches!(&refused, Err(why) if why.message.contains("effort=low") && why.message.contains("no answer is accepted")),
        "{refused:?}"
    );
    let record = refused
        .as_ref()
        .err()
        .map(Failure::record)
        .expect("refused");
    assert_eq!(
        record["failure"],
        json!({"class":"selection","code":"NIKA-1805","transient":false,"cause":"unknown"})
    );
    assert_eq!(record["answer_accepted"], false);
    let mut drifted = nika_kernel::ai::harness::HarnessOutcome::new("answer");
    drifted.selection.changed_mid_turn = vec!["model=gpt-5.4".into()];
    let (answer, record) = completed(drifted, "test").expect("nothing applied moved");
    assert_eq!(answer, "answer");
    assert_eq!(record["changed_mid_turn"], json!(["model=gpt-5.4"]));
    let (_, plain) = completed(
        nika_kernel::ai::harness::HarnessOutcome::new("answer"),
        "test",
    )
    .expect("a plain answer");
    assert!(plain.get("changed_mid_turn").is_none(), "{plain}");
    assert_eq!(plain["stop_reason"], ACCEPTED_STOP, "{plain}");
}

/// A one-shot that names no model is held like an agent session: the client selects the model
/// the session reports and reads it back before its single prompt.
#[tokio::test]
async fn a_one_shot_that_names_no_model_holds_the_one_the_session_reports() {
    let model = |current: &str| {
        json!({"configOptions":[{"id":"model","name":"Model","category":"model",
            "type":"select","currentValue":current,
            "options":[{"value":"haiku","name":"Haiku"},{"value":"opus","name":"Opus"}]}]})
    };
    let (ours, theirs) = tokio::io::duplex(65536);
    let (r, w) = tokio::io::split(ours);
    let (r2, mut w2) = tokio::io::split(theirs);
    let peer = tokio::spawn(async move {
        let mut r2 = BufReader::new(r2);
        let init = read(&mut r2).await;
        write(
            &mut w2,
            json!({"jsonrpc":"2.0","id":init["id"],"result":identity()}),
        )
        .await;
        let new = read(&mut r2).await;
        assert_eq!(new["method"], "session/new");
        let mut opened = model("haiku");
        opened["sessionId"] = json!("s");
        write(
            &mut w2,
            json!({"jsonrpc":"2.0","id":new["id"],"result":opened}),
        )
        .await;
        let hold = read(&mut r2).await;
        assert_eq!(hold["method"], "session/set_config_option");
        assert_eq!(
            hold["params"],
            json!({"sessionId":"s","configId":"model","value":"haiku"})
        );
        write(
            &mut w2,
            json!({"jsonrpc":"2.0","id":hold["id"],"result":model("haiku")}),
        )
        .await;
        let prompt = read(&mut r2).await;
        assert_eq!(prompt["method"], "session/prompt");
        write(
            &mut w2,
            json!({"jsonrpc":"2.0","id":prompt["id"],"result":{"stopReason":"end_turn"}}),
        )
        .await;
    });
    let mut stream = crate::client::drive_profile(
        r,
        w,
        HarnessRequest::new("private prompt", "/tmp"),
        Duration::from_secs(1),
        Some(claude()),
        None,
        claude().session_meta(),
        None,
    );
    let mut answer = None;
    while let Some(event) = std::future::poll_fn(|cx| Pin::new(&mut stream).poll_next(cx)).await {
        if let HarnessEvent::Completed { outcome } = event.unwrap() {
            answer = Some(outcome);
            break;
        }
    }
    let answer = answer.unwrap();
    assert_eq!(answer.observed_model.as_deref(), Some("haiku"));
    assert_eq!(answer.selection.transmitted_model.as_deref(), Some("haiku"));
    assert!(
        answer.selection.held_model,
        "the one-shot named no model: a hold"
    );
    tokio::time::timeout(Duration::from_secs(1), peer)
        .await
        .unwrap()
        .unwrap();
}
