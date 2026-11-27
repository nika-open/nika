// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! A limit an `agent:` task declares on a seat, end to end on the REAL ACP client against
//! scripted duplex peers (no host program, no adapter): `max_turns` reaches the Claude Code
//! session as `maxTurns` beside its closed options and nothing else moves; on Codex and on a
//! seat whose turn limit is not read, and `temperature` on any seat, the task is refused before
//! the peer reads a request; a first prompt stopped at the limit fails with its code and no
//! repair prompt is written.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use nika_kernel::ai::harness::{DynAgentBackend, HarnessError, HarnessEventStream, HarnessRequest};
use nika_kernel_mock::{
    MockClock, MockProvider, MockShell, MockToolDefinitionProvider, MockToolExecutor,
};
use nika_types::access::HarnessBoundary;
use nika_verb_agent::AgentVerb;
use nika_verb_exec::ExecVerb;
use nika_verb_infer::InferVerb;
use nika_verb_invoke::InvokeVerb;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use super::*;

type Reader = BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>;
type Writer = tokio::io::WriteHalf<tokio::io::DuplexStream>;

async fn send(w: &mut Writer, message: &Value) {
    let mut bytes = serde_json::to_vec(message).expect("JSON");
    bytes.push(b'\n');
    let _ = w.write_all(&bytes).await;
    let _ = w.flush().await;
}

fn select(id: &str, category: &str, current: &str, values: &[&str]) -> Value {
    json!({"id":id,"name":id,"category":category,"type":"select","currentValue":current,
        "options":values.iter().map(|v| json!({"value":v,"name":v})).collect::<Vec<_>>()})
}

/// One scripted session: answers `initialize` under `name`, keeps a model and a mode option,
/// and ends every prompt with `result` after one message chunk. Every request is recorded as
/// it is read.
async fn session(
    mut r: Reader,
    mut w: Writer,
    name: &'static str,
    result: Value,
    heard: Arc<Mutex<Vec<Value>>>,
) {
    let (mut model, mut mode) = ("echo".to_owned(), "default".to_owned());
    let mut line = String::new();
    loop {
        line.clear();
        if !matches!(r.read_line(&mut line).await, Ok(n) if n > 0) {
            return;
        }
        let request: Value = serde_json::from_str(&line).expect("JSON request");
        heard.lock().expect("heard").push(request.clone());
        let config = |model: &str, mode: &str| {
            json!([
                select("model", "model", model, &["default", "echo"]),
                select("mode", "mode", mode, &["default", "plan"])
            ])
        };
        let answer = match request["method"].as_str() {
            Some("initialize") => {
                json!({"protocolVersion":1,"agentInfo":{"name":name,"version":"0.81.1"}})
            }
            Some("session/new") => json!({"sessionId":"s","configOptions":config(&model, &mode)}),
            Some("session/set_config_option") => {
                let value = request["params"]["value"].as_str().unwrap_or_default();
                match request["params"]["configId"].as_str() {
                    Some("model") => value.clone_into(&mut model),
                    Some("mode") => value.clone_into(&mut mode),
                    _ => {}
                }
                json!({"configOptions":config(&model, &mode)})
            }
            Some("session/prompt") => {
                let chunk = json!({"sessionUpdate":"agent_message_chunk",
                    "content":{"type":"text","text":"{\"partial\": true}"}});
                send(
                    &mut w,
                    &json!({"jsonrpc":"2.0","method":"session/update",
                        "params":{"sessionId":"s","update":chunk}}),
                )
                .await;
                result.clone()
            }
            _ => json!({}),
        };
        if request.get("id").is_some() {
            send(
                &mut w,
                &json!({"jsonrpc":"2.0","id":request["id"],"result":answer}),
            )
            .await;
        }
    }
}

/// A scripted seat: each session it opens is one [`session`], awaited after the run so every
/// request the client wrote is read.
struct Seat {
    name: &'static str,
    result: Value,
    opened: AtomicUsize,
    heard: Arc<Mutex<Vec<Value>>>,
    peers: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl DynAgentBackend for Seat {
    fn run_agent_boxed(
        &self,
        request: HarnessRequest,
    ) -> Pin<Box<dyn Future<Output = Result<HarnessEventStream, HarnessError>> + Send + '_>> {
        self.opened.fetch_add(1, Ordering::SeqCst);
        let (ours, theirs) = tokio::io::duplex(1024 * 1024);
        let (client_read, client_write) = tokio::io::split(ours);
        let (peer_read, peer_write) = tokio::io::split(theirs);
        let peer = tokio::spawn(session(
            BufReader::new(peer_read),
            peer_write,
            self.name,
            self.result.clone(),
            Arc::clone(&self.heard),
        ));
        self.peers.lock().expect("peers").push(peer);
        Box::pin(async move { Ok(nika_harness::drive(client_read, client_write, request)) })
    }
}

fn claude() -> &'static str {
    HarnessBoundary::CLAUDE_AGENT_ACP.agents[0]
}

