// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! The session's title over a duplex pipe, against scripted adapters: a codex-acp that lets a
//! client set the title receives `session/set_title` right after `session/new`, with Nika's
//! word for the kind of session and nothing the task wrote; another adapter, or one without
//! the capability, receives nothing; an error answer, a missing answer and a late answer do
//! not fail the session; a refused beat or a closed pipe before the answer still ends it.
//!
//! An agent task's peer reports codex-acp 2.12.0, a release that advertises the capability. A
//! one-shot or a conversation admits codex-acp 1.13.1 alone, which does not advertise it: those
//! peers pair the admitted identity with the capability, as an admitted release that gains it
//! would answer.

use std::path::PathBuf;
use std::pin::Pin;

use futures_core::Stream as _;
use nika_kernel::ai::harness::{HarnessError, HarnessEvent, HarnessEventStream, HarnessRequest};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, ReadHalf, WriteHalf};

use super::label;
use crate::authoring::acp::{Completion, OneShot, Profile, codex};
use crate::client::conversation::{Opening, open};
use crate::conversation::{ConversationSetup, ToolOffer};

/// The agent's end of the pipe.
struct Agent {
    reader: BufReader<ReadHalf<DuplexStream>>,
    writer: WriteHalf<DuplexStream>,
}

impl Agent {
    /// The client's next message, or `None` once the client closed the pipe.
    async fn read(&mut self) -> Option<Value> {
        let mut line = String::new();
        let read = (self.reader.read_line(&mut line).await).expect("the agent reads");
        (read > 0).then(|| serde_json::from_str(line.trim_end()).expect("one JSON message a line"))
    }

    async fn expect(&mut self, method: &str) -> Value {
        let message = self.read().await.expect("the client writes");
        assert_eq!(message["method"], method, "{message}");
        message
    }

    async fn send(&mut self, message: &Value) {
        let line = format!("{message}\n");
        (self.writer.write_all(line.as_bytes()).await).expect("the agent writes");
    }

    async fn reply(&mut self, id: &Value, result: Value) {
        self.send(&json!({"jsonrpc": "2.0", "id": id, "result": result}))
            .await;
    }

    async fn error(&mut self, id: &Value, code: i64, message: &str) {
        let error = json!({"code": code, "message": message});
        self.send(&json!({"jsonrpc": "2.0", "id": id, "error": error}))
            .await;
    }

    async fn update(&mut self, update: Value) {
        let params = json!({"sessionId": "s-title", "update": update});
        let note = json!({"jsonrpc": "2.0", "method": "session/update", "params": params});
        self.send(&note).await;
    }

    /// Accept the title call `titled` as codex-acp 2.12.0 does: the session's new title told as
    /// a `session_info_update` first, then the answer naming it.
    async fn accept(&mut self, titled: &Value) {
        let title = titled["params"]["title"].clone();
        self.update(json!({"sessionUpdate": "session_info_update", "title": title}))
            .await;
        self.reply(&titled["id"], json!({"title": title})).await;
    }

    /// End the turn the prompt `prompt` opened, with one answer chunk.
    async fn end_turn(&mut self, prompt: &Value) {
        let text = json!({"type": "text", "text": "ok"});
        self.update(json!({"sessionUpdate": "agent_message_chunk", "content": text}))
            .await;
        self.reply(&prompt["id"], json!({"stopReason": "end_turn"}))
            .await;
    }
}

fn pipe() -> (Agent, ReadHalf<DuplexStream>, WriteHalf<DuplexStream>) {
    let (client, agent) = tokio::io::duplex(1 << 16);
    let (client_read, client_write) = tokio::io::split(client);
    let (agent_read, agent_write) = tokio::io::split(agent);
    let agent = Agent {
        reader: BufReader::new(agent_read),
        writer: agent_write,
    };
    (agent, client_read, client_write)
}

/// What codex-acp 2.12.0 advertises on wire v1 that this client reads: the title capability.
fn titles() -> Value {
    json!({"sessionCapabilities": {"title": {}}})
}

/// An `initialize` answer of the adapter `name` at `version` with `capabilities`.
fn initialized(name: &str, version: &str, capabilities: &Value) -> Value {
    json!({"protocolVersion": 1, "agentInfo": {"name": name, "version": version},
        "agentCapabilities": capabilities})
}

fn model_options(current: &str) -> Value {
    let options: Vec<Value> = ["gpt-a", "gpt-b"]
        .iter()
        .map(|model| json!({"value": model, "name": model}))
        .collect();
    json!([{"id": "model", "name": "Model", "category": "model", "type": "select",
        "currentValue": current, "options": options}])
}

