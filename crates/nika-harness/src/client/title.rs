// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! The session's title on the Codex seat, given by Nika before anything else is asked of the
//! session, when the adapter lets a client give one (`session/set_title`, unstable in ACP v1,
//! advertised as `agentCapabilities.sessionCapabilities.title`).
//!
//! codex-acp names a new session itself after its first completed turn. 1.13.1 sends the text
//! of the session's first prompt to a second model of the same account for that, and offers a
//! client no way to set a title. A release that advertises the capability (2.12.0, as read in
//! its source and its `docs/session-title.md`) stops its own title writes once a client has
//! set one, by that documentation; the same documentation says Codex may refuse to rename a
//! session that has no first message yet. Neither was observed on a live seat. So an agent,
//! `infer:`, authoring or conversation session whose adapter is codex-acp (its audited profile,
//! or the name it reports on `initialize`) and advertises the capability receives
//! `session/set_title` right after `session/new`, before any selection and any prompt; any
//! other adapter, and an offer listing, receives nothing.
//!
//! The title is Nika's own word for the kind of session (`label`): never the prompt, the
//! system text or a value. The call's answer is waited for as long as an `initialize` answer
//! (`start_timeout`: 30 s unless the probe deadline is set), and never more than half the
//! session's frame bound, so that bound cannot end the wait first. What the adapter answered is
//! what is kept (`TitleCall`): accepted, refused (an error answer) or unanswered in that time;
//! the session goes on in each case, and an answer that arrives after the wait, a result or an
//! error, is passed over. Every other frame that arrives before the answer is judged as before
//! any prompt, so a refusal there still ends the session, and so does a transport that fails.

use nika_kernel::ai::harness::HarnessError;
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncWrite};

use super::Driver;
use crate::authoring::acp::{Completion, OneShot, Profile, codex};
use crate::wire;

/// The request id of the title call: clear of the handshake's and the selections'. The turn
/// passes over a late error answer to it.
pub(super) const ID_SESSION_TITLE: u64 = 10;

/// Whether an `initialize` answer advertises the session title capability: an object, as ACP
/// states a capability.
pub(super) fn advertised(initialize: &Value) -> bool {
    (initialize.pointer("/agentCapabilities/sessionCapabilities/title"))
        .is_some_and(Value::is_object)
}

/// The title a session receives: the kind of session, in Nika's words.
pub(super) const fn label(completion: Option<OneShot>) -> &'static str {
    match completion {
        None => "Nika agent",
        Some(one_shot) => match one_shot.role {
            Completion::Authoring => "Nika authoring",
            Completion::Infer => "Nika infer",
            Completion::Conversation => "Nika conversation",
        },
    }
}

/// What the adapter answered to the session's title call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TitleCall {
    /// The adapter answered the call with a result.
    Accepted,
    /// The adapter answered the call with an error.
    Refused,
    /// The adapter did not answer the call in its time.
    Unanswered,
}

impl TitleCall {
    /// The record's word.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Refused => "refused",
            Self::Unanswered => "unanswered",
        }
    }
}

impl<R, W> Driver<R, W>
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
{
    /// Whether the session's adapter is codex-acp: a one-shot or a conversation on the Codex
    /// profile, which opens only on the admitted codex-acp, or a session whose adapter reported
    /// one of codex-acp's names on `initialize` (an agent task; a conversation records none).
    fn on_codex_acp(&self) -> bool {
        (self.completion).is_some_and(|one_shot| one_shot.profile == Profile::Codex)
            || (self.agent_name.as_deref()).is_some_and(codex::reports_as_codex_acp)
    }

    /// Give the session `session_id` its title when its adapter is codex-acp and advertised the
    /// capability. `None` otherwise: nothing is sent.
    ///
    /// # Errors
    /// The transport failed, or a frame before the answer was refused as before any prompt.
    pub(super) async fn give_title(
        &mut self,
        session_id: &str,
    ) -> Result<Option<TitleCall>, HarnessError> {
        if !(self.title_offered && self.on_codex_acp()) {
            return Ok(None);
        }
        let params = wire::SetSessionTitleParams {
            session_id: session_id.to_owned(),
            title: label(self.completion).to_owned(),
        };
        (self.send_request(ID_SESSION_TITLE, wire::METHOD_SET_SESSION_TITLE, &params)).await?;
        let bound = super::start::start_timeout().min(self.idle / 2);
        let answer = tokio::time::timeout(bound, self.await_answer(ID_SESSION_TITLE)).await;
        Ok(Some(match answer {
            Err(_) => TitleCall::Unanswered,
            Ok(answered) => match answered? {
                Ok(_) => TitleCall::Accepted,
                Err(_) => TitleCall::Refused,
            },
        }))
    }
}

#[cfg(test)]
mod tests;
