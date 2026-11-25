// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! A judged session start against scripted peers: the registry row's own
//! judgment meets the session's first answer, a refused identity never
//! reaches `session/new`, and each failure keeps the class the standalone
//! probe gave it.

use std::pin::Pin;
use std::time::Duration;

use futures_core::Stream as _;
use nika_kernel::ai::harness::{HarnessError, HarnessEvent, HarnessOutcome, HarnessRequest};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::probe::VersionPin;
use crate::spawn::{HarnessAdapter, SpawnedHarness};

type PeerReader = BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>;
type PeerWriter = tokio::io::WriteHalf<tokio::io::DuplexStream>;

/// One line from the client, `None` at its EOF (the client gave up).
async fn read(peer: &mut PeerReader) -> Option<Value> {
    let mut line = String::new();
    let size = peer.read_line(&mut line).await.expect("read peer");
    (size != 0).then(|| serde_json::from_str(&line).expect("JSON request"))
}

async fn reply(peer: &mut PeerWriter, request: &Value, result: Value) {
    let mut bytes =
        serde_json::to_vec(&json!({"jsonrpc":"2.0","id":request["id"],"result":result}))
            .expect("JSON");
    bytes.push(b'\n');
    peer.write_all(&bytes).await.expect("write peer");
    peer.flush().await.expect("flush peer");
}

/// The judgment of the shipped `codex` row: it answers as `codex-acp` (its
/// command), under its npm scope or as `Codex`, from 0.16 up to any 1.x.
fn codex_judge() -> super::StartJudge {
    let row = HarnessAdapter::new("codex", "codex-acp")
        .expect("row")
        .with_identities(vec!["Codex".to_owned()])
        .with_handshake_probe()
        .with_version_pin(VersionPin::new((0, 16), 1));
    SpawnedHarness::new(row).start_judge()
}

/// Drive one judged session with the frame deadline `idle` against a scripted
/// peer. The peer task is awaited, so an assertion it makes fails the test.
async fn judged<F, Fut>(idle: Duration, peer: F) -> Result<HarnessOutcome, HarnessError>
where
    F: FnOnce(PeerReader, PeerWriter) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let (ours, theirs) = tokio::io::duplex(64 * 1024);
    let (client_read, client_write) = tokio::io::split(ours);
    let (peer_read, peer_write) = tokio::io::split(theirs);
    let agent = tokio::spawn(peer(BufReader::new(peer_read), peer_write));
    let mut stream = crate::client::drive_profile(
        client_read,
        client_write,
        HarnessRequest::new("hello", "/tmp"),
        idle,
        None,
        None,
        None,
        Some(codex_judge()),
    );
    let outcome = loop {
        match std::future::poll_fn(|cx| Pin::new(&mut stream).poll_next(cx)).await {
            Some(Ok(HarnessEvent::Completed { outcome })) => break Ok(*outcome),
            Some(Ok(_)) => {}
            Some(Err(error)) => break Err(error),
            None => panic!("a scripted session completes or refuses"),
        }
    };
    drop(stream);
    agent.await.expect("scripted peer completes");
    outcome
}

const IDLE: Duration = Duration::from_secs(crate::IDLE_TIMEOUT_SECS);

/// Answer `initialize` with `agent_info`, then assert the client hung up:
/// a refused identity never reaches `session/new`.
async fn refused_at_initialize(mut r: PeerReader, mut w: PeerWriter, agent_info: Value) {
    let init = read(&mut r).await.expect("initialize");
    assert_eq!(init["method"], "initialize");
    reply(
        &mut w,
        &init,
        json!({"protocolVersion":1,"agentInfo":agent_info}),
    )
    .await;
    let next = read(&mut r).await;
    assert!(next.is_none(), "no session/new after a refusal: {next:?}");
}

fn unavailable(result: Result<HarnessOutcome, HarnessError>) -> String {
    let error = result.expect_err("refused at the start");
    assert!(!error.is_transient(), "no retry heals it: {error:?}");
    let HarnessError::Unavailable { reason } = error else {
        panic!("an unavailable adapter, got {error:?}")
    };
    assert!(reason.starts_with("adapter `codex`: "), "{reason}");
    reason
}

/// The session's own first answer is the adapter's identity: admitted, the
/// session goes on to `session/new` and its prompt on the same connection.
#[tokio::test]
async fn the_sessions_own_initialize_admits_the_adapter() {
    let outcome = judged(IDLE, |mut r, mut w| async move {
        let init = read(&mut r).await.expect("initialize");
        assert_eq!(init["method"], "initialize");
        assert!(
            init["params"]["clientCapabilities"].is_object(),
            "the session's own request, with its real capabilities: {init}"
        );
        let info = json!({"name":"@agentclientprotocol/codex-acp","version":"1.13.1"});
        reply(&mut w, &init, json!({"protocolVersion":1,"agentInfo":info})).await;
        let new = read(&mut r).await.expect("session/new");
        assert_eq!(new["method"], "session/new");
        reply(&mut w, &new, json!({"sessionId":"s-start"})).await;
        let prompt = read(&mut r).await.expect("prompt");
        assert_eq!(prompt["method"], "session/prompt");
        reply(&mut w, &prompt, json!({"stopReason":"end_turn"})).await;
    })
    .await
    .expect("admitted and answered");
    assert_eq!(outcome.output, "");
}

