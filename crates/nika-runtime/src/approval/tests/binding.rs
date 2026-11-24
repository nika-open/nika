// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! What a paused answer is bound to (NEP-0013 law 1): the question as shown, the gate as
//! written, and the definition of every task the answer unleashes. Each fixture pauses a run at
//! its gate, changes one thing while it waits, and resumes it with `approve=true`.

use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::json;

use super::*;

/// A confirm gate over one literal write.
const WRITE_WF: &str = r#"nika: bound-write
permits:
  fs: { write: ["./out/approved.txt", "./out/other.txt"] }
  tools: ["nika:prompt", "nika:write"]
tasks:
  approve:
    invoke:
      tool: "nika:prompt"
      args: { mode: "confirm", message: "Write the approved note to out/approved.txt?" }
  write_note:
    with: { go: "${{ tasks.approve.output }}" }
    when: "${{ with.go == true }}"
    invoke:
      tool: "nika:write"
      args: { path: "./out/approved.txt", content: "the approved note" }
"#;

/// A confirm gate over a task that carries no effect class: an inference.
const INFER_WF: &str = r#"nika: bound-infer
model: mock/echo
permits:
  tools: ["nika:prompt"]
tasks:
  approve:
    invoke:
      tool: "nika:prompt"
      args: { mode: "confirm", message: "Send the note for a summary?" }
  summarize:
    with: { go: "${{ tasks.approve.output }}" }
    when: "${{ with.go == true }}"
    infer: { prompt: "Summarize the approved note." }
"#;

/// A confirm gate over a child workflow call.
const CHILD_WF: &str = r#"nika: bound-child
permits:
  tools: ["nika:prompt"]
tasks:
  approve:
    invoke:
      tool: "nika:prompt"
      args: { mode: "confirm", message: "Run the release child?" }
  release:
    with: { go: "${{ tasks.approve.output }}" }
    when: "${{ with.go == true }}"
    invoke:
      workflow: "./child.nika"
      args: { channel: "stable" }
"#;

/// One leg of a run: its outcome, its journal, and every tool call it made.
type Leg = (RunOutcome, VecSink, Vec<ToolCall>);

fn said_yes() -> ToolResult {
    ToolResult::success("tc", "true").with_structured(Value::Bool(true))
}

/// The child closure digest a composer resolved for `./child.nika`.
fn child_at(digest: &str, runs: &Arc<AtomicUsize>) -> Composed {
    Composed {
        closures: BTreeMap::from([("./child.nika".to_owned(), digest.to_owned())]),
        runner: Some(Arc::new(CountingChild(Arc::clone(runs)))),
    }
}

/// A child runner that counts the children it was asked to run.
struct CountingChild(Arc<AtomicUsize>);

impl crate::child::ChildRunner for CountingChild {
    fn run_child<'a>(
        &'a self,
        _call: crate::child::ChildCall,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<crate::child::ChildOutcome, crate::child::ChildRunRefusal>,
                > + 'a,
        >,
    > {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(std::future::ready(Ok(crate::child::ChildOutcome {
            ok: true,
            outputs: BTreeMap::new(),
            cost_usd: None,
            unpriced_calls: 0,
            trace: None,
            failure: None,
            paused: None,
        })))
    }
}

/// The first leg: headless, the run reaches the gate and pauses on it.
async fn pause_at_the_gate(yaml: &str, shell: Vec<&'static str>, composed: Composed) -> VecSink {
    let (outcome, sink, calls) = run_composed(
        yaml,
        Seams {
            shell,
            tool: vec![blocked_prompt()],
            pause: true,
            plan: None,
            answers: BTreeMap::new(),
            paused: None,
        },
        composed,
    )
    .await;
    assert_eq!(
        outcome.paused.as_ref().map(|pause| pause.task.as_str()),
        Some("approve"),
        "the run pauses at its gate"
    );
    assert_eq!(
        calls.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
        ["nika:prompt"],
        "only the question ran"
    );
    sink
}

/// The second leg: `yaml` resumed from the first leg's journal with `approve=true`, holding
/// the ticket `paused` (the one the first leg journaled, unless a fixture forges another).
async fn answer_yes(
    yaml: &str,
    first: &VecSink,
    paused: PausedApproval,
    (shell, tool): (Vec<&'static str>, Vec<ToolResult>),
    composed: Composed,
) -> Leg {
    run_composed(
        yaml,
        Seams {
            shell,
            tool,
            pause: true,
            plan: Some(plan_from(first)),
            answers: BTreeMap::from([("approve".to_owned(), Value::Bool(true))]),
            paused: Some(paused),
        },
        composed,
    )
    .await
}

fn pause_field<'a>(sink: &'a VecSink, key: &str) -> &'a str {
    sink.events()
        .iter()
        .rev()
        .find(|e| e.kind == EventKind::WorkflowPaused)
        .and_then(|e| str_field(e, key))
        .unwrap_or_else(|| panic!("the pause frame carries {key}"))
}