fn codex() -> &'static str {
    HarnessBoundary::CODEX_ACP.agents[0]
}

fn ended() -> Value {
    json!({"stopReason":"end_turn"})
}

/// What one run left: its outcome, how many sessions the seat was asked to open, and every
/// request its peers read.
struct Ran {
    outcome: RunOutcome,
    opened: usize,
    heard: Vec<Value>,
}

impl Ran {
    fn methods(&self) -> Vec<&str> {
        (self.heard.iter())
            .filter_map(|request| request["method"].as_str())
            .collect()
    }

    fn session_new(&self) -> &Value {
        (self.heard.iter())
            .find(|request| request["method"] == "session/new")
            .expect("a session/new")
    }

    /// The task's failure: its code and message.
    fn failure(&self) -> (String, String) {
        assert!(!self.outcome.ok, "the task fails");
        let error = self.outcome.records["ask"]
            .error
            .clone()
            .expect("a failure");
        (error.code, error.message)
    }
}

/// One run of the agent task `ask` (its body extended by `body`) on the seat `id`, whose
/// adapter answers `initialize` as `name` and ends each prompt with `result`.
async fn run_seat(id: &str, name: &'static str, body: &str, result: Value) -> Ran {
    let yaml = format!(
        "nika: seat-limits\nmodel: mock/echo\npermits: {{}}\ntasks:\n  ask:\n    agent:\n      \
         prompt: \"audit the parser\"\n{body}"
    );
    let wf = nika_schema::parse(
        &yaml,
        nika_schema::FileId::new(0),
        nika_schema::ParseMode::Strict,
    )
    .expect("fixture parses");
    let report = nika_check::check(&wf);
    assert!(report.is_clean(), "{report:?}");
    let seat = Arc::new(Seat {
        name,
        result,
        opened: AtomicUsize::new(0),
        heard: Arc::new(Mutex::new(Vec::new())),
        peers: Mutex::new(Vec::new()),
    });
    let invoke = Arc::new(InvokeVerb::new(Arc::new(MockToolExecutor::new())));
    let api = Arc::new(MockProvider::new("mock"));
    let runtime = Runtime::new(
        ExecVerb::new(Arc::new(MockShell::new())),
        Arc::clone(&invoke),
        InferVerb::new(
            Arc::new(nika_providers::ProviderRegistry::without_http(
                nika_providers::ProvidersConfig::new(),
            )),
            "mock/echo",
        ),
        AgentVerb::new(
            Arc::clone(&api),
            invoke,
            Arc::new(MockToolDefinitionProvider::new()),
            "mock/echo",
        ),
        MockClock::new(),
        RuntimeConfig::default(),
    )
    .with_harness_backend(Arc::clone(&seat) as Arc<dyn DynAgentBackend>, id.into())
    .expect("seated");
    let mut sink = VecSink::new();
    let outcome = runtime
        .run(&wf, &report, &mut DeterministicStamper::new(), &mut sink)
        .await
        .expect("the run settles");
    assert!(
        api.captured_requests().is_empty(),
        "the seat answers, never the API"
    );
    let peers = std::mem::take(&mut *seat.peers.lock().expect("peers"));
    for peer in peers {
        tokio::time::timeout(std::time::Duration::from_secs(5), peer)
            .await
            .expect("the client closed the session")
            .expect("the peer ran");
    }
    let heard = seat.heard.lock().expect("heard").clone();
    Ran {
        outcome,
        opened: seat.opened.load(Ordering::SeqCst),
        heard,
    }
}

/// The Claude Code agent session's options, every isolation key at its exact value, with the
/// declared turn limit when there is one.
fn claude_options(max_turns: Option<u32>) -> Value {
    let mut meta = json!({"claudeCode": {"options": {
        "settingSources": [], "strictMcpConfig": true, "mcpServers": {},
        "plugins": [], "skills": [], "agents": {},
        "allowDangerouslySkipPermissions": false,
        "env": {"CLAUDE_CODE_DISABLE_AUTO_MEMORY": "1",
                "ENABLE_CLAUDEAI_MCP_SERVERS": "false"},
        "thinking": {"type": "adaptive", "display": "summarized"}}}});
    if let Some(limit) = max_turns {
        meta["claudeCode"]["options"]["maxTurns"] = json!(limit);
    }
    meta
}

/// `max_turns: 7` on the Claude Code seat: `session/new` carries `maxTurns: 7` beside the exact
/// isolation keys. Without it the same session carries no `maxTurns`.
#[tokio::test]
async fn a_declared_turn_limit_reaches_the_claude_code_session_as_max_turns() {
    for (body, max_turns) in [("      max_turns: 7\n", Some(7)), ("", None)] {
        let ran = run_seat("claude-code", claude(), body, ended()).await;
        assert!(ran.outcome.ok, "{body}: {:?}", ran.outcome.records);
        assert_eq!(
            ran.session_new()["params"]["_meta"],
            claude_options(max_turns),
            "{body}"
        );
        assert_eq!(ran.methods().last(), Some(&"session/prompt"), "{body}");
    }
}

