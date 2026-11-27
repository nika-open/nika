// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>
//! Model choice dialogues: an explicit variant never degrades to its family.
use super::*;

const BASE: &str = "gpt-6-astra";
const HIGH: &str = "gpt-6-astra[high]";
type PeerReader = BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>;
type PeerWriter = tokio::io::WriteHalf<tokio::io::DuplexStream>;

async fn read(peer: &mut PeerReader) -> Option<Value> {
    let mut line = String::new();
    let size = tokio::time::timeout(std::time::Duration::from_secs(3), peer.read_line(&mut line))
        .await
        .expect("scripted dialogue must settle")
        .expect("read peer");
    (size != 0).then(|| serde_json::from_str(&line).expect("JSON request"))
}

async fn write(peer: &mut PeerWriter, message: Value) {
    let mut bytes = serde_json::to_vec(&message).expect("JSON response");
    bytes.push(b'\n');
    peer.write_all(&bytes).await.expect("write peer");
    peer.flush().await.expect("flush peer");
}

async fn reply(peer: &mut PeerWriter, request: &Value, result: Value) {
    write(
        peer,
        serde_json::json!({"jsonrpc":"2.0","id":request["id"],"result":result}),
    )
    .await;
}

fn advertised(base: &str, legacy: &[&str]) -> Value {
    serde_json::json!({
        "configOptions":[{"id":"model","category":"model","currentValue":base,
            "options":[{"value":base,"name":base}]}],
        "models":{"currentModelId":base,"availableModels":legacy.iter()
            .map(|id| serde_json::json!({"modelId":id,"name":id})).collect::<Vec<_>>()}
    })
}

async fn offer(peer: &mut PeerReader, writer: &mut PeerWriter, mut choices: Value) {
    let initialize = read(peer).await.expect("initialize");
    assert_eq!(initialize["method"], "initialize");
    reply(
        writer,
        &initialize,
        serde_json::json!({"protocolVersion":1}),
    )
    .await;
    let session = read(peer).await.expect("session/new");
    assert_eq!(session["method"], "session/new");
    choices["sessionId"] = Value::String("model-choice".to_owned());
    reply(writer, &session, choices).await;
}

async fn end(peer: &mut PeerReader, writer: &mut PeerWriter) {
    let prompt = read(peer)
        .await
        .expect("prompt follows accepted exact selection");
    assert_eq!(prompt["method"], "session/prompt");
    assert_eq!(prompt["params"]["sessionId"], "model-choice");
    reply(
        writer,
        &prompt,
        serde_json::json!({"stopReason":"end_turn"}),
    )
    .await;
}

async fn result(mut stream: HarnessEventStream) -> Result<HarnessOutcome, HarnessError> {
    loop {
        match std::future::poll_fn(|cx| Pin::new(&mut stream).poll_next(cx)).await {
            Some(Ok(HarnessEvent::Completed { outcome })) => return Ok(*outcome),
            Some(Ok(_)) => {}
            Some(Err(error)) => return Err(error),
            None => panic!("scripted turn must complete or refuse"),
        }
    }
}

#[tokio::test]
async fn an_exact_legacy_variant_wins_over_a_config_family_before_the_prompt() {
    let (ours, theirs) = tokio::io::duplex(64 * 1024);
    let (client_read, client_write) = tokio::io::split(ours);
    let (peer_read, mut peer_write) = tokio::io::split(theirs);
    let peer = tokio::spawn(async move {
        let mut reader = BufReader::new(peer_read);
        offer(
            &mut reader,
            &mut peer_write,
            advertised(BASE, &["gpt-6-astra[low]", HIGH]),
        )
        .await;
        let select = read(&mut reader).await.expect("exact legacy selection");
        assert_eq!(select["method"], "session/set_model");
        assert_eq!(select["params"]["modelId"], HIGH);
        assert_eq!(select["params"]["sessionId"], "model-choice");
        reply(&mut peer_write, &select, serde_json::json!({})).await;
        end(&mut reader, &mut peer_write).await;
    });
    let outcome = result(drive(
        client_read,
        client_write,
        HarnessRequest::new("hello", "/tmp").with_requested_model(format!("openai/{HIGH}")),
    ))
    .await
    .expect("offered complete variant accepted");
    peer.await.expect("scripted peer");
    assert_eq!(outcome.observed_model.as_deref(), Some(HIGH));
    assert_eq!(
        outcome.observed_model_source,
        Some(ModelProvenance::AcceptedRequest),
        "legacy acceptance is not an echoed configuration or response attestation"
    );
}