fn started(sink: &VecSink, task: &str) -> bool {
    sink.events()
        .iter()
        .any(|e| e.kind == EventKind::TaskStarted && str_field(e, "task") == Some(task))
}

/// The answer was refused under `why`: the gate fails typed, one engine deny signs the ticket
/// the first leg showed, and nothing behind the gate ran — neither `gated` nor any tool.
fn assert_refused(first: &VecSink, (outcome, sink, calls): &Leg, why: &str, gated: &str) {
    assert!(!outcome.ok, "the refused answer halts the run");
    assert!(outcome.paused.is_none(), "a refusal never re-asks");
    let error = outcome.records["approve"]
        .error
        .as_ref()
        .expect("the typed refusal");
    assert_eq!(error.code, APPROVAL_CODE);
    assert!(error.message.contains(why), "{error:?}");
    let ds = decisions(sink);
    assert_eq!(ds.len(), 1, "{ds:?}");
    let deny = &ds[0];
    assert_eq!(deny.task, "approve");
    assert_eq!(deny.decision, "deny");
    assert_eq!(deny.source, "engine");
    assert_eq!(deny.why.as_deref(), Some(why));
    assert_eq!(deny.answer, None, "nothing was decided");
    assert_eq!(
        deny.digest.as_deref(),
        Some(pause_field(first, "approval_digest")),
        "the deny signs the shown ticket"
    );
    assert!(!started(sink, gated), "`{gated}` never starts");
    assert!(calls.is_empty(), "no tool ran behind the gate: {calls:?}");
}

/// The answer bound: one `allow` whose authority is the paused ticket, over the content the
/// first leg showed.
fn assert_bound(first: &VecSink, (_, sink, _): &Leg) {
    let ds = decisions(sink);
    assert_eq!(ds.len(), 1, "{ds:?}");
    let allow = &ds[0];
    assert_eq!(allow.task, "approve");
    assert_eq!(allow.decision, "allow");
    assert_eq!(allow.source, "resume");
    assert_eq!(allow.why, None);
    assert_eq!(allow.answer.as_deref(), Some("true"));
    assert_eq!(
        allow.shown_hash,
        pause_field(first, "approval_shown_hash"),
        "what the answer signs is what the gate showed"
    );
    assert_eq!(
        allow.digest.as_deref(),
        Some(pause_field(first, "approval_digest"))
    );
}

fn written() -> (Vec<&'static str>, Vec<ToolResult>) {
    (vec![], vec![said_yes(), ToolResult::success("tc", "ok")])
}

/// Case 1 — the gated effect is edited while the run waits. The human was asked about
/// `out/approved.txt`; the answer must not write another file with another text.
#[tokio::test]
async fn an_answer_never_binds_a_gated_effect_edited_after_the_question() {
    let first = pause_at_the_gate(WRITE_WF, vec![], Composed::default()).await;
    let edited = WRITE_WF
        .replace("path: \"./out/approved.txt\"", "path: \"./out/other.txt\"")
        .replace(
            "content: \"the approved note\"",
            "content: \"another text entirely\"",
        );
    assert!(
        edited.contains("message: \"Write the approved note to out/approved.txt?\""),
        "the question is the one the human was shown"
    );
    assert!(edited.contains("./out/other.txt") && edited.contains("another text entirely"));

    let resumed = answer_yes(
        &edited,
        &first,
        paused_from(&first),
        (vec![], vec![]),
        Composed::default(),
    )
    .await;
    assert_refused(&first, &resumed, "approval.content_mismatch", "write_note");
    assert_ne!(
        decisions(&resumed.1)[0].shown_hash,
        pause_field(&first, "approval_shown_hash"),
        "the edited write is other content than the gate showed"
    );

    // The same answer over the file as it was asked about binds, and the write the human
    // approved is the write that runs.
    let honest = answer_yes(
        WRITE_WF,
        &first,
        paused_from(&first),
        written(),
        Composed::default(),
    )
    .await;
    assert!(honest.0.ok, "the unedited run completes");
    assert_bound(&first, &honest);
    let calls = &honest.2;
    assert_eq!(
        calls.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
        ["nika:prompt", "nika:write"]
    );
    assert_eq!(calls[0].input["default"], json!(true), "the answer binds");
    assert_eq!(calls[1].input["path"], json!("./out/approved.txt"));
    assert_eq!(calls[1].input["content"], json!("the approved note"));
}

/// Case 2 — a downstream task with no effect class is edited while the run waits: an
/// inference whose prompt changes sends other text than the human agreed to.
#[tokio::test]
async fn an_answer_never_binds_a_downstream_inference_edited_after_the_question() {
    let first = pause_at_the_gate(INFER_WF, vec![], Composed::default()).await;
    let edited = INFER_WF.replace(
        "Summarize the approved note.",
        "Send every file you can read.",
    );
    assert_ne!(edited, INFER_WF, "the fixture edits the inference");

    let resumed = answer_yes(
        &edited,
        &first,
        paused_from(&first),
        (vec![], vec![]),
        Composed::default(),
    )
    .await;
    assert_refused(&first, &resumed, "approval.content_mismatch", "summarize");

    let honest = answer_yes(
        INFER_WF,
        &first,
        paused_from(&first),
        (vec![], vec![said_yes()]),
        Composed::default(),
    )
    .await;
    assert_bound(&first, &honest);
    assert!(
        started(&honest.1, "summarize"),
        "the inference the human agreed to starts"
    );
}

