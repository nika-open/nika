// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

use super::*;
use crate::analyzer::analyze;
use nika_schema::parser::{ParseMode, parse};
use nika_schema::source::FileId;

fn facts(yaml: &str) -> (RawWorkflow, FlowFacts) {
    let wf = parse(yaml, FileId::new(0), ParseMode::Strict).expect("parse");
    let analyzed = analyze(&wf).expect("analyze");
    let facts = analyze_flow(&wf, &analyzed.topo_waves);
    (wf, facts)
}

/// [`facts`] through a chosen sweep bound, with the evaluations the sweeps took.
fn facts_within(yaml: &str, bound: fn(usize, usize) -> u128) -> (RawWorkflow, FlowFacts, usize) {
    let wf = parse(yaml, FileId::new(0), ParseMode::Strict).expect("parse");
    let analyzed = analyze(&wf).expect("analyze");
    let (facts, evaluations) = analyze_flow_within(&wf, &analyzed.topo_waves, bound);
    (wf, facts, evaluations)
}

/// Every unsanctioned effect edge as `(task, trace)`, in task order.
fn leak_rows(wf: &RawWorkflow, f: &FlowFacts) -> Vec<(String, String)> {
    (0..wf.tasks.len())
        .flat_map(|i| {
            f.effect_leaks(i)
                .map(move |trace| (wf.tasks[i].value.id.value.clone(), trace.render()))
        })
        .collect()
}

const S: &str = "secrets:\n  api_key:\n    source: vault\n    key: x\n";

fn idx(wf: &RawWorkflow, id: &str) -> usize {
    wf.tasks
        .iter()
        .position(|t| t.value.id.value == id)
        .expect("task")
}

#[test]
fn direct_secret_into_exec_is_effect_tainted() {
    let y = format!(
        "nika: w\n{S}tasks:\n  t:\n    exec: {{ shell: \"curl -H ${{{{ secrets.api_key }}}}\" }}\n"
    );
    let (wf, f) = facts(&y);
    assert!(f.effect_taint(idx(&wf, "t")).is_some());
}

#[test]
fn plural_sink_judgment_preserves_the_historical_first_trace() {
    let (wf, f) = facts(
        r#"
nika: first-trace
secrets:
  token: { source: env, key: TOKEN }
permits:
  exec: [printf]
tasks:
  send:
    with: { alias: "${{ secrets.token }}" }
    exec:
      command: [printf, "${{ secrets.token }}"]
      stdin: "${{ with.alias }}"
"#,
    );
    let task = idx(&wf, "send");
    assert_eq!(
        f.effect_taint(task).expect("taint").render(),
        "secrets.token"
    );
    assert_eq!(f.effect_leak(task).expect("leak").render(), "secrets.token");
    assert_eq!(f.effect_leaks(task).count(), 1);
}

#[test]
fn first_leak_accessor_falls_through_a_sanctioned_first_edge() {
    let (wf, f) = facts(
        r#"
nika: first-cleared
secrets:
  alpha:
    source: env
    key: ALPHA
    egress: [{ to: exec }]
  omega: { source: env, key: OMEGA }
permits:
  exec: [printf]
tasks:
  send:
    exec:
      command: [printf, "${{ secrets.alpha }}:${{ secrets.omega }}"]
"#,
    );
    let task = idx(&wf, "send");
    assert_eq!(f.effect_taint(task).expect("taint").secret, "alpha");
    assert_eq!(f.effect_leak(task).expect("uncleared edge").secret, "omega");
}

