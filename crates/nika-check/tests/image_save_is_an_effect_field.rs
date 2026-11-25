// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>
#![allow(clippy::expect_used)]

//! An `agent:`'s `images.save:` is a field of its effect (spec 02 §images): the run renders it
//! with the task's scope, writes each image the turn delivered there and names the path in the
//! output. Every reader of the one effect table (`action_effect_fields`) therefore reads it, as
//! it reads the prompt:
//!
//! - an input only the save path reads is one the body reads (`requirements.inputs_read`);
//! - a `lift: taint` whose binding only the save path reads is a live door, never
//!   `NIKA-AUTH-011`;
//! - fetched content that picks the save path reaches the task's output, which names the path.
//!
//! Each law is shown through the save path and through the prompt, which every reader already
//! counted, and on a file that reads the value nowhere.

use nika_check::{CheckReport, check};
use nika_schema::{FileId, ParseMode, parse};

fn run(yaml: &str) -> CheckReport {
    check(&parse(yaml, FileId::new(0), ParseMode::Strict).expect("the fixture parses"))
}

/// `draw` asks `PROMPT` and saves at `SAVE`, under a `lift: taint` on `inputs.name`.
fn lifted(prompt: &str, save: &str) -> String {
    format!(
        "nika: lifted-save
model: mock/echo
inputs:
  name: {{ type: string, default: \"hero\" }}
permits:
  fs: {{ write: [\"./out/**\"] }}
tasks:
  draw:
    agent: {{ prompt: \"{prompt}\", images: {{ save: \"{save}\" }} }}
    lift:
      - law: taint
        from: inputs.name
        because: \"the caller names the picture\"
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
fn an_input_only_the_save_path_reads_is_a_requirement() {
    for (prompt, save) in [
        ("draw a boat", "./out/${{ inputs.name }}.png"),
        ("draw ${{ inputs.name }}", "./out/boat.png"),
    ] {
        let report = run(&lifted(prompt, save));
        assert_eq!(
            report.requirements.inputs_read,
            vec!["name".to_owned()],
            "{prompt} {save}"
        );
    }
    let report = run(&lifted("draw a boat", "./out/boat.png"));
    assert_eq!(report.requirements.inputs_read, Vec::<String>::new());
}

#[test]
fn a_taint_door_on_an_input_only_the_save_path_reads_is_live() {
    for (prompt, save) in [
        ("draw a boat", "./out/${{ inputs.name }}.png"),
        ("draw ${{ inputs.name }}", "./out/boat.png"),
    ] {
        let report = run(&lifted(prompt, save));
        assert_eq!(idle_doors(&report), Vec::new(), "{prompt} {save}");
    }
    let report = run(&lifted("draw a boat", "./out/boat.png"));
    assert_eq!(
        idle_doors(&report),
        vec![("taint".to_owned(), Some("inputs.name".to_owned()))],
        "the save path counts only when it reads the binding"
    );
}
