// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>
#![allow(clippy::expect_used)]

//! An `agent:`'s `images.save:` is a path the run renders with secrets resolved, judged by the
//! secret flow as a route.
//!
//! The run writes each image the turn delivered to that path, and the task's output names it, so
//! a secret there would become a file name and a value of the run. A path is never a secret, so a
//! secret reaching `images.save:` is `NIKA-SEC-006` whatever its `egress:`, a rule for the agent's
//! own sink included, with the path from the secret to the field; the run refuses that task
//! before it renders the path (`nika-runtime`'s harness image tests). The output names the path,
//! so a later reader of the task's output sees the secret too. A path the author names, or reads
//! from `inputs:` or `const:`, stays clean, and so does a prompt send its rule allows.
//!
//! Every way a secret travels counts: a direct reference, a `with:` alias, a task output that
//! carries it, a `group.*` fold, a `for_each` item, an `on_error.recover` value.

use nika_check::{CheckReport, LeakReason, check, secret_route_leaks};
use nika_schema::{FileId, ParseMode, parse};

/// The rule that lets `token` reach a program, and nothing else.
const EXEC: &str = r#"{ to: "exec" }"#;

fn parsed(yaml: &str) -> nika_schema::raw::RawWorkflow {
    parse(yaml, FileId::new(0), ParseMode::Strict).expect("the fixture parses")
}

fn run(yaml: &str) -> CheckReport {
    check(&parsed(yaml))
}