#[tokio::test]
async fn an_absent_variant_refuses_without_selection_or_prompt_even_when_its_base_is_offered() {
    for legacy in [Vec::new(), vec!["gpt-6-astra[low]"]] {
        let (ours, theirs) = tokio::io::duplex(64 * 1024);
        let (client_read, client_write) = tokio::io::split(ours);
        let (peer_read, mut peer_write) = tokio::io::split(theirs);
        let peer = tokio::spawn(async move {
            let mut reader = BufReader::new(peer_read);
            offer(&mut reader, &mut peer_write, advertised(BASE, &legacy)).await;
            assert!(
                read(&mut reader).await.is_none(),
                "no base selection or prompt is authorized"
            );
        });
        let error = result(drive(
            client_read,
            client_write,
            HarnessRequest::new("hello", "/tmp").with_requested_model(format!("openai/{HIGH}")),
        ))
        .await
        .expect_err("unoffered variant refuses");
        peer.await.expect("scripted peer sees EOF");
        match error {
            HarnessError::Selection { reason } => {
                assert!(reason.contains(HIGH) && reason.contains(BASE), "{reason}");
            }
            other => panic!("expected a model refusal, got {other}"),
        }
    }
}

#[tokio::test]
async fn a_rejected_exact_legacy_selection_never_retries_on_the_base_or_prompts() {
    let (ours, theirs) = tokio::io::duplex(64 * 1024);
    let (client_read, client_write) = tokio::io::split(ours);
    let (peer_read, mut peer_write) = tokio::io::split(theirs);
    let peer = tokio::spawn(async move {
        let mut reader = BufReader::new(peer_read);
        offer(&mut reader, &mut peer_write, advertised(BASE, &[HIGH])).await;
        let select = read(&mut reader).await.expect("exact legacy selection");
        assert_eq!(select["method"], "session/set_model");
        assert_eq!(select["params"]["modelId"], HIGH);
        write(
            &mut peer_write,
            serde_json::json!({"jsonrpc":"2.0","id":select["id"],
            "error":{"code":-32602,"message":"variant rejected"}}),
        )
        .await;
        assert!(
            read(&mut reader).await.is_none(),
            "no fallback or prompt after refusal"
        );
    });
    let error = result(drive(
        client_read,
        client_write,
        HarnessRequest::new("hello", "/tmp").with_requested_model(format!("openai/{HIGH}")),
    ))
    .await
    .expect_err("peer rejected selection");
    peer.await.expect("scripted peer sees EOF");
    assert!(error.to_string().contains("variant rejected"));
}

#[tokio::test]
async fn exact_config_choices_keep_their_selection_including_base_variant_and_claude_alias() {
    for (provider, name) in [
        ("openai", BASE),
        ("openai", HIGH),
        ("anthropic", "opus[1m]"),
    ] {
        let (ours, theirs) = tokio::io::duplex(64 * 1024);
        let (client_read, client_write) = tokio::io::split(ours);
        let (peer_read, mut peer_write) = tokio::io::split(theirs);
        let peer = tokio::spawn(async move {
            let mut reader = BufReader::new(peer_read);
            let choices = advertised(name, &[name]);
            offer(&mut reader, &mut peer_write, choices.clone()).await;
            let select = read(&mut reader).await.expect("exact config selection");
            assert_eq!(select["method"], "session/set_config_option");
            assert_eq!(select["params"]["configId"], "model");
            assert_eq!(select["params"]["value"], name);
            reply(&mut peer_write, &select, choices).await;
            end(&mut reader, &mut peer_write).await;
        });
        let outcome = result(drive(
            client_read,
            client_write,
            HarnessRequest::new("hello", "/tmp").with_requested_model(format!("{provider}/{name}")),
        ))
        .await
        .expect("exact advertised config choice");
        peer.await.expect("scripted peer");
        assert_eq!(outcome.observed_model.as_deref(), Some(name));
        assert_eq!(
            outcome.observed_model_source,
            Some(ModelProvenance::ConfirmedSelection)
        );
    }
}

