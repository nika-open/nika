// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>
#![allow(clippy::expect_used)]

//! A media builtin's `args.model` (`nika:tts_generate` · `nika:image_generate`) is the model the
//! tool calls, and without a `provider:` the provider it infers: a route, judged by the secret
//! flow as a task's `model:` is.
//!
//! The tool names its model in the file it saves and in its output, so a secret there would
//! become a file name and a value of the run. A model name is never a secret, so a secret
//! reaching `args.model` is `NIKA-SEC-006` whatever its `egress:`, a rule for the tool itself
//! included, with the path from the secret to the field; the run refuses that task before it
//! renders the call (`nika-runtime`'s `secret_route_refusal` suite). Every other argument stays
//! a send its rule decides, and a model the author names or reads from `inputs:` stays clean.

use nika_check::{CheckReport, LeakReason, check, secret_route_leaks};
use nika_schema::{FileId, ParseMode, parse};

fn parsed(yaml: &str) -> nika_schema::raw::RawWorkflow {
    parse(yaml, FileId::new(0), ParseMode::Strict).expect("the fixture parses")
}

fn run(yaml: &str) -> CheckReport {
    check(&parsed(yaml))
}

/// A leak as `(task, sink, path)`.
fn leaks(report: &CheckReport) -> Vec<(String, &'static str, String)> {
    report
        .secret_leaks
        .iter()
        .map(|l| (l.task.clone(), l.sink, l.trace.clone()))
        .collect()
}

/// Every SECRETS-lane leak finding as `(code, task, message)`.
fn secret_rows(report: &CheckReport) -> Vec<(Option<String>, Option<String>, String)> {
    report
        .findings
        .iter()
        .filter(|f| f.kind == "secret_leak")
        .map(|f| (f.code.clone(), f.task.clone(), f.message.clone()))
        .collect()
}

/// `say` calls `TOOL` with `ARGS` (the tool's text argument included), under a rule that lets
/// `token` reach the tool.
fn media(tool: &str, with: &str, args: &str) -> String {
    let text = if tool == "nika:tts_generate" {
        "text"
    } else {
        "prompt"
    };
    format!(
        r#"nika: secret-tool-model
secrets:
  token: {{ source: env, key: T, egress: [{{ to: "{tool}" }}] }}
inputs:
  model: {{ type: string, required: false, default: mock-1 }}
permits:
  tools: ["{tool}"]
  fs: {{ write: ["./out/**"] }}
tasks:
  say:
{with}    invoke: {{ tool: "{tool}", args: {{ provider: mock, {text}: hi, output_dir: "./out", {args} }} }}
"#
    )
}

const TOOLS: [&str; 2] = ["nika:tts_generate", "nika:image_generate"];

#[test]
fn a_secret_in_a_media_model_is_refused_whatever_its_rule() {
    for tool in TOOLS {
        for (with, model, path) in [
            (
                "",
                "${{ secrets.token }}",
                "secrets.token → args.model @ say",
            ),
            (
                "    with: { m: \"${{ secrets.token }}\" }\n",
                "${{ with.m }}",
                "secrets.token → with.m @ say → args.model @ say",
            ),
        ] {
            let yaml = media(tool, with, &format!("model: \"{model}\""));
            let report = run(&yaml);
            assert_eq!(
                leaks(&report),
                [("say".to_owned(), "invoke.args.model", path.to_owned())],
                "{tool} {with}"
            );
            assert_eq!(report.secret_leaks[0].reason, LeakReason::ModelRoute);
            assert_eq!(
                secret_rows(&report),
                [(
                    Some("NIKA-SEC-006".to_owned()),
                    Some("say".to_owned()),
                    format!(
                        "leak into invoke.args.model (task `say`) — {path} · fix: a model is a \
                         route, not a secret, and no `egress:` rule clears `secrets.token` in \
                         `args.model:` — name the model there, or read it from `inputs:` or \
                         `const:` (`model: \"${{{{ inputs.model }}}}\"`)"
                    )
                )],
                "{tool} {with}"
            );
            // The list the run reads before it starts a task is the check's own.
            assert_eq!(secret_route_leaks(&parsed(&yaml)), report.secret_leaks);
        }
    }
}

/// The other arguments stay sends the tool's rule decides: a secret in the text goes to the
/// tool it may reach, and a model the author names or reads from `inputs:` is no route a secret
/// rides.
#[test]
fn every_other_argument_and_a_named_model_stay_as_they_were() {
    for tool in TOOLS {
        for args in [
            "model: mock-1, metadata: { note: \"${{ secrets.token }}\" }",
            "model: \"${{ inputs.model }}\"",
        ] {
            let report = run(&media(tool, "", args));
            assert_eq!(leaks(&report), Vec::new(), "{tool} {args}");
            assert!(report.is_clean(), "{tool} {args}: {report:?}");
        }
        // Without the rule, the text send is the leak it always was, and no route.
        let unsanctioned = media(tool, "", "metadata: { note: \"${{ secrets.token }}\" }")
            .replace(&format!("egress: [{{ to: \"{tool}\" }}]"), "egress: []");
        let report = run(&unsanctioned);
        assert_eq!(
            report
                .secret_leaks
                .iter()
                .map(|l| (l.task.as_str(), l.reason.clone()))
                .collect::<Vec<_>>(),
            [("say", LeakReason::NoEgress)],
            "{tool}"
        );
    }
}

/// A builtin with no model argument keeps every argument a send: `nika:write`'s content goes to
/// the file its rule allows.
#[test]
fn a_builtin_without_a_model_argument_has_no_model_route() {
    let report = run(r#"nika: write-secret
secrets:
  token: { source: env, key: T, egress: [{ to: "nika:write" }] }
permits:
  tools: ["nika:write"]
  fs: { write: ["./out/**"] }
tasks:
  keep:
    invoke: { tool: "nika:write", args: { path: "./out/k.txt", content: "${{ secrets.token }}" } }
"#);
    assert_eq!(leaks(&report), Vec::new());
}

/// The journey names `say` in the routes of `token`: the provider receives the model. A route is
/// no send, so the tool is no destination of the secret.
#[test]
fn the_journey_names_a_media_model_as_a_model_route() {
    for tool in TOOLS {
        let report = run(&media(tool, "", "model: \"${{ secrets.token }}\""));
        let used: Vec<_> = report
            .data_journey
            .secrets_used
            .iter()
            .map(|s| {
                (
                    s.name.as_str(),
                    s.tasks.clone(),
                    s.flows_to.clone(),
                    s.routes.clone(),
                )
            })
            .collect();
        assert_eq!(
            used,
            [(
                "token",
                vec!["say".to_owned()],
                Vec::<String>::new(),
                vec!["say".to_owned()]
            )],
            "{tool}"
        );
    }
}
