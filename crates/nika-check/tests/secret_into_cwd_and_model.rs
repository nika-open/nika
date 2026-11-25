// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>
#![allow(clippy::expect_used)]

//! Two fields the run renders with secrets resolved, judged by the secret flow.
//!
//! An `exec:`'s `cwd:` is the directory the run spawns the program in, and an `infer:`/`agent:`'s
//! `model:` is the route the run sends as the provider and model it calls. Neither a directory nor
//! a model name is ever a secret, so a secret reaching either is `NIKA-SEC-006` whatever its
//! `egress:`, with the path from the secret to the field, and the run refuses that task before it
//! renders the field (`nika-runtime`'s `secret_route_refusal` suite). A program can print its
//! working directory, so its output carries the secret too. The envelope `model:` is not rendered
//! at run and stays as it was.
//!
//! Every way a secret travels counts: a direct reference, a `with:` alias, a task output that
//! carries it, a `group.*` fold, a `for_each` item, an `on_error.recover` value.

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

/// The full finding of a cwd leak.
fn cwd_row(path: &str) -> String {
    format!(
        "leak into exec.cwd (task `run`) — {path} · fix: a working directory is a path, not a \
         secret, and no `egress:` rule clears `secrets.token` in `cwd:` — name the directory \
         there, or read it from `inputs:` or `const:` (`cwd: \"${{{{ inputs.dir }}}}\"`)"
    )
}

#[test]
fn a_secret_in_cwd_is_refused_on_every_way_in() {
    for (with, dir, path) in [
        ("", "${{ secrets.token }}", "secrets.token → cwd @ run"),
        (
            "    with: { dir: \"${{ secrets.token }}\" }\n",
            "${{ with.dir }}",
            "secrets.token → with.dir @ run → cwd @ run",
        ),
        (
            "    with: { dir: \"${{ tasks.pick.output }}\" }\n",
            "${{ with.dir }}",
            "secrets.token → tasks.pick.output → with.dir @ run → cwd @ run",
        ),
        (
            "    with: { dir: \"${{ group.picks }}\" }\n",
            "${{ with.dir }}",
            "secrets.token → tasks.pick.output → with.dir @ run → cwd @ run",
        ),
    ] {
        let report = run(&cwd(with, dir, CRM));
        assert_eq!(
            leaks(&report),
            [leak("run", "exec.cwd", path)],
            "{dir} {with}"
        );
        // The reason as the JSON report carries it.
        assert_eq!(
            serde_json::to_value(&report.secret_leaks[0].reason).expect("the reason serializes"),
            serde_json::json!("WorkingDirectory")
        );
        assert_eq!(secret_rows(&report), [sec_006("run", &cwd_row(path))]);
        assert!(!report.is_clean(), "{dir}");
    }
}

