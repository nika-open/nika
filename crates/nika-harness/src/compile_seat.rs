// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! An ACP harness as an AUTHORING BACKEND (the 2026-09-22 addendum): the operator's own agent
//! harness (Claude Code · Codex · Gemini CLI · Kimi Code · Qwen Code), driven through the Agent
//! Client Protocol the engine already speaks for the `agent:` verb and the run door's
//! `--access`, answers the compiler's authoring calls behind the same `ProviderInferDyn` seam
//! a direct API provider answers. The compile core stays backend-neutral. What the harness
//! received is what the compiler sent: the system message as the session's system instruction,
//! the conversation folded into one prompt, the answer schema stated in words (ACP enforces
//! none, so the first complete JSON object of the answer is the answer). Every tool permission
//! the harness asks for is DENIED — authoring is tool-free by contract — and the receipt names
//! the backend, the adapter, the observed model, whether usage was reported and the cost basis
//! (a subscription: no token meter is fabricated). Each ask is refused with the harness's first
//! one-time refusal (`cancelled` when it offers none) and counted in the receipt; the seat sets
//! no cap on asks and reads the session to its end.
//!
//! A turn answers only when it ended `end_turn`, or when its backend names no stop (the ACP
//! driver always names one). A turn the model refused, cut at its output limit, stopped at
//! the harness's own limit of model requests, called off, or ended with any other stop is
//! refused as a provider error that names the harness and the stop, never the text: what such
//! a turn said is not the answer, so it never reaches the compiler as one. Its call stays in
//! the receipt with the asks it denied. A harness that goes on after a denied ask and ends
//! `end_turn` answers; one that calls its turn off instead (it stops `cancelled`) is refused,
//! even when its text held a whole answer.
//!
//! Codex is the exception, and the same rule the Session door applies: its ACP completion
//! profile is not an empty-tools profile (`apply_patch` stays callable inside the scratch), so
//! `codex/<model>` seats the native infer-grade transport under its measured empty-tools
//! profile instead, never the ACP agent.
use crate::authoring::HarnessAuthoring;
use crate::{SpawnedHarness, seat_from_id};
use nika_kernel::ai::harness::{
    AgentBackendDyn, HarnessEvent, HarnessEventStream, HarnessOutcome, HarnessRequest,
    HarnessStopReason, PermissionDecision,
};
use nika_kernel::ai::provider::{
    ContentBlock, InferRequest, InferResponse, Message, ProviderError, ProviderInferDyn,
    ResponseFormat, Role, StopReason, TokenUsage,
};
use nika_types::access::HarnessRuntime;
use serde_json::{Value, json};
use std::sync::Mutex;

/// One harness seat for the length of a compile: one ACP session per authoring call, or the
/// native tool-free transport where the ACP profile is not empty-tools (Codex).
pub struct HarnessSeat {
    seat: Backend,
    runtime: HarnessRuntime,
    requested_model: Option<String>,
    cwd: std::path::PathBuf,
    /// What each call observed: the model the harness named, whether it reported usage, the
    /// tool asks denied.
    observed: Mutex<Vec<Value>>,
}

enum Backend {
    Acp(Box<SpawnedHarness>),
    Native(Box<HarnessAuthoring>),
}

impl HarnessSeat {
    /// Whether `<adapter>/<model>` names a harness the engine knows (`claude-code/default`).
    #[must_use]
    pub fn names_a_harness(spec: &str) -> bool {
        spec.split_once('/')
            .is_some_and(|(id, _)| HarnessRuntime::lookup(id).is_some())
    }