#[tokio::test]
async fn default_and_absent_requests_observe_the_current_choice_without_setting_any_model() {
    for requested in [None, Some("openai/default")] {
        let (ours, theirs) = tokio::io::duplex(64 * 1024);
        let (client_read, client_write) = tokio::io::split(ours);
        let (peer_read, mut peer_write) = tokio::io::split(theirs);
        let peer = tokio::spawn(async move {
            let mut reader = BufReader::new(peer_read);
            offer(&mut reader, &mut peer_write, advertised(BASE, &[HIGH])).await;
            end(&mut reader, &mut peer_write).await;
        });
        let mut request = HarnessRequest::new("hello", "/tmp");
        if let Some(model) = requested {
            request = request.with_requested_model(model);
        }
        let outcome = result(drive(client_read, client_write, request))
            .await
            .expect("default turn");
        peer.await.expect("scripted peer");
        assert_eq!(outcome.observed_model.as_deref(), Some(BASE));
        assert_eq!(
            outcome.observed_model_source,
            Some(ModelProvenance::SessionConfig)
        );
    }
}

const CLAUDE: &str = "@agentclientprotocol/claude-agent-acp";

/// claude-agent-acp's model option as `session/new` and a selection's answer report it.
fn claude_models(current: Option<&str>) -> Value {
    let mut option = serde_json::json!({"id":"model","name":"Model","category":"model",
        "type":"select","options":[{"value":"default","name":"Default"},
            {"value":"haiku","name":"Haiku"},{"value":"opus","name":"Opus"}]});
    if let Some(current) = current {
        option["currentValue"] = Value::String(current.to_owned());
    }
    serde_json::json!({"configOptions":[option]})
}

/// [`offer`] with the adapter naming itself on `initialize` (`None`: it names nothing).
async fn offer_as(
    peer: &mut PeerReader,
    writer: &mut PeerWriter,
    agent: Option<&str>,
    mut choices: Value,
) {
    let initialize = read(peer).await.expect("initialize");
    assert_eq!(initialize["method"], "initialize");
    let mut answer = serde_json::json!({"protocolVersion":1});
    if let Some(agent) = agent {
        answer["agentInfo"] = serde_json::json!({"name":agent,"version":"0.81.1"});
    }
    reply(writer, &initialize, answer).await;
    let session = read(peer).await.expect("session/new");
    assert_eq!(session["method"], "session/new");
    choices["sessionId"] = Value::String("model-choice".to_owned());
    reply(writer, &session, choices).await;
}