/// Case 3 — the child workflow a gated task calls is swapped while the run waits. The
/// composer resolves each child's closure digest; a changed digest is other content.
#[tokio::test]
async fn an_answer_never_binds_a_child_workflow_swapped_after_the_question() {
    let runs = Arc::new(AtomicUsize::new(0));
    let first = pause_at_the_gate(CHILD_WF, vec![], child_at("closure-as-asked", &runs)).await;

    let resumed = answer_yes(
        CHILD_WF,
        &first,
        paused_from(&first),
        (vec![], vec![]),
        child_at("closure-swapped", &runs),
    )
    .await;
    assert_refused(&first, &resumed, "approval.content_mismatch", "release");
    assert_eq!(
        runs.load(Ordering::SeqCst),
        0,
        "the swapped child never runs"
    );

    let honest = answer_yes(
        CHILD_WF,
        &first,
        paused_from(&first),
        (vec![], vec![said_yes()]),
        child_at("closure-as-asked", &runs),
    )
    .await;
    assert!(honest.0.ok, "the unswapped child runs to completion");
    assert_bound(&first, &honest);
    assert_eq!(
        runs.load(Ordering::SeqCst),
        1,
        "the child asked about runs once"
    );
}

/// A gated task the engine holds no definition identity for cannot carry a paused answer:
/// here a child call whose closure digest no composer resolved. The question may be asked;
/// the answer is refused rather than bound to a child the ticket could not name.
#[tokio::test]
async fn an_answer_never_binds_a_gated_task_without_a_definition_identity() {
    let runs = Arc::new(AtomicUsize::new(0));
    let undigested = || Composed {
        closures: BTreeMap::new(),
        runner: Some(Arc::new(CountingChild(Arc::clone(&runs)))),
    };
    let first = pause_at_the_gate(CHILD_WF, vec![], undigested()).await;

    let resumed = answer_yes(
        CHILD_WF,
        &first,
        paused_from(&first),
        (vec![], vec![]),
        undigested(),
    )
    .await;
    assert_refused(&first, &resumed, "approval.content_unhashable", "release");
    let error = resumed.0.records["approve"].error.as_ref().expect("typed");
    assert!(
        error.message.contains("`release`"),
        "the refusal names the task it cannot bind: {error:?}"
    );
    assert_eq!(
        runs.load(Ordering::SeqCst),
        0,
        "the unnamed child never runs"
    );
}

/// The gate's own definition is bound, not only its question: an `extract:` that rewrites
/// what downstream reads as the answer changes what the answer means.
#[tokio::test]
async fn an_answer_never_binds_a_gate_whose_own_definition_changed() {
    let gated = WRITE_WF
        .replace(
            "      args: { mode: \"confirm\", message: \"Write the approved note to out/approved.txt?\" }\n",
            "      args: { mode: \"confirm\", message: \"Write the approved note to out/approved.txt?\" }\n    extract: { verdict: \".\" }\n",
        )
        .replace("tasks.approve.output", "tasks.approve.verdict");
    assert!(gated.contains("extract: { verdict: \".\" }"));
    let first = pause_at_the_gate(&gated, vec![], Composed::default()).await;
    // The question is unchanged; the gate now reports `true` whatever was answered.
    let edited = gated.replace(
        "extract: { verdict: \".\" }",
        "extract: { verdict: \"true\" }",
    );
    assert_ne!(edited, gated);

    let resumed = answer_yes(
        &edited,
        &first,
        paused_from(&first),
        (vec![], vec![]),
        Composed::default(),
    )
    .await;
    assert_refused(&first, &resumed, "approval.content_mismatch", "write_note");
}

/// Bound: what the task means. Not bound: how its text is laid out — comments, flow or block
/// style, key order. A reformatted gated task keeps the answer.
#[tokio::test]
async fn a_whitespace_or_comment_edit_of_a_gated_task_keeps_the_answer_bound() {
    const REFORMATTED: &str = r#"nika: bound-write
# a note the operator left while the run waited
permits:
  tools: ["nika:prompt", "nika:write"]
  fs:
    write: ["./out/approved.txt", "./out/other.txt"]
tasks:
  approve:
    invoke:
      tool: "nika:prompt"
      args:
        message: "Write the approved note to out/approved.txt?"
        mode: "confirm"

  write_note:
    # block style, keys in another order: the same task
    when: "${{ with.go == true }}"
    with:
      go: "${{ tasks.approve.output }}"
    invoke:
      args:
        content: "the approved note"   # unchanged
        path: "./out/approved.txt"
      tool: "nika:write"
"#;
    let first = pause_at_the_gate(WRITE_WF, vec![], Composed::default()).await;
    let resumed = answer_yes(
        REFORMATTED,
        &first,
        paused_from(&first),
        written(),
        Composed::default(),
    )
    .await;
    assert!(resumed.0.ok, "the reformatted run completes");
    assert_bound(&first, &resumed);
    assert_eq!(resumed.2[1].input["path"], json!("./out/approved.txt"));
}