/// A leak as `(task, sink, path)`.
type Leak = (String, &'static str, String);

fn leaks(report: &CheckReport) -> Vec<Leak> {
    report
        .secret_leaks
        .iter()
        .map(|l| (l.task.clone(), l.sink, l.trace.clone()))
        .collect()
}

fn leak(task: &str, sink: &'static str, path: &str) -> Leak {
    (task.to_owned(), sink, path.to_owned())
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

/// The full `NIKA-SEC-006` finding of a save-path leak of `draw`.
fn save_row(path: &str) -> (Option<String>, Option<String>, String) {
    (
        Some("NIKA-SEC-006".to_owned()),
        Some("draw".to_owned()),
        format!(
            "leak into agent.images.save (task `draw`) — {path} · fix: an image's path is a path, \
             not a secret, and no `egress:` rule clears `secrets.token` in `images.save:` — name \
             the path there, or read it from `inputs:` or `const:` (`save: \
             \"./out/${{{{ inputs.name }}}}.png\"`)"
        ),
    )
}

/// `pick` prints the token to a program, where it may go, and joins `picks`; `draw` binds `WITH`
/// and saves its images at `SAVE`.
fn save(with: &str, path: &str, egress: &str) -> String {
    format!(
        r#"nika: secret-save
model: mock/echo
secrets:
  token: {{ source: env, key: T, egress: [{egress}] }}
inputs:
  name: {{ type: string, required: false, default: hero }}
const:
  banner: "./out/banner.png"
permits:
  exec: ["printf"]
  fs: {{ write: ["./out/**"] }}
tasks:
  pick:
    group: picks
    exec: {{ command: [printf, "${{{{ secrets.token }}}}"] }}
  draw:
{with}    agent: {{ prompt: hi, images: {{ save: "{path}" }} }}
"#
    )
}

#[test]
fn a_secret_in_an_image_save_path_is_refused_on_every_way_in() {
    for (with, path, trace) in [
        (
            "",
            "./out/${{ secrets.token }}.png",
            "secrets.token → images.save @ draw",
        ),
        (
            "    with: { name: \"${{ secrets.token }}\" }\n",
            "./out/${{ with.name }}.png",
            "secrets.token → with.name @ draw → images.save @ draw",
        ),
        (
            "    with: { name: \"${{ tasks.pick.output }}\" }\n",
            "./out/${{ with.name }}.png",
            "secrets.token → tasks.pick.output → with.name @ draw → images.save @ draw",
        ),
        (
            "    with: { name: \"${{ group.picks }}\" }\n",
            "./out/${{ with.name }}.png",
            "secrets.token → tasks.pick.output → with.name @ draw → images.save @ draw",
        ),
    ] {
        let yaml = save(with, path, EXEC);
        let report = run(&yaml);
        assert_eq!(
            leaks(&report),
            [leak("draw", "agent.images.save", trace)],
            "{path} {with}"
        );
        // The reason as the JSON report carries it.
        assert_eq!(
            serde_json::to_value(&report.secret_leaks[0].reason).expect("the reason serializes"),
            serde_json::json!("ImageSavePath")
        );
        assert_eq!(secret_rows(&report), [save_row(trace)], "{path} {with}");
        assert!(!report.is_clean(), "{path} {with}");
        // The list the run reads before it starts a task is the check's own.
        assert_eq!(secret_route_leaks(&parsed(&yaml)), report.secret_leaks);
    }
}

/// No rule clears a save path: not the agent's own sink, not every sink at once.
#[test]
fn no_egress_rule_clears_a_secret_in_an_image_save_path() {
    for egress in [
        format!(r#"{{ to: "agent" }}, {EXEC}"#),
        format!(
            r#"{{ to: "infer" }}, {{ to: "agent" }}, {EXEC}, {{ to: "nika:write" }}, {{ to: "nika:fetch", host: "crm.example.com" }}"#
        ),
    ] {
        let report = run(&save("", "./out/${{ secrets.token }}.png", &egress));
        assert_eq!(
            secret_rows(&report),
            [save_row("secrets.token → images.save @ draw")],
            "{egress}"
        );
        assert_eq!(report.secret_leaks[0].reason, LeakReason::ImageSavePath);
    }
}

/// A path the author names, or reads from `inputs:` or `const:`, stays clean, and so does a
/// prompt send the agent's rule allows beside it.
#[test]
fn a_named_an_input_or_a_const_save_path_stays_clean() {
    for (with, path) in [
        ("", "./out/hero.png"),
        ("", "./out/${{ inputs.name }}.png"),
        ("", "${{ const.banner }}"),
        (
            "    with: { name: \"${{ inputs.name }}\" }\n",
            "./out/${{ with.name }}.png",
        ),
    ] {
        let report = run(&save(with, path, EXEC));
        assert_eq!(leaks(&report), Vec::<Leak>::new(), "{path} {with}");
        assert!(report.is_clean(), "{path} {with}: {report:?}");
    }
    let prompt = save(
        "",
        "./out/hero.png",
        &format!(r#"{{ to: "agent" }}, {EXEC}"#),
    )
    .replace(
        "agent: { prompt: hi,",
        "agent: { prompt: \"use ${{ secrets.token }}\",",
    );
    let report = run(&prompt);
    assert_eq!(leaks(&report), Vec::<Leak>::new(), "{prompt}");
    assert!(report.is_clean(), "{report:?}");
}

/// `draw` saves one image per token of a literal list.
const ITEM_SAVE: &str = r#"nika: item-save
model: mock/echo
secrets:
  token: { source: env, key: T, egress: [{ to: "agent" }] }
permits:
  fs: { write: ["./out/**"] }
tasks:
  draw:
    for_each: { items: ["${{ secrets.token }}"] }
    agent: { prompt: hi, images: { save: "./out/${{ item }}.png" } }
"#;

/// `pick` fails and recovers to the token; `draw` saves at the name `pick`'s record then holds.
const RECOVER_SAVE: &str = r#"nika: recover-save
model: mock/echo
secrets:
  token: { source: env, key: T, egress: [{ to: "agent" }] }
permits:
  exec: ["false"]
  fs: { write: ["./out/**"] }
tasks:
  pick:
    exec: { command: ["false"] }
    on_error: { recover: "${{ secrets.token }}" }
  draw:
    with: { name: "${{ tasks.pick.output }}" }
    agent: { prompt: hi, images: { save: "./out/${{ with.name }}.png" } }
"#;

#[test]
fn an_item_or_a_recovered_value_carries_a_secret_into_an_image_save_path() {
    for (yaml, trace) in [
        (
            ITEM_SAVE,
            "secrets.token → item @ draw → images.save @ draw",
        ),
        (
            RECOVER_SAVE,
            "secrets.token → on_error.recover @ pick → tasks.pick.output → with.name @ draw → \
             images.save @ draw",
        ),
    ] {
        let report = run(yaml);
        assert_eq!(
            leaks(&report),
            [leak("draw", "agent.images.save", trace)],
            "{yaml}"
        );
        assert_eq!(secret_rows(&report), [save_row(trace)], "{yaml}");
    }
}

/// The output names each saved image's path, so a program that prints `draw`'s output sees the
/// secret the path read, and its own rule decides that send.
#[test]
fn the_output_of_a_task_that_saved_at_a_secret_path_carries_the_secret() {
    let report = run(r#"nika: save-then-print
model: mock/echo
secrets:
  token: { source: env, key: T, egress: [{ to: "agent" }] }
permits:
  exec: ["printf"]
  fs: { write: ["./out/**"] }
tasks:
  draw:
    agent: { prompt: hi, images: { save: "./out/${{ secrets.token }}.png" } }
  show:
    with: { shown: "${{ tasks.draw.output }}" }
    exec: { command: [printf, "${{ with.shown }}"] }
"#);
    assert_eq!(
        leaks(&report),
        [
            leak(
                "draw",
                "agent.images.save",
                "secrets.token → images.save @ draw"
            ),
            leak(
                "show",
                "exec",
                "secrets.token → tasks.draw.output → with.shown @ show"
            ),
        ]
    );
}

/// The journey names `draw` among the secret's tasks: the run writes the file here, so the save
/// is no destination and no model route.
#[test]
fn the_journey_counts_a_save_path_as_a_task_with_no_destination() {
    let report = run(&save("", "./out/${{ secrets.token }}.png", EXEC));
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
            vec!["draw".to_owned(), "pick".to_owned()],
            Vec::<String>::new(),
            Vec::<String>::new(),
        )]
    );
}
