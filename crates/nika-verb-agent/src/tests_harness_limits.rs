// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! A limit an `agent:` task declares on a seat is applied by the seat or refused before
//! anything is sent: `max_turns` reaches a seat that applies it (the composer says so from the
//! seat's id) and is refused on every other, `temperature` is refused on every seat, and a
//! first turn stopped at the limit fails typed without a repair prompt.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use nika_error::traits::NikaErrorCode as _;
use nika_kernel::ai::harness::{
    DynAgentBackend, FollowUpReply, HarnessError, HarnessEvent, HarnessEventStream, HarnessOutcome,
    HarnessRequest, HarnessStopReason,
};
use nika_kernel_mock::{MockProvider, MockToolDefinitionProvider, MockToolExecutor};
use serde_json::json;

use super::*;
use crate::harness_path::HarnessSeat;

/// What the seat received: every request it was asked to open, and every follow-up prompt.
#[derive(Default)]
struct Heard {
    requests: Mutex<Vec<HarnessRequest>>,
    follow_ups: Mutex<Vec<String>>,
}

/// A seat whose first turn is `first`, ended with a lane when the request keeps room for a
/// follow-up (as the ACP client does), closed otherwise.
struct Seat {
    first: HarnessOutcome,
    heard: Arc<Heard>,
}

struct Turn {
    event: Option<HarnessEvent>,
}

impl futures_core::Stream for Turn {
    type Item = Result<HarnessEvent, HarnessError>;
    fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(self.get_mut().event.take().map(Ok))
    }
}

impl DynAgentBackend for Seat {
    fn run_agent_boxed(
        &self,
        request: HarnessRequest,
    ) -> Pin<
        Box<dyn std::future::Future<Output = Result<HarnessEventStream, HarnessError>> + Send + '_>,
    > {
        let open = request.follow_up;
        self.heard.requests.lock().unwrap().push(request);
        let outcome = Box::new(self.first.clone());
        let heard = Arc::clone(&self.heard);
        let event = if open {
            let follow_up = FollowUpReply::new(Box::new(move |prompt| {
                heard.follow_ups.lock().unwrap().push(prompt);
            }));
            HarnessEvent::TurnEnded { outcome, follow_up }
        } else {
            HarnessEvent::Completed { outcome }
        };
        Box::pin(async move { Ok(Box::pin(Turn { event: Some(event) }) as HarnessEventStream) })
    }
}

type Verb = AgentVerb<MockProvider, MockToolExecutor, MockToolDefinitionProvider>;