/// A task that names no model, on the adapter that reports a session's model apart from its
/// program: the client selects the reported model and reads it back before the prompt, and
/// records a hold, never a pin.
#[tokio::test]
async fn a_task_that_names_no_model_holds_the_one_a_claude_session_reports() {
    let (ours, theirs) = tokio::io::duplex(64 * 1024);
    let (client_read, client_write) = tokio::io::split(ours);
    let (peer_read, mut peer_write) = tokio::io::split(theirs);
    let peer = tokio::spawn(async move {
        let mut reader = BufReader::new(peer_read);
        offer_as(
            &mut reader,
            &mut peer_write,
            Some(CLAUDE),
            claude_models(Some("haiku")),
        )
        .await;
        let hold = read(&mut reader)
            .await
            .expect("the hold precedes the prompt");
        assert_eq!(hold["method"], "session/set_config_option");
        assert_eq!(
            hold["params"],
            serde_json::json!({"sessionId":"model-choice","configId":"model","value":"haiku"})
        );
        reply(&mut peer_write, &hold, claude_models(Some("haiku"))).await;
        end(&mut reader, &mut peer_write).await;
    });
    let outcome = result(drive(
        client_read,
        client_write,
        HarnessRequest::new("hello", "/tmp"),
    ))
    .await
    .expect("a kept model lets the turn run");
    peer.await.expect("scripted peer");
    assert_eq!(outcome.observed_model.as_deref(), Some("haiku"));
    assert_eq!(
        outcome.observed_model_source,
        Some(ModelProvenance::ConfirmedSelection)
    );
    let selection = &outcome.selection;
    assert_eq!(selection.model_option.as_deref(), Some("model"));
    assert_eq!(selection.transmitted_model.as_deref(), Some("haiku"));
    assert!(selection.held_model, "the task named no model: a hold");
    let mut moved = selection.clone();
    moved.changed_mid_turn = vec!["model=opus".to_owned()];
    assert!(
        moved.moved_refusal().is_none(),
        "a held model that moves stays a fact"
    );
}

/// A session that answers the hold with another model than the one it reported is refused
/// before any prompt, with both models named.
#[tokio::test]
async fn a_claude_session_that_does_not_keep_its_reported_model_is_refused() {
    for answered in [Some("default"), None] {
        let (ours, theirs) = tokio::io::duplex(64 * 1024);
        let (client_read, client_write) = tokio::io::split(ours);
        let (peer_read, mut peer_write) = tokio::io::split(theirs);
        let peer = tokio::spawn(async move {
            let mut reader = BufReader::new(peer_read);
            offer_as(
                &mut reader,
                &mut peer_write,
                Some(CLAUDE),
                claude_models(Some("haiku")),
            )
            .await;
            let hold = read(&mut reader).await.expect("the hold");
            assert_eq!(hold["params"]["value"], "haiku");
            reply(&mut peer_write, &hold, claude_models(answered)).await;
            assert!(read(&mut reader).await.is_none(), "no prompt is sent");
        });
        let error = result(drive(
            client_read,
            client_write,
            HarnessRequest::new("hello", "/tmp"),
        ))
        .await
        .expect_err("a model that is not kept refuses");
        peer.await.expect("scripted peer sees EOF");
        let reported = answered.map_or_else(|| "none".to_owned(), |m| format!("`{m}`"));
        match error {
            HarnessError::Selection { reason } => {
                assert!(reason.contains("it reported (`haiku`)"), "{reason}");
                assert!(
                    reason.contains(&format!("it reports {reported};")),
                    "{reason}"
                );
                assert!(reason.contains("no prompt was sent"), "{reason}");
            }
            other => panic!("expected a selection refusal, got {other}"),
        }
    }
}

/// Nothing is sent where the hold has no ground: another adapter, an adapter that names
/// nothing, or a model option that states no current value. The prompt follows `session/new`
/// and the record stays what the session stated.
#[tokio::test]
async fn no_hold_is_sent_to_another_adapter_or_without_a_reported_model() {
    let codex = nika_types::access::HarnessBoundary::CODEX_ACP.agents[0];
    for (agent, current) in [
        (Some(codex), Some("haiku")),
        (None, Some("haiku")),
        (Some(CLAUDE), None),
    ] {
        let (ours, theirs) = tokio::io::duplex(64 * 1024);
        let (client_read, client_write) = tokio::io::split(ours);
        let (peer_read, mut peer_write) = tokio::io::split(theirs);
        let peer = tokio::spawn(async move {
            let mut reader = BufReader::new(peer_read);
            offer_as(&mut reader, &mut peer_write, agent, claude_models(current)).await;
            end(&mut reader, &mut peer_write).await;
        });
        let outcome = result(drive(
            client_read,
            client_write,
            HarnessRequest::new("hello", "/tmp"),
        ))
        .await
        .expect("the turn runs on the session's own model");
        peer.await.expect("scripted peer");
        assert_eq!(outcome.observed_model.as_deref(), current, "{agent:?}");
        assert_eq!(
            outcome.observed_model_source,
            current.map(|_| ModelProvenance::SessionConfig),
            "{agent:?}"
        );
        assert_eq!(outcome.selection.transmitted_model, None, "{agent:?}");
        assert_eq!(outcome.selection.model_option, None, "{agent:?}");
        assert!(!outcome.selection.held_model, "{agent:?}");
    }
}

