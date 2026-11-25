// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>
#![allow(clippy::expect_used)]

//! An `exec:`'s `cwd:` is a field of its effect (spec 02 §exec): the run renders it, labels it
//! and re-gates a tainted one as a path against `permits.fs.read` (`NIKA-SEC-004`). Every
//! reader of the one effect table (`action_effect_fields`) therefore reads it too, as it reads
//! the command, its `stdin:` and its `env:` values:
//!
//! - a `lift: taint` whose binding only the `cwd:` reads is a live door, never `NIKA-AUTH-011`;
//! - an input only the `cwd:` reads is one the body reads (`requirements.inputs_read`);
//! - fetched content that picks the `cwd:` of an `exec:` reaches that egress task, so the
//!   lethal trifecta names it (`NIKA-SEC-009`), as it does when the content is an argument.
//!
//! Each law is shown through the `cwd:` and through an argument, which every reader already
//! counted. A `tasks.<id>` read cannot sit in a `cwd:` of a valid file (`NIKA-VAR-021` sends it
//! through `with:`), so the output walk shows no difference here.

use nika_check::{CheckReport, check};
use nika_schema::{FileId, ParseMode, parse};

fn run(yaml: &str) -> CheckReport {
    check(&parse(yaml, FileId::new(0), ParseMode::Strict).expect("the fixture parses"))
}

/// `ls` runs in `cwd` with `argv` as its argument, under a `lift: taint` on `inputs.dir`.
fn lifted(cwd: &str, argv: &str) -> String {
    format!(
        "nika: lifted-cwd
inputs:
  dir: {{ type: string, default: \"data/a\" }}
permits:
  exec: [\"ls\"]
  fs: {{ read: [\"data/**\"] }}
tasks:
  list:
    exec: {{ command: [\"ls\", \"{argv}\"], cwd: \"{cwd}\" }}
    lift:
      - law: taint
        from: inputs.dir
        because: \"the caller names a data folder\"
"
    )
}

/// The lift findings of a report, as `(law, from)` pairs.
fn idle_doors(report: &CheckReport) -> Vec<(String, Option<String>)> {
    report
        .lift_findings
        .iter()
        .map(|f| (f.law.to_owned(), f.from.clone()))
        .collect()
}

#[test]
fn a_taint_door_on_an_input_only_the_cwd_reads_is_live() {
    let report = run(&lifted("${{ inputs.dir }}", "-1"));
    assert_eq!(
        idle_doors(&report),
        Vec::new(),
        "the run re-gates this cwd, so the lift engages"
    );
}

#[test]
fn the_same_door_on_an_input_the_argument_reads_is_live() {
    let report = run(&lifted("data", "${{ inputs.dir }}"));
    assert_eq!(idle_doors(&report), Vec::new());
}

#[test]
fn a_taint_door_on_an_input_nothing_reads_is_still_refused() {
    let report = run(&lifted("data", "-1"));
    assert_eq!(
        idle_doors(&report),
        vec![("taint".to_owned(), Some("inputs.dir".to_owned()))],
        "the cwd counts only when it reads the binding"
    );
}

#[test]
fn an_input_only_the_cwd_reads_is_a_requirement() {
    let report = run(&lifted("${{ inputs.dir }}", "-1"));
    assert_eq!(report.requirements.inputs_read, vec!["dir".to_owned()]);
}

#[test]
fn an_input_the_argument_reads_is_a_requirement() {
    let report = run(&lifted("data", "${{ inputs.dir }}"));
    assert_eq!(report.requirements.inputs_read, vec!["dir".to_owned()]);
}

/// `dl` fetches a folder name; `list` runs `ls` with it in `cwd` or `argv`, holding a secret.
/// Private read, untrusted ingress and external egress are all permitted, and no gate stands
/// between them.
fn steered(cwd: &str, argv: &str) -> String {
    format!(
        "nika: steered-cwd
secrets:
  token: {{ source: env, key: T }}
permits:
  tools: [\"nika:fetch\"]
  net: {{ http: [\"example.com\"] }}
  exec: [\"ls\"]
  fs: {{ read: [\"data/**\"] }}
tasks:
  dl:
    invoke: {{ tool: \"nika:fetch\", args: {{ url: \"https://example.com/dir\" }} }}
  list:
    with: {{ dir: \"${{{{ tasks.dl.output }}}}\" }}
    exec: {{ command: [\"ls\", \"{argv}\"], cwd: \"{cwd}\", env: {{ TOKEN: \"${{{{ secrets.token }}}}\" }} }}
"
    )
}

/// The trifecta findings of a report, as `(source, task)` pairs.
fn trifectas(report: &CheckReport) -> Vec<(Option<String>, String)> {
    report
        .trifecta_findings
        .iter()
        .map(|v| (v.source.clone(), v.task.clone()))
        .collect()
}

#[test]
fn fetched_content_that_picks_the_cwd_completes_the_trifecta() {
    let report = run(&steered("${{ with.dir }}", "-1"));
    assert_eq!(
        trifectas(&report),
        vec![(Some("dl".to_owned()), "list".to_owned())]
    );
}

#[test]
fn fetched_content_in_an_argument_completes_the_trifecta() {
    let report = run(&steered("data", "${{ with.dir }}"));
    assert_eq!(
        trifectas(&report),
        vec![(Some("dl".to_owned()), "list".to_owned())]
    );
}

#[test]
fn a_bound_value_the_exec_never_reads_completes_no_trifecta() {
    let report = run(&steered("data", "-1"));
    assert_eq!(trifectas(&report), Vec::new());
}
