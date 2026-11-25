// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>
#![allow(clippy::expect_used)]

//! Two fields the run renders with secrets resolved, judged by the secret flow.
//!
//! An `exec:`'s `cwd:` is handed to the spawn: a secret there reaches the exec effect exactly as
//! one in argv or env does. The same `{ to: "exec" }` rule clears it, and the output the program
//! prints then carries it.
//!
//! An `infer:`/`agent:`'s `model:` is the route the run sends as the provider and model it calls.
//! A model name is never a secret, so a secret reaching `model:` is `NIKA-SEC-006` whatever its
//! `egress:`, with the path from the secret to the field. The envelope `model:` is not rendered
//! at run and stays as it was.
//!
//! Every way a secret travels counts: a direct reference, a `with:` alias, a task output that
//! carries it, a `group.*` fold.

use nika_check::{CheckReport, LeakReason, check};
use nika_schema::{FileId, ParseMode, parse};

/// The rule that lets `token` go to crm.example.com through `nika:fetch`, and no further.
const CRM: &str = r#"{ to: "nika:fetch", host: "crm.example.com" }"#;

fn run(yaml: &str) -> CheckReport {
    check(&parse(yaml, FileId::new(0), ParseMode::Strict).expect("the fixture parses"))
}

/// A leak as `(task, secret, sink, path)`.
type Leak = (String, String, &'static str, String);

fn leaks(report: &CheckReport) -> Vec<Leak> {
    report
        .secret_leaks
        .iter()
        .map(|l| (l.task.clone(), l.secret.clone(), l.sink, l.trace.clone()))
        .collect()
}

fn leak(task: &str, sink: &'static str, path: &str) -> Leak {
    (task.to_owned(), "token".to_owned(), sink, path.to_owned())
}

/// A SECRETS-lane leak finding as `(code, task, message)`.
type Row = (Option<String>, Option<String>, String);

/// Every leak finding.
fn secret_rows(report: &CheckReport) -> Vec<Row> {
    report
        .findings
        .iter()
        .filter(|f| f.kind == "secret_leak")
        .map(|f| (f.code.clone(), f.task.clone(), f.message.clone()))
        .collect()
}

fn sec_006(task: &str, message: &str) -> Row {
    (
        Some("NIKA-SEC-006".to_owned()),
        Some(task.to_owned()),
        message.to_owned(),
    )
}

// ── exec.cwd ────────────────────────────────────────────────────────────────

/// `pick` sends the token to crm.example.com, where it may go, and joins `picks`; `run` binds
/// `WITH` and runs `true` in `CWD`.
fn cwd(with: &str, dir: &str, egress: &str) -> String {
    format!(
        r#"nika: secret-cwd
secrets:
  token: {{ source: env, key: T, egress: [{egress}] }}
permits:
  tools: ["nika:fetch"]
  net: {{ http: ["crm.example.com"] }}
  exec: ["true"]
tasks:
  pick:
    group: picks
    invoke: {{ tool: "nika:fetch", args: {{ url: "https://crm.example.com/a", headers: {{ authorization: "Bearer ${{{{ secrets.token }}}}" }} }} }}
  run:
{with}    exec: {{ command: ["true"], cwd: "{dir}" }}
"#
    )
}

/// The fix of a cwd leak when the token's only rule names another sink.
const ADD_EXEC: &str = "fix: add `\"exec\"` to the `to:` list of the existing `egress:` on \
                        `secrets.token`";

#[test]
fn a_secret_in_cwd_reaches_the_exec_effect_on_every_way_in() {
    for (with, dir, path) in [
        ("", "${{ secrets.token }}", "secrets.token"),
        (
            "    with: { dir: \"${{ secrets.token }}\" }\n",
            "${{ with.dir }}",
            "secrets.token → with.dir @ run",
        ),
        (
            "    with: { dir: \"${{ tasks.pick.output }}\" }\n",
            "${{ with.dir }}",
            "secrets.token → tasks.pick.output → with.dir @ run",
        ),
        (
            "    with: { dir: \"${{ group.picks }}\" }\n",
            "${{ with.dir }}",
            "secrets.token → tasks.pick.output → with.dir @ run",
        ),
    ] {
        let report = run(&cwd(with, dir, CRM));
        assert_eq!(leaks(&report), [leak("run", "exec", path)], "{dir} {with}");
        assert_eq!(
            report.secret_leaks[0].reason,
            LeakReason::SinkNotCleared {
                sink: "exec".to_owned()
            }
        );
        assert_eq!(
            secret_rows(&report),
            [sec_006(
                "run",
                &format!("leak into exec (task `run`) — {path} · {ADD_EXEC}")
            )]
        );
        assert!(!report.is_clean(), "{dir}");
    }
}

#[test]
fn the_exec_rule_clears_a_secret_in_cwd_as_it_clears_one_in_argv() {
    for (with, dir) in [
        ("", "${{ secrets.token }}"),
        (
            "    with: { dir: \"${{ tasks.pick.output }}\" }\n",
            "${{ with.dir }}",
        ),
    ] {
        let report = run(&cwd(with, dir, &format!("{{ to: \"exec\" }}, {CRM}")));
        assert_eq!(leaks(&report), Vec::<Leak>::new(), "{dir}");
        assert_eq!(secret_rows(&report), Vec::<Row>::new(), "{dir}");
    }
}

#[test]
fn a_cwd_without_a_secret_stays_clean() {
    for (with, dir) in [
        ("", "./work"),
        ("    with: { dir: \"./work\" }\n", "${{ with.dir }}"),
    ] {
        let report = run(&cwd(with, dir, CRM));
        assert_eq!(leaks(&report), Vec::<Leak>::new(), "{dir}");
        assert_eq!(secret_rows(&report), Vec::<Row>::new(), "{dir}");
    }
}

/// The program can print its working directory, so its output carries the token: `send`, which
/// posts that output to other.example.com, sends the token where it may not go.
#[test]
fn the_output_of_a_command_run_in_a_secret_cwd_carries_the_secret() {
    let yaml = r#"nika: secret-cwd-output
secrets:
  token: { source: env, key: T, egress: [{ to: "exec" }] }
permits:
  tools: ["nika:fetch"]
  net: { http: ["other.example.com"] }
  exec: ["pwd"]
tasks:
  run:
    exec: { command: ["pwd"], cwd: "${{ secrets.token }}" }
  send:
    with: { out: "${{ tasks.run.output }}" }
    invoke: { tool: "nika:fetch", args: { url: "https://other.example.com/in", method: POST, body: "${{ with.out }}" } }
"#;
    let report = run(yaml);
    assert_eq!(
        leaks(&report),
        [leak(
            "send",
            "invoke",
            "secrets.token → tasks.run.output → with.out @ send"
        )]
    );
}

// ── infer.model and agent.model ─────────────────────────────────────────────

/// `pick` prints the token (it may reach `exec`) and joins `picks`; `ask` binds `WITH` and calls
/// `VERB` with `model: MODEL`.
fn model(verb: &str, with: &str, model: &str, egress: &str) -> String {
    let limit = if verb == "agent" {
        "max_tokens_total"
    } else {
        "max_tokens"
    };
    format!(
        r#"nika: secret-model
secrets:
  token: {{ source: env, key: T, egress: [{egress}] }}
permits:
  exec: ["printf"]
tasks:
  pick:
    group: picks
    exec: {{ command: [printf, "${{{{ secrets.token }}}}"] }}
  ask:
{with}    {verb}: {{ model: "{model}", prompt: hi, {limit}: 10 }}
"#
    )
}

/// The full finding of a route leak.
fn route_row(verb: &str, path: &str) -> String {
    format!(
        "leak into {verb}.model (task `ask`) — {path} · fix: a model is a route, not a secret, and \
         no `egress:` rule clears `secrets.token` in `model:` — name the model there, or read it \
         from `inputs:` or `const:` (`model: \"${{{{ inputs.model }}}}\"`)"
    )
}

const EXEC: &str = r#"{ to: "exec" }"#;

#[test]
fn a_secret_in_a_task_model_is_refused_on_every_way_in() {
    for verb in ["infer", "agent"] {
        let sink = if verb == "agent" {
            "agent.model"
        } else {
            "infer.model"
        };
        for (with, read, path) in [
            ("", "${{ secrets.token }}", "secrets.token → model @ ask"),
            (
                "    with: { m: \"${{ secrets.token }}\" }\n",
                "${{ with.m }}",
                "secrets.token → with.m @ ask → model @ ask",
            ),
            (
                "    with: { m: \"${{ tasks.pick.output }}\" }\n",
                "${{ with.m }}",
                "secrets.token → tasks.pick.output → with.m @ ask → model @ ask",
            ),
            (
                "    with: { m: \"${{ group.picks }}\" }\n",
                "${{ with.m }}",
                "secrets.token → tasks.pick.output → with.m @ ask → model @ ask",
            ),
        ] {
            let report = run(&model(verb, with, read, EXEC));
            assert_eq!(leaks(&report), [leak("ask", sink, path)], "{verb} {with}");
            // The reason as the JSON report carries it.
            assert_eq!(
                serde_json::to_value(&report.secret_leaks[0].reason)
                    .expect("the reason serializes"),
                serde_json::json!("ModelRoute")
            );
            assert_eq!(
                secret_rows(&report),
                [sec_006("ask", &route_row(verb, path))],
                "{verb} {with}"
            );
            assert!(!report.is_clean(), "{verb} {with}");
        }
    }
}

/// No rule clears a route: not the provider's own sink, not every sink at once.
#[test]
fn no_egress_rule_clears_a_secret_in_a_task_model() {
    for verb in ["infer", "agent"] {
        for egress in [
            format!("{{ to: \"{verb}\" }}, {EXEC}"),
            format!("{{ to: \"infer\" }}, {{ to: \"agent\" }}, {EXEC}, {CRM}"),
        ] {
            let report = run(&model(verb, "", "${{ secrets.token }}", &egress));
            assert_eq!(
                secret_rows(&report),
                [sec_006(
                    "ask",
                    &route_row(verb, "secrets.token → model @ ask")
                )],
                "{verb} {egress}"
            );
        }
    }
}

/// A model the author does not hard-code comes from `inputs:` or `const:`; a literal names it.
#[test]
fn an_input_a_const_or_a_literal_model_stays_clean() {
    for verb in ["infer", "agent"] {
        for (with, read) in [
            ("", "mock/echo"),
            ("", "${{ inputs.model }}"),
            ("", "${{ const.model }}"),
            (
                "    with: { m: \"${{ inputs.model }}\" }\n",
                "${{ with.m }}",
            ),
        ] {
            let yaml = model(verb, with, read, EXEC).replace(
                "permits:",
                "inputs:\n  model: { type: string, required: false, default: \"mock/echo\" }\nconst:\n  model: mock/echo\npermits:",
            );
            let report = run(&yaml);
            assert_eq!(leaks(&report), Vec::<Leak>::new(), "{verb} {read}");
            assert_eq!(secret_rows(&report), Vec::<Row>::new(), "{verb} {read}");
        }
    }
}

/// The envelope `model:` is never rendered at run (a task with no model of its own runs on the
/// default the operator's composition gives): its judgment is unchanged.
#[test]
fn a_secret_in_the_envelope_model_stays_as_it_was() {
    let yaml = r#"nika: q-envelope-model
model: "${{ secrets.token }}"
secrets:
  token: { source: env, key: T, egress: [{ to: "nika:fetch", host: "crm.example.com" }] }
tasks:
  ask:
    infer: { prompt: hi, max_tokens: 10 }
"#;
    let report = run(yaml);
    assert_eq!(leaks(&report), Vec::<Leak>::new());
    assert_eq!(secret_rows(&report), Vec::<Row>::new());
}
