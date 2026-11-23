// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! The call gate at the seam it bounds: real round-trips against a transport
//! that answers only when the test says so. In-flight calls are counted
//! INSIDE the transport, below the gate, so the bound is observed, not
//! restated.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use nika_kernel::ai::provider::{InferRequest, Message, ProviderInferDyn, ProviderStreamDyn, Role};
use nika_kernel::http::{HttpError, HttpPostDyn, HttpRequest, HttpResponse, HttpStreamResponse};
use nika_kernel::secret::Secret;

use crate::registry::{ProviderRegistry, ProvidersConfig, ResolvedProvider};
use crate::{ProviderCallEntry, ProviderCallGate, ProviderCallRefused, ProviderCallSlot};

const ANSWER: &str = r#"{"id":"cc_g","model":"compat-test","choices":[{"message":{"content":"gated"},"finish_reason":"stop"}],"usage":{"prompt_tokens":2,"completion_tokens":1}}"#;
const BUSY: &str = r#"{"error":{"message":"slow down"}}"#;

/// A transport whose answers wait for the test. It counts the posts inside
/// it and remembers the most it ever held.
struct Held {
    status: u16,
    body: &'static str,
    inside: AtomicUsize,
    peak: AtomicUsize,
    answered: AtomicUsize,
    open: tokio::sync::Semaphore,
}

impl Held {
    fn answering(status: u16, body: &'static str) -> Arc<Self> {
        Arc::new(Self {
            status,
            body,
            inside: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            answered: AtomicUsize::new(0),
            open: tokio::sync::Semaphore::new(0),
        })
    }

    fn inside(&self) -> usize {
        self.inside.load(Ordering::SeqCst)
    }

    fn peak(&self) -> usize {
        self.peak.load(Ordering::SeqCst)
    }

    fn answered(&self) -> usize {
        self.answered.load(Ordering::SeqCst)
    }

    fn release(&self, count: usize) {
        self.open.add_permits(count);
    }