#[test]
fn long_secret_chain_stays_linear_not_quadratic() {
    // DoS regression — Gate-11 finding 2026-06-16. A length-N secret-taint
    // chain (each task reads the prior task's tainted output) gives task k
    // a length-k trace. With the old `Vec<String>` hops cloned 2-3× per
    // task that was O(N²) time + memory — a 0.89 MiB workflow OOM'd at
    // 5.2 GB. The Arc cons-list makes via()/clone() O(1), so this chain
    // settles in milliseconds. The assertions are correctness (the secret
    // propagates the whole way + the trace stays intact) and the work: a
    // forward chain settles in one sweep, one evaluation per task.
    use std::fmt::Write as _;
    const N: usize = 3_000;
    let mut y = format!("nika: w\n{S}tasks:\n");
    y.push_str("  t0:\n    exec: { command: [\"echo\", \"${{ secrets.api_key }}\"] }\n");
    for k in 1..N {
        let p = k - 1;
        write!(
                y,
                "  t{k}:\n    with: {{ prev: \"${{{{ tasks.t{p}.output }}}}\" }}\n    exec: {{ shell: \"echo ${{{{ with.prev }}}}\" }}\n"
            )
            .expect("write to String is infallible");
    }
    let (wf, f, evaluations) = facts_within(&y, sweep_bound);
    assert_eq!(evaluations, N, "one sweep, each task evaluated once");
    let tail = f
        .effect_taint(idx(&wf, &format!("t{}", N - 1)))
        .expect("the secret must propagate through the whole chain");
    assert_eq!(tail.secret, "api_key");
    assert_eq!(
        tail.hops,
        2 * (N - 1),
        "one output and one binding hop per link"
    );
    let rendered = tail.render();
    assert!(
        rendered.starts_with("secrets.api_key"),
        "trace must be source-first: {rendered:.40}"
    );
    assert!(
        rendered.contains("tasks.t0.output"),
        "the first propagation hop must be in the intact trace"
    );
}

/// The worst sweep order: each task recovers to the record of the task declared after it
/// and only the last one sends the secret, so every sweep settles one more record, from the
/// back. Re-evaluating only the tasks whose read record moved keeps that linear: the first
/// sweep evaluates every task, each later one only the reader of the record that moved, so
/// `2N - 1` evaluations in all (a full re-sweep would take `N²`). The secret still reaches
/// the head, through every recover.
#[test]
fn a_backward_recover_chain_settles_in_linear_work() {
    use std::fmt::Write as _;
    const N: usize = 3_000;
    let mut y = format!("nika: w\n{S}tasks:\n");
    for k in 0..N - 1 {
        let next = k + 1;
        write!(
                y,
                "  t{k}:\n    exec: {{ command: [\"echo\", \"{k}\"] }}\n    on_error: {{ recover: \"${{{{ tasks.t{next}.output }}}}\" }}\n"
            )
            .expect("write to String is infallible");
    }
    write!(
        y,
        "  t{}:\n    exec: {{ command: [\"echo\", \"${{{{ secrets.api_key }}}}\"] }}\n",
        N - 1
    )
    .expect("write to String is infallible");
    let (wf, f, evaluations) = facts_within(&y, sweep_bound);
    assert_eq!(
        evaluations,
        2 * N - 1,
        "N in the first sweep, then one per later sweep: only the reader of the record that \
             moved"
    );
    let head = f
        .output_taint
        .get(&idx(&wf, "t0"))
        .and_then(|taints| taints.get("api_key"))
        .expect("the secret reaches the head of the chain");
    assert_eq!(
        head.hops,
        1 + 2 * (N - 1),
        "one recover and one output hop per link"
    );
    let rendered = head.render();
    assert!(
        rendered.starts_with(&format!(
            "secrets.api_key → tasks.t{}.output → on_error.recover @ t{}",
            N - 1,
            N - 2
        )),
        "trace must be source-first: {rendered:.80}"
    );
    assert!(
        rendered.ends_with("on_error.recover @ t0 → tasks.t0.output"),
        "the trace ends at the head of the chain"
    );
}

/// Every `outputs:` entry as `(output, secret, trace)`, the rows `NIKA-SEC-007` reads.
fn egress_rows(f: &FlowFacts) -> Vec<(String, String, String)> {
    crate::secrets::scan_egresses(f)
        .into_iter()
        .map(|e| (e.output, e.secret, e.trace))
        .collect()
}