/// The stream's beats until it ends: the outcome's text, or the error it ended with.
async fn settle(mut stream: HarnessEventStream) -> Result<String, HarnessError> {
    while let Some(beat) = std::future::poll_fn(|cx| Pin::new(&mut stream).poll_next(cx)).await {
        if let HarnessEvent::Completed { outcome } = beat? {
            return Ok(outcome.output);
        }
    }
    Err(HarnessError::Session {
        reason: "the stream ended without a Completed beat".to_owned(),
    })
}

const PROMPT: &str = "Summarize the quarterly ledger for the Lisbon office";
const SYSTEM: &str = "Answer tersely in Portuguese";

/// Each kind of session has its own title, Nika's word for it.
#[test]
fn each_kind_of_session_has_its_own_title() {
    let one_shot = |role| {
        Some(OneShot {
            role,
            profile: Profile::Codex,
        })
    };
    assert_eq!(label(None), "Nika agent");
    assert_eq!(label(one_shot(Completion::Infer)), "Nika infer");
    assert_eq!(label(one_shot(Completion::Authoring)), "Nika authoring");
    assert_eq!(
        label(one_shot(Completion::Conversation)),
        "Nika conversation"
    );
}

/// A codex-acp that lets a client set the title receives `session/set_title` as the first
/// request after `session/new`, before the model selection and the prompt, titled with Nika's
/// word for an agent task: none of the prompt's or the system text's words.
#[tokio::test]
async fn an_adapter_that_lets_a_client_title_the_session_is_titled_first() {
    let (mut agent, read, write) = pipe();
    let script = tokio::spawn(async move {
        let hello = agent.expect("initialize").await;
        let init = initialized(codex::NAME, "2.12.0", &titles());
        agent.reply(&hello["id"], init).await;
        let new = agent.expect("session/new").await;
        let opened = json!({"sessionId": "s-title", "configOptions": model_options("gpt-a")});
        agent.reply(&new["id"], opened).await;
        let titled = agent.expect("session/set_title").await;
        agent.accept(&titled).await;
        let model = agent.expect("session/set_config_option").await;
        agent
            .reply(
                &model["id"],
                json!({"configOptions": model_options("gpt-b")}),
            )
            .await;
        let prompt = agent.expect("session/prompt").await;
        agent.end_turn(&prompt).await;
        titled
    });
    let request = HarnessRequest::new(PROMPT, "/scratch/project")
        .with_system(SYSTEM)
        .with_requested_model("codex/gpt-b");
    let output = settle(crate::client::drive(read, write, request)).await;
    let titled = script.await.expect("the script ends");
    assert_eq!(output.expect("the session completes"), "ok");
    assert_eq!(titled["id"], 10);
    assert_eq!(
        titled["params"],
        json!({"sessionId": "s-title", "title": "Nika agent"})
    );
    let title = titled["params"]["title"]
        .as_str()
        .expect("a title")
        .to_lowercase();
    for word in PROMPT.split_whitespace().chain(SYSTEM.split_whitespace()) {
        let word = word.to_lowercase();
        assert!(
            !title.split_whitespace().any(|own| own == word),
            "`{word}` of the task in the title `{title}`"
        );
    }
}

/// No title call goes to a codex-acp without the capability (a capability that is no object is
/// not one), nor to another adapter that advertises it: the prompt follows `session/new`.
#[tokio::test]
async fn an_adapter_without_the_capability_or_another_adapter_receives_no_title() {
    let claude = "@agentclientprotocol/claude-agent-acp";
    for (name, version, capabilities) in [
        (codex::NAME, "2.12.0", json!({})),
        (
            codex::NAME,
            "2.12.0",
            json!({"sessionCapabilities": {"title": true}}),
        ),
        (claude, "0.112.0", titles()),
    ] {
        let (mut agent, read, write) = pipe();
        let script = tokio::spawn(async move {
            let hello = agent.expect("initialize").await;
            let init = initialized(name, version, &capabilities);
            agent.reply(&hello["id"], init).await;
            let new = agent.expect("session/new").await;
            agent
                .reply(&new["id"], json!({"sessionId": "s-title"}))
                .await;
            let next = agent.read().await.expect("the client writes");
            if next["method"] == "session/prompt" {
                agent.end_turn(&next).await;
            }
            next
        });
        let request = HarnessRequest::new(PROMPT, "/scratch/project");
        let output = settle(crate::client::drive(read, write, request)).await;
        let next = script.await.expect("the script ends");
        assert_eq!(next["method"], "session/prompt", "{name} {version}: {next}");
        assert_eq!(output.expect("the session completes"), "ok");
    }
}

