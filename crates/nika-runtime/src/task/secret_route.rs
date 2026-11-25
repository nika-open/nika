// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! The run never renders a secret into a task's `model:`, an `exec:`'s `cwd:`
//! or an `agent:`'s `images.save:` (spec 10 · `NIKA-SEC-006`, the run-time
//! twin of the check).
//!
//! Each field decides where the effect goes: the provider and model the run
//! calls, the directory it spawns the program in, the file it writes an image
//! to. Rendered with secrets resolved, one would carry the value into every
//! place that names a route (the call's frame, the ledger, the settlement, the
//! access lane), into the spawn and the sandbox profile, or into a file name
//! and the output that names it. The check refuses such a file, and the run
//! refuses a dirty report, but a report can be older than the file: so the run
//! asks the check's own secret flow ([`nika_check::secret_route_leaks`]) which
//! tasks those fields would carry a secret into, every way a secret travels,
//! and refuses each such task before it starts. Nothing is rendered, sent,
//! written or debited for it, and the journal holds only the refusal: the
//! record and the event name the task, the field and the path from the secret,
//! never a value.

use std::collections::BTreeMap;

use nika_check::{LeakReason, SecretLeak};
use nika_dataflow::TaskErrorRecord;
use nika_schema::raw::{RawTask, RawWorkflow};

use super::{Finish, SettleAs, null_bindings};

/// The public code of the refusal: the secret-flow class the check reports
/// for the same field (`NIKA-SEC-006`), so a run and its check name one fault.
pub(crate) const SECRET_ROUTE_CODE: &str = "NIKA-SEC-006";

/// Task id → the refusal of the task whose route a secret reaches, built once
/// per run from the workflow.
#[derive(Default)]
pub(crate) struct SecretRoutes(BTreeMap<String, TaskErrorRecord>);

impl SecretRoutes {
    /// The refusals of `wf`: none when it declares no secret.
    pub(crate) fn of(wf: &RawWorkflow) -> Self {
        if wf.secrets.is_empty() {
            return Self::default();
        }
        let mut refused = BTreeMap::new();
        for leak in nika_check::secret_route_leaks(wf) {
            // One refusal per task: the first secret the flow names.
            refused
                .entry(leak.task.clone())
                .or_insert_with(|| refusal(&leak));
        }
        Self(refused)
    }

    /// The refusal of `task`, when a secret reaches its route.
    pub(crate) fn refusal(&self, task: &str) -> Option<&TaskErrorRecord> {
        self.0.get(task)
    }
}

/// The record of one refused route: the task, the field and the path from
/// the secret, never a value.
fn refusal(leak: &SecretLeak) -> TaskErrorRecord {
    // The field is the sink without its verb: `model`, `cwd`, `images.save`,
    // `args.model`.
    let field = leak
        .sink
        .split_once('.')
        .map_or(leak.sink, |(_, field)| field);
    let remedy = match leak.reason {
        LeakReason::WorkingDirectory => {
            "name the directory there, or read it from `inputs:` or `const:`"
        }
        LeakReason::ModelRoute => "name the model there, or read it from `inputs:` or `const:`",
        LeakReason::ImageSavePath => "name the path there, or read it from `inputs:` or `const:`",
        // A route the run does not know yet is refused under its field.
        _ => "keep the secret out of it",
    };
    TaskErrorRecord::new(
        SECRET_ROUTE_CODE,
        format!(
            "refused before the task ran: a secret reaches the `{field}:` of task `{}` ({}), and \
             the run renders no secret there · {remedy}",
            leak.task, leak.trace
        ),
        false,
    )
}