/// `t0` recovers to the record of `t1`, declared after it, so the flow needs a second sweep.
/// Stopped after one, the facts are incomplete: every task is then judged as if every
/// declared secret reached it, so the send and the `outputs:` entry no `egress:` rule allows
/// refuse with the reason in their trace, and the sends the rule allows stay allowed.
#[test]
fn a_flow_stopped_at_its_bound_fails_closed() {
    const STOPPED: &str = "secrets.api_key → the secret flow stopped at its sweep bound (1) \
                               before settling: judged as if every declared secret reached this \
                               task";
    let y = "nika: w\nsecrets:\n  api_key: { source: env, key: K, egress: [{ to: \"nika:fetch\", host: \"crm.example.com\" }] }\npermits:\n  tools: [\"nika:fetch\"]\n  net: { http: [\"crm.example.com\", \"other.example.com\"] }\ntasks:\n  t0:\n    invoke: { tool: \"nika:fetch\", args: { url: \"https://crm.example.com/a\" } }\n    on_error: { recover: \"${{ tasks.t1.output }}\" }\n  t1:\n    invoke: { tool: \"nika:fetch\", args: { url: \"https://crm.example.com/b\", headers: { authorization: \"Bearer ${{ secrets.api_key }}\" } } }\n  sink:\n    with: { x: \"${{ tasks.t0.output }}\" }\n    invoke: { tool: \"nika:fetch\", args: { url: \"https://other.example.com/in\", method: POST, body: \"${{ with.x }}\" } }\noutputs:\n  r: \"${{ tasks.t0.output }}\"\n";
    let real = "secrets.api_key → tasks.t1.output → on_error.recover @ t0 → tasks.t0.output";
    let (wf, settled, _) = facts_within(y, |_, _| 2);
    assert_eq!(
        leak_rows(&wf, &settled),
        [("sink".to_owned(), format!("{real} → with.x @ sink"))],
        "two sweeps settle it: the real path"
    );
    let real_egress = [(
        "r".to_owned(),
        "api_key".to_owned(),
        format!("{real} → outputs"),
    )];
    assert_eq!(egress_rows(&settled), real_egress);
    let mut report = crate::check(&wf);
    let reported: Vec<_> = report
        .secret_egresses
        .iter()
        .map(|e| (e.output.clone(), e.secret.clone(), e.trace.clone()))
        .collect();
    assert_eq!(reported, real_egress, "the real bound settles it too");

    let (wf, stopped, _) = facts_within(y, |_, _| 1);
    assert_eq!(
        leak_rows(&wf, &stopped),
        [("sink".to_owned(), STOPPED.to_owned())],
        "one sweep leaves work: every task carries every secret, the allowed sends stay allowed"
    );
    assert_eq!(
        egress_rows(&stopped),
        [(
            "r".to_owned(),
            "api_key".to_owned(),
            format!("{STOPPED} → outputs")
        )]
    );
    // The incomplete facts refuse through the report's one voice: the rows the report would
    // hold under the stopped facts, folded as `check` folds them.
    report.secret_leaks = crate::secrets::scan_leaks(&wf, &stopped);
    report.secret_egresses = crate::secrets::scan_egresses(&stopped);
    let refused: Vec<_> = crate::findings::collect(&report)
        .into_iter()
        .filter(|f| matches!(f.kind, "secret_leak" | "secret_egress"))
        .map(|f| (f.kind, f.code, f.task, f.message))
        .collect();
    assert_eq!(refused.len(), 2, "{refused:#?}");
    assert_eq!(
        (
            refused[0].0,
            refused[0].1.as_deref(),
            refused[0].2.as_deref()
        ),
        ("secret_leak", Some("NIKA-SEC-006"), Some("sink"))
    );
    assert!(
        refused[0]
            .3
            .starts_with(&format!("leak into invoke (task `sink`) — {STOPPED} · ")),
        "{}",
        refused[0].3
    );
    assert_eq!(
        refused[1],
        (
            "secret_egress",
            Some("NIKA-SEC-007".to_owned()),
            None,
            format!(
                "EGRESS via outputs.r — {STOPPED} → outputs · fix: remove `outputs.r`; if \
                     exporting this secret is intended, add `{{ to: \"outputs\" }}` to \
                     `secrets.api_key.egress` (keep existing rules)"
            )
        )
    );
}