    /// Seat the harness `<adapter>/<model>` names (`default` or an empty model leaves the
    /// harness's own choice).
    ///
    /// # Errors
    /// The spec is not `<adapter>/<model>`, names no known harness, or the harness cannot be
    /// seated (an unattested Codex, a missing ACP adapter row).
    pub fn meet(spec: &str) -> Result<Self, String> {
        let (id, model) = spec.split_once('/').ok_or_else(|| {
            format!("a harness seat is `<adapter>/<model>` (`claude-code/default`), not `{spec}`")
        })?;
        let runtime = HarnessRuntime::lookup(id).ok_or_else(|| {
            format!(
                "`{id}` is not a harness the engine knows; the tokens are {}",
                HarnessRuntime::ALL
                    .iter()
                    .map(|r| r.id)
                    .collect::<Vec<_>>()
                    .join(" · ")
            )
        })?;
        let seat = if runtime.id == "codex" {
            Backend::Native(Box::new(HarnessAuthoring::meet(runtime.id, Some(spec))?))
        } else {
            Backend::Acp(Box::new(
                seat_from_id(id)?.ok_or_else(|| format!("harness `{id}`: no adapter row"))?,
            ))
        };
        let cwd = std::env::current_dir()
            .map_err(|e| format!("harness seat: no working directory: {e}"))?;
        let requested_model = match model.trim() {
            "" | "default" => None,
            m => Some(m.to_owned()),
        };
        Ok(Self {
            seat,
            runtime,
            requested_model,
            cwd,
            observed: Mutex::new(Vec::new()),
        })
    }

    /// The receipt's backend descriptor: the addendum's fields, observed where observable.
    pub fn descriptor(&self) -> Value {
        if let Backend::Native(native) = &self.seat {
            return native.descriptor().unwrap_or_else(|why| {
                json!({"kind": "harness_infer", "adapter": self.runtime.id,
                    "observed_error": why})
            });
        }
        let observed = self.observed.lock().map(|o| o.clone()).unwrap_or_default();
        json!({
            "kind": "acp_harness",
            "adapter": self.runtime.id,
            "display": self.runtime.display,
            "acp_bin": self.runtime.acp_bin,
            "requested_model": self.requested_model,
            "observed": observed,
            "protocol": "ACP · one session per authoring call · the harness's own tools ride its permission bridge and every ask is denied here",
            "tools_exposed": "the harness's own (every ask denied for authoring)",
            "context_exposed": "the compiler's messages only (card · callables · references · request · diagnostics)",
            "effort": "the harness's default; not configurable through this seat",
            "cost_basis": "subscription-backed/unknown",
        })
    }

    /// The answer of a completed session, its call recorded first so that a refused turn keeps
    /// the asks it saw. A turn that ended any other way than `end_turn` is refused with its stop
    /// named and its text kept out of the answer; a backend that names no stop answers.
    fn finish(&self, outcome: HarnessOutcome, denied: u32) -> Result<InferResponse, ProviderError> {
        let reported = outcome.usage.is_some();
        if let Ok(mut observed) = self.observed.lock() {
            observed.push(json!({
                "observed_model": outcome.observed_model,
                "usage_reported": reported,
                "asks_denied": denied,
            }));
        }
        if let Some(stop) =
            (outcome.stop_reason.as_ref()).filter(|s| **s != HarnessStopReason::EndTurn)
        {
            return Err(self.refused(format_args!(
                "the authoring turn ended without completing (stop reason `{}`): {}; no answer \
                 accepted",
                stop.as_str(),
                unfinished(stop)
            )));
        }
        let text =
            json_object(&outcome.output).map_or_else(|| outcome.output.clone(), str::to_owned);
        let mut response = InferResponse::new(
            vec![ContentBlock::Text { text }],
            outcome.usage.unwrap_or_else(|| TokenUsage::new(0, 0)),
            StopReason::EndTurn,
        );
        // A subscription quota is not a token meter: only what the harness itself reported.
        response.usage_reported = reported;
        Ok(response)
    }

    /// A refusal of this seat's call, the harness named.
    fn refused(&self, why: impl std::fmt::Display) -> ProviderError {
        ProviderError::Other {
            reason: format!("harness `{}`: {why}", self.runtime.id),
        }
    }

