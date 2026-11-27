// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! The model of a session whose task names none, on an adapter that reports it apart from the
//! program it drives. claude-agent-acp reads the user's settings files itself to state a
//! session's current model, and does not send that model to its CLI when it believes the CLI
//! read the same files. A session opened without those files breaks that belief: the adapter
//! reports the settings' model while the CLI runs its own default. So the client selects the
//! model the session reports through the session's own model option and reads it back before
//! the prompt: the model the outcome records is then the one the session runs.
//!
//! This is a hold, not a pin. The workflow asked for no model, so a model the agent moves
//! during the turn stays a recorded fact (`HarnessSelection::held_model`), never a refusal.

use nika_kernel::ai::harness::{HarnessError, ModelProvenance};
use nika_types::access::HarnessBoundary;
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncWrite};

use super::{Driver, ID_SESSION_SEAT, seats};
use crate::wire;

/// Whether the adapter that named itself `agent_name` states a session's model from its own
/// reading of the user's settings, apart from the program it drives (measured on
/// claude-agent-acp 0.81.1). Another adapter, or one that named nothing, is left alone.
fn reports_its_model_apart(agent_name: Option<&str>) -> bool {
    agent_name
        .and_then(HarnessBoundary::for_agent)
        .is_some_and(|row| row.seat == HarnessBoundary::CLAUDE_AGENT_ACP.seat)
}

/// The id of a model option and the value it reports current, when it states both.
fn reported(model_option: Option<&Value>) -> Option<(String, String)> {
    let option = model_option?;
    let id = option.get("id")?.as_str()?;
    let current = option.get("currentValue")?.as_str()?;
    Some((id.to_owned(), current.to_owned()))
}

impl<R, W> Driver<R, W>
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
{
    /// For a task that names no model: select the model the session reports as current and
    /// read it back, on an adapter that reports it apart from its program. `Ok(Some(_))` is
    /// the configuration the session answered; `Ok(None)` means nothing was sent (another
    /// adapter, or a session whose model option states no current value). A session that
    /// does not keep the model it reported itself is refused before the prompt.
    pub(super) async fn hold_reported_model(
        &mut self,
        sid: &str,
        model_option: Option<&Value>,
    ) -> Result<Option<Value>, HarnessError> {
        if !reports_its_model_apart(self.agent_name.as_deref()) {
            return Ok(None);
        }
        let Some((config_id, model)) = reported(model_option) else {
            return Ok(None);
        };
        self.selection.model_option = Some(config_id.clone());
        self.selection.transmitted_model = Some(model.clone());
        self.selection.held_model = true;
        self.send_request(
            ID_SESSION_SEAT,
            wire::METHOD_SET_CONFIG_OPTION,
            &wire::SetConfigOptionParams {
                session_id: sid.to_owned(),
                config_id,
                value: Value::String(model.clone()),
            },
        )
        .await?;
        let answered: Value = self
            .await_response(ID_SESSION_SEAT, "session/set_config_option")
            .await?;
        // The answer carries the complete configuration: only its own current value is read,
        // never the value sent or the one `session/new` reported.
        let configuration = answered.get("configOptions");
        let kept = seats::current_model(seats::model_option(configuration), None);
        self.observe(kept, ModelProvenance::ConfirmedSelection);
        if self.observed_model.as_deref() != Some(model.as_str()) {
            return Err(HarnessError::Selection {
                reason: format!(
                    "the harness did not keep the model it reported (`{model}`) when the client \
                     selected it (it reports {}; name a model on the task to choose it, no \
                     prompt was sent)",
                    self.observed_model
                        .as_deref()
                        .map_or_else(|| "none".to_owned(), |m| format!("`{m}`"))
                ),
            });
        }
        Ok(configuration.cloned())
    }
}
