// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

use super::*;
use nika_schema::parser::{ParseMode, parse};
use nika_schema::source::FileId;

fn workflow(yaml: &str) -> RawWorkflow {
    parse(yaml, FileId::new(0), ParseMode::Strict).expect("fixture parses")
}

fn only_task(body: &str, verb: &str) -> RawTask {
    workflow(&format!("nika: t\ntasks:\n  t:\n{body}    {verb}\n"))
        .tasks
        .remove(0)
        .value
}

fn names(w: &RawWorkflow, picked: &[usize]) -> Vec<String> {
    picked
        .iter()
        .filter_map(|i| w.tasks.get(*i).map(|t| t.value.id.value.clone()))
        .collect()
}

/// A choice's answer is tested when the guard reads its exact carrier and some listed choice
/// does not prove it true; with no literal list, the read, unless the guard is proven true with
/// nothing substituted or only tests the binding against `null`. Each case is `(with, when,
/// tested under ["send", "no"], tested with no list)`.
#[test]
fn a_text_answer_is_tested_where_the_guard_can_close() {
    let pick = "    with: { pick: \"${{ tasks.ask.output }}\", n: 3 }\n";
    let choices = ["send".to_owned(), "no".to_owned()];
    let tested = |with: &str, when: &str, choices: Option<&[String]>| {
        let gate = if when.is_empty() {
            String::new()
        } else {
            format!("    when: \"${{{{ {when} }}}}\"\n")
        };
        tests_answer(
            &only_task(&format!("{with}{gate}"), "exec: { command: [\"true\"] }"),
            "ask",
            choices,
            &BTreeSet::new(),
        )
    };
    for (with, when, listed, unlisted) in [
        (pick, "with.pick == 'send'", true, true),
        (pick, "with.pick != 'no'", true, true),
        (pick, "with.pick in ['send']", true, true),
        (pick, "with.n > 0 && with.pick == 'send'", true, true),
        (pick, "with.pick.startsWith('s')", true, true),
        (pick, "with.pick", true, true),
        (pick, "with.pick == null || with.pick == 'send'", true, true),
        (pick, "with.pick != null", false, false),
        (pick, "null != with.pick", false, false),
        (pick, "with.pick == 'send' || true", false, false),
        (pick, "with.pick in ['send', 'no']", false, true),
        (pick, "with.pick != 'maybe'", false, true),
        (pick, "with.n > 0", false, false),
        (pick, "", false, false),
        (
            "    with: { pick: \"answer=${{ tasks.ask.output }}\" }\n",
            "with.pick == 'send'",
            false,
            false,
        ),
        (
            "    with: { pick: \"${{ tasks.other.output }}\" }\n",
            "with.pick == 'send'",
            false,
            false,
        ),
    ] {
        assert_eq!(tested(with, when, Some(&choices)), listed, "{with}{when}");
        assert_eq!(tested(with, when, None), unlisted, "{with}{when} · no list");
    }
    assert!(
        !tests_answer(
            &only_task(
                &format!("{pick}    when: false\n"),
                "exec: { command: [\"true\"] }"
            ),
            "ask",
            Some(&choices),
            &BTreeSet::new(),
        ),
        "a literal guard reads no answer"
    );
}

/// The gate `ask`, then STAGES, then `send` guarded by GUARD on the output of READER.
const FLOW: &str = r#"nika: t
tasks:
  ask:
    invoke: { tool: "nika:prompt", args: { mode: choice, message: "go?", choices: ["go", "no"] } }
STAGES  send:
    with: { plan: "${{ tasks.READER.output }}" }
    when: "GUARD"
    exec: { command: ["true"] }
"#;

/// A `nika:jq` stage ID binding the output of FROM as `a`; EXTRA lines before its `with:`.
const JQ: &str = r#"  ID:
EXTRA    with: { a: "${{ tasks.FROM.output }}" }
    invoke:
      tool: "nika:jq"
      args:
        input: INPUT
        expression: 'PROGRAM'
"#;

fn jq(id: &str, from: &str, extra: &str, input: &str, program: &str) -> String {
    JQ.replace("EXTRA", extra)
        .replace("INPUT", input)
        .replace("PROGRAM", program)
        .replace("ID", id)
        .replace("FROM", from)
}

/// The usual stage: the answer is the input, and the program reads it.
fn stage(id: &str, from: &str) -> String {
    jq(id, from, "", "\"${{ with.a }}\"", "{go: (. != \"no\")}")
}

fn flow(stages: &str, reader: &str, guard: &str) -> RawWorkflow {
    workflow(
        &FLOW
            .replace("STAGES", stages)
            .replace("READER", reader)
            .replace("GUARD", guard),
    )
}

