// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>
#![allow(clippy::expect_used)]

//! A builtin's path argument names the file or the directory its call reads or writes: `nika:read`
//! and `nika:write` `path`, a media generator's `output_dir` and `filename_prefix`,
//! `nika:image_compose` `inputs[]`, and every other filesystem slot `nika_cap::path_arguments`
//! lists. A path is never a secret: the run renders it into the call, judges it against the
//! boundary, writes it into the trace's `permits.fs` lines, names it in the event, the manifest
//! and the output, and a media generator folds it into the name of the file it saves. So a
//! secret reaching a path argument is `NIKA-SEC-006` whatever its
//! `egress:`, a rule for the tool itself included, with the path from the secret to the field,
//! and the run refuses that task before it renders the call (`nika-runtime`'s
//! `secret_route_refusal` suite). Every other argument stays a send its rule decides, and a path
//! the author names or reads from `inputs:` stays clean.

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

/// Task `go` calls `TOOL` with `ARGS`, under a rule that lets `token` reach the tool, and with
/// the boundary every fixture's literal paths need.
fn call(tool: &str, with: &str, args: &str) -> String {
    format!(
        r#"nika: secret-tool-path
secrets:
  token: {{ source: env, key: T, egress: [{{ to: "{tool}" }}] }}
inputs:
  name: {{ type: string, required: false, default: hello }}
permits:
  tools: ["{tool}"]
  fs: {{ read: ["./in/**"], write: ["./out/**"] }}
tasks:
  go:
{with}    invoke: {{ tool: "{tool}", args: {{ {args} }} }}
"#
    )
}

