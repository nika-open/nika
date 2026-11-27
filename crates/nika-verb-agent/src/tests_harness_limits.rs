// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! A limit an `agent:` task declares on a seat is sent to the seat or refused before anything
//! is sent: `max_turns` reaches a seat the composer says is sent one (from the seat's id) and is
//! refused on every other, `temperature` is refused on every seat, and a first turn stopped at
//! the limit fails without a repair prompt, with the code its shape and the declared access give.

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
use nika_types::access::{AccessFallback, AccessProtocol, AccessRequirement};
use serde_json::json;

use super::*;
use crate::harness_path::HarnessSeat;

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

/// A first turn stopped by its turn limit, as the verb fails it.
const STOPPED_AT_THE_LIMIT: &str = "agent harness ended its turn without completing it (stop \
     reason `max_turn_requests`): the harness reached its limit of model requests for the turn \
     (the task's `max_turns:` when the seat was sent one, otherwise the harness's own); recover \
     the text received through on_error:";

/// The client's reason for a turn the adapter failed at its turn limit: its session failure,
/// with the words claude-agent-acp 0.81.1 gives on `error_max_turns` (the SDK's error for a
/// limit of 3, as `nika-harness`'s recorded result carries it for 1).
const FAILED_AT_THE_LIMIT: &str =
    "the harness reported a failed turn (error · limit): Reached maximum number of turns (3)";

/// What the seat received: every request it was asked to open, and every follow-up prompt.
#[derive(Default)]
struct Heard {
    requests: Mutex<Vec<HarnessRequest>>,
    follow_ups: Mutex<Vec<String>>,
}