/// No rule clears a directory: not the exec sink that runs the program, not every sink at once.
#[test]
fn no_egress_rule_clears_a_secret_in_cwd() {
    for egress in [
        format!("{{ to: \"exec\" }}, {CRM}"),
        format!("{{ to: \"infer\" }}, {{ to: \"agent\" }}, {{ to: \"exec\" }}, {CRM}"),
    ] {
        let report = run(&cwd("", "${{ secrets.token }}", &egress));
        assert_eq!(
            secret_rows(&report),
            [sec_006("run", &cwd_row("secrets.token → cwd @ run"))],
            "{egress}"
        );
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

/// The program can print its working directory, so its output carries the token: `run` is refused
/// for the directory, and `send`, which posts that output to other.example.com, sends the token
/// where it may not go.
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
        [
            leak("run", "exec.cwd", "secrets.token → cwd @ run"),
            leak(
                "send",
                "invoke",
                "secrets.token → tasks.run.output → with.out @ send"
            )
        ]
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

// ── a for_each item and an on_error.recover value ───────────────────────────

/// `run` runs `true` in each item of `[secrets.token]`.
const ITEM_CWD: &str = r#"nika: item-cwd
secrets:
  token: { source: env, key: T, egress: [{ to: "nika:fetch", host: "crm.example.com" }] }
permits:
  exec: ["true"]
tasks:
  run:
    for_each: { items: ["${{ secrets.token }}"] }
    exec: { command: ["true"], cwd: "${{ item }}" }
"#;

/// `pick` fails and recovers to the token; `run` runs `true` in the directory `pick`'s record
/// then names.
const RECOVER_CWD: &str = r#"nika: recover-cwd
secrets:
  token: { source: env, key: T, egress: [{ to: "nika:fetch", host: "crm.example.com" }] }
permits:
  exec: ["false", "true"]
tasks:
  pick:
    exec: { command: ["false"] }
    on_error: { recover: "${{ secrets.token }}" }
  run:
    with: { dir: "${{ tasks.pick.output }}" }
    exec: { command: ["true"], cwd: "${{ with.dir }}" }
"#;

#[test]
fn an_item_or_a_recovered_value_carries_a_secret_into_cwd() {
    for (yaml, path) in [
        (ITEM_CWD, "secrets.token → item @ run → cwd @ run"),
        (
            RECOVER_CWD,
            "secrets.token → on_error.recover @ pick → tasks.pick.output → with.dir @ run → \
             cwd @ run",
        ),
    ] {
        let report = run(yaml);
        assert_eq!(leaks(&report), [leak("run", "exec.cwd", path)], "{yaml}");
        assert_eq!(report.secret_leaks[0].reason, LeakReason::WorkingDirectory);
        assert_eq!(
            secret_rows(&report),
            [sec_006("run", &cwd_row(path))],
            "{yaml}"
        );
    }
}

/// `secret_route_leaks`, the list the run reads before it starts a task, is the check's own: the
/// same rows for a model and a directory, none for the sends an `egress:` rule may clear.
#[test]
fn the_route_leaks_the_run_reads_are_the_checks_own() {
    let parse_it = |yaml: &str| parse(yaml, FileId::new(0), ParseMode::Strict).expect("parses");
    for yaml in [
        ITEM_CWD.to_owned(),
        RECOVER_CWD.to_owned(),
        cwd("", "${{ secrets.token }}", CRM),
        model("infer", "", "${{ secrets.token }}", EXEC),
        model(
            "agent",
            "    with: { m: \"${{ group.picks }}\" }\n",
            "${{ with.m }}",
            EXEC,
        ),
    ] {
        let wf = parse_it(&yaml);
        let route = nika_check::secret_route_leaks(&wf);
        let checked: Vec<_> = check(&wf)
            .secret_leaks
            .into_iter()
            .filter(|l| {
                matches!(
                    l.reason,
                    LeakReason::ModelRoute | LeakReason::WorkingDirectory
                )
            })
            .collect();
        assert_eq!(route, checked, "{yaml}");
        assert_eq!(route.len(), 1, "{yaml}");
    }
    // A send no `egress:` rule clears is a leak, and no route.
    let sends =
        parse_it(&cwd("", "./work", CRM).replace("crm.example.com/a", "other.example.com/a"));
    assert_eq!(
        leaks(&check(&sends)),
        [leak("pick", "invoke", "secrets.token")]
    );
    assert_eq!(
        nika_check::secret_route_leaks(&sends),
        Vec::<nika_check::SecretLeak>::new()
    );
}

/// Two paths of one length reach `ask`'s model, through `m`, which runs on `a`'s and `b`'s
/// outputs: `c → a` and `d → b`. A sweep in declaration order meets `b`'s path first (`a` waits
/// for `c`, declared after it) and keeps it on the tie; the check's wave order meets both at once
/// and keeps `a`'s, read first.
const TIE_MODEL: &str = r#"nika: tie-model
secrets:
  token: { source: env, key: T, egress: [{ to: "exec" }] }
permits:
  exec: ["echo"]
tasks:
  d:
    exec: { command: ["echo", "${{ secrets.token }}"] }
  a:
    with: { v: "${{ tasks.c.output }}" }
    exec: { command: ["echo", "${{ with.v }}"] }
  b:
    with: { v: "${{ tasks.d.output }}" }
    exec: { command: ["echo", "${{ with.v }}"] }
  m:
    with: { x: "${{ tasks.a.output }}", y: "${{ tasks.b.output }}" }
    exec: { command: ["echo", "${{ with.x }}", "${{ with.y }}"] }
  c:
    exec: { command: ["echo", "${{ secrets.token }}"] }
  ask:
    with: { m: "${{ tasks.m.output }}" }
    infer: { model: "${{ with.m }}", prompt: hi, max_tokens: 10 }
"#;

/// On a tie the run's refusal names the path the check finding names: the run reads the flow
/// in the check's own wave order.
#[test]
fn a_tie_between_two_paths_names_the_checks_path_at_run() {
    let wf = parse(TIE_MODEL, FileId::new(0), ParseMode::Strict).expect("parses");
    let checked: Vec<_> = check(&wf)
        .secret_leaks
        .into_iter()
        .filter(|l| matches!(l.reason, LeakReason::ModelRoute))
        .collect();
    assert_eq!(
        checked.iter().map(|l| l.trace.as_str()).collect::<Vec<_>>(),
        [
            "secrets.token → tasks.c.output → with.v @ a → tasks.a.output → with.x @ m → \
         tasks.m.output → with.m @ ask → model @ ask"
        ]
    );
    assert_eq!(nika_check::secret_route_leaks(&wf), checked);
}

/// `ask` calls `VERB` with each item of `[secrets.token]` as its model.
const ITEM_MODEL: &str = r#"nika: item-model
secrets:
  token: { source: env, key: T, egress: [{ to: "exec" }] }
tasks:
  ask:
    for_each: { items: ["${{ secrets.token }}"] }
    VERB: { model: "${{ item }}", prompt: hi, LIMIT: 10 }
"#;

/// `pick` fails and recovers to the token; `ask` calls `VERB` on the model `pick`'s record then
/// names.
const RECOVER_MODEL: &str = r#"nika: recover-model
secrets:
  token: { source: env, key: T, egress: [{ to: "exec" }] }
permits:
  exec: ["false"]
tasks:
  pick:
    exec: { command: ["false"] }
    on_error: { recover: "${{ secrets.token }}" }
  ask:
    with: { m: "${{ tasks.pick.output }}" }
    VERB: { model: "${{ with.m }}", prompt: hi, LIMIT: 10 }
"#;

#[test]
fn an_item_or_a_recovered_value_carries_a_secret_into_a_task_model() {
    for verb in ["infer", "agent"] {
        let (sink, limit) = if verb == "agent" {
            ("agent.model", "max_tokens_total")
        } else {
            ("infer.model", "max_tokens")
        };
        for (fixture, path) in [
            (ITEM_MODEL, "secrets.token → item @ ask → model @ ask"),
            (
                RECOVER_MODEL,
                "secrets.token → on_error.recover @ pick → tasks.pick.output → with.m @ ask → \
                 model @ ask",
            ),
        ] {
            let yaml = fixture.replace("VERB", verb).replace("LIMIT", limit);
            let report = run(&yaml);
            assert_eq!(leaks(&report), [leak("ask", sink, path)], "{yaml}");
            assert_eq!(report.secret_leaks[0].reason, LeakReason::ModelRoute);
            assert_eq!(
                secret_rows(&report),
                [sec_006("ask", &route_row(verb, path))],
                "{yaml}"
            );
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