    async fn until(&self, what: &str, reached: impl Fn(&Self) -> bool) {
        let waited = tokio::time::timeout(Duration::from_secs(10), async {
            while !reached(self) {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await;
        assert!(
            waited.is_ok(),
            "{what}: inside={} answered={}",
            self.inside(),
            self.answered()
        );
    }

    /// Nothing more enters while the answers are withheld.
    async fn holds(&self, inside: usize) {
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(self.inside(), inside);
    }
}

impl HttpPostDyn for Held {
    async fn post(&self, request: HttpRequest) -> Result<HttpResponse, HttpError> {
        let inside = self.inside.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(inside, Ordering::SeqCst);
        let answer = self.open.acquire().await;
        self.inside.fetch_sub(1, Ordering::SeqCst);
        answer
            .map_err(|_| HttpError::Unsupported {
                reason: "the test transport closed".to_owned(),
            })?
            .forget();
        self.answered.fetch_add(1, Ordering::SeqCst);
        let body = bytes::Bytes::from_static(self.body.as_bytes());
        Ok(HttpResponse::new(
            self.status,
            BTreeMap::new(),
            body,
            request.url,
        ))
    }

    async fn send_streaming(&self, _: HttpRequest) -> Result<HttpStreamResponse, HttpError> {
        Err(HttpError::Unsupported {
            reason: "a buffered transport only".to_owned(),
        })
    }
}

/// A gate that bounds one provider and leaves every other alone.
#[derive(Debug)]
struct Limit {
    provider: &'static str,
    slots: Arc<tokio::sync::Semaphore>,
}

impl Limit {
    fn of(provider: &'static str, slots: usize) -> Arc<Self> {
        Arc::new(Self {
            provider,
            slots: Arc::new(tokio::sync::Semaphore::new(slots)),
        })
    }

    fn free(&self) -> usize {
        self.slots.available_permits()
    }
}

impl ProviderCallGate for Limit {
    fn enter<'a>(&'a self, provider: &'a str) -> ProviderCallEntry<'a> {
        Box::pin(async move {
            if provider != self.provider {
                return Ok(ProviderCallSlot::unbounded());
            }
            // A closed gate withdraws every call that waits on it.
            match Arc::clone(&self.slots).acquire_owned().await {
                Ok(permit) => Ok(ProviderCallSlot::holding(permit)),
                Err(_) => Err(ProviderCallRefused::new("the run was stopped")),
            }
        })
    }
}

fn request() -> InferRequest {
    InferRequest::new("ignored", vec![Message::text(Role::User, "gate check")])
}

fn gated(transport: &Arc<Held>, gate: &Arc<Limit>) -> Arc<ResolvedProvider<Held>> {
    let config = ProvidersConfig::new().with_key("groq", Secret::new("gate-test-key"));
    let gate: Arc<dyn ProviderCallGate> = gate.clone();
    Arc::new(
        ProviderRegistry::new(Arc::clone(transport), config)
            .with_call_gate(gate)
            .resolve("groq/test-model")
            .expect("the profile resolves"),
    )
}

/// Five calls, two slots: the transport never holds more than two
/// round-trips, each answer lets exactly one more in, and all five answer.
#[tokio::test]
async fn the_round_trips_in_flight_never_exceed_the_slots() {
    let transport = Held::answering(200, ANSWER);
    let gate = Limit::of("groq", 2);
    let provider = gated(&transport, &gate);
    let calls: Vec<_> = (0..5)
        .map(|_| {
            let provider = Arc::clone(&provider);
            tokio::spawn(async move { provider.infer(request()).await })
        })
        .collect();
    transport
        .until("two calls enter", |t| t.inside() == 2)
        .await;
    transport.holds(2).await;
    assert_eq!(gate.free(), 0);

    transport.release(1);
    transport
        .until("the third takes the freed slot", |t| {
            t.answered() == 1 && t.inside() == 2
        })
        .await;
    transport.holds(2).await;

    transport.release(4);
    for call in calls {
        let answer = call.await.expect("join").expect("the call answers");
        assert!(
            answer.content.iter().any(|block| matches!(
                block,
                nika_kernel::ai::provider::ContentBlock::Text { text } if text == "gated"
            )),
            "{:?}",
            answer.content
        );
    }
    assert_eq!(transport.answered(), 5);
    assert_eq!(transport.peak(), 2, "never three round-trips at once");
    assert_eq!(gate.free(), 2, "every slot came back");
}

/// A call cancelled while it waits its turn leaves the line: it takes no
/// slot, and the calls behind it are not stuck.
#[tokio::test]
async fn a_call_cancelled_while_it_waits_takes_no_slot() {
    let transport = Held::answering(200, ANSWER);
    let gate = Limit::of("groq", 1);
    let provider = gated(&transport, &gate);
    let spawn = || {
        let provider = Arc::clone(&provider);
        tokio::spawn(async move { provider.infer(request()).await })
    };
    let first = spawn();
    transport
        .until("the first enters", |t| t.inside() == 1)
        .await;
    let waiting = spawn();
    transport.holds(1).await;
    waiting.abort();
    assert!(waiting.await.expect_err("aborted").is_cancelled());

    let third = spawn();
    transport.release(2);
    first.await.expect("join").expect("the first answers");
    third.await.expect("join").expect("the third answers");
    assert_eq!(transport.answered(), 2, "the cancelled call never left");
    assert_eq!(transport.peak(), 1);
    assert_eq!(gate.free(), 1);

    // A call cancelled while in flight gives its slot back too.
    let cut = spawn();
    transport.until("it enters", |t| t.inside() == 1).await;
    cut.abort();
    assert!(cut.await.expect_err("aborted").is_cancelled());
    assert_eq!(gate.free(), 1, "a dropped round-trip frees its slot");
}

/// The gate owns no retry (ADR-019). A transient rejection ends the
/// round-trip and returns the slot at once: the verb's backoff holds none.
#[tokio::test]
async fn a_rejected_call_gives_its_slot_back_before_any_retry() {
    let transport = Held::answering(429, BUSY);
    let gate = Limit::of("groq", 1);
    let provider = gated(&transport, &gate);
    let call = {
        let provider = Arc::clone(&provider);
        tokio::spawn(async move { provider.infer(request()).await })
    };
    transport
        .until("the call enters", |t| t.inside() == 1)
        .await;
    assert_eq!(gate.free(), 0, "the round-trip holds the slot");
    transport.release(1);
    let rejected = call.await.expect("join").expect_err("a 429 ends the call");
    assert!(rejected.is_transient(), "{rejected:?}");
    assert_eq!(
        transport.answered(),
        1,
        "one round-trip: the gate re-sends nothing"
    );
    assert_eq!(gate.free(), 1, "the slot is free while a retry would wait");
}

/// A call the gate withdraws while it waits sends nothing, says so, and is
/// not an error a verb retries.
#[tokio::test]
async fn a_withdrawn_call_sends_nothing_and_is_not_retried() {
    let transport = Held::answering(200, ANSWER);
    let gate = Limit::of("groq", 1);
    let provider = gated(&transport, &gate);
    let first = {
        let provider = Arc::clone(&provider);
        tokio::spawn(async move { provider.infer(request()).await })
    };
    transport
        .until("the first enters", |t| t.inside() == 1)
        .await;
    let waiting = {
        let provider = Arc::clone(&provider);
        tokio::spawn(async move { provider.infer(request()).await })
    };
    transport.holds(1).await;

    gate.slots.close();
    let withdrawn = waiting
        .await
        .expect("join")
        .expect_err("the waiting call is withdrawn");
    assert!(!withdrawn.is_transient(), "{withdrawn:?}");
    assert!(withdrawn.inference_calls().is_empty(), "nothing was sent");
    assert_eq!(
        withdrawn.to_string(),
        "provider error: no request was sent to groq: the run was stopped"
    );
    transport.release(1);
    first
        .await
        .expect("join")
        .expect("the call in flight answers");
    assert_eq!(transport.answered(), 1, "one request reached the provider");

    // The test transport opens no stream at all, so only the words tell a
    // stream the gate withdrew from one the transport refused.
    let stream = provider.infer_stream(request()).await;
    assert_eq!(
        stream.err().map(|refused| refused.to_string()).as_deref(),
        Some("provider error: no request was sent to groq: the run was stopped"),
        "a withdrawn stream never reaches the transport"
    );
    assert_eq!(transport.answered(), 1);
}

/// A provider the gate does not bound is not delayed by the one it does,
/// and a registry without a gate sends as it always did.
#[tokio::test]
async fn only_the_bounded_provider_waits() {
    let gate = Limit::of("groq", 1);
    let held = Arc::clone(&gate.slots)
        .acquire_owned()
        .await
        .expect("the only slot");
    let shared: Arc<dyn ProviderCallGate> = gate.clone();
    let mock = ProviderRegistry::without_http(ProvidersConfig::new())
        .with_call_gate(shared)
        .resolve("mock/echo")
        .expect("mock resolves");
    let asked = mock.infer(InferRequest::new(
        "echo",
        vec![Message::text(Role::User, "hello")],
    ));
    let answer = tokio::time::timeout(Duration::from_secs(5), asked)
        .await
        .expect("another provider does not wait")
        .expect("the mock answers");
    assert!(!answer.content.is_empty());
    drop(held);

    let ungated = ProviderRegistry::without_http(ProvidersConfig::new())
        .resolve("mock/echo")
        .expect("mock resolves");
    ungated
        .infer(InferRequest::new(
            "echo",
            vec![Message::text(Role::User, "hello")],
        ))
        .await
        .expect("no gate, no wait");
}

/// A stream is in flight until it is dropped: it keeps its slot that long.
#[tokio::test]
async fn a_stream_keeps_its_slot_until_it_is_dropped() {
    let gate = Limit::of("mock", 1);
    let shared: Arc<dyn ProviderCallGate> = gate.clone();
    let mock = ProviderRegistry::without_http(ProvidersConfig::new())
        .with_call_gate(shared)
        .resolve("mock/echo")
        .expect("mock resolves");
    let ask = || InferRequest::new("echo", vec![Message::text(Role::User, "hello")]);
    let stream = mock.infer_stream(ask()).await.expect("the stream opens");
    assert_eq!(gate.free(), 0, "an open stream holds the slot");
    let events = crate::test_support::collect(stream).await;
    assert!(!events.is_empty());
    assert_eq!(gate.free(), 1, "the consumed stream gave it back");

    let unread = mock.infer_stream(ask()).await.expect("a second stream");
    assert_eq!(gate.free(), 0);
    drop(unread);
    assert_eq!(gate.free(), 1, "a dropped stream gave it back");
}