/// An error answer to the title call does not fail the session: the prompt is sent and the
/// turn completes.
#[tokio::test]
async fn an_error_answer_to_the_title_does_not_fail_the_session() {
    let (mut agent, read, write) = pipe();
    let script = tokio::spawn(async move {
        let hello = agent.expect("initialize").await;
        agent
            .reply(&hello["id"], initialized(codex::NAME, "2.12.0", &titles()))
            .await;
        let new = agent.expect("session/new").await;
        agent
            .reply(&new["id"], json!({"sessionId": "s-title"}))
            .await;
        let titled = agent.expect("session/set_title").await;
        agent
            .error(
                &titled["id"],
                -32002,
                "Resource not found: no rollout found",
            )
            .await;
        let prompt = agent.expect("session/prompt").await;
        agent.end_turn(&prompt).await;
    });
    let request = HarnessRequest::new(PROMPT, "/scratch/project");
    let output = settle(crate::client::drive(read, write, request)).await;
    script.await.expect("the script ends");
    assert_eq!(output.expect("the session completes"), "ok");
}

/// A title call left unanswered ends its wait at its own bound (30 s of paused time, under the
/// default frame bound) and the session goes on; the answer that arrives late, during the
/// turn, is ignored by the turn and the turn completes.
#[tokio::test(start_paused = true)]
async fn an_unanswered_title_lets_the_session_go_on_and_its_late_answer_is_ignored() {
    let (mut agent, read, write) = pipe();
    let script = tokio::spawn(async move {
        let hello = agent.expect("initialize").await;
        agent
            .reply(&hello["id"], initialized(codex::NAME, "2.12.0", &titles()))
            .await;
        let new = agent.expect("session/new").await;
        agent
            .reply(&new["id"], json!({"sessionId": "s-title"}))
            .await;
        let titled = agent.expect("session/set_title").await;
        let asked = tokio::time::Instant::now();
        let prompt = agent.expect("session/prompt").await;
        let waited = asked.elapsed();
        agent
            .reply(&titled["id"], json!({"title": "Nika agent"}))
            .await;
        agent.end_turn(&prompt).await;
        waited
    });
    let request = HarnessRequest::new(PROMPT, "/scratch/project");
    let output = settle(crate::client::drive(read, write, request)).await;
    let waited = script.await.expect("the script ends");
    assert_eq!(output.expect("the late answer is ignored"), "ok");
    let bound = crate::client::start::start_timeout();
    assert!(
        waited >= bound && waited < bound + std::time::Duration::from_secs(1),
        "the prompt followed the title's own bound ({bound:?}), after {waited:?}"
    );
}

/// A pipe that closes while the title call waits ends the session as a closed transport, and
/// no prompt is sent.
#[tokio::test]
async fn a_closed_pipe_during_the_title_wait_ends_the_session() {
    let (mut agent, read, write) = pipe();
    let script = tokio::spawn(async move {
        let hello = agent.expect("initialize").await;
        agent
            .reply(&hello["id"], initialized(codex::NAME, "2.12.0", &titles()))
            .await;
        let new = agent.expect("session/new").await;
        agent
            .reply(&new["id"], json!({"sessionId": "s-title"}))
            .await;
        agent.expect("session/set_title").await
    });
    let request = HarnessRequest::new(PROMPT, "/scratch/project");
    let ended = settle(crate::client::drive(read, write, request)).await;
    let titled = script.await.expect("the script ends");
    assert_eq!(titled["params"]["title"], "Nika agent");
    let Err(HarnessError::Session { reason }) = &ended else {
        panic!("a closed transport: {ended:?}");
    };
    assert_eq!(reason, "transport: the agent closed the pipe mid-run");
}