/// Every route as `(task, trace)`, in task order.
fn route_rows(wf: &RawWorkflow, f: &FlowFacts) -> Vec<(String, String)> {
    (0..wf.tasks.len())
        .flat_map(|i| {
            f.route_taints(i)
                .map(move |(_, trace)| (wf.tasks[i].value.id.value.clone(), trace.render()))
        })
        .collect()
}

/// Stopped at its bound, the flow judges every `model:` that reads a slot, an item, a record
/// or a secret as if every declared secret reached it. A literal model, or one read from an
/// input, can carry none and stays clean; settled, none of the three carries a secret.
#[test]
fn a_flow_stopped_at_its_bound_fails_closed_on_routes() {
    let y = "nika: w\nsecrets:\n  api_key: { source: env, key: K }\ninputs:\n  m: { type: string, required: false, default: mock/echo }\ntasks:\n  lit:\n    infer: { prompt: hi, model: mock/echo, max_tokens: 8 }\n  inp:\n    infer: { prompt: hi, model: \"${{ inputs.m }}\", max_tokens: 8 }\n  slot:\n    with: { m: \"${{ inputs.m }}\" }\n    infer: { prompt: hi, model: \"${{ with.m }}\", max_tokens: 8 }\n";
    let (wf, settled, _) = facts_within(y, sweep_bound);
    assert_eq!(route_rows(&wf, &settled), Vec::<(String, String)>::new());
    let (wf, stopped, _) = facts_within(y, |_, _| 0);
    assert_eq!(
        route_rows(&wf, &stopped),
        [(
            "slot".to_owned(),
            "secrets.api_key → the secret flow stopped at its sweep bound (0) before \
                 settling: judged as if every declared secret reached this task → model @ slot"
                .to_owned()
        )]
    );
}

#[test]
fn the_sweep_bound_cannot_overflow() {
    assert_eq!(sweep_bound(0, 0), 1);
    assert_eq!(sweep_bound(3, 2), 7);
    let max = u128::try_from(usize::MAX).expect("a usize fits in a u128");
    assert_eq!(sweep_bound(usize::MAX, usize::MAX), max * max + 1);
}

#[test]
fn with_aliased_secret_is_traced_to_the_effect() {
    // The false negative the review found: with: { tok: secret } then
    // ${{ with.tok }} into exec — one hop the old substring scan missed.
    let y = format!(
        "nika: w\n{S}tasks:\n  t:\n    with: {{ tok: \"${{{{ secrets.api_key }}}}\" }}\n    exec: {{ shell: \"curl -H ${{{{ with.tok }}}}\" }}\n"
    );
    let (wf, f) = facts(&y);
    let trace = f
        .effect_taint(idx(&wf, "t"))
        .expect("with-aliased taint caught");
    assert_eq!(trace.secret, "api_key");
    assert!(
        trace.render().contains("with.tok"),
        "trace shows the alias: {}",
        trace.render()
    );
}

#[test]
fn taint_propagates_transitively_through_task_output() {
    // a leaks secret into its exec → a.output tainted → b consuming
    // a.output into ITS exec is also tainted (transitive, via the DAG).
    let y = format!(
        "nika: w\n{S}tasks:\n  a:\n    exec: {{ shell: \"echo ${{{{ secrets.api_key }}}}\" }}\n  b:\n    with: {{ upstream: \"${{{{ tasks.a.output }}}}\" }}\n    exec: {{ shell: \"echo ${{{{ with.upstream }}}}\" }}\n"
    );
    let (wf, f) = facts(&y);
    let tb = f.effect_taint(idx(&wf, "b")).expect("transitive taint");
    assert_eq!(tb.secret, "api_key");
    assert!(
        tb.render().contains("tasks.a.output"),
        "trace: {}",
        tb.render()
    );
}