fn derived(w: &RawWorkflow) -> Vec<String> {
    answer_derivations(w, "ask").into_iter().collect()
}

fn tested(w: &RawWorkflow) -> Vec<String> {
    let choices = ["go".to_owned(), "no".to_owned()];
    names(w, &answer_tested_by(w, "ask", Some(&choices)))
}

const GO: &str = "${{ with.plan.go == true }}";

/// A `nika:jq` transform carries the answer on, directly or through another such transform,
/// when the answer reaches its `args.input` and its program reads that value; any other verb or
/// tool, a recovered value, a fan-out, a constant or templated program, a binding the input
/// does not read or a field the program skips does not.
#[test]
fn a_jq_stage_carries_the_answer_only_when_its_program_reads_it() {
    let one = flow(&stage("plan", "ask"), "plan", GO);
    assert_eq!(derived(&one), ["plan"]);
    assert_eq!(tested(&one), ["send"]);
    let chain = flow(
        &format!("{}{}", stage("first", "ask"), stage("plan", "first")),
        "plan",
        GO,
    );
    assert_eq!(derived(&chain), ["first", "plan"]);
    assert_eq!(tested(&chain), ["send"]);
    let field = jq(
        "plan",
        "ask",
        "",
        "{ a: \"${{ with.a }}\", n: 1 }",
        "{go: (.a != \"no\")}",
    );
    assert_eq!(derived(&flow(&field, "plan", GO)), ["plan"]);
    for (why, stage) in [
        (
            "a jq stage that binds the status, not the answer",
            stage("plan", "ask").replace("tasks.ask.output", "tasks.ask.status"),
        ),
        (
            "a recovered value",
            jq(
                "plan",
                "ask",
                "    on_error: { recover: { go: true } }\n",
                "\"${{ with.a }}\"",
                ".",
            ),
        ),
        (
            "a fan-out",
            jq(
                "plan",
                "ask",
                "    for_each: { items: [1, 2] }\n",
                "\"${{ with.a }}\"",
                ".",
            ),
        ),
        (
            "a constant program",
            jq("plan", "ask", "", "\"${{ with.a }}\"", "{go: true}"),
        ),
        (
            "a binding the input does not read",
            jq("plan", "ask", "", "\"go\"", "{go: (. != \"no\")}"),
        ),
        (
            "a field the program skips",
            jq(
                "plan",
                "ask",
                "",
                "{ a: \"${{ with.a }}\", n: 1 }",
                "{go: (.n > 0)}",
            ),
        ),
        (
            "a templated program",
            jq("plan", "ask", "", "\"${{ with.a }}\"", "${{ const.p }}"),
        ),
        (
            "a model in between",
            "  plan:\n    with: { a: \"${{ tasks.ask.output }}\" }\n    infer: { prompt: \
             \"${{ with.a }}\", max_tokens: 9 }\n"
                .to_owned(),
        ),
        (
            "a subprocess in between",
            "  plan:\n    with: { a: \"${{ tasks.ask.output }}\" }\n    exec: { command: \
             [\"echo\", \"${{ with.a }}\"] }\n"
                .to_owned(),
        ),
    ] {
        let w = flow(&stage, "plan", GO);
        assert!(derived(&w).is_empty(), "{why}: {:?}", derived(&w));
        assert!(tested(&w).is_empty(), "{why}: {:?}", tested(&w));
    }
}

/// On the derived path, a guard proven true with the derived binding unknown tests nothing, and
/// neither does a `null` test of the binding itself (a jq stage always yields a value); a
/// `null` test of a field, or a read beside the `null` test, does.
#[test]
fn a_derived_guard_must_be_able_to_close() {
    for (guard, expected) in [
        ("${{ with.plan.go == true || true }}", false),
        ("${{ true || with.plan.go == true }}", false),
        ("${{ with.plan != null }}", false),
        ("${{ null == with.plan }}", false),
        ("${{ with.plan != null && with.plan.go == true }}", true),
        ("${{ with.plan.go != null }}", true),
        ("${{ with.plan.go }}", true),
    ] {
        let w = flow(&stage("plan", "ask"), "plan", guard);
        assert_eq!(!tested(&w).is_empty(), expected, "{guard}");
    }
}