/// A one-shot is titled with its own word, and a beat its profile refuses while the title call
/// waits for its answer still ends the session: no prompt is sent.
#[tokio::test]
async fn a_refused_beat_before_the_title_answer_still_ends_a_one_shot() {
    let (mut agent, read, write) = pipe();
    let script = tokio::spawn(async move {
        let hello = agent.expect("initialize").await;
        let init = initialized(codex::NAME, codex::VERSION, &titles());
        agent.reply(&hello["id"], init).await;
        let new = agent.expect("session/new").await;
        agent
            .reply(&new["id"], json!({"sessionId": "s-title"}))
            .await;
        let titled = agent.expect("session/set_title").await;
        let call = json!({"sessionUpdate": "tool_call", "toolCallId": "t1", "kind": "execute",
            "title": "cat secrets.txt", "status": "pending"});
        agent.update(call).await;
        (titled, agent.read().await)
    });
    let one_shot = OneShot {
        role: Completion::Infer,
        profile: Profile::Codex,
    };
    let stream = crate::client::drive_profile(
        read,
        write,
        HarnessRequest::new(PROMPT, "/scratch/infer"),
        std::time::Duration::from_secs(20),
        Some(one_shot),
        None,
        None,
        None,
    );
    let ended = settle(stream).await;
    let (titled, after) = script.await.expect("the script ends");
    assert_eq!(titled["params"]["title"], "Nika infer");
    let Err(HarnessError::Refused { reason }) = &ended else {
        panic!("the profile refuses the beat: {ended:?}");
    };
    assert_eq!(
        reason,
        "ACP infer emitted a tool, media or unsupported event (`tool_call` · kind `execute`); \
         no answer accepted"
    );
    assert_eq!(after, None, "no prompt after the refusal");
}

/// Under a frame bound shorter than the title's own (20 s here), the title waits half of it,
/// so the frame bound never ends the session first: an unanswered title lets a one-shot go on
/// to its mode after 10 s of paused time, and its turn completes.
#[tokio::test(start_paused = true)]
async fn an_unanswered_title_under_a_short_frame_bound_lets_the_session_go_on() {
    let (mut agent, read, write) = pipe();
    let script = tokio::spawn(async move {
        let hello = agent.expect("initialize").await;
        let init = initialized(codex::NAME, codex::VERSION, &titles());
        agent.reply(&hello["id"], init).await;
        let new = agent.expect("session/new").await;
        let opened = json!({"sessionId": "s-title", "configOptions": codex_modes("agent")});
        agent.reply(&new["id"], opened).await;
        agent.expect("session/set_title").await;
        let asked = tokio::time::Instant::now();
        let mode = agent.expect("session/set_config_option").await;
        let waited = asked.elapsed();
        let confined = json!({"configOptions": codex_modes("read-only")});
        agent.reply(&mode["id"], confined).await;
        let prompt = agent.expect("session/prompt").await;
        agent.end_turn(&prompt).await;
        waited
    });
    let one_shot = OneShot {
        role: Completion::Infer,
        profile: Profile::Codex,
    };
    let stream = crate::client::drive_profile(
        read,
        write,
        HarnessRequest::new(PROMPT, "/scratch/infer"),
        std::time::Duration::from_secs(20),
        Some(one_shot),
        None,
        None,
        None,
    );
    let output = settle(stream).await;
    let waited = (script.await).unwrap_or_else(|e| panic!("{e:?} · the session: {output:?}"));
    assert_eq!(output.expect("the session goes on"), "ok");
    let half = std::time::Duration::from_secs(10);
    assert!(
        waited >= half && waited < half + std::time::Duration::from_secs(1),
        "the mode followed half the frame bound, after {waited:?}"
    );
}

/// A title call left unanswered, whose error answer arrives late during an agent task's turn:
/// the turn passes over it and completes with its own answer.
#[tokio::test(start_paused = true)]
async fn a_late_error_answer_to_the_title_is_ignored_by_an_agent_turn() {
    let (mut agent, read, write) = pipe();
    let script = tokio::spawn(async move {
        let hello = agent.expect("initialize").await;
        let init = initialized(codex::NAME, "2.12.0", &titles());
        agent.reply(&hello["id"], init).await;
        let new = agent.expect("session/new").await;
        agent
            .reply(&new["id"], json!({"sessionId": "s-title"}))
            .await;
        let titled = agent.expect("session/set_title").await;
        let prompt = agent.expect("session/prompt").await;
        agent
            .error(
                &titled["id"],
                -32002,
                "Resource not found: no rollout found",
            )
            .await;
        agent.end_turn(&prompt).await;
        titled["id"].clone()
    });
    let request = HarnessRequest::new(PROMPT, "/scratch/project");
    let output = settle(crate::client::drive(read, write, request)).await;
    let late = (script.await).unwrap_or_else(|e| panic!("{e:?} · the session: {output:?}"));
    assert_eq!(late, 10, "the late error answers the title's id");
    assert_eq!(output.expect("the late error is passed over"), "ok");
}