/// Another program answering is unavailable, in the probe's own words,
/// before any session exists.
#[tokio::test]
async fn another_program_is_unavailable_before_any_session() {
    let reason = unavailable(
        judged(IDLE, |r, w| {
            refused_at_initialize(r, w, json!({"name":"qwen-code","version":"0.21.0"}))
        })
        .await,
    );
    assert!(
        reason.contains("the handshake answered as `qwen-code`")
            && reason.contains("it answers as `codex` · `Codex`"),
        "{reason}"
    );
}

/// A release outside the row's pin, or an answer that names none, is
/// unavailable before any session.
#[tokio::test]
async fn a_release_outside_the_pin_is_unavailable_before_any_session() {
    for (agent_info, needle) in [
        (
            json!({"name":"codex-acp","version":"2.6.0"}),
            "handshake version 2.6 is outside the pin (>= 0.16 · major <= 1)",
        ),
        (
            json!({"name":"codex-acp","version":"0.9.0"}),
            "handshake version 0.9 is outside the pin",
        ),
        (
            json!({"name":"codex-acp"}),
            "the initialize answer carried no `agentInfo.version`",
        ),
    ] {
        let reason =
            unavailable(judged(IDLE, move |r, w| refused_at_initialize(r, w, agent_info)).await);
        assert!(reason.contains(needle), "{needle}: {reason}");
    }
}

/// An adapter that reads `initialize` and never answers is unanswered at the
/// start bound, well before the five-minute frame deadline: unknown and
/// transient (a loaded machine), never the absent adapter a retry cannot heal.
#[tokio::test(start_paused = true)]
async fn a_silent_adapter_is_unanswered_and_transient_at_the_start_bound() {
    let started = tokio::time::Instant::now();
    let error = judged(IDLE, |mut r, w| async move {
        let init = read(&mut r).await.expect("initialize");
        assert_eq!(init["method"], "initialize");
        // Hold the pipe open and say nothing; the client gives up first.
        assert!(read(&mut r).await.is_none(), "nothing follows");
        drop(w);
    })
    .await
    .expect_err("silent past the start bound");
    assert!(error.is_transient(), "a plain retry: replays it: {error:?}");
    assert_eq!(
        nika_error::traits::NikaErrorCode::nika_code(&error).to_string(),
        "NIKA-1804"
    );
    let HarnessError::Session { reason } = error else {
        panic!("an unanswered start, got {error:?}")
    };
    assert_eq!(
        reason,
        "adapter `codex`: the initialize handshake did not answer in 30s"
    );
    let waited = started.elapsed();
    assert!(
        waited >= super::start_timeout()
            && waited < super::start_timeout() + Duration::from_secs(1),
        "bounded by the start timeout, not the frame deadline: {waited:?}"
    );
}

/// An adapter that exits before answering is unavailable, not a session
/// death a retry would replay.
#[tokio::test]
async fn an_adapter_that_closes_its_pipe_is_unavailable() {
    let reason = unavailable(
        judged(IDLE, |mut r, w| async move {
            let init = read(&mut r).await.expect("initialize");
            assert_eq!(init["method"], "initialize");
            drop(w);
            drop(r);
        })
        .await,
    );
    assert_eq!(
        reason,
        "adapter `codex`: the adapter closed its pipe without answering initialize"
    );
}