/// The run's refusal of a `max_turns` on a seat that is sent none.
const MAX_TURNS_REFUSED: &str = "invalid `agent` parameter `max_turns`: a turn limit is not \
     enforceable on this harness access — the harness runs its OWN loop and Nika sends this seat \
     no turn limit; drop `max_turns:` or run the task on the `claude-code` seat or an API or \
     local model";

/// The run's refusal of a `temperature` on any seat.
const TEMPERATURE_REFUSED: &str = "invalid `agent` parameter `temperature`: a sampling \
     temperature is not enforceable on a harness access — the harness samples its OWN model and \
     Nika sends no seat a temperature; drop `temperature:` or run the task on an API or local \
     model";

/// `max_turns` on Codex and on a seat whose turn limit is not read (Kimi Code): the task is
/// refused before the seat is asked for a session, in the run's exact words, and the peer reads
/// nothing.
#[tokio::test]
async fn a_turn_limit_on_a_seat_that_takes_none_is_refused_before_any_request() {
    for (id, name) in [("codex", codex()), ("kimi-code", "Kimi Code CLI")] {
        let ran = run_seat(id, name, "      max_turns: 6\n", ended()).await;
        assert_eq!(
            ran.failure(),
            ("NIKA-465".to_owned(), MAX_TURNS_REFUSED.to_owned()),
            "{id}"
        );
        assert_eq!((ran.opened, ran.heard.len()), (0, 0), "{id}: nothing sent");
    }
}

/// `temperature` on the Claude Code seat and on Codex: refused the same way, the peer reads
/// nothing.
#[tokio::test]
async fn a_temperature_on_any_seat_is_refused_before_any_request() {
    for (id, name) in [("claude-code", claude()), ("codex", codex())] {
        let ran = run_seat(id, name, "      temperature: 0.2\n", ended()).await;
        assert_eq!(
            ran.failure(),
            ("NIKA-465".to_owned(), TEMPERATURE_REFUSED.to_owned()),
            "{id}"
        );
        assert_eq!((ran.opened, ran.heard.len()), (0, 0), "{id}: nothing sent");
    }
}

/// A first turn stopped by its turn limit, as the run fails it.
const STOPPED_AT_THE_LIMIT: &str = "agent harness ended its turn without completing it (stop \
     reason `max_turn_requests`): the harness reached its limit of model requests for the turn \
     (the task's `max_turns:` when the seat was sent one, otherwise the harness's own); recover \
     the text received through on_error:";

/// A turn the adapter failed at its turn limit, as a run without a declared access fails it.
const FAILED_AT_THE_LIMIT: &str = "agent inference failed: provider error: harness refused: the \
     harness reported a failed turn (error · limit): Reached maximum number of turns (3)";

/// A first prompt that ends at the declared limit fails and writes no second prompt, with a
/// `schema:` (whose session keeps room for a repair) as without. This run declares no access
/// (`run.access`), so both shapes fail `NIKA-INFER-001`; the verb's tests pin `NIKA-1805` for a
/// failed turn under a declared one. claude-agent-acp 0.81.1 ends such a turn with the stop
/// reason `max_turn_requests`, or, when the SDK's result is an error (`error_max_turns`), with
/// a `limit` session failure titled with the SDK's own error words (`errors.join(", ")`, here
/// "Reached maximum number of turns (3)", the words of the CLI result `nika-harness` records):
/// both are scripted.
#[tokio::test]
async fn a_first_prompt_at_the_declared_limit_fails_and_writes_no_second_prompt() {
    let stopped = json!({"stopReason":"max_turn_requests"});
    let failed = json!({"stopReason":"end_turn","_meta":{"jetbrains":{"air":{"version":1,
        "sessionFailure":{"id":"turn-1:error","revision":1,"category":"limit",
        "severity":"error","title":"Reached maximum number of turns (3)",
        "actions":["new_session"]}}}}});
    let schema = "      schema: { type: object, required: [title], properties: { title: { type: \
                  string } } }\n";
    for with_schema in [false, true] {
        let body = format!(
            "      max_turns: 3\n{}",
            if with_schema { schema } else { "" }
        );
        for (result, code, text) in [
            (&stopped, "NIKA-INFER-001", STOPPED_AT_THE_LIMIT),
            (&failed, "NIKA-INFER-001", FAILED_AT_THE_LIMIT),
        ] {
            let ran = run_seat("claude-code", claude(), &body, result.clone()).await;
            assert_eq!(
                ran.failure(),
                (code.to_owned(), text.to_owned()),
                "schema {with_schema} {result}"
            );
            assert_eq!(
                ran.session_new()["params"]["_meta"]["claudeCode"]["options"]["maxTurns"],
                3
            );
            let prompts = (ran.methods().iter())
                .filter(|method| **method == "session/prompt")
                .count();
            assert_eq!(
                prompts, 1,
                "schema {with_schema} {result}: no repair prompt"
            );
            assert_eq!(ran.opened, 1, "schema {with_schema} {result}: one session");
        }
    }
}