/// A model the task names is selected as before on the same adapter, and is a pin: the
/// record carries no hold, so its move refuses.
#[tokio::test]
async fn a_named_model_on_a_claude_session_is_a_pin_not_a_hold() {
    let (ours, theirs) = tokio::io::duplex(64 * 1024);
    let (client_read, client_write) = tokio::io::split(ours);
    let (peer_read, mut peer_write) = tokio::io::split(theirs);
    let peer = tokio::spawn(async move {
        let mut reader = BufReader::new(peer_read);
        offer_as(
            &mut reader,
            &mut peer_write,
            Some(CLAUDE),
            claude_models(Some("haiku")),
        )
        .await;
        let select = read(&mut reader).await.expect("the named model");
        assert_eq!(select["method"], "session/set_config_option");
        assert_eq!(select["params"]["value"], "opus");
        reply(&mut peer_write, &select, claude_models(Some("opus"))).await;
        end(&mut reader, &mut peer_write).await;
    });
    let outcome = result(drive(
        client_read,
        client_write,
        HarnessRequest::new("hello", "/tmp").with_requested_model("anthropic/opus".to_owned()),
    ))
    .await
    .expect("an offered model is selected");
    peer.await.expect("scripted peer");
    assert_eq!(outcome.observed_model.as_deref(), Some("opus"));
    assert_eq!(outcome.selection.transmitted_model.as_deref(), Some("opus"));
    assert!(!outcome.selection.held_model, "a named model is a pin");
    let mut moved = outcome.selection.clone();
    moved.changed_mid_turn = vec!["model=haiku".to_owned()];
    assert!(
        moved.moved_refusal().is_some(),
        "a pinned model that moves refuses"
    );
}

/// One Claude-shaped turn whose adapter reports `reported`, then one more model line, as the
/// model that answered, after the client selected `selected` and read it back.
async fn reported_turn(
    request: HarnessRequest,
    selected: &'static str,
    reported: &'static str,
) -> HarnessOutcome {
    let (ours, theirs) = tokio::io::duplex(64 * 1024);
    let (client_read, client_write) = tokio::io::split(ours);
    let (peer_read, mut peer_write) = tokio::io::split(theirs);
    let peer = tokio::spawn(async move {
        let mut reader = BufReader::new(peer_read);
        offer_as(
            &mut reader,
            &mut peer_write,
            Some(CLAUDE),
            claude_models(Some("haiku")),
        )
        .await;
        let select = read(&mut reader)
            .await
            .expect("the selection precedes the prompt");
        assert_eq!(select["method"], "session/set_config_option");
        assert_eq!(select["params"]["value"], selected);
        reply(&mut peer_write, &select, claude_models(Some(selected))).await;
        let prompt = read(&mut reader).await.expect("the prompt");
        assert_eq!(prompt["method"], "session/prompt");
        for model in [reported, "claude-opus-4-1-20250805"] {
            write(
                &mut peer_write,
                serde_json::json!({"jsonrpc":"2.0","method":"session/update",
                    "params":{"sessionId":"model-choice","update":{
                        "sessionUpdate":"usage_update","used":3,"size":200_000,
                        "_meta":{"_claude/model":model}}}}),
            )
            .await;
        }
        reply(
            &mut peer_write,
            &prompt,
            serde_json::json!({"stopReason":"end_turn"}),
        )
        .await;
    });
    let outcome = result(drive(client_read, client_write, request))
        .await
        .expect("the client reports the turn");
    peer.await.expect("scripted peer");
    outcome
}