/// `(tool, args with the secret in one path argument, the field)`.
const ROUTES: [(&str, &str, &str); 7] = [
    (
        "nika:tts_generate",
        r#"provider: mock, text: hi, output_dir: "./out", filename_prefix: "SECRET""#,
        "args.filename_prefix",
    ),
    (
        "nika:tts_generate",
        r#"provider: mock, text: hi, output_dir: "./out/SECRET""#,
        "args.output_dir",
    ),
    (
        "nika:image_generate",
        r#"provider: mock, prompt: hi, output_dir: "./out", filename_prefix: "SECRET""#,
        "args.filename_prefix",
    ),
    (
        "nika:image_generate",
        r#"provider: mock, prompt: hi, output_dir: "./out/SECRET""#,
        "args.output_dir",
    ),
    (
        "nika:write",
        r#"path: "./out/SECRET.txt", content: hi"#,
        "args.path",
    ),
    ("nika:read", r#"path: "./in/SECRET.txt""#, "args.path"),
    (
        "nika:image_compose",
        r#"inputs: ["./in/a.png", "./in/SECRET.png"], out: "./out/sheet.png", layout: { kind: grid }"#,
        "args.inputs",
    ),
];

#[test]
fn a_secret_in_a_path_argument_is_refused_whatever_its_rule() {
    for (tool, args, field) in ROUTES {
        for (with, secret, path) in [
            (
                String::new(),
                "${{ secrets.token }}",
                format!("secrets.token → {field} @ go"),
            ),
            (
                "    with: { s: \"${{ secrets.token }}\" }\n".to_owned(),
                "${{ with.s }}",
                format!("secrets.token → with.s @ go → {field} @ go"),
            ),
        ] {
            let yaml = call(tool, &with, &args.replace("SECRET", secret));
            let report = run(&yaml);
            let sink = format!("invoke.{field}");
            assert_eq!(
                leaks(&report)
                    .into_iter()
                    .map(|(task, sink, trace)| (task, sink.to_owned(), trace))
                    .collect::<Vec<_>>(),
                [("go".to_owned(), sink.clone(), path.clone())],
                "{tool} {field} {with}"
            );
            assert_eq!(report.secret_leaks[0].reason, LeakReason::ToolPath);
            assert_eq!(
                secret_rows(&report),
                [(
                    Some("NIKA-SEC-006".to_owned()),
                    Some("go".to_owned()),
                    format!(
                        "leak into {sink} (task `go`) — {path} · fix: a path is a route, not a \
                         secret, and no `egress:` rule clears `secrets.token` in `{field}:` — \
                         name the path there, or read it from `inputs:` or `const:`"
                    )
                )],
                "{tool} {field} {with}"
            );
            // The list the run reads before it starts a task is the check's own.
            assert_eq!(secret_route_leaks(&parsed(&yaml)), report.secret_leaks);
        }
    }
}

/// Without any `egress:` rule, a secret in a path is the route, never a send an added rule could
/// clear: the fix the finding teaches is the path's, not « add `egress:` ».
#[test]
fn without_a_rule_the_path_is_still_the_route() {
    for (tool, args, field) in ROUTES {
        let yaml = call(tool, "", &args.replace("SECRET", "${{ secrets.token }}"))
            .replace(&format!("egress: [{{ to: \"{tool}\" }}]"), "egress: []");
        let report = run(&yaml);
        assert_eq!(
            report
                .secret_leaks
                .iter()
                .map(|l| (l.sink.to_owned(), l.reason.clone()))
                .collect::<Vec<_>>(),
            [(format!("invoke.{field}"), LeakReason::ToolPath)],
            "{tool} {field}"
        );
    }
}

/// The other arguments stay sends the tool's rule decides: the content `nika:write` writes and the
/// text a media generator speaks or draws go to the tool their rule allows, and a path the author
/// names or reads from `inputs:` is no route a secret rides.
#[test]
fn every_other_argument_and_a_named_path_stay_as_they_were() {
    for (tool, args) in [
        (
            "nika:write",
            r#"path: "./out/note.txt", content: "${{ secrets.token }}""#,
        ),
        (
            "nika:write",
            r#"path: "./out/${{ inputs.name }}.txt", content: hi"#,
        ),
        (
            "nika:tts_generate",
            r#"provider: mock, text: "${{ secrets.token }}", output_dir: "./out", filename_prefix: "${{ inputs.name }}""#,
        ),
        (
            "nika:image_generate",
            r#"provider: mock, prompt: "${{ secrets.token }}", output_dir: "./out""#,
        ),
    ] {
        let report = run(&call(tool, "", args));
        assert_eq!(leaks(&report), Vec::new(), "{tool} {args}");
        assert!(
            secret_rows(&report).is_empty(),
            "{tool} {args}: {:?}",
            secret_rows(&report)
        );
    }
    // Without the rule, the content send is the leak it always was, and no route.
    let unsanctioned = call(
        "nika:write",
        "",
        r#"path: "./out/note.txt", content: "${{ secrets.token }}""#,
    )
    .replace("egress: [{ to: \"nika:write\" }]", "egress: []");
    let report = run(&unsanctioned);
    assert_eq!(
        report
            .secret_leaks
            .iter()
            .map(|l| (l.task.as_str(), l.sink, l.reason.clone()))
            .collect::<Vec<_>>(),
        [("go", "invoke", LeakReason::NoEgress)]
    );
}

/// The journey counts a path with the call's effect, as it counts an exec's `cwd:` with the
/// program: the tool receives the path like its other arguments. So a secret in a path names the
/// same tasks and the same destinations as the same secret in a send of that call, and no model
/// route.
#[test]
fn the_journey_counts_a_path_with_the_call() {
    /// Each used secret as `(name, tasks, flows_to, routes)`.
    type Journey = Vec<(String, Vec<String>, Vec<String>, Vec<String>)>;
    let journey = |yaml: &str| -> Journey {
        run(yaml)
            .data_journey
            .secrets_used
            .iter()
            .map(|s| {
                (
                    s.name.clone(),
                    s.tasks.clone(),
                    s.flows_to.clone(),
                    s.routes.clone(),
                )
            })
            .collect()
    };
    for (tool, path, send) in [
        (
            "nika:write",
            r#"path: "./out/${{ secrets.token }}.txt", content: hi"#,
            r#"path: "./out/k.txt", content: "${{ secrets.token }}""#,
        ),
        (
            "nika:tts_generate",
            r#"provider: mock, text: hi, output_dir: "./out", filename_prefix: "${{ secrets.token }}""#,
            r#"provider: mock, text: "${{ secrets.token }}", output_dir: "./out""#,
        ),
        (
            "nika:image_generate",
            r#"provider: mock, prompt: hi, output_dir: "./out/${{ secrets.token }}""#,
            r#"provider: mock, prompt: "${{ secrets.token }}", output_dir: "./out""#,
        ),
    ] {
        let routed = journey(&call(tool, "", path));
        assert_eq!(routed, journey(&call(tool, "", send)), "{tool}");
        assert_eq!(
            routed
                .iter()
                .map(|(name, tasks, _, routes)| (name.as_str(), tasks.clone(), routes.clone()))
                .collect::<Vec<_>>(),
            [("token", vec!["go".to_owned()], Vec::<String>::new())],
            "{tool}"
        );
    }
}