/// The Finish of a task refused for its route, before anything renders: it
/// settles failure outside `on_error` scope, like a boundary refusal.
pub(super) fn refused_finish(id: &str, task: &RawTask, routes: &SecretRoutes) -> Option<Finish> {
    let error = routes.refusal(id)?.clone();
    Some(Finish {
        id: id.to_owned(),
        settle: SettleAs::FailedBeforeStart {
            stage: "secret",
            error,
        },
        named: null_bindings(task),
        resume: None,
        // Never started: no content flowed.
        integrity: nika_cap::Integrity::trusted(),
        declassified: Vec::new(),
        approval: None,
    })
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use nika_schema::{FileId, ParseMode, parse};

    use super::*;

    fn routes(yaml: &str) -> SecretRoutes {
        SecretRoutes::of(&parse(yaml, FileId::new(0), ParseMode::Strict).expect("parses"))
    }

    #[test]
    fn each_refused_route_names_its_task_field_and_path() {
        let refused = routes(
            r#"nika: two-routes
secrets:
  k: { source: env, key: K, egress: [{ to: exec }] }
permits:
  exec: ["true"]
tasks:
  run:
    exec: { command: ["true"], cwd: "${{ secrets.k }}" }
  ask:
    infer: { model: "openai/${{ secrets.k }}", prompt: hi, max_tokens: 10 }
  fine:
    infer: { model: "openai/gpt-4.1", prompt: hi, max_tokens: 10 }
"#,
        );
        let refusal = |task: &str, field: &str, remedy: &str| {
            TaskErrorRecord::new(
                "NIKA-SEC-006",
                format!(
                    "refused before the task ran: a secret reaches the `{field}:` of task \
                     `{task}` (secrets.k → {field} @ {task}), and the run renders no secret \
                     there · name the {remedy} there, or read it from `inputs:` or `const:`"
                ),
                false,
            )
        };
        assert_eq!(
            refused.0,
            BTreeMap::from([
                ("ask".to_owned(), refusal("ask", "model", "model")),
                ("run".to_owned(), refusal("run", "cwd", "directory")),
            ])
        );
    }

    #[test]
    fn a_secret_in_an_image_save_path_is_refused_by_its_field() {
        let refused = routes(
            r#"nika: image-save
secrets:
  k: { source: env, key: K, egress: [{ to: agent }] }
permits:
  fs: { write: ["./out/**"] }
tasks:
  draw:
    agent: { prompt: hi, images: { save: "./out/${{ secrets.k }}.png" } }
  plain:
    agent: { prompt: hi, images: { save: "./out/plain.png" } }
"#,
        );
        assert_eq!(
            refused.0,
            BTreeMap::from([(
                "draw".to_owned(),
                TaskErrorRecord::new(
                    "NIKA-SEC-006",
                    "refused before the task ran: a secret reaches the `images.save:` of task \
                     `draw` (secrets.k → images.save @ draw), and the run renders no secret \
                     there · name the path there, or read it from `inputs:` or `const:`",
                    false,
                ),
            )])
        );
    }

    #[test]
    fn a_secret_in_a_media_model_is_refused_by_its_argument() {
        let refused = routes(
            r#"nika: tool-model
secrets:
  k: { source: env, key: K, egress: [{ to: "nika:tts_generate" }] }
permits:
  tools: ["nika:tts_generate"]
  fs: { write: ["./out/**"] }
tasks:
  say:
    invoke: { tool: "nika:tts_generate", args: { model: "${{ secrets.k }}", text: hi, output_dir: "./out" } }
  plain:
    invoke: { tool: "nika:tts_generate", args: { model: mock-tts-1, text: "${{ secrets.k }}", output_dir: "./out" } }
"#,
        );
        assert_eq!(
            refused.0,
            BTreeMap::from([(
                "say".to_owned(),
                TaskErrorRecord::new(
                    "NIKA-SEC-006",
                    "refused before the task ran: a secret reaches the `args.model:` of task \
                     `say` (secrets.k → args.model @ say), and the run renders no secret there \
                     · name the model there, or read it from `inputs:` or `const:`",
                    false,
                ),
            )])
        );
    }

    #[test]
    fn a_workflow_without_secrets_refuses_nothing() {
        let refused = routes(
            r#"nika: no-secret
inputs:
  m: { type: string, required: false, default: "openai/gpt-4.1" }
tasks:
  ask:
    infer: { model: "${{ inputs.m }}", prompt: hi, max_tokens: 10 }
"#,
        );
        assert_eq!(refused.0, BTreeMap::new());
    }
}