/// The syntactic read of a jq program: path starts outside strings and comments, the field
/// each names; interpolations are code; a variable, a number or a path step starts none.
#[test]
fn a_jq_program_is_read_for_its_path_starts() {
    for (program, any, fields) in [
        (".", true, &[][..]),
        (".answer != \"no\"", true, &["answer"][..]),
        (
            "(.answer | length) > 0 and .now != null",
            true,
            &["answer", "now"][..],
        ),
        ("., .x", true, &["x"][..]),
        ("..", true, &[][..]),
        (".\"answer\" | .[\"now\"]", true, &["answer", "now"][..]),
        (".a.b | .[0].c", true, &["a"][..]),
        ("\"\\(.answer)\"", true, &["answer"][..]),
        ("\"\\(\"(\" + .x)\" | .y", true, &["x", "y"][..]),
        ("{changed: true}", false, &[][..]),
        ("\"the .answer\" + $ENV.HOME + env.USER", false, &[][..]),
        ("1.5 + 2", false, &[][..]),
        ("# .answer\n{a: 1}", false, &[][..]),
        ("\"\\\" .x\"", false, &[][..]),
    ] {
        let reads = jq_reads(program);
        assert_eq!(reads.any_path, any, "{program}");
        let want: BTreeSet<String> = fields.iter().map(|f| (*f).to_owned()).collect();
        assert_eq!(reads.fields, want, "{program}");
    }
}

const POST: &str = "invoke: { tool: \"nika:fetch\", args: { url: \
                    \"https://api.example.com/hook\", method: POST, body: \"${{ with.text }}\" } }";

/// An effect sends nothing but the typed answer when its `with:` binds the answer (or a field)
/// and otherwise only constants and secrets, and its verb and guard read nothing else.
#[test]
fn an_effect_that_sends_only_the_typed_answer() {
    let answer = "    with: { text: \"${{ tasks.ask.output }}\" }\n";
    let sends = |body: &str, verb: &str| sends_only_answer(&only_task(body, verb), "ask");
    assert!(sends(answer, POST));
    assert!(sends(
        "    with: { text: \"${{ tasks.ask.output.title }}\" }\n",
        POST
    ));
    assert!(sends(
        "    with: { text: \"${{ tasks.ask.output }}\", key: \"${{ secrets.token }}\" }\n",
        "invoke: { tool: \"nika:fetch\", args: { url: \"${{ const.hook }}\", method: POST, \
         headers: { Authorization: \"Bearer ${{ with.key }}\" }, body: \"${{ with.text }}\" } }",
    ));
    assert!(sends(
        &format!("{answer}    when: \"${{{{ with.text != '' }}}}\"\n"),
        POST
    ));
    assert!(sends(
        answer,
        "exec: { command: [\"echo\", \"${{ with.text }}\"] }"
    ));
    for (why, body, verb) in [
        (
            "another task's output beside the answer",
            "    with: { text: \"${{ tasks.ask.output }}\", draft: \"${{ tasks.draft.output }}\" }\n",
            POST,
        ),
        (
            "another task's output instead of the answer",
            "    with: { text: \"${{ tasks.draft.output }}\" }\n",
            POST,
        ),
        (
            "an answer inside a string",
            "    with: { text: \"answer=${{ tasks.ask.output }}\" }\n",
            POST,
        ),
        (
            "a caller input bound",
            "    with: { text: \"${{ tasks.ask.output }}\", to: \"${{ inputs.to }}\" }\n",
            POST,
        ),
        (
            "a caller input in the arguments",
            answer,
            "invoke: { tool: \"nika:fetch\", args: { url: \"${{ inputs.hook }}\", method: POST, \
             body: \"${{ with.text }}\" } }",
        ),
        (
            "a run fact in the arguments",
            answer,
            "invoke: { tool: \"nika:fetch\", args: { url: \"https://api.example.com/hook\", \
             method: POST, body: \"${{ with.text }} ${{ run.scheduled_for }}\" } }",
        ),
        (
            "an undeclared binding",
            answer,
            "invoke: { tool: \"nika:fetch\", args: { url: \"https://api.example.com/hook\", \
             method: POST, body: \"${{ with.other }}\" } }",
        ),
        (
            "a caller input in the guard",
            "    with: { text: \"${{ tasks.ask.output }}\" }\n    when: \"${{ inputs.go == true \
             }}\"\n",
            POST,
        ),
        (
            "a fan-out",
            "    with: { text: \"${{ tasks.ask.output }}\" }\n    for_each: { items: [1, 2] }\n",
            POST,
        ),
        (
            "a model writes what it sends",
            answer,
            "infer: { prompt: \"${{ with.text }}\", max_tokens: 9 }",
        ),
    ] {
        assert!(!sends(body, verb), "{why}");
    }
    let w = workflow(&format!(
        "nika: t\ntasks:\n  ask:\n    invoke: {{ tool: \"nika:prompt\", args: {{ mode: input, \
         message: \"text?\" }} }}\n  send:\n{answer}    {POST}\n"
    ));
    assert_eq!(names(&w, &answer_sent_by(&w, "ask")), ["send"]);
}