/// How the seat's first turn ends.
#[derive(Clone, Copy)]
enum First {
    /// The turn ends with this stop reason, with a lane when the request keeps room for a
    /// follow-up (as the ACP client does), closed otherwise.
    Stopped(&'static str),
    /// The client fails the turn with the adapter's session failure (a refusal in its words).
    Failed(&'static str),
}

struct Seat {
    first: First,
    heard: Arc<Heard>,
}

struct Turn {
    item: Option<Result<HarnessEvent, HarnessError>>,
}

impl futures_core::Stream for Turn {
    type Item = Result<HarnessEvent, HarnessError>;
    fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(self.get_mut().item.take())
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
        let item = match self.first {
            First::Failed(reason) => Err(HarnessError::Refused {
                reason: reason.to_owned(),
            }),
            First::Stopped(stop) => {
                let outcome = Box::new(
                    HarnessOutcome::new(r#"{"partial": true}"#)
                        .with_stop_reason(HarnessStopReason::parse(stop)),
                );
                let heard = Arc::clone(&self.heard);
                Ok(if open {
                    let follow_up = FollowUpReply::new(Box::new(move |prompt| {
                        heard.follow_ups.lock().unwrap().push(prompt);
                    }));
                    HarnessEvent::TurnEnded { outcome, follow_up }
                } else {
                    HarnessEvent::Completed { outcome }
                })
            }
        };
        Box::pin(async move { Ok(Box::pin(Turn { item: Some(item) }) as HarnessEventStream) })
    }
}

type Verb = AgentVerb<MockProvider, MockToolExecutor, MockToolDefinitionProvider>;

/// The verb on a seat its composer says is sent a declared turn limit (`limit_sent`) or not,
/// whose first turn ends as `first` says.
fn seated(limit_sent: bool, first: First) -> (Verb, Arc<Heard>) {
    let heard = Arc::new(Heard::default());
    let seat = Arc::new(Seat {
        first,
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
        HarnessSeat::new(seat as Arc<dyn DynAgentBackend>, "/tmp").applying_max_turns(limit_sent),
    );
    (verb, heard)
}

fn limited(max_turns: u32) -> AgentInput {
    let mut input = AgentInput::new("audit the parser");
    input.max_turns = Some(max_turns);
    input
}

/// A declared Claude Code seat over ACP, as a workflow's `run.access` gives the task.
fn declared() -> AccessRequirement {
    AccessRequirement::new()
        .with_via(Some("claude-code".into()))
        .with_protocol(Some(AccessProtocol::Acp))
        .with_fallback(Some(AccessFallback::None))
}

/// The refusal of `param`, with its code and its whole text.
fn refused(err: &VerbAgentError, param: &str) -> (bool, String, String) {
    let named = matches!(err, VerbAgentError::InvalidParam { param: p, .. } if *p == param);
    (named, err.spec_code(), err.to_string())
}

/// A seat its composer does not say is sent a turn limit (Codex's, one whose turn limit is not
/// read) refuses a declared `max_turns` before its session: the backend is never asked to open
/// one.
#[tokio::test]
async fn a_turn_limit_on_a_seat_that_takes_none_is_refused_before_the_session() {
    let (verb, heard) = seated(false, First::Stopped("end_turn"));
    let err = verb.run(limited(7)).await.expect_err("refused");
    assert_eq!(
        refused(&err, "max_turns"),
        (true, "NIKA-465".to_owned(), MAX_TURNS_REFUSED.to_owned())
    );
    assert!(
        heard.requests.lock().unwrap().is_empty(),
        "no session asked"
    );
}

/// A sampling temperature is refused on every seat, the one that is sent a turn limit too.
#[tokio::test]
async fn a_temperature_is_refused_on_every_seat_before_the_session() {
    for sent in [false, true] {
        let (verb, heard) = seated(sent, First::Stopped("end_turn"));
        let mut input = AgentInput::new("audit the parser");
        input.temperature = Some(0.2);
        let err = verb.run(input).await.expect_err("refused");
        assert_eq!(
            refused(&err, "temperature"),
            (true, "NIKA-465".to_owned(), TEMPERATURE_REFUSED.to_owned()),
            "{sent}"
        );
        assert!(heard.requests.lock().unwrap().is_empty(), "{sent}");
    }
}

/// A seat that is sent the limit receives exactly the declared value, inside the range the
/// native loop accepts; a task without one sends none (the native default of 10 is the native
/// loop's, never a seat's).
#[tokio::test]
async fn a_seat_that_is_sent_the_limit_receives_the_declared_value_alone() {
    let (verb, heard) = seated(true, First::Stopped("end_turn"));
    verb.run(limited(7)).await.expect("completed");
    verb.run(AgentInput::new("audit the parser"))
        .await
        .expect("completed");
    let sent: Vec<Option<u32>> = (heard.requests.lock().unwrap().iter())
        .map(|request| request.max_turns)
        .collect();
    assert_eq!(sent, [Some(7), None]);
    for out_of_range in [0, MAX_TURNS_CEILING + 1] {
        let (verb, heard) = seated(true, First::Stopped("end_turn"));
        let err = verb.run(limited(out_of_range)).await.expect_err("range");
        assert_eq!(
            refused(&err, "max_turns"),
            (
                true,
                "NIKA-465".to_owned(),
                format!(
                    "invalid `agent` parameter `max_turns`: max_turns must be 1-1000, got \
                     {out_of_range}"
                )
            )
        );
        assert!(heard.requests.lock().unwrap().is_empty(), "{out_of_range}");
    }
}

/// Without a declared access, a first turn stopped at the limit (`max_turn_requests`) fails
/// `NIKA-INFER-001` with its stop named, and no repair prompt is sent, with a `schema:` (whose
/// session keeps room for one) as without.
#[tokio::test]
async fn a_first_turn_at_the_limit_without_a_declared_access_sends_no_repair() {
    for with_schema in [false, true] {
        let (verb, heard) = seated(true, First::Stopped("max_turn_requests"));
        let mut input = limited(3);
        if with_schema {
            input.schema = Some(json!({"type": "object", "required": ["title"]}));
        }
        let err = verb.run(input).await.expect_err("stopped");
        assert!(
            matches!(&err, VerbAgentError::HarnessStopped { stop_reason, .. }
                if stop_reason == "max_turn_requests"),
            "{with_schema}: {err}"
        );
        assert_eq!(
            (err.spec_code(), err.to_string()),
            ("NIKA-INFER-001".to_owned(), STOPPED_AT_THE_LIMIT.to_owned()),
            "{with_schema}"
        );
        let requests = heard.requests.lock().unwrap();
        assert_eq!(requests.len(), 1, "{with_schema}: one session");
        assert_eq!(requests[0].max_turns, Some(3), "{with_schema}");
        assert_eq!(
            requests[0].follow_up, with_schema,
            "{with_schema}: room for a repair"
        );
        assert!(
            heard.follow_ups.lock().unwrap().is_empty(),
            "{with_schema}: no repair prompt"
        );
    }
}

/// The four ways a first turn ends at the limit, each with its exact code and text, and no
/// repair prompt: the stop reason `max_turn_requests` fails `NIKA-INFER-001` with or without a
/// declared access; a turn the adapter reports as failed fails `NIKA-1805` under a declared
/// access, which keeps the harness's own class, and `NIKA-INFER-001` without one.
#[tokio::test]
async fn a_first_turn_at_the_limit_fails_with_the_code_its_shape_and_access_give() {
    let undeclared_failure =
        format!("agent inference failed: provider error: harness refused: {FAILED_AT_THE_LIMIT}");
    let declared_failure = format!("agent harness: harness refused: {FAILED_AT_THE_LIMIT}");
    for (access, first, code, text) in [
        (
            false,
            First::Stopped("max_turn_requests"),
            "NIKA-INFER-001",
            STOPPED_AT_THE_LIMIT,
        ),
        (
            true,
            First::Stopped("max_turn_requests"),
            "NIKA-INFER-001",
            STOPPED_AT_THE_LIMIT,
        ),
        (
            false,
            First::Failed(FAILED_AT_THE_LIMIT),
            "NIKA-INFER-001",
            undeclared_failure.as_str(),
        ),
        (
            true,
            First::Failed(FAILED_AT_THE_LIMIT),
            "NIKA-1805",
            declared_failure.as_str(),
        ),
    ] {
        for with_schema in [false, true] {
            let (verb, heard) = seated(true, first);
            let mut input = limited(3);
            if access {
                input = input.with_requirement(Some(&declared()));
            }
            if with_schema {
                input.schema = Some(json!({"type": "object", "required": ["title"]}));
            }
            let err = verb.run(input).await.expect_err("stopped");
            assert_eq!(
                (err.spec_code(), err.to_string()),
                (code.to_owned(), text.to_owned()),
                "access {access} schema {with_schema}"
            );
            assert_eq!(heard.requests.lock().unwrap().len(), 1, "one session");
            assert!(
                heard.follow_ups.lock().unwrap().is_empty(),
                "access {access} schema {with_schema}: no repair prompt"
            );
        }
    }
}