/// The verb on a seat that applies a declared turn limit (`applies`) or does not, whose first
/// turn ends with `stop`.
fn seated(applies: bool, stop: &str) -> (Verb, Arc<Heard>) {
    let heard = Arc::new(Heard::default());
    let seat = Arc::new(Seat {
        first: HarnessOutcome::new(r#"{"partial": true}"#)
            .with_stop_reason(HarnessStopReason::parse(stop)),
        heard: Arc::clone(&heard),
    });
    let verb = AgentVerb::new(
        Arc::new(MockProvider::new("mock")),
        Arc::new(nika_verb_invoke::InvokeVerb::new(Arc::new(
            MockToolExecutor::new(),
        ))),
        Arc::new(MockToolDefinitionProvider::new()),
        "openai/gpt-6-luna",
    )
    .with_harness_seat(
        HarnessSeat::new(seat as Arc<dyn DynAgentBackend>, "/tmp").applying_max_turns(applies),
    );
    (verb, heard)
}

fn limited(max_turns: u32) -> AgentInput {
    let mut input = AgentInput::new("audit the parser");
    input.max_turns = Some(max_turns);
    input
}

fn refused_naming(err: &VerbAgentError, named: &str) -> bool {
    matches!(err, VerbAgentError::InvalidParam { param, .. } if *param == named)
        && err.spec_code() == "NIKA-465"
        && err.to_string().contains(&format!("`{named}`"))
}

/// A seat built without saying it applies a turn limit (Codex's, an unmeasured one) refuses a
/// declared `max_turns` before its session: the backend is never asked to open one.
#[tokio::test]
async fn a_turn_limit_on_a_seat_that_takes_none_is_refused_before_the_session() {
    let (verb, heard) = seated(false, "end_turn");
    let err = verb.run(limited(7)).await.expect_err("refused");
    assert!(refused_naming(&err, "max_turns"), "{err}");
    assert!(
        err.to_string()
            .contains("drop `max_turns:` or run the task on a seat or a model that does"),
        "{err}"
    );
    assert!(
        heard.requests.lock().unwrap().is_empty(),
        "no session asked"
    );
}

/// A sampling temperature is refused on every seat, the one that applies a turn limit too.
#[tokio::test]
async fn a_temperature_is_refused_on_every_seat_before_the_session() {
    for applies in [false, true] {
        let (verb, heard) = seated(applies, "end_turn");
        let mut input = AgentInput::new("audit the parser");
        input.temperature = Some(0.2);
        let err = verb.run(input).await.expect_err("refused");
        assert!(refused_naming(&err, "temperature"), "{applies}: {err}");
        assert!(
            err.to_string()
                .contains("drop `temperature:` or run the task on an API or local model"),
            "{err}"
        );
        assert!(heard.requests.lock().unwrap().is_empty(), "{applies}");
    }
}

/// A seat that applies the limit receives exactly the declared value, inside the range the
/// native loop accepts; a task without one sends none (the native default of 10 is the native
/// loop's, never a seat's).
#[tokio::test]
async fn a_seat_that_applies_the_limit_receives_the_declared_value_alone() {
    let (verb, heard) = seated(true, "end_turn");
    verb.run(limited(7)).await.expect("completed");
    verb.run(AgentInput::new("audit the parser"))
        .await
        .expect("completed");
    let sent: Vec<Option<u32>> = (heard.requests.lock().unwrap().iter())
        .map(|request| request.max_turns)
        .collect();
    assert_eq!(sent, [Some(7), None]);
    for out_of_range in [0, MAX_TURNS_CEILING + 1] {
        let (verb, heard) = seated(true, "end_turn");
        let err = verb.run(limited(out_of_range)).await.expect_err("range");
        assert!(refused_naming(&err, "max_turns"), "{out_of_range}: {err}");
        assert!(heard.requests.lock().unwrap().is_empty(), "{out_of_range}");
    }
}

/// A first turn that stopped at the declared limit (`max_turn_requests`) fails typed with its
/// stop named, and no repair prompt is sent, with a `schema:` (whose session keeps room for one)
/// as without.
#[tokio::test]
async fn a_first_turn_at_the_declared_limit_fails_typed_and_sends_no_repair() {
    for typed in [false, true] {
        let (verb, heard) = seated(true, "max_turn_requests");
        let mut input = limited(3);
        if typed {
            input.schema = Some(json!({"type": "object", "required": ["title"]}));
        }
        let err = verb.run(input).await.expect_err("stopped");
        assert!(
            matches!(&err, VerbAgentError::HarnessStopped { stop_reason, .. }
                if stop_reason == "max_turn_requests"),
            "{typed}: {err}"
        );
        assert_eq!(err.spec_code(), "NIKA-INFER-001", "{typed}");
        assert!(
            err.to_string().contains("the task's `max_turns:`"),
            "{typed}: {err}"
        );
        let requests = heard.requests.lock().unwrap();
        assert_eq!(requests.len(), 1, "{typed}: one session");
        assert_eq!(requests[0].max_turns, Some(3), "{typed}");
        assert_eq!(requests[0].follow_up, typed, "{typed}: room for a repair");
        assert!(
            heard.follow_ups.lock().unwrap().is_empty(),
            "{typed}: no repair prompt"
        );
    }
}