/// A held model is not a pin for the adapter's own report either: a session that answers as
/// another model line is recorded as one move and refuses nothing, where the same report
/// under a model the task named refuses the answer.
#[tokio::test]
async fn a_report_of_another_model_refuses_a_pin_and_not_a_hold() {
    const SONNET: &str = "claude-sonnet-4-5-20250929";
    let held = reported_turn(HarnessRequest::new("hello", "/tmp"), "haiku", SONNET).await;
    assert!(held.selection.held_model, "the task named no model");
    assert_eq!(
        held.selection.changed_mid_turn,
        [format!("model={SONNET}")],
        "recorded once"
    );
    assert!(
        held.selection.moved_refusal().is_none(),
        "a held model that moved stays a fact"
    );

    let pinned = reported_turn(
        HarnessRequest::new("hello", "/tmp").with_requested_model("anthropic/opus".to_owned()),
        "opus",
        SONNET,
    )
    .await;
    assert!(!pinned.selection.held_model, "a named model is a pin");
    assert_eq!(
        pinned.selection.changed_mid_turn,
        [format!("model={SONNET}")]
    );
    let reason = match pinned.selection.moved_refusal() {
        Some(HarnessError::Selection { reason }) => reason,
        _ => String::new(),
    };
    assert!(reason.contains(&format!("model={SONNET}")), "{reason:?}");
}

/// A session that rejects the selection of the model it reported itself fails before any
/// prompt, in the adapter's own words: the task named no model, and nothing ran.
#[tokio::test]
async fn a_claude_session_that_rejects_the_hold_fails_before_the_prompt() {
    let (ours, theirs) = tokio::io::duplex(64 * 1024);
    let (client_read, client_write) = tokio::io::split(ours);
    let (peer_read, mut peer_write) = tokio::io::split(theirs);
    let peer = tokio::spawn(async move {
        let mut reader = BufReader::new(peer_read);
        offer_as(
            &mut reader,
            &mut peer_write,
            Some(CLAUDE),
            claude_models(Some("haiku")),
        )
        .await;
        let hold = read(&mut reader)
            .await
            .expect("the hold precedes the prompt");
        assert_eq!(hold["method"], "session/set_config_option");
        write(
            &mut peer_write,
            serde_json::json!({"jsonrpc":"2.0","id":hold["id"],
            "error":{"code":-32602,"message":"model rejected"}}),
        )
        .await;
        assert!(
            read(&mut reader).await.is_none(),
            "no prompt after a rejected hold"
        );
    });
    let error = result(drive(
        client_read,
        client_write,
        HarnessRequest::new("hello", "/tmp"),
    ))
    .await
    .expect_err("the session rejected the hold");
    peer.await.expect("scripted peer sees EOF");
    assert!(error.to_string().contains("model rejected"), "{error}");
}

/// A session that states its model only through the legacy model list has no model option to
/// select through: no hold is sent, and the prompt is the next request.
#[tokio::test]
async fn no_hold_is_sent_when_only_the_legacy_model_list_states_a_model() {
    let (ours, theirs) = tokio::io::duplex(64 * 1024);
    let (client_read, client_write) = tokio::io::split(ours);
    let (peer_read, mut peer_write) = tokio::io::split(theirs);
    let peer = tokio::spawn(async move {
        let mut reader = BufReader::new(peer_read);
        let legacy = serde_json::json!({"models":{"currentModelId":"haiku","availableModels":[
            {"modelId":"haiku","name":"Haiku"},{"modelId":"opus","name":"Opus"}]}});
        offer_as(&mut reader, &mut peer_write, Some(CLAUDE), legacy).await;
        end(&mut reader, &mut peer_write).await;
    });
    let outcome = result(drive(
        client_read,
        client_write,
        HarnessRequest::new("hello", "/tmp"),
    ))
    .await
    .expect("the turn runs on the session's own model");
    peer.await.expect("scripted peer");
    assert_eq!(outcome.selection.transmitted_model, None);
    assert_eq!(outcome.selection.model_option, None);
    assert!(!outcome.selection.held_model);
}