/// Bound: the gate and every task downstream of it. Not bound: a task the answer does not
/// unleash. `side` neither reads the gate nor runs after it, so editing it keeps the answer.
#[tokio::test]
async fn an_edit_to_a_task_outside_the_gates_closure_keeps_the_answer_bound() {
    let with_side = |word: &str| {
        format!(
            "{}  side:\n    exec: {{ command: [\"echo\", \"{word}\"] }}\n",
            WRITE_WF.replace(
                "  tools: [\"nika:prompt\", \"nika:write\"]\n",
                "  tools: [\"nika:prompt\", \"nika:write\"]\n  exec: [\"echo\"]\n"
            )
        )
    };
    let first = pause_at_the_gate(&with_side("one"), vec!["one\n"], Composed::default()).await;
    let (_, tool) = written();
    let resumed = answer_yes(
        &with_side("two"),
        &first,
        paused_from(&first),
        (vec!["two\n"], tool),
        Composed::default(),
    )
    .await;
    assert!(resumed.0.ok, "the run completes");
    assert_bound(&first, &resumed);
    assert!(
        started(&resumed.1, "side") && !resumed.0.cache_hits.contains(&"side".to_owned()),
        "the edit was real: `side` ran again instead of being served from the journal"
    );
    assert_eq!(resumed.0.records["side"].output, json!("two"));
}

/// A ticket minted under the previous content recipe — the question, and each effectful
/// descendant by name and class only — carries no definition. The engine cannot tell it from
/// an answer over edited content, so it meets the same law: `approval.content_mismatch`, a
/// halt, never a bind and never a silent re-ask. Its digest still names the paused ticket.
/// Resumed without an answer, the run asks again under a ticket of the current recipe.
#[tokio::test]
async fn an_old_recipe_ticket_halts_with_content_mismatch_and_never_binds() {
    let first = pause_at_the_gate(WRITE_WF, vec![], Composed::default()).await;
    let minted = paused_from(&first);
    let old_content_hash = crate::resume::jcs_blake3_hex(&json!({
        "v": 2,
        "approval": {
            "action": { "tool": "nika:prompt", "verb": "invoke" },
            "choices": [],
            "effects": ["tools"],
            "gated": [{ "classes": ["tools", "write"], "task": "write_note" }],
            "message": "Write the approved note to out/approved.txt?",
            "mode": "confirm",
            "step": "approve",
        },
    }))
    .expect("the old content canonicalizes");
    assert_ne!(old_content_hash, minted.ticket.content_hash);
    let old_ticket = ApprovalTicket::new(
        old_content_hash.clone(),
        minted.ticket.run_nonce.clone(),
        minted.ticket.step.clone(),
        minted.ticket.minted_at_ms,
        minted.ticket.ttl_seconds,
    );
    // The digest the old engine journaled on its pause frame for that ticket.
    let journaled_digest = crate::resume::jcs_blake3_hex(&json!({
        "v": 2,
        "ticket": {
            "content_hash": old_content_hash,
            "run_nonce": old_ticket.run_nonce,
            "step": old_ticket.step,
            "minted_at_ms": old_ticket.minted_at_ms,
            "ttl_seconds": old_ticket.ttl_seconds,
        },
    }))
    .expect("the old ticket canonicalizes");
    let old = PausedApproval::new(old_ticket, minted.trace_nonce.clone());

    let (outcome, sink, calls) = answer_yes(
        WRITE_WF,
        &first,
        old.clone(),
        (vec![], vec![]),
        Composed::default(),
    )
    .await;
    assert!(!outcome.ok, "the old answer halts the run");
    assert!(outcome.paused.is_none(), "it is not re-asked in silence");
    let error = outcome.records["approve"].error.as_ref().expect("typed");
    assert_eq!(error.code, APPROVAL_CODE);
    assert!(
        error.message.contains("approval.content_mismatch"),
        "{error:?}"
    );
    let ds = decisions(&sink);
    assert_eq!(ds.len(), 1, "{ds:?}");
    assert_eq!(ds[0].decision, "deny");
    assert_eq!(ds[0].source, "engine");
    assert_eq!(ds[0].why.as_deref(), Some("approval.content_mismatch"));
    assert_eq!(
        ds[0].digest.as_deref(),
        Some(journaled_digest.as_str()),
        "the deny names the ticket the old engine journaled"
    );
    assert!(
        !started(&sink, "write_note"),
        "the gated write never starts"
    );
    assert!(calls.is_empty(), "{calls:?}");

    let (reasked, sink, _) = run_composed(
        WRITE_WF,
        Seams {
            shell: vec![],
            tool: vec![blocked_prompt()],
            pause: true,
            plan: Some(plan_from(&first)),
            answers: BTreeMap::new(),
            paused: Some(old),
        },
        Composed::default(),
    )
    .await;
    assert_eq!(
        reasked.paused.as_ref().map(|pause| pause.task.as_str()),
        Some("approve"),
        "without an answer the gate asks again"
    );
    assert!(decisions(&sink).is_empty(), "nothing is decided by asking");
    assert_eq!(
        pause_field(&sink, "approval_shown_hash"),
        minted.ticket.content_hash,
        "the new ticket binds the current recipe's content"
    );
}