    /// One session's events, read to its answer: each tool ask is refused (the harness's first
    /// one-time refusal, `cancelled` when it offers none) and counted, never allowed.
    async fn denying(
        &self,
        mut stream: HarnessEventStream,
    ) -> Result<InferResponse, ProviderError> {
        let mut denied: u32 = 0;
        loop {
            let next =
                std::future::poll_fn(|cx| futures_core::Stream::poll_next(stream.as_mut(), cx))
                    .await;
            match next {
                Some(Ok(
                    HarnessEvent::PermissionAsked {
                        question, reply, ..
                    }
                    | HarnessEvent::EscalationAsked {
                        question, reply, ..
                    },
                )) => {
                    denied += 1;
                    reply.respond(PermissionDecision::Deny);
                    if let Ok(mut observed) = self.observed.lock() {
                        observed.push(json!({"denied_ask": question}));
                    }
                }
                Some(Ok(HarnessEvent::Completed { outcome })) => {
                    return self.finish(*outcome, denied);
                }
                Some(Ok(_)) => {}
                Some(Err(e)) => return Err(self.refused(e)),
                None => return Err(self.refused("the session ended without an answer")),
            }
        }
    }
}

/// What a turn that did not end `end_turn` means for the answer it left, in the seat's words.
fn unfinished(stop: &HarnessStopReason) -> &'static str {
    match stop {
        HarnessStopReason::Refusal => "the model refused to continue",
        HarnessStopReason::MaxTokens => {
            "the model stopped at its output limit, which this seat does not set, so the answer \
             is cut"
        }
        HarnessStopReason::MaxTurnRequests => {
            "the harness reached its own limit of model requests for the turn"
        }
        HarnessStopReason::Cancelled => {
            "the turn was called off, by the harness on its own or after a tool ask this seat \
             denied"
        }
        HarnessStopReason::Unknown(_) => "the harness named a stop word this engine does not know",
        _ => "this seat reads no answer from that stop",
    }
}

/// The system instruction and the one prompt a session receives: every system message joined,
/// then the conversation in order, each earlier answer labeled; the answer schema stated in
/// words since ACP enforces none.
fn fold(messages: &[Message], schema: Option<&Value>) -> (Option<String>, String) {
    let text_of = |m: &Message| -> String {
        m.content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let mut system: Vec<String> = Vec::new();
    let mut prompt: Vec<String> = Vec::new();
    for message in messages {
        match message.role {
            Role::System => system.push(text_of(message)),
            Role::Assistant => {
                prompt.push(format!("[your previous answer]\n{}", text_of(message)));
            }
            _ => prompt.push(text_of(message)),
        }
    }
    if let Some(schema) = schema {
        prompt.push(format!(
            "Answer with ONLY one JSON object — no prose before or after, no code fence — that matches this JSON Schema:\n{schema}"
        ));
    }
    let system = (!system.is_empty()).then(|| system.join("\n\n"));
    (system, prompt.join("\n\n"))
}

/// The first complete JSON object in a harness answer (a harness may wrap it in prose or a
/// fence); None when the text carries no balanced object.
fn json_object(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let mut depth: i32 = 0;
    let mut in_string = false;
    let mut escaped = false;
    for (i, ch) in text[start..].char_indices() {
        if in_string {
            match ch {
                '\\' if !escaped => {
                    escaped = true;
                    continue;
                }
                '"' if !escaped => in_string = false,
                _ => {}
            }
            escaped = false;
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[start..start + i + ch.len_utf8()]);
                }
            }
            _ => {}
        }
    }
    None
}