/// An error answer to `session/new` is read like one to the prompt: an internal error
/// (codex-acp's `-32603` on a loaded machine) is a session that did not open, transient; the
/// adapter's `authentication_failed` kind is a lost sign-in, final; a refusal it types with a
/// JSON-RPC request code (`-32602`, a value it rejects) stays the final refusal it is. No
/// prompt follows any of them.
#[tokio::test]
async fn an_error_answer_to_session_new_is_classified_like_one_to_the_prompt() {
    let busy = json!({"code":-32603,"message":"Internal error",
        "data":{"details":"workspace routing discovery timed out"}});
    let expired = json!({"code":-32603,"message":"Internal error",
        "data":{"errorKind":"authentication_failed"}});
    let rejected = json!({"code":-32602,"message":"Invalid params"});
    let cases = [
        (
            busy,
            "NIKA-1804",
            "harness session failed: Internal error — workspace routing discovery timed out \
             (jsonrpc -32603)",
        ),
        (
            expired,
            "NIKA-1803",
            "harness unavailable: the app's sign-in has expired or was revoked",
        ),
        (
            rejected,
            "NIKA-1805",
            "harness refused: Invalid params (jsonrpc -32602)",
        ),
    ];
    for (answer, code, words) in cases {
        let error = judged(IDLE, move |mut r, mut w| async move {
            let init = read(&mut r).await.expect("initialize");
            let info = json!({"name":"@agentclientprotocol/codex-acp","version":"1.13.1"});
            reply(&mut w, &init, json!({"protocolVersion":1,"agentInfo":info})).await;
            let new = read(&mut r).await.expect("session/new");
            assert_eq!(new["method"], "session/new");
            let line = json!({"jsonrpc":"2.0","id":new["id"],"error":answer});
            let mut bytes = serde_json::to_vec(&line).expect("JSON");
            bytes.push(b'\n');
            w.write_all(&bytes).await.expect("write peer");
            w.flush().await.expect("flush peer");
            assert!(
                read(&mut r).await.is_none(),
                "no prompt after a failed session/new"
            );
        })
        .await
        .expect_err("session/new answered an error");
        assert_eq!(
            nika_error::traits::NikaErrorCode::nika_code(&error).to_string(),
            code
        );
        assert_eq!(error.is_transient(), code == "NIKA-1804", "{error:?}");
        assert_eq!(error.to_string(), words);
    }
}

/// Each ACP error code answered to `session/new` is read by its own meaning, and a code the
/// schema does not define fails closed: `-32000` (authentication required) is a lost sign-in,
/// final, like the typed kind; `-32002` (resource not found) and an undefined code are final
/// refusals, never a retried session.
#[tokio::test]
async fn an_auth_required_or_undefined_code_at_session_new_is_final() {
    let cases = [
        (
            json!({"code":-32000,"message":"Authentication required"}),
            "NIKA-1803",
            "harness unavailable: the app's sign-in has expired or was revoked",
        ),
        (
            json!({"code":-32002,"message":"Resource not found"}),
            "NIKA-1805",
            "harness refused: Resource not found (jsonrpc -32002)",
        ),
        (
            json!({"code":-32042,"message":"Quota spent"}),
            "NIKA-1805",
            "harness refused: Quota spent (jsonrpc -32042)",
        ),
    ];
    for (answer, code, words) in cases {
        let error = judged(IDLE, move |mut r, mut w| async move {
            let init = read(&mut r).await.expect("initialize");
            let info = json!({"name":"@agentclientprotocol/codex-acp","version":"1.13.1"});
            reply(&mut w, &init, json!({"protocolVersion":1,"agentInfo":info})).await;
            let new = read(&mut r).await.expect("session/new");
            let line = json!({"jsonrpc":"2.0","id":new["id"],"error":answer});
            let mut bytes = serde_json::to_vec(&line).expect("JSON");
            bytes.push(b'\n');
            w.write_all(&bytes).await.expect("write peer");
            w.flush().await.expect("flush peer");
            assert!(read(&mut r).await.is_none(), "no prompt after it");
        })
        .await
        .expect_err("session/new answered an error");
        assert_eq!(
            nika_error::traits::NikaErrorCode::nika_code(&error).to_string(),
            code,
            "{error:?}"
        );
        assert!(!error.is_transient(), "{error:?}");
        assert_eq!(error.to_string(), words);
    }
}

/// An internal error answered to `initialize` is the same session that did not open.
#[tokio::test]
async fn an_internal_error_answer_to_initialize_is_transient() {
    let error = judged(IDLE, |mut r, mut w| async move {
        let init = read(&mut r).await.expect("initialize");
        let line = json!({"jsonrpc":"2.0","id":init["id"],
            "error":{"code":-32603,"message":"Internal error"}});
        let mut bytes = serde_json::to_vec(&line).expect("JSON");
        bytes.push(b'\n');
        w.write_all(&bytes).await.expect("write peer");
        w.flush().await.expect("flush peer");
        assert!(read(&mut r).await.is_none(), "no session/new after it");
    })
    .await
    .expect_err("initialize answered an error");
    assert!(error.is_transient(), "{error:?}");
    let HarnessError::Session { reason } = error else {
        panic!("a session that did not open, got {error:?}")
    };
    assert_eq!(reason, "Internal error (jsonrpc -32603)");
}

/// A call whose own frame deadline is shorter than the start bound keeps
/// its class: the silence is the session death it always was.
#[tokio::test(start_paused = true)]
async fn a_shorter_frame_deadline_keeps_its_session_class() {
    let error = judged(Duration::from_secs(5), |mut r, w| async move {
        let init = read(&mut r).await.expect("initialize");
        assert_eq!(init["method"], "initialize");
        assert!(read(&mut r).await.is_none(), "nothing follows");
        drop(w);
    })
    .await
    .expect_err("silent past the call's own deadline");
    assert!(error.is_transient(), "{error:?}");
    assert!(error.to_string().contains("sent nothing for 5s"), "{error}");
}