/// The bound content, key for key (recipe 3): each definition is named by the definition hash
/// the journal carries for that task once it ran.
#[tokio::test]
async fn the_bound_content_is_recipe_three_over_the_journaled_definition_hashes() {
    let first = pause_at_the_gate(WRITE_WF, vec![], Composed::default()).await;
    let honest = answer_yes(
        WRITE_WF,
        &first,
        paused_from(&first),
        written(),
        Composed::default(),
    )
    .await;
    let definition = |task: &str| {
        let def_hash = (honest.1.events().iter())
            .find(|e| e.kind == EventKind::TaskCompleted && str_field(e, "task") == Some(task))
            .and_then(|e| str_field(e, fields::DEF_HASH))
            .unwrap_or_else(|| panic!("`{task}` journals its definition hash"));
        json!({ "def_hash": def_hash, "group": null, "lift": [], "returns": null })
    };
    let expected = crate::resume::jcs_blake3_hex(&json!({
        "v": 3,
        "approval": {
            "action": { "tool": "nika:prompt", "verb": "invoke" },
            "choices": [],
            "definition": definition("approve"),
            "effects": ["tools"],
            "gated": [{
                "classes": ["tools", "write"],
                "definition": definition("write_note"),
                "task": "write_note",
            }],
            "message": "Write the approved note to out/approved.txt?",
            "mode": "confirm",
            "step": "approve",
        },
    }))
    .expect("the content canonicalizes");
    assert_eq!(pause_field(&first, "approval_shown_hash"), expected);
}

/// A closure holds every task the answer unleashes: one with no effect class (an inference)
/// beside one with an effect, a reader reached only through a group fold, and never a task that
/// neither reads the gate nor runs after it.
#[test]
fn a_closure_holds_every_downstream_task_effectful_or_not_and_follows_group_folds() {
    let wf = parsed(
        r#"nika: closure
model: mock/echo
permits:
  exec: ["echo"]
  tools: ["nika:prompt"]
tasks:
  approve:
    invoke:
      tool: "nika:prompt"
      args: { mode: "confirm", message: "Publish the notes?" }
  draft:
    after: { approve: success }
    group: notes
    infer: { prompt: "Draft the note." }
  publish:
    with: { notes: "${{ group.notes }}" }
    exec: { command: ["echo", "published"] }
  aside:
    exec: { command: ["echo", "aside"] }
"#,
    );
    let closures = gated_closures(&wf, &bare_ctx(&wf));
    let closure = &closures["approve"];
    let held: Vec<(&str, Vec<&str>)> = (closure.gated.iter())
        .map(|gated| (gated.task.as_str(), gated.classes.clone()))
        .collect();
    assert_eq!(held, [("draft", vec![]), ("publish", vec!["exec"])]);
    assert_eq!(closure.unbound("approve"), None, "every task is named");
    let draft = closure.gated[0].definition.as_ref().expect("named");
    assert_eq!(draft["group"], json!("notes"), "membership is bound");
    assert_eq!(
        draft["def_hash"].as_str().map(str::len),
        Some(64),
        "a definition is named by its hash, never by its text"
    );
}

/// Not bound: a value a gated task reads from outside its own definition, unless the question
/// shows it. Here a constant — the write still reads `const.target`, so its definition is the
/// one asked about: an edit to the constant keeps the answer bound and moves the write.
#[tokio::test]
async fn a_constant_a_gated_task_reads_is_bound_only_when_the_question_shows_it() {
    const CONST_WF: &str = r#"nika: bound-const
const:
  target: "./out/approved.txt"
permits:
  fs: { write: ["./out/approved.txt", "./out/other.txt"] }
  tools: ["nika:prompt", "nika:write"]
tasks:
  approve:
    invoke:
      tool: "nika:prompt"
      args: { mode: "confirm", message: "Write the approved note?" }
  write_note:
    with: { go: "${{ tasks.approve.output }}" }
    when: "${{ with.go == true }}"
    invoke:
      tool: "nika:write"
      args: { path: "${{ const.target }}", content: "the approved note" }
"#;
    let retarget = |yaml: &str| {
        yaml.replace(
            "target: \"./out/approved.txt\"",
            "target: \"./out/other.txt\"",
        )
    };
    let first = pause_at_the_gate(CONST_WF, vec![], Composed::default()).await;
    let resumed = answer_yes(
        &retarget(CONST_WF),
        &first,
        paused_from(&first),
        written(),
        Composed::default(),
    )
    .await;
    assert_bound(&first, &resumed);
    assert_eq!(
        resumed.2[1].input["path"],
        json!("./out/other.txt"),
        "today the write follows the edited constant under the bound answer"
    );

    // A question that shows the constant binds it: the same edit is other content.
    let shown = CONST_WF.replace(
        "Write the approved note?",
        "Write the approved note to ${{ const.target }}?",
    );
    let first = pause_at_the_gate(&shown, vec![], Composed::default()).await;
    let resumed = answer_yes(
        &retarget(&shown),
        &first,
        paused_from(&first),
        (vec![], vec![]),
        Composed::default(),
    )
    .await;
    assert_refused(&first, &resumed, "approval.content_mismatch", "write_note");
}