/// The same late error answer during a one-shot's turn (ACP `infer:` on the Codex profile, its
/// mode set after the title's wait): the turn passes over it and completes.
#[tokio::test(start_paused = true)]
async fn a_late_error_answer_to_the_title_is_ignored_by_a_one_shot_turn() {
    let (mut agent, read, write) = pipe();
    let script = tokio::spawn(async move {
        let hello = agent.expect("initialize").await;
        let init = initialized(codex::NAME, codex::VERSION, &titles());
        agent.reply(&hello["id"], init).await;
        let new = agent.expect("session/new").await;
        let opened = json!({"sessionId": "s-title", "configOptions": codex_modes("agent")});
        agent.reply(&new["id"], opened).await;
        let titled = agent.expect("session/set_title").await;
        let mode = agent.expect("session/set_config_option").await;
        let confined = json!({"configOptions": codex_modes("read-only")});
        agent.reply(&mode["id"], confined).await;
        let prompt = agent.expect("session/prompt").await;
        agent
            .error(
                &titled["id"],
                -32002,
                "Resource not found: no rollout found",
            )
            .await;
        agent.end_turn(&prompt).await;
        titled["id"].clone()
    });
    let one_shot = OneShot {
        role: Completion::Infer,
        profile: Profile::Codex,
    };
    let stream = crate::client::drive_profile(
        read,
        write,
        HarnessRequest::new(PROMPT, "/scratch/infer"),
        std::time::Duration::from_secs(20),
        Some(one_shot),
        None,
        None,
        None,
    );
    let output = settle(stream).await;
    let late = (script.await).unwrap_or_else(|e| panic!("{e:?} · the session: {output:?}"));
    assert_eq!(late, 10, "the late error answers the title's id");
    assert_eq!(output.expect("the late error is passed over"), "ok");
}

fn codex_offer() -> ToolOffer {
    let tools = ["candidate_read", "candidate_write"].map(str::to_owned);
    ToolOffer::new("nika", tools.to_vec()).with_http("http://127.0.0.1:4242/mcp", "b".repeat(64))
}

fn codex_modes(current: &str) -> Value {
    let options: Vec<Value> = ["read-only", "agent"]
        .iter()
        .map(|mode| json!({"value": mode, "name": mode}))
        .collect();
    json!([{"id": "mode", "name": "Mode", "category": "mode", "type": "select",
        "currentValue": current, "options": options}])
}

/// How the scripted adapter answers a conversation's title call.
#[derive(Clone, Copy, Debug)]
enum Answer {
    Accept,
    Refuse,
    Silent,
}

/// A Codex conversation whose adapter lets a client set the title is titled with its own word
/// before its mode, and its opening record says what the adapter answered: `accepted`,
/// `refused` (an error answer) or `unanswered` (no answer within the wait, paused time). An
/// adapter without the capability leaves the record without the key.
#[tokio::test(start_paused = true)]
async fn a_conversation_records_what_the_adapter_answered_to_its_title() {
    for (capabilities, answer, recorded) in [
        (titles(), Some(Answer::Accept), Some("accepted")),
        (titles(), Some(Answer::Refuse), Some("refused")),
        (titles(), Some(Answer::Silent), Some("unanswered")),
        (json!({}), None, None),
    ] {
        let (mut agent, read, write) = pipe();
        let script = tokio::spawn(async move {
            let hello = agent.expect("initialize").await;
            let init = initialized(codex::NAME, codex::VERSION, &capabilities);
            agent.reply(&hello["id"], init).await;
            let new = agent.expect("session/new").await;
            let configured = json!({"sessionId": "s-title", "configOptions": codex_modes("agent")});
            agent.reply(&new["id"], configured).await;
            let mut title = None;
            if let Some(answer) = answer {
                let titled = agent.expect("session/set_title").await;
                match answer {
                    Answer::Accept => agent.accept(&titled).await,
                    Answer::Refuse => {
                        agent
                            .error(&titled["id"], -32002, "Resource not found")
                            .await;
                    }
                    Answer::Silent => {}
                }
                title = Some(titled["params"]["title"].clone());
            }
            let mode = agent.expect("session/set_config_option").await;
            agent
                .reply(
                    &mode["id"],
                    json!({"configOptions": codex_modes("read-only")}),
                )
                .await;
            title
        });
        let opening = Opening {
            setup: ConversationSetup::new(),
            offer: codex_offer(),
            profile: Profile::Codex,
            cwd: PathBuf::from("/scratch/conversation"),
        };
        let conversation = open(read, write, opening, ()).await.expect("opens");
        let title = script.await.expect("the script ends");
        assert_eq!(
            title,
            answer.map(|_| json!("Nika conversation")),
            "{recorded:?}"
        );
        assert_eq!(
            conversation
                .record()
                .get("title_call")
                .and_then(Value::as_str),
            recorded
        );
    }
}