impl ProviderInferDyn for HarnessSeat {
    async fn infer(&self, request: InferRequest) -> Result<InferResponse, ProviderError> {
        let seat = match &self.seat {
            Backend::Native(native) => return native.infer(request).await,
            Backend::Acp(seat) => seat,
        };
        let schema = match &request.response_format {
            ResponseFormat::JsonSchema(schema) => Some(schema),
            _ => None,
        };
        let (system, prompt) = fold(&request.messages, schema);
        let mut session = HarnessRequest::new(prompt, self.cwd.clone());
        if let Some(system) = system {
            session = session.with_system(system);
        }
        if let Some(model) = &self.requested_model {
            session = session.with_requested_model(model.clone());
        }
        // Authoring is tool-free by contract: a plan / read-only mode when the harness has one
        // (every ask is denied besides), so a silent read never runs either.
        session = session.with_requested_mode("read-only");
        let stream = seat.run_agent(session).await.map_err(|e| self.refused(e))?;
        self.denying(stream).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_conversation_folds_into_one_system_and_one_labeled_prompt_with_the_schema_in_words() {
        let messages = vec![
            Message::text(Role::System, "CARD"),
            Message::text(Role::User, "{\"request\":\"x\"}"),
            Message::text(Role::Assistant, "{\"candidate\":\"nika: a\"}"),
            Message::text(Role::User, "COMPILER DIAGNOSTICS"),
        ];
        let (system, prompt) = fold(&messages, Some(&json!({"type": "object"})));
        assert_eq!(system.as_deref(), Some("CARD"));
        assert!(prompt.starts_with(
            "{\"request\":\"x\"}\n\n[your previous answer]\n{\"candidate\":\"nika: a\"}\n\nCOMPILER DIAGNOSTICS\n\nAnswer with ONLY one JSON object"
        ));
        assert!(prompt.ends_with("{\"type\":\"object\"}"));
    }

    #[test]
    fn the_first_complete_json_object_is_the_answer_whatever_wraps_it() {
        assert_eq!(
            json_object("Sure!\n```json\n{\"candidate\": \"a}b\", \"n\": {\"k\": 1}}\n```"),
            Some("{\"candidate\": \"a}b\", \"n\": {\"k\": 1}}")
        );
        assert_eq!(json_object("no object here"), None);
        assert_eq!(json_object("{\"open\": true"), None);
        assert_eq!(
            json_object("{\"q\": \"\\\"}\"}"),
            Some("{\"q\": \"\\\"}\"}")
        );
    }

    #[test]
    fn a_harness_seat_is_named_by_a_token_the_engine_knows() {
        assert!(HarnessSeat::names_a_harness("claude-code/default"));
        assert!(HarnessSeat::names_a_harness("kimi-code/kimi-k2"));
        assert!(!HarnessSeat::names_a_harness("xai/grok-4"));
        assert!(!HarnessSeat::names_a_harness("claude-code"));
    }

    /// A tool ask on an ACP compile seat is refused with the one-time refusal the harness
    /// offers, never an allow, counted in the receipt, and the seat still answers.
    #[tokio::test]
    async fn an_acp_seat_refuses_an_ask_counts_it_and_still_answers() {
        let runtime = HarnessRuntime::lookup("claude-code").expect("a shipped harness");
        let rows = crate::registry::registry_with(&|_| None).expect("the shipped rows");
        let row = (rows.iter().find(|row| row.adapter.id == runtime.id)).expect("its row");
        let seat = HarnessSeat {
            seat: Backend::Acp(Box::new(SpawnedHarness::new(row.adapter.clone()))),
            runtime,
            requested_model: None,
            cwd: std::path::PathBuf::from("/tmp"),
            observed: Mutex::new(Vec::new()),
        };
        let options = json!([
            {"optionId": "allow", "name": "Allow", "kind": "allow_once"},
            {"optionId": "always", "name": "Always allow", "kind": "allow_always"},
            {"optionId": "reject", "name": "Reject", "kind": "reject_once"}
        ]);
        let answer = "{\"candidate\": \"nika: a\"}";
        let (ours, theirs) = tokio::io::duplex(64 * 1024);
        let (read, write) = tokio::io::split(ours);
        let agent = crate::client::refusal_tests::one_ask(theirs, options, answer);
        let stream = crate::client::drive(read, write, HarnessRequest::new("author", "/tmp"));
        let response = seat.denying(stream).await.expect("the seat answers");
        assert_eq!(
            agent.await.expect("the scripted agent ends"),
            json!({"outcome": "selected", "optionId": "reject"})
        );
        assert!(
            matches!(response.content.as_slice(), [ContentBlock::Text { text }] if text == answer),
            "{:?}",
            response.content
        );
        let receipt = seat.descriptor();
        assert_eq!(
            receipt["observed"],
            json!([
                {"denied_ask": crate::client::refusal_tests::ASKED},
                {"observed_model": null, "usage_reported": false, "asks_denied": 1}
            ]),
            "{receipt}"
        );
    }

    /// A claude-code seat over its shipped row; nothing is spawned, its sessions are driven here.
    fn claude_code_seat() -> HarnessSeat {
        let runtime = HarnessRuntime::lookup("claude-code").expect("a shipped harness");
        let rows = crate::registry::registry_with(&|_| None).expect("the shipped rows");
        let row = (rows.iter().find(|row| row.adapter.id == runtime.id)).expect("its row");
        HarnessSeat {
            seat: Backend::Acp(Box::new(SpawnedHarness::new(row.adapter.clone()))),
            runtime,
            requested_model: None,
            cwd: std::path::PathBuf::from("/tmp"),
            observed: Mutex::new(Vec::new()),
        }
    }

    /// A turn that did not end `end_turn` is no answer, however complete its text looks: the
    /// seat refuses it with the harness, the stop and what it means, never the text, and the
    /// receipt still counts the ask denied during that turn.
    #[tokio::test]
    async fn an_acp_seat_refuses_a_turn_that_did_not_end_and_still_counts_its_ask() {
        use nika_error::traits::NikaErrorCode as _;
        let options = json!([
            {"optionId": "allow", "name": "Allow", "kind": "allow_once"},
            {"optionId": "reject", "name": "Reject", "kind": "reject_once"}
        ]);
        let partial = "{\"candidate\": \"nika: partial\"}";
        for (stop, meaning) in [
            ("refusal", "the model refused to continue"),
            (
                "max_tokens",
                "the model stopped at its output limit, which this seat does not set, so the \
                 answer is cut",
            ),
            (
                "max_turn_requests",
                "the harness reached its own limit of model requests for the turn",
            ),
            (
                "cancelled",
                "the turn was called off, by the harness on its own or after a tool ask this \
                 seat denied",
            ),
            (
                "paused",
                "the harness named a stop word this engine does not know",
            ),
        ] {
            let seat = claude_code_seat();
            let (ours, theirs) = tokio::io::duplex(64 * 1024);
            let (read, write) = tokio::io::split(ours);
            let agent = crate::client::refusal_tests::one_ask_ending(
                theirs,
                options.clone(),
                partial,
                stop,
            );
            let stream = crate::client::drive(read, write, HarnessRequest::new("author", "/tmp"));
            let error = match seat.denying(stream).await {
                Err(error) => error,
                Ok(response) => panic!("`{stop}` answered: {:?}", response.content),
            };
            assert_eq!(
                agent.await.expect("the scripted agent ends"),
                json!({"outcome": "selected", "optionId": "reject"}),
                "{stop}"
            );
            let reason = format!(
                "harness `claude-code`: the authoring turn ended without completing (stop reason \
                 `{stop}`): {meaning}; no answer accepted"
            );
            assert!(
                matches!(&error, ProviderError::Other { reason: said } if *said == reason),
                "{stop}: {error:?}"
            );
            assert_eq!(error.to_string(), format!("provider error: {reason}"));
            assert_eq!(error.nika_code().to_string(), "NIKA-339", "{stop}");
            assert!(!error.is_transient(), "{stop}");
            assert_eq!(
                seat.descriptor()["observed"],
                json!([
                    {"denied_ask": crate::client::refusal_tests::ASKED},
                    {"observed_model": null, "usage_reported": false, "asks_denied": 1}
                ]),
                "{stop}"
            );
        }
    }

    /// A harness that offers no way to refuse hears `cancelled` for its ask. When it then ends
    /// its turn `cancelled`, the seat refuses the turn and keeps the ask in the receipt, however
    /// whole the text it left looks.
    #[tokio::test]
    async fn an_acp_seat_refuses_the_turn_a_harness_cancels_after_an_ask_it_could_not_reject() {
        let seat = claude_code_seat();
        let options = json!([{"optionId": "allow", "name": "Allow", "kind": "allow_once"}]);
        let (ours, theirs) = tokio::io::duplex(64 * 1024);
        let (read, write) = tokio::io::split(ours);
        let agent = crate::client::refusal_tests::one_ask_ending(
            theirs,
            options,
            "{\"candidate\": \"nika: whole\"}",
            "cancelled",
        );
        let stream = crate::client::drive(read, write, HarnessRequest::new("author", "/tmp"));
        let error = match seat.denying(stream).await {
            Err(error) => error,
            Ok(response) => panic!("answered: {:?}", response.content),
        };
        assert_eq!(
            agent.await.expect("the scripted agent ends"),
            json!({"outcome": "cancelled"})
        );
        assert_eq!(
            error.to_string(),
            "provider error: harness `claude-code`: the authoring turn ended without completing \
             (stop reason `cancelled`): the turn was called off, by the harness on its own or \
             after a tool ask this seat denied; no answer accepted"
        );
        assert_eq!(
            seat.descriptor()["observed"],
            json!([
                {"denied_ask": crate::client::refusal_tests::ASKED},
                {"observed_model": null, "usage_reported": false, "asks_denied": 1}
            ])
        );
    }

    /// A turn that ended `end_turn` and one whose backend named no stop both answer with the
    /// object the text holds, and each call is in the receipt.
    #[test]
    fn a_turn_that_ended_or_named_no_stop_answers() {
        let seat = claude_code_seat();
        let answer = "{\"candidate\": \"nika: a\"}";
        let said = format!("Here it is:\n{answer}");
        for outcome in [
            HarnessOutcome::new(said.clone()),
            HarnessOutcome::new(said.clone()).with_stop_reason(HarnessStopReason::EndTurn),
        ] {
            let named = outcome.stop_reason.clone();
            let response = match seat.finish(outcome, 0) {
                Ok(response) => response,
                Err(error) => panic!("{named:?} refused: {error}"),
            };
            assert_eq!(response.stop_reason, StopReason::EndTurn, "{named:?}");
            let [ContentBlock::Text { text }] = response.content.as_slice() else {
                panic!("{named:?}: {:?}", response.content);
            };
            assert_eq!(text, answer, "{named:?}");
            assert!(!response.usage_reported, "{named:?}");
        }
        let entry = json!({"observed_model": null, "usage_reported": false, "asks_denied": 0});
        assert_eq!(seat.descriptor()["observed"], json!([entry, entry]));
    }

    /// A stop word that is not a protocol word rides the refusal as `unrecognized`, and the
    /// turn's text never rides it; the call keeps its model and its reported usage.
    #[test]
    fn a_refused_turn_names_only_a_protocol_stop_word_and_never_its_text() {
        let seat = claude_code_seat();
        let partial = "{\"candidate\": \"nika: partial\"}";
        let outcome = HarnessOutcome::new(partial)
            .with_stop_reason(HarnessStopReason::parse("Paused: see the log"))
            .with_usage(TokenUsage::new(120, 40))
            .with_observed_model("author-model");
        let error = match seat.finish(outcome, 2) {
            Err(error) => error,
            Ok(response) => panic!("answered: {:?}", response.content),
        };
        assert_eq!(
            error.to_string(),
            "provider error: harness `claude-code`: the authoring turn ended without completing \
             (stop reason `unrecognized`): the harness named a stop word this engine does not \
             know; no answer accepted"
        );
        assert_eq!(
            seat.descriptor()["observed"],
            json!([{"observed_model": "author-model", "usage_reported": true, "asks_denied": 2}])
        );
    }

    #[test]
    fn codex_seats_the_native_tool_free_transport_never_the_acp_agent() {
        let seat = HarnessSeat::meet("codex/gpt-6-astra").expect("native codex row");
        assert!(matches!(seat.seat, Backend::Native(_)));
        let receipt = seat.descriptor();
        assert_eq!(receipt["kind"], "harness_infer", "{receipt}");
        assert_eq!(
            receipt["forwarded_model"], "openai/gpt-6-astra",
            "{receipt}"
        );
        assert!(
            receipt["tools_exposed"]
                .as_str()
                .is_some_and(|t| t.contains("pre-execution empty-tools profile")),
            "{receipt}"
        );
        assert!(HarnessSeat::meet("codex/anthropic/x").is_err());
    }
}