/// Two gates in a chain: `approve` decides `write_a` and the second gate `confirm_b`, which
/// decides `write_b`.
const CHAIN_WF: &str = r#"nika: bound-chain
permits:
  fs: { write: ["./out/a.txt", "./out/b.txt", "./out/elsewhere.txt"] }
  tools: ["nika:prompt", "nika:write"]
tasks:
  approve:
    invoke:
      tool: "nika:prompt"
      args: { mode: "confirm", message: "Write a?" }
  write_a:
    with: { go: "${{ tasks.approve.output }}" }
    when: "${{ with.go == true }}"
    invoke:
      tool: "nika:write"
      args: { path: "./out/a.txt", content: "a" }
  confirm_b:
    after: { write_a: success }
    invoke:
      tool: "nika:prompt"
      args: { mode: "confirm", message: "Write b?" }
  write_b:
    with: { go: "${{ tasks.confirm_b.output }}" }
    when: "${{ with.go == true }}"
    invoke:
      tool: "nika:write"
      args: { path: "./out/b.txt", content: "b" }
"#;

/// `write_a` rewritten to another file while the run waits at its second gate.
fn chain_with_write_a_edited() -> String {
    let edited = CHAIN_WF.replace("./out/a.txt\"", "./out/elsewhere.txt\"");
    assert!(edited.contains("path: \"./out/elsewhere.txt\""));
    assert!(
        edited.contains("message: \"Write a?\""),
        "the first question is the one the human answered"
    );
    edited
}

fn wrote() -> ToolResult {
    ToolResult::success("tc", "ok")
}

/// The first two legs of [`CHAIN_WF`]: the run pauses at `approve`, is resumed with `true`,
/// writes `a`, and now waits at `confirm_b`. Returns that second leg's journal.
async fn decided_then_waiting() -> VecSink {
    let first = pause_at_the_gate(CHAIN_WF, vec![], Composed::default()).await;
    let (second, sink, _) = answer_yes(
        CHAIN_WF,
        &first,
        paused_from(&first),
        (vec![], vec![said_yes(), wrote(), blocked_prompt()]),
        Composed::default(),
    )
    .await;
    assert_eq!(
        second.paused.as_ref().map(|pause| pause.task.as_str()),
        Some("confirm_b"),
        "the first answer bound, `write_a` ran, and the run waits at its second gate"
    );
    sink
}

/// A leg of `yaml` resumed over `plan`: `confirm_b=true` answered under the ticket `journal`
/// paused on, when it paused.
async fn resume_chain(
    yaml: &str,
    journal: &VecSink,
    plan: ResumePlan,
    tool: Vec<ToolResult>,
) -> Leg {
    let waiting = journal
        .events()
        .iter()
        .any(|e| e.kind == EventKind::WorkflowPaused);
    run_composed(
        yaml,
        Seams {
            shell: vec![],
            tool,
            pause: true,
            plan: Some(plan),
            answers: if waiting {
                BTreeMap::from([("confirm_b".to_owned(), Value::Bool(true))])
            } else {
                BTreeMap::new()
            },
            paused: waiting.then(|| paused_from(journal)),
        },
        Composed::default(),
    )
    .await
}

/// One string field of the `kind` frame naming `task`.
fn frame_field<'a>(sink: &'a VecSink, kind: EventKind, task: &str, key: &str) -> Option<&'a str> {
    (sink.events().iter())
        .find(|e| e.kind == kind && str_field(e, "task") == Some(task))
        .and_then(|e| str_field(e, key))
}