#[test]
fn infer_prompt_is_a_sink_but_does_not_taint_output() {
    // BUG#3: a secret in an infer prompt IS a sink (it leaves the run to
    // a third-party provider) — but the model's RESPONSE is NOT tainted
    // (not a verbatim echo · the OUTPUT carve-out is preserved · ADR-092).
    let y = format!(
        "nika: w\n{S}tasks:\n  a:\n    infer: {{ prompt: \"use ${{{{ secrets.api_key }}}}\", max_tokens: 10 }}\n  b:\n    with: {{ upstream: \"${{{{ tasks.a.output }}}}\" }}\n    exec: {{ shell: \"echo ${{{{ with.upstream }}}}\" }}\n"
    );
    let (wf, f) = facts(&y);
    // The prompt is now a sink (an unsanctioned provider egress · leak).
    assert!(
        f.effect_leak(idx(&wf, "a")).is_some(),
        "infer prompt is a provider-egress sink (no egress → leak)"
    );
    // …but task a's OUTPUT is NOT tainted — so b consuming a.output is clean.
    assert!(
        f.effect_taint(idx(&wf, "b")).is_none(),
        "infer output not tainted → downstream stays clean"
    );
}

#[test]
fn outputs_egress_declass_clears_the_boundary_report() {
    // The api-upload class (night battery 2026-07-10): the capture
    // stays tainted, outputs is where it LEAVES — `to: "outputs"` is
    // the owner's declassification of the workflow boundary itself.
    let y = "\
nika: w
secrets:
  api_key:
    source: vault
    key: x
    egress:
      - to: \"nika:fetch\"
      - to: \"outputs\"
tasks:
  up:
    invoke:
      tool: \"nika:fetch\"
      args: { url: \"https://api.example.com/u\", headers: { x-api-key: \"${{ secrets.api_key }}\" } }
outputs:
  result: \"${{ tasks.up.output }}\"
";
    let (_wf, f) = facts(y);
    assert!(
        f.egresses().is_empty(),
        "the declared boundary declass clears the outputs report"
    );
}

#[test]
fn outputs_egress_stays_default_deny_without_the_rule() {
    // Same workflow, no `to: "outputs"` — the report STANDS.
    let y = "\
nika: w
secrets:
  api_key:
    source: vault
    key: x
    egress:
      - to: \"nika:fetch\"
tasks:
  up:
    invoke:
      tool: \"nika:fetch\"
      args: { url: \"https://api.example.com/u\", headers: { x-api-key: \"${{ secrets.api_key }}\" } }
outputs:
  result: \"${{ tasks.up.output }}\"
";
    let (_wf, f) = facts(y);
    assert_eq!(f.egresses().len(), 1, "default-deny · the report stands");
}

#[test]
fn outputs_egress_never_authorizes_the_send() {
    // `to: "outputs"` ALONE: the boundary is cleared but the SEND to
    // nika:fetch is still an unsanctioned leak (no cross-sink grant).
    let y = "\
nika: w
secrets:
  api_key:
    source: vault
    key: x
    egress:
      - to: \"outputs\"
tasks:
  up:
    invoke:
      tool: \"nika:fetch\"
      args: { url: \"https://api.example.com/u\", headers: { x-api-key: \"${{ secrets.api_key }}\" } }
outputs:
  result: \"${{ tasks.up.output }}\"
";
    let (wf, f) = facts(y);
    assert!(
        f.effect_leak(idx(&wf, "up")).is_some(),
        "the send stays a leak — outputs clears only the boundary"
    );
    assert!(f.egresses().is_empty(), "the boundary itself is cleared");
}

#[test]
fn sanctioned_infer_prompt_is_not_a_leak() {
    // BUG#3: an explicit `to: "infer"` egress sanctions the prompt send.
    let y = "\
nika: w
secrets:
  api_key:
    source: vault
    key: x
    egress:
      - to: \"infer\"
tasks:
  a:
    infer: { prompt: \"use ${{ secrets.api_key }}\", max_tokens: 10 }
";
    let (wf, f) = facts(y);
    assert!(
        f.effect_leak(idx(&wf, "a")).is_none(),
        "a sanctioned `to: infer` egress clears the prompt send"
    );
}

#[test]
fn secret_reaching_outputs_is_an_egress() {
    // a secret leaving the run as the return value — the literal exfil.
    let y = format!(
        "nika: w\n{S}tasks:\n  a:\n    exec: {{ shell: \"echo ${{{{ secrets.api_key }}}}\" }}\noutputs:\n  leaked: ${{{{ tasks.a.output }}}}\n"
    );
    let (_wf, f) = facts(&y);
    let eg = f.egresses();
    assert_eq!(eg.len(), 1);
    assert_eq!(eg[0].0, "leaked");
    assert_eq!(eg[0].1.secret, "api_key");
}

#[test]
fn for_each_item_from_tainted_source_taints_the_body() {
    // for_each over a tainted upstream output → item is tainted → an
    // exec using ${{ item }} re-emits it.
    let y = format!(
        "nika: w\n{S}tasks:\n  a:\n    exec: {{ shell: \"echo ${{{{ secrets.api_key }}}}\" }}\n  b:\n    with: {{ items: \"${{{{ tasks.a.output }}}}\" }}\n    for_each: {{ items: \"${{{{ with.items }}}}\" }}\n    exec: {{ shell: \"echo ${{{{ item }}}}\" }}\n"
    );
    let (wf, f) = facts(&y);
    assert!(
        f.effect_taint(idx(&wf, "b")).is_some(),
        "item taint reaches the effect"
    );
}

#[test]
fn no_secrets_declared_is_empty() {
    let y = "nika: w\ntasks:\n  t:\n    exec: { command: [\"echo\", \"hi\"] }\n";
    let (_wf, f) = facts(y);
    assert!(f.effect_taint(0).is_none());
    assert!(f.egresses().is_empty());
}

#[test]
fn unsanctioned_secret_into_a_provider_call_is_a_leak() {
    // BUG#3: an UNSANCTIONED secret into an infer prompt is a leak (it
    // leaves the run to a third-party provider). The legitimate pattern
    // requires an explicit `to: "infer"` egress (see
    // `sanctioned_infer_prompt_is_not_a_leak`).
    let y = format!(
        "nika: w\n{S}tasks:\n  t:\n    infer: {{ prompt: \"hi ${{{{ secrets.api_key }}}}\", max_tokens: 5 }}\n"
    );
    let (wf, f) = facts(&y);
    let leak = f
        .effect_leak(idx(&wf, "t"))
        .expect("unsanctioned provider send is a leak");
    assert_eq!(leak.secret, "api_key");
    // No workflow `outputs:` egress (the prompt sink is the leak, not
    // a return-value exfiltration).
    assert!(f.egresses().is_empty());
}

#[test]
fn agent_prompt_is_a_sink_too() {
    // BUG#3: the agent verb's prompt is the same provider-egress sink.
    let y = format!(
        "nika: w\n{S}tasks:\n  t:\n    agent: {{ prompt: \"do ${{{{ secrets.api_key }}}}\", max_turns: 2 }}\n"
    );
    let (wf, f) = facts(&y);
    assert!(
        f.effect_leak(idx(&wf, "t")).is_some(),
        "agent prompt is a provider-egress sink"
    );
}

#[test]
fn secret_in_infer_system_field_is_a_sink() {
    // The `system:` field is part of the prompt egress surface too.
    let y = format!(
        "nika: w\n{S}tasks:\n  t:\n    infer: {{ prompt: \"go\", system: \"key ${{{{ secrets.api_key }}}}\", max_tokens: 5 }}\n"
    );
    let (wf, f) = facts(&y);
    assert!(
        f.effect_leak(idx(&wf, "t")).is_some(),
        "a secret in system: is a provider-egress sink"
    );
}

#[test]
fn secret_nested_in_invoke_args_array_is_a_sink() {
    // Kills `collect_json_strings_into` 470 — delete the
    // `Value::Array(items)` arm. The invoke args carry the secret leaf
    // inside a JSON ARRAY (`items: ["${{ secrets.api_key }}"]`). The
    // string collector MUST recurse into the array to surface the leaf →
    // the effect sees `secrets.api_key` → tainted. Deleting the Array arm
    // drops the array's strings, so the secret is invisible and the
    // effect wrongly reads clean.
    let y = format!(
        "nika: w\n{S}tasks:\n  t:\n    invoke: {{ tool: \"mcp:srv/run\", args: {{ items: [\"${{{{ secrets.api_key }}}}\"] }} }}\n"
    );
    let (wf, f) = facts(&y);
    let trace = f
        .effect_taint(idx(&wf, "t"))
        .expect("secret nested in the invoke args ARRAY must be seen by the effect");
    assert_eq!(trace.secret, "api_key");
}

#[test]
fn undeclared_secret_ref_is_not_tainted() {
    // Kills the `Scope::resolve` guard mutant — forcing
    // `declared.contains(name.as_str())` → `true`. The resolver is
    // handed a `secrets.unknown` ref while `declared` lists only
    // `api_key`; the guard must REJECT the undeclared name, so nothing is
    // carried. The forced-true mutant would treat ANY `secrets.<name>` as
    // a live secret and carry `source("unknown")`. (Driven directly
    // because the public `analyze` path rejects an undeclared `secrets.X`
    // ref before the IFC pass ever runs, so the guard cannot be reached
    // end-to-end.)
    let scope = Scope {
        declared: BTreeSet::from(["api_key"]),
        id_of: BTreeMap::new(),
        groups: BTreeMap::new(),
    };
    let no_records = Records::new();

    // Undeclared name → the guard fails → no taint.
    let undeclared = scope.resolve(vec![NamespaceRef::Secrets("unknown".to_owned())]);
    assert_eq!(
        carried(&undeclared, None, None, &no_records),
        Taints::new(),
        "an undeclared secret name must NOT be classified as a live secret"
    );

    // Sanity anchor · the SAME resolver with the DECLARED name DOES taint,
    // proving the test exercises the live guard (not a dead path).
    let declared = scope.resolve(vec![NamespaceRef::Secrets("api_key".to_owned())]);
    let got = carried(&declared, None, None, &no_records);
    let rendered: Vec<(&str, String)> = got
        .iter()
        .map(|(secret, trace)| (secret.as_str(), trace.render()))
        .collect();
    assert_eq!(rendered, [("api_key", "secrets.api_key".to_owned())]);
}

/// E39 10C case F5-P03 (N7), its program verbatim: `egress: [{ to: outputs }]` and an outputs
/// entry that reads the secret itself. The export masks a secret's own value on every surface
/// it writes (`***`), so the boundary declassifies what is derived from a secret, never the
/// value: the report stands, and the whole check refuses NIKA-SEC-007.
#[test]
fn a_secrets_own_value_is_never_declassified_to_outputs() {
    let y = "nika: \"e10c-f5-p03\"\nsecrets:\n  token:\n    source: \"env\"\n    key: \"E10C_TOKEN\"\n    egress:\n      - to: \"outputs\"\npermits:\n  tools:\n    - \"nika:log\"\ntasks:\n  noop:\n    invoke:\n      tool: \"nika:log\"\n      args:\n        message: \"outputs case\"\n        level: \"info\"\noutputs:\n  tok: \"${{ secrets.token }}\"\n";
    let (_wf, f) = facts(y);
    let egress = f.egresses();
    assert_eq!(egress.len(), 1, "the report stands: {egress:?}");
    assert_eq!((egress[0].0, egress[0].1.secret.as_str()), ("tok", "token"));
    let report = crate::check(&parse(y, FileId::new(0), ParseMode::Strict).expect("parse"));
    assert!(!report.is_clean(), "{report:?}");
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.code.as_deref() == Some("NIKA-SEC-007")),
        "{:#?}",
        report.findings
    );
}