/// The journaled decision of `approve` was refused under `why`: the gate fails typed under one
/// engine deny that names no ticket (none was asked), the error teaches how to ask again, and
/// nothing behind the gate starts, the second gate included.
fn assert_journaled_refused((outcome, sink, calls): &Leg, why: &str) -> String {
    assert!(!outcome.ok, "the refused decision halts the run");
    assert!(
        outcome.paused.is_none(),
        "a refusal never re-asks in silence"
    );
    assert!(
        !outcome.cache_hits.contains(&"approve".to_owned()),
        "the journal does not serve the gate: {:?}",
        outcome.cache_hits
    );
    let error = outcome.records["approve"]
        .error
        .as_ref()
        .expect("the typed refusal");
    assert_eq!(error.code, APPROVAL_CODE);
    assert!(error.message.starts_with("task 'approve' · "), "{error:?}");
    assert!(error.message.contains(why), "{error:?}");
    assert!(
        error
            .message
            .contains("resume with `--from approve` to ask it again"),
        "the refusal says how to proceed: {error:?}"
    );
    let ds = decisions(sink);
    assert_eq!(ds.len(), 1, "{ds:?}");
    let deny = &ds[0];
    assert_eq!(
        (
            deny.task.as_str(),
            deny.decision.as_str(),
            deny.source.as_str()
        ),
        ("approve", "deny", "engine")
    );
    assert_eq!(deny.why.as_deref(), Some(why));
    assert_eq!(deny.answer, None, "nothing was decided");
    assert_eq!(
        deny.digest, None,
        "no ticket was minted or held for the gate"
    );
    assert_eq!(deny.shown_hash.len(), 64, "the deny names the content now");
    for gated in ["write_a", "confirm_b", "write_b"] {
        assert!(!started(sink, gated), "`{gated}` never starts");
        assert!(
            frame_field(sink, EventKind::TaskCancelled, gated, "task").is_some(),
            "`{gated}` settles cancelled behind the refused gate"
        );
    }
    assert!(calls.is_empty(), "no tool ran: {calls:?}");
    error.message.clone()
}

/// A decision already journaled is bound like a paused answer. The second leg journals, on the
/// first gate's record, the content hash its decision bound; `write_a` is then edited while the
/// run waits at its second gate. Resuming with the second answer no longer serves the first
/// decision: the first gate fails `approval.content_mismatch`, so neither the edited write nor
/// the second gate starts. `--from approve` asks the first gate again, over the edited write.
#[tokio::test]
async fn a_journaled_decision_never_carries_over_to_a_gated_task_edited_after_it() {
    let second = decided_then_waiting().await;
    let journaled = frame_field(
        &second,
        EventKind::TaskCompleted,
        "approve",
        fields::APPROVAL_SHOWN_HASH,
    )
    .expect("the decided gate's record names the content its decision bound");
    let allow = decisions(&second);
    assert_eq!(
        (allow[0].task.as_str(), allow[0].decision.as_str()),
        ("approve", "allow")
    );
    assert_eq!(
        journaled, allow[0].shown_hash,
        "the record names what the decision signed"
    );
    assert_eq!(
        frame_field(
            &second,
            EventKind::TaskCompleted,
            "write_a",
            fields::APPROVAL_SHOWN_HASH
        ),
        None,
        "only a gate's record carries a decision"
    );

    let edited = chain_with_write_a_edited();
    let refused = resume_chain(&edited, &second, plan_from(&second), vec![]).await;
    let message = assert_journaled_refused(&refused, "approval.content_mismatch");
    assert!(
        message.contains(&format!("bound content `{journaled}`")),
        "the refusal names the content the decision bound: {message}"
    );
    assert_ne!(decisions(&refused.1)[0].shown_hash, journaled);

    // `--from approve` drops the gate and every task after it from the plan: here, all of it.
    // The gate runs again and, headless, asks again about the write as it is now.
    let (reasked, sink, calls) = run_composed(
        &edited,
        Seams {
            shell: vec![],
            tool: vec![blocked_prompt()],
            pause: true,
            plan: Some(ResumePlan::new()),
            answers: BTreeMap::new(),
            paused: Some(paused_from(&second)),
        },
        Composed::default(),
    )
    .await;
    assert_eq!(
        reasked.paused.as_ref().map(|pause| pause.task.as_str()),
        Some("approve"),
        "the first gate asks again"
    );
    assert!(decisions(&sink).is_empty(), "nothing is decided by asking");
    assert_ne!(
        pause_field(&sink, "approval_shown_hash"),
        journaled,
        "the new question binds the edited write"
    );
    assert_eq!(
        calls.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
        ["nika:prompt"]
    );
}

/// Over the content it bound, a journaled decision is served as before, and its cache hit
/// carries the content hash on: a trace resumed from that leg binds the decision the same way.
#[tokio::test]
async fn a_journaled_decision_over_unchanged_content_is_served_and_carried_forward() {
    let second = decided_then_waiting().await;
    let journaled = frame_field(
        &second,
        EventKind::TaskCompleted,
        "approve",
        fields::APPROVAL_SHOWN_HASH,
    )
    .expect("the decided gate's record names its content")
    .to_owned();

    let third = resume_chain(
        CHAIN_WF,
        &second,
        plan_from(&second),
        vec![said_yes(), wrote()],
    )
    .await;
    assert!(third.0.ok, "the run completes");
    assert_eq!(third.0.cache_hits, ["approve", "write_a"]);
    let ds = decisions(&third.1);
    assert_eq!(ds.len(), 1, "{ds:?}");
    assert_eq!(
        (
            ds[0].task.as_str(),
            ds[0].decision.as_str(),
            ds[0].source.as_str()
        ),
        ("confirm_b", "allow", "resume")
    );
    assert_eq!(
        third.2.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
        ["nika:prompt", "nika:write"]
    );
    assert_eq!(third.2[1].input["path"], json!("./out/b.txt"));
    assert_eq!(
        frame_field(
            &third.1,
            EventKind::TaskCacheHit,
            "approve",
            fields::APPROVAL_SHOWN_HASH
        ),
        Some(journaled.as_str()),
        "the served gate carries the content its decision bound"
    );

    // Resumed from the leg that served it, the decision is judged against the hash carried on.
    let edited = chain_with_write_a_edited();
    let refused = resume_chain(&edited, &third.1, plan_from(&third.1), vec![]).await;
    assert_journaled_refused(&refused, "approval.content_mismatch");
    let (fourth, sink, calls) = resume_chain(CHAIN_WF, &third.1, plan_from(&third.1), vec![]).await;
    assert!(fourth.ok, "the unedited run completes from its journal");
    assert_eq!(
        fourth.cache_hits,
        ["approve", "write_a", "confirm_b", "write_b"]
    );
    assert!(decisions(&sink).is_empty(), "no gate is decided again");
    assert!(calls.is_empty(), "{calls:?}");
}

/// An earlier engine journaled a decision without the content it bound. Nothing ties that
/// decision to the tasks behind the gate as they are now, so it meets the same law as a
/// paused ticket of an earlier recipe: `approval.content_mismatch`, a halt, never a replay.
#[tokio::test]
async fn a_journaled_decision_without_its_content_hash_halts_with_content_mismatch() {
    let second = decided_then_waiting().await;
    let mut plan = plan_from(&second);
    let prior = plan
        .get_mut("approve")
        .expect("the decided gate is in the plan");
    assert!(prior.approval_shown_hash.is_some());
    prior.approval_shown_hash = None;

    let refused = resume_chain(CHAIN_WF, &second, plan, vec![]).await;
    let message = assert_journaled_refused(&refused, "approval.content_mismatch");
    assert!(
        message.contains("journals no content hash for the decision of this gate"),
        "{message}"
    );
}

/// The judgment, law by law: a journaled decision binds only the content hash the gate binds
/// now, and only over a closure the engine names whole.
#[test]
fn a_journaled_decision_binds_its_own_content_over_a_closure_named_whole() {
    let book = opened(WRITE_WF, "00000000-0000-0000-0000-000000000001");
    assert!(
        (book.journaled_refusal("approve", "confirm", "same", Some("same"))).is_none(),
        "the same content over a named closure binds"
    );
    for (journaled, teach) in [
        (Some("then"), "bound content `then`, not the content `now`"),
        (None, "journals no content hash"),
    ] {
        let refused = (book.journaled_refusal("approve", "confirm", "now", journaled))
            .expect("other content is refused");
        assert_eq!(refused.attestation.why, Some("approval.content_mismatch"));
        assert_eq!(refused.attestation.decision, "deny");
        assert_eq!(refused.attestation.source, "engine");
        assert_eq!(refused.attestation.shown_hash, "now");
        assert_eq!(refused.attestation.digest, None);
        assert!(refused.detail.contains(teach), "{}", refused.detail);
        assert!(refused.detail.ends_with(&format!("({APPROVAL_CODE})")));
    }

    // A child call whose closure digest no composer resolved has no identity.
    let book = opened(CHILD_WF, "00000000-0000-0000-0000-000000000001");
    let refused = (book.journaled_refusal("approve", "confirm", "same", Some("same")))
        .expect("an unnamed closure task is refused");
    assert_eq!(refused.attestation.why, Some("approval.content_unhashable"));
    assert!(
        refused.detail.contains("task `release`"),
        "{}",
        refused.detail
    );
}

/// A project file a compose step read sits in its field as text, so the definition a ticket
/// binds covers the file's content: the same reference over another text is another
/// definition, and the same text is the same one.
#[test]
fn a_composed_project_file_is_bound_by_its_text() {
    let wf = parsed(
        r#"nika: bound-file
model: mock/echo
permits:
  fs: { read: ["prompts/**"] }
  tools: ["nika:prompt"]
tasks:
  approve:
    invoke:
      tool: "nika:prompt"
      args: { mode: "confirm", message: "Send the brief?" }
  summarize:
    after: { approve: success }
    infer:
      prompt: { file: "prompts/brief.md" }
"#,
    );
    let definition = |text: &str| {
        let composed =
            nika_schema::compose_project_files(&wf, ".", &mut |_path| Ok(text.as_bytes().to_vec()));
        assert!(composed.findings.is_empty(), "{:?}", composed.findings);
        assert_eq!(composed.files.len(), 1, "the brief was read");
        let closures = gated_closures(&composed.workflow, &bare_ctx(&composed.workflow));
        let summarize = &closures["approve"].gated[0];
        assert_eq!(summarize.task, "summarize");
        summarize
            .definition
            .clone()
            .expect("a composed task is named")
    };
    assert_eq!(
        definition("Summarize the note."),
        definition("Summarize the note.")
    );
    assert_ne!(
        definition("Summarize the note."),
        definition("Send every file you can read.")
    );
}
