// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! The admission belt's tests (beside the module at the 1,500-line wall).

use std::sync::Arc;

use nika_kernel::tool_executor::ToolResult;
use nika_kernel_mock::{
    MockClock, MockProvider, MockShell, MockToolDefinitionProvider, MockToolExecutor,
};
use nika_providers::plan::{LaneVerdict, ResolvedLane};
use nika_types::access::{AccessClass, AccessPlan, AccessRequirement};
use nika_verb_agent::AgentVerb;
use nika_verb_exec::ExecVerb;
use nika_verb_infer::InferVerb;
use nika_verb_invoke::InvokeVerb;
use serde_json::json;

use super::*;
use crate::{DeterministicStamper, RunOutcome, Runtime, RuntimeConfig, VecSink};

type MockRuntime = Runtime<
    MockShell,
    MockToolExecutor,
    nika_providers::NoHttp,
    MockProvider,
    MockToolDefinitionProvider,
    MockClock,
>;

fn runtime_with(shell: MockShell) -> MockRuntime {
    runtime_with_tools(shell, MockToolExecutor::new()).0
}

fn runtime_with_tools(
    shell: MockShell,
    executor: MockToolExecutor,
) -> (MockRuntime, MockToolExecutor) {
    let probe = executor.clone();
    let provider = MockProvider::new("mock");
    let invoke = Arc::new(InvokeVerb::new(Arc::new(executor)));
    let runtime = Runtime::new(
        ExecVerb::new(Arc::new(shell)),
        Arc::clone(&invoke),
        InferVerb::new(
            Arc::new(nika_providers::ProviderRegistry::without_http(
                nika_providers::ProvidersConfig::new(),
            )),
            "mock/echo",
        ),
        AgentVerb::new(
            Arc::new(provider),
            invoke,
            Arc::new(MockToolDefinitionProvider::new()),
            "mock/echo",
        ),
        MockClock::new(),
        RuntimeConfig::default(),
    );
    (runtime, probe)
}

/// One `nika:image_generate` task. Check's infer envelope is $0
/// (invoke is skipped); the catalog floor is the admission number.
fn image_generate_wf(provider: &str) -> String {
    format!(
        "nika: b24\npermits: {{ tools: [\"nika:image_generate\"], fs: {{ write: [\"./out/**\"] }} }}\ntasks:\n  og:\n    invoke: {{ tool: \"nika:image_generate\", args: {{ provider: {provider}, prompt: \"a monarch butterfly\", output_dir: \"./out\" }} }}\n"
    )
}

fn parse(yaml: &str) -> RawWorkflow {
    nika_schema::parse(
        yaml,
        nika_schema::FileId::new(0),
        nika_schema::ParseMode::Strict,
    )
    .unwrap_or_else(|_| panic!("fixture must parse"))
}

/// One `exec` task inside its declared boundary (clean report) · the
/// `inputs:` block varies per case. NOTHING reads the input — the
/// preflight judges the DECLARATION ⊕ the overrides, never the reads.
fn fixture(inputs: &str) -> String {
    format!(
        "nika: admit\n{inputs}permits: {{ exec: [\"true\"] }}\ntasks:\n  t:\n    exec: {{ command: [\"true\"] }}\n"
    )
}

async fn run(runtime: &MockRuntime, wf: &RawWorkflow) -> RunOutcome {
    let report = nika_check::check(wf);
    assert!(report.is_clean(), "the fixture checks clean");
    let mut stamper = DeterministicStamper::new();
    let mut sink = VecSink::new();
    runtime
        .run(wf, &report, &mut stamper, &mut sink)
        .await
        .unwrap_or_else(|_| panic!("run must settle"))
}

/// A refused run ABORTS at the preflight — the error, for inspection.
/// The refusal precedes the prologue (fail-closed): the sink MUST
/// stay empty (the trust-gate pin, mirrored once here).
async fn run_refused(runtime: &MockRuntime, wf: &RawWorkflow) -> RuntimeError {
    let report = nika_check::check(wf);
    assert!(report.is_clean(), "the fixture checks clean");
    let mut stamper = DeterministicStamper::new();
    let mut sink = VecSink::new();
    let err = runtime
        .run(wf, &report, &mut stamper, &mut sink)
        .await
        .err()
        .unwrap_or_else(|| panic!("a refused run must never reach dispatch"));
    assert!(
        sink.events().is_empty(),
        "refusal must happen before any event, including the prologue"
    );
    err
}

#[test]
fn required_without_default_or_override_is_the_missing_set() {
    let wf = parse(&fixture(
        "inputs:\n  needle: { type: string, required: true }\n  region: { type: string, required: true }\n  limit: { type: integer, default: 3 }\n  note: { type: string }\n",
    ));
    let err = required_inputs_refusal(&wf, &BTreeMap::new()).expect("two unsatisfied");
    let RuntimeError::MissingRequiredInputs { missing, declared } = &err else {
        panic!("expected MissingRequiredInputs");
    };
    // Declaration order — the author's own reading order.
    assert_eq!(missing, &["needle", "region"]);
    assert_eq!(declared, &["needle", "region", "limit", "note"]);
}

#[test]
fn satisfied_cases_pass_the_predicate() {
    let no_override = BTreeMap::new();
    // A declared `default:` satisfies (required or not).
    let wf = parse(&fixture(
        "inputs:\n  needle: { type: string, required: true, default: \"x\" }\n",
    ));
    assert!(required_inputs_refusal(&wf, &no_override).is_none());
    // A `--var` override satisfies a default-less required input.
    let wf = parse(&fixture(
        "inputs:\n  needle: { type: string, required: true }\n",
    ));
    let overrides = BTreeMap::from([("needle".to_owned(), json!("ok"))]);
    assert!(required_inputs_refusal(&wf, &overrides).is_none());
    // A NON-required input without a default is unaffected (an unbound
    // OPTIONAL read stays the read-time NIKA-VAR-001).
    let wf = parse(&fixture("inputs:\n  note: { type: string }\n"));
    assert!(required_inputs_refusal(&wf, &no_override).is_none());
    // No `inputs:` block at all — nothing to refuse.
    let wf = parse(&fixture(""));
    assert!(required_inputs_refusal(&wf, &no_override).is_none());
}

/// (a) The #603 mechanism, refused: a default-less `required: true`
/// input with no override ABORTS the run at admission — before one
/// event, one task, one effect (the mock shell is the spend probe).
#[tokio::test]
async fn a_missing_required_input_is_refused_before_any_event() {
    let wf = parse(&fixture(
        "inputs:\n  needle: { type: string, required: true }\n",
    ));
    let shell = MockShell::new(); // EMPTY — any execution is a bug
    let probe = shell.clone();
    let runtime = runtime_with(shell);
    let err = run_refused(&runtime, &wf).await;
    assert_eq!(err.spec_code(), "NIKA-1708");
    let msg = err.to_string();
    assert!(msg.contains("`needle`"), "the input must be named");
    assert!(
        msg.contains("--var needle=<value>"),
        "the remediation must ride"
    );
    assert!(
        probe.executed_commands().is_empty(),
        "not one task may execute"
    );
}

/// (b) The positive control: a `--var` override on the required input
/// passes admission and the run completes (F4 — the override IS the
/// input's value).
#[tokio::test]
async fn a_var_override_satisfies_the_admission_gate() {
    let wf = parse(&fixture(
        "inputs:\n  needle: { type: string, required: true }\n",
    ));
    let shell = MockShell::new().enqueue_ok("ok");
    let probe = shell.clone();
    let runtime = runtime_with(shell)
        .with_var_overrides(BTreeMap::from([("needle".to_owned(), json!("ok"))]));
    let outcome = run(&runtime, &wf).await;
    assert!(outcome.ok, "the satisfied gate is inert — no false refusal");
    assert_eq!(probe.executed_commands().len(), 1, "the exec ran");
}

/// (c) The author-side satisfaction: a declared `default:` passes
/// admission with NO override.
#[tokio::test]
async fn a_declared_default_satisfies_the_admission_gate() {
    let wf = parse(&fixture(
        "inputs:\n  needle: { type: string, required: true, default: \"x\" }\n",
    ));
    let shell = MockShell::new().enqueue_ok("ok");
    let probe = shell.clone();
    let runtime = runtime_with(shell);
    let outcome = run(&runtime, &wf).await;
    assert!(outcome.ok, "a defaulted required input never refuses");
    assert_eq!(probe.executed_commands().len(), 1, "the exec ran");
}

// ── `--task` scope (descended from the run verb 2026-07-22) ──────

/// `--task` scope · the diamond proves ancestors-only semantics: the
/// target + transitive upstream survive · siblings and downstream drop
/// · outputs clear (they may read unscoped tasks).
#[test]
fn scope_to_task_keeps_the_ancestor_cone() {
    let yaml = "nika: diamond\nmodel: mock/echo\ntasks:\n  discover:\n    invoke: { tool: \"nika:glob\", args: { pattern: \"*.md\" } }\n  stats:\n    with:\n      found: ${{ tasks.discover.output }}\n    infer: { prompt: \"count ${{ with.found }}\" }\n  digest:\n    with:\n      found: ${{ tasks.discover.output }}\n    infer: { prompt: \"sum ${{ with.found }}\" }\n  report:\n    with:\n      stats: ${{ tasks.stats.output }}\n      digest: ${{ tasks.digest.output }}\n    infer: { prompt: \"merge ${{ with.stats }} ${{ with.digest }}\" }\noutputs:\n  all: ${{ tasks.report.output }}\n";
    let wf = parse(yaml);

    let stats_only = scope_to_task(wf.clone(), "stats").expect("stats scopes");
    let ids: Vec<&str> = stats_only
        .tasks
        .iter()
        .map(|t| t.value.id.value.as_str())
        .collect();
    assert_eq!(
        ids,
        vec!["discover", "stats"],
        "target + its one ancestor · document order"
    );
    assert!(stats_only.outputs.is_empty(), "outputs drop under scope");

    let full = scope_to_task(wf.clone(), "report").expect("report scopes");
    assert_eq!(full.tasks.len(), 4, "the sink's cone is the whole diamond");

    let err = scope_to_task(wf, "nope")
        .err()
        .unwrap_or_else(|| panic!("unknown task id must be refused"));
    assert!(
        err.contains("nope") && err.contains("discover"),
        "names the id + the available set"
    );
}

/// The scoped sub-workflow re-checks CLEAN — the plan/waves/cost the
/// run renders describe exactly the cone, not the original file.
#[test]
fn scoped_workflow_rechecks_clean() {
    let yaml = "nika: pair\nmodel: mock/echo\ntasks:\n  a:\n    infer: { prompt: \"hi\" }\n  b:\n    with:\n      prev: ${{ tasks.a.output }}\n    infer: { prompt: \"use ${{ with.prev }}\" }\n";
    let wf = parse(yaml);
    let sub = scope_to_task(wf, "a").expect("a scopes");
    let report = nika_check::check(&sub);
    assert!(
        report.is_clean(),
        "the cone stands alone (no dangling refs)"
    );
    assert_eq!(sub.tasks.len(), 1);
}

// ── the budget floor (descended from the run verb 2026-07-22) ────

#[test]
fn floor_above_budget_refuses_with_both_numbers() {
    let msg = floor_refusal(0.000_019, 0.000_001).expect("refuses");
    assert!(msg.contains("$0.000019"), "floor must ride");
    assert!(msg.contains("$0.000001"), "budget must ride");
    assert!(msg.contains("refusing to start"));
    assert!(msg.contains("nika check"), "must point at the envelope");
}

#[test]
fn floor_at_or_under_budget_passes() {
    // Spending exactly the budget is not over it (mirrors the
    // ledger's crossing semantics).
    assert!(floor_refusal(0.05, 0.05).is_none());
    assert!(floor_refusal(0.0, 0.05).is_none());
    assert!(floor_refusal(0.0, 0.0).is_none());
}

/// The admission form (NIKA-1709 · the 2026-07-29 composition bypass
/// closed): a run launched under a budget its floor already crosses is
/// aborted by the LAUNCH GATES — before the prologue, for EVERY
/// embedder (the composed child included). `None` budget never fires,
/// and a floor at/under the budget admits.
#[test]
fn budget_floor_refusal_is_the_embedders_gate() {
    let wf = parse(
        "nika: m\ntasks:\n  \
         a:\n    infer: { prompt: hi, max_tokens: 1000000, model: \"anthropic/claude-sonnet-5\" }\n",
    );
    let report = nika_check::check(&wf);
    assert!(
        report.cost.min_path_total_usd > 0.000_001,
        "the fixture's floor dwarfs the budget"
    );
    let err = budget_floor_refusal(&wf, &report, Some(0.000_001), None)
        .expect("a floor above the budget refuses at admission");
    assert!(
        matches!(err, RuntimeError::BudgetFloor { .. }),
        "must use the launch-abort variant"
    );
    assert!(err.to_string().starts_with("NIKA-1709"));
    assert!(err.to_string().contains("refusing to start"));
    // The sparing arms: no budget · a floor that fits.
    assert!(budget_floor_refusal(&wf, &report, None, None).is_none());
    assert!(budget_floor_refusal(&wf, &report, Some(999.0), None).is_none());
}

/// The gate prices the EFFECTIVE model (#342's law at the admission
/// layer): a file on mock overridden to a priced model refuses; a
/// priced file overridden to mock passes.
#[test]
fn the_gate_prices_the_model_the_run_will_use() {
    let yaml = "nika: m\nmodel: \"mock/echo\"\ntasks:\n  \
         a:\n    infer: { prompt: hi, max_tokens: 1000000 }\n";
    let wf = parse(yaml);
    let report = nika_check::check(&wf);
    assert_eq!(
        report.cost.min_path_total_usd, 0.0,
        "the file's floor is zero"
    );
    let err = budget_floor_refusal(
        &wf,
        &report,
        Some(0.000_001),
        Some("anthropic/claude-sonnet-5"),
    );
    assert!(
        matches!(err, Some(RuntimeError::BudgetFloor { .. })),
        "the overridden effective model's floor must trip the gate"
    );

    let yaml = "nika: m\nmodel: \"anthropic/claude-sonnet-5\"\ntasks:\n  \
         a:\n    infer: { prompt: hi, max_tokens: 1000000 }\n";
    let wf = parse(yaml);
    let report = nika_check::check(&wf);
    assert!(
        budget_floor_refusal(&wf, &report, Some(0.000_001), Some("mock/echo")).is_none(),
        "the effective mock floor is zero — the file's priced floor never fires"
    );
}

/// B11 · B1: the launch floor counts a `for_each` over an input from the
/// value the invocation binds, as the run binds it (a bound value replaces
/// the declared default). Five items bound over a one-item default refuse
/// where the default alone fits; one item bound over a five-item default
/// fits where the default alone refuses; a bound value that is not an array
/// is an unknown count and never falls back to the default's five.
#[test]
fn the_floor_counts_the_fan_the_invocation_binds() {
    let fan = |default: &str| {
        parse(&format!(
            "nika: fan\ninputs:\n  xs: {{ type: {{ array: string }}, required: false, default: {default} }}\ntasks:\n  ask:\n    for_each: {{ items: \"${{{{ inputs.xs }}}}\" }}\n    infer: {{ prompt: hi, max_tokens: 512, model: \"deepseek/deepseek-v4-pro\" }}\n"
        ))
    };
    let one = fan("[\"a\"]");
    let five = fan("[\"a\", \"b\", \"c\", \"d\", \"e\"]");
    let per_call = nika_check::check(&one).cost.min_path_total_usd;
    assert!(per_call > 0.0, "the fixture's call is priced");
    let floor = |wf: &RawWorkflow, bound: &BTreeMap<String, Value>| {
        let report = nika_check::check(wf);
        budget_floor_refusal_bound(wf, &report, Some(3.0 * per_call), None, bound, false)
    };
    let bind = |value: Value| BTreeMap::from([("xs".to_owned(), value)]);
    let unbound = BTreeMap::new();
    assert!(floor(&one, &unbound).is_none(), "the one-item default fits");
    assert!(
        floor(&five, &unbound).is_some(),
        "the five-item default does not"
    );
    assert!(
        floor(&one, &bind(json!(["a", "b", "c", "d", "e"]))).is_some(),
        "five items bound over a one-item default refuse"
    );
    assert!(
        floor(&five, &bind(json!(["a"]))).is_none(),
        "one item bound over a five-item default fits"
    );
    assert!(
        floor(&five, &bind(json!("a"))).is_none(),
        "a bound non-array is an unknown count, never the default's five"
    );
}

/// B24 / issue 1296: check's envelope skips `invoke:`, so a priced
/// `nika:image_generate` (xAI workhorse $0.02) used to launch, hit
/// HTTP, then abort NIKA-1704. The admission floor must refuse
/// BEFORE the executor is called (NIKA-1709 · zero events · zero spend).
#[test]
fn priced_image_builtin_floor_exceeds_a_tiny_cap() {
    let wf = parse(&image_generate_wf("xai"));
    let report = nika_check::check(&wf);
    assert!(report.is_clean(), "the fixture must check clean");
    assert_eq!(
        report.cost.min_path_total_usd, 0.0,
        "check still skips invoke — the hole this gate closes"
    );
    let err = budget_floor_refusal(&wf, &report, Some(0.001), None)
        .expect("a $0.02 builtin floor refuses a $0.001 cap");
    assert_eq!(err.spec_code(), "NIKA-1709");
    let msg = err.to_string();
    assert!(msg.contains("refusing to start"));
    assert!(msg.contains("$0.020000"), "catalog floor must ride");
    assert!(msg.contains("$0.001000"), "cap must ride");
    assert!(
        !msg.contains("spent $"),
        "preflight must not report post-spend state"
    );
    assert!(budget_floor_refusal(&wf, &report, Some(1.00), None).is_none());
    assert!(budget_floor_refusal(&wf, &report, None, None).is_none());
}

/// B20 / issue 1297: an unpriced CLOUD seat under `--max-cost-usd`
/// must refuse before the prologue. The canary is a gemini id the
/// snapshot does not price — mock/local stay the sparing arms.
fn unpriced_cloud_wf() -> String {
    "nika: b20\nmodel: gemini/nika-b20-unpriced-canary\npermits: {}\ntasks:\n  ping:\n    infer: { prompt: \"PONG\", max_tokens: 16 }\n"
        .to_owned()
}

#[test]
fn unpriced_cloud_plus_cap_refuses_to_start() {
    let wf = parse(&unpriced_cloud_wf());
    let report = nika_check::check(&wf);
    assert!(
        report.is_clean(),
        "the canary must check clean so admission owns the refusal"
    );
    let ep = report
        .data_journey
        .model_endpoints
        .iter()
        .find(|e| e.task == "ping")
        .expect("canary endpoint");
    assert!(!ep.priced, "canary must stay unpriced");
    assert_eq!(ep.locus, nika_check::EndpointLocus::Cloud);
    let err = budget_floor_refusal(&wf, &report, Some(0.01), None)
        .expect("unpriced cloud + cap 0.01 must refuse");
    assert_eq!(err.spec_code(), "NIKA-1709");
    let msg = err.to_string();
    assert!(msg.contains("refusing to start"));
    assert!(msg.contains("unpriced"));
    assert!(msg.contains("nika-b20-unpriced-canary"));
    assert!(msg.contains("0.010000"), "cap must ride");
    assert!(
        budget_floor_refusal(&wf, &report, None, None).is_none(),
        "no cap → unpriced cloud may still start"
    );
}

const CANARY: &str = "gemini/nika-b20-unpriced-canary";
/// A second id no catalog row prices, for a model the plan carries no lane for.
const UNLANED_CANARY: &str = "gemini/nika-seat-unlaned-canary";

/// One admitted lane of a frozen plan: the model, its provider, the access that carries it and
/// that access's class.
type Lane<'a> = (&'a str, &'a str, &'a str, AccessClass);

const SEATED: Lane<'static> = (CANARY, "gemini", "gemini-cli", AccessClass::Harness);
const KEYED: Lane<'static> = (CANARY, "gemini", "gemini", AccessClass::Api);

/// A frozen plan that admits `lanes`, as the resolver leaves it with no pin: its seat is the
/// first harness lane's access and serves the lanes that chose it.
fn plan_of(lanes: &[Lane<'_>]) -> ExecutionAccessPlan {
    let seat = lanes
        .iter()
        .find(|lane| lane.3 == AccessClass::Harness)
        .map(|lane| lane.2.to_owned());
    let lanes = lanes
        .iter()
        .map(|&(model, provider, access, chosen)| {
            let lane = AccessPlan::new(
                model,
                provider,
                access,
                chosen,
                chosen.default_billing(),
                false,
                Vec::new(),
            );
            (
                model.to_owned(),
                LaneVerdict::Admitted(ResolvedLane::new(lane, 1)),
            )
        })
        .collect();
    ExecutionAccessPlan::new(lanes, None, seat, None)
}

/// The launch gates over `wf` under a cap of one cent, with `plan` as the run's frozen plan
/// and the declared defaults as the only bindings.
fn launch_under_a_cap(
    wf: &RawWorkflow,
    plan: Option<&ExecutionAccessPlan>,
) -> Result<(), RuntimeError> {
    let report = nika_check::check(wf);
    assert!(report.is_clean(), "the fixture checks clean: {report:?}");
    gates(
        wf,
        &report,
        &BTreeMap::new(),
        Some(0.01),
        None,
        (None, &[], plan),
    )
}

/// The cap's refusal of an unpriced cloud model: NIKA-1709 naming `model` and the cap.
fn assert_unpriced_refusal(result: Result<(), RuntimeError>, model: &str) {
    let err = result.expect_err("an unpriced cloud model off a seat refuses under a cap");
    assert_eq!(err.spec_code(), "NIKA-1709");
    let msg = err.to_string();
    assert!(msg.contains("refusing to start: cloud model"), "{msg}");
    assert!(msg.contains(model), "{msg}");
    assert!(msg.contains("is unpriced"), "{msg}");
    assert!(msg.contains("$0.010000"), "the cap rides: {msg}");
}

/// The canary on a seat beside a task whose model a declared default decides: the plan holds
/// no lane for a templated model, so the router sends that call to the provider path.
fn seated_beside_a_bound_model_wf() -> String {
    format!(
        "nika: seated-beside-bound\ninputs:\n  m: {{ type: string, default: \"{UNLANED_CANARY}\" }}\npermits: {{}}\ntasks:\n  ping:\n    infer: {{ model: \"{CANARY}\", prompt: \"PONG\", max_tokens: 16 }}\n  ask:\n    infer: {{ model: \"${{{{ inputs.m }}}}\", prompt: \"PONG\", max_tokens: 16 }}\n"
    )
}

/// The launch gate reads the answer the host's preflight reads. A run whose model rides a
/// harness seat starts under a cap although the catalog does not price that model: the seat's
/// subscription bounds it. The same file and cap on an API lane, or with no plan, still
/// refuse. The launch used to judge every run as unseated, so it refused the run its own
/// preflight had just admitted.
#[test]
fn a_run_seated_on_a_harness_starts_under_a_cap_with_an_unpriced_cloud_model() {
    let wf = parse(&unpriced_cloud_wf());
    let seated = launch_under_a_cap(&wf, Some(&plan_of(&[SEATED])));
    assert!(
        seated.is_ok(),
        "seated on a harness, the cap does not refuse an unpriced cloud model: {seated:?}"
    );
    assert_unpriced_refusal(launch_under_a_cap(&wf, Some(&plan_of(&[KEYED]))), CANARY);
    assert_unpriced_refusal(launch_under_a_cap(&wf, None), CANARY);
}

/// Seated is the router's word, model by model. A plan whose only lane is a harness lane does
/// not seat a model it holds no lane for: that call rides the provider path and bills a key,
/// so the cap still refuses it. Here a declared default decides the second task's model.
#[test]
fn a_model_the_plan_holds_no_lane_for_unseats_the_run() {
    let wf = parse(&seated_beside_a_bound_model_wf());
    let plan = plan_of(&[SEATED]);
    assert!(
        !seated_on_harness(&wf, (None, &BTreeMap::new()), &plan),
        "the bound model has no lane, so no seat serves it"
    );
    assert_unpriced_refusal(launch_under_a_cap(&wf, Some(&plan)), UNLANED_CANARY);
}

/// A pinned seat serves every model, a bound one included: the same file then starts.
#[test]
fn a_pinned_seat_serves_a_model_only_a_binding_names() {
    let wf = parse(&seated_beside_a_bound_model_wf());
    let pinned = ExecutionAccessPlan::new(
        BTreeMap::new(),
        Some("codex".to_owned()),
        Some("codex".to_owned()),
        None,
    );
    assert!(seated_on_harness(&wf, (None, &BTreeMap::new()), &pinned));
    let launch = launch_under_a_cap(&wf, Some(&pinned));
    assert!(
        launch.is_ok(),
        "a pinned seat bounds both models: {launch:?}"
    );

    // A route the file declares (`run.access.via`) pins the seat as the operator's flag does.
    let declared = ExecutionAccessPlan::new(BTreeMap::new(), None, Some("codex".to_owned()), None)
        .with_requirement(Some(
            AccessRequirement::new().with_via(Some("codex".to_owned())),
        ));
    assert!(seated_on_harness(&wf, (None, &BTreeMap::new()), &declared));
    let launch = launch_under_a_cap(&wf, Some(&declared));
    assert!(
        launch.is_ok(),
        "a declared seat bounds both models: {launch:?}"
    );
}

/// The answer over each shape of run: seated only when every model the launch resolves rides
/// a seat. A lane the run does not use changes nothing; one model on a key unseats the run; a
/// run that names no model is not seated.
#[test]
fn a_run_is_seated_only_when_a_seat_serves_every_model_it_names() {
    const PRICED: &str = "openai/gpt-4.1-mini";
    let other_key: Lane<'_> = (PRICED, "openai", "openai", AccessClass::Api);
    let one = parse(&unpriced_cloud_wf());
    let two = parse(&format!(
        "nika: two-models\npermits: {{}}\ntasks:\n  ping:\n    infer: {{ model: \"{CANARY}\", prompt: \"PONG\", max_tokens: 16 }}\n  pong:\n    infer: {{ model: \"{PRICED}\", prompt: \"PONG\", max_tokens: 16 }}\n"
    ));
    let none = parse(&fixture(""));
    let none_bound = BTreeMap::new();
    let seated = |wf: &RawWorkflow, lanes: &[Lane<'_>]| {
        seated_on_harness(wf, (None, &none_bound), &plan_of(lanes))
    };

    assert!(seated(&one, &[SEATED]));
    assert!(
        seated(&one, &[SEATED, other_key]),
        "a lane the run does not use changes nothing"
    );
    assert!(!seated(&one, &[KEYED]));
    assert!(!seated(&one, &[]), "no lane, no seat");
    assert!(
        !seated(&two, &[SEATED, other_key]),
        "one model on a key unseats the run"
    );
    assert!(
        !seated(&none, &[SEATED]),
        "a run that names no model is not seated"
    );
    let agent = parse(&format!(
        "nika: one-agent\npermits: {{}}\ntasks:\n  work:\n    agent: {{ model: \"{CANARY}\", prompt: \"PONG\" }}\n"
    ));
    assert!(
        seated(&agent, &[SEATED]),
        "an agent task's model is judged like an infer task's"
    );
    assert!(!seated(&agent, &[KEYED]));
    assert!(
        seated_on_harness(&one, (Some(CANARY), &none_bound), &plan_of(&[SEATED])),
        "the operator's model is judged like the file's"
    );
    assert!(
        !seated_on_harness(&one, (Some(PRICED), &none_bound), &plan_of(&[SEATED])),
        "the operator's model replaces the envelope's, and no seat serves it"
    );
}

/// A run that mixes a seated model with a model on a key keeps the whole cap: the unpriced
/// model is refused although it rides the seat. The cap has one answer for the run.
#[test]
fn a_run_with_one_model_on_a_key_keeps_the_whole_cap() {
    let wf = parse(&format!(
        "nika: two-models\npermits: {{}}\ntasks:\n  ping:\n    infer: {{ model: \"{CANARY}\", prompt: \"PONG\", max_tokens: 16 }}\n  pong:\n    infer: {{ model: \"openai/gpt-4.1-mini\", prompt: \"PONG\", max_tokens: 16 }}\n"
    ));
    let plan = plan_of(&[
        SEATED,
        ("openai/gpt-4.1-mini", "openai", "openai", AccessClass::Api),
    ]);
    assert_unpriced_refusal(launch_under_a_cap(&wf, Some(&plan)), CANARY);
}

/// Seated on a harness, the stronger arm still judges: an id this binary cannot resolve
/// refuses under a cap. A bare id has no lane of its own: only a pinned or declared seat
/// serves it.
#[test]
fn a_seated_run_under_a_cap_still_refuses_an_id_the_binary_cannot_resolve() {
    let wf = parse(
        "nika: seated-unresolvable\nmodel: \"claude-opus-4.1\"\npermits: {}\ntasks:\n  ping:\n    infer: { prompt: \"PONG\", max_tokens: 16 }\n",
    );
    let plan = ExecutionAccessPlan::new(
        BTreeMap::new(),
        Some("codex".to_owned()),
        Some("codex".to_owned()),
        None,
    );
    assert!(seated_on_harness(&wf, (None, &BTreeMap::new()), &plan));
    let err = launch_under_a_cap(&wf, Some(&plan))
        .expect_err("an id no resolver names is not the subscription's to bound");
    assert_eq!(err.spec_code(), "NIKA-1709");
    let msg = err.to_string();
    assert!(msg.contains("`claude-opus-4.1`"), "{msg}");
    assert!(
        msg.contains("cannot bound a model this binary cannot resolve"),
        "{msg}"
    );
}

/// Seated on a harness, the cap still meters what bills a provider key: a priced image above
/// the cap refuses with its floor, and an image no catalog row prices refuses as unknown
/// spend.
#[test]
fn a_seated_run_under_a_cap_still_refuses_the_media_it_cannot_afford_or_price() {
    let with_image = |args: &str| {
        parse(&format!(
            "nika: seated-media\nmodel: \"{CANARY}\"\npermits: {{ tools: [\"nika:image_generate\"], fs: {{ write: [\"./out/**\"] }} }}\ntasks:\n  ping:\n    infer: {{ prompt: \"PONG\", max_tokens: 16 }}\n  og:\n    invoke: {{ tool: \"nika:image_generate\", args: {{ {args}, prompt: \"a monarch butterfly\", output_dir: \"./out\" }} }}\n"
        ))
    };
    let plan = plan_of(&[SEATED]);

    let priced = with_image("provider: xai");
    assert!(seated_on_harness(&priced, (None, &BTreeMap::new()), &plan));
    let err = launch_under_a_cap(&priced, Some(&plan))
        .expect_err("a $0.02 image floor refuses a $0.01 cap on any access");
    assert_eq!(err.spec_code(), "NIKA-1709");
    let msg = err.to_string();
    assert!(msg.contains("$0.020000"), "the catalog floor rides: {msg}");
    assert!(msg.contains("$0.010000"), "the cap rides: {msg}");

    let unpriced = with_image("model: gpt-image-1.5");
    let err = launch_under_a_cap(&unpriced, Some(&plan))
        .expect_err("the seat never bounds a paid media call");
    assert_eq!(err.spec_code(), "NIKA-1709");
    let msg = err.to_string();
    assert!(msg.contains("no catalog price for task `og`"), "{msg}");
}

/// A run with no frozen plan is not seated for the cap, whatever seat it declares: a seat id
/// alone does not prove a seat is attached for every call, so the cap still refuses.
#[test]
fn a_declared_seat_without_a_plan_does_not_seat_the_cap() {
    let wf = parse(&unpriced_cloud_wf());
    let report = nika_check::check(&wf);
    for planless_harness in [false, true] {
        let launch = gates_with_transport(
            &wf,
            &report,
            &BTreeMap::new(),
            (Some(0.01), &BTreeSet::new()),
            None,
            (None, &[], None),
            planless_harness,
        );
        assert_unpriced_refusal(launch, CANARY);
    }
}

#[test]
fn mock_plus_cap_still_admits() {
    let wf = parse(
        "nika: b20-mock\nmodel: mock/echo\npermits: {}\ntasks:\n  ping:\n    infer: { prompt: \"PONG\", max_tokens: 16 }\n",
    );
    let report = nika_check::check(&wf);
    assert!(report.is_clean(), "mock rehearsal must check clean");
    assert!(
        budget_floor_refusal(&wf, &report, Some(0.01), None).is_none(),
        "mock is a proven zero — a cap must not refuse the rehearsal"
    );
    assert!(budget_floor_refusal(&wf, &report, None, None).is_none());
}

#[test]
fn local_unpriced_without_cap_still_admits() {
    let wf = parse(
        "nika: b20-local\nmodel: ollama/llama3\npermits: {}\ntasks:\n  ping:\n    infer: { prompt: \"PONG\", max_tokens: 16 }\n",
    );
    let report = nika_check::check(&wf);
    let ep = report
        .data_journey
        .model_endpoints
        .iter()
        .find(|e| e.task == "ping")
        .expect("local endpoint");
    assert!(!ep.priced, "local must stay unpriced-not-free");
    assert_eq!(ep.locus, nika_check::EndpointLocus::Local);
    assert!(
        budget_floor_refusal(&wf, &report, None, None).is_none(),
        "local unpriced without a cap still runs"
    );
    assert!(
        budget_floor_refusal(&wf, &report, Some(0.01), None).is_none(),
        "a cap on local watts is not a cloud-price bound"
    );
}

#[test]
fn priced_gemini_flash_under_a_generous_cap_admits() {
    let wf = parse(
        "nika: b20-flash\nmodel: gemini/gemini-2.5-flash\npermits: {}\ntasks:\n  ping:\n    infer: { prompt: \"PONG\", max_tokens: 256 }\n",
    );
    let report = nika_check::check(&wf);
    let ep = report
        .data_journey
        .model_endpoints
        .iter()
        .find(|e| e.task == "ping")
        .expect("flash endpoint");
    assert!(ep.priced, "flash must be the snapshot row");
    assert!(
        budget_floor_refusal(&wf, &report, Some(0.20), None).is_none(),
        "priced flash under $0.20 must start"
    );
}

#[tokio::test]
async fn unpriced_cloud_under_cap_refuses_before_any_infer() {
    let wf = parse(&unpriced_cloud_wf());
    let runtime = runtime_with(MockShell::new()).with_max_cost_usd(Some(0.01));
    let err = run_refused(&runtime, &wf).await;
    assert_eq!(err.spec_code(), "NIKA-1709");
    let msg = err.to_string();
    assert!(msg.contains("refusing to start"));
    assert!(msg.contains("unpriced"));
}

#[test]
fn mock_image_builtin_has_no_static_floor() {
    let wf = parse(&image_generate_wf("mock"));
    let report = nika_check::check(&wf);
    assert!(report.is_clean(), "mock image fixture must check clean");
    assert!(
        budget_floor_refusal(&wf, &report, Some(0.001), None).is_none(),
        "mock/local image is unpriced — a tight cap must not refuse the rehearsal"
    );
}

/// Mutation pair with `priced_image_builtin_floor_exceeds_a_tiny_cap`:
/// cap 0.001 refuses before any tool call; cap 1.00 lets the stub through.
#[tokio::test]
async fn priced_image_builtin_under_tiny_cap_refuses_before_the_executor() {
    let wf = parse(&image_generate_wf("xai"));
    let executor = MockToolExecutor::new();
    let (runtime, probe) = runtime_with_tools(MockShell::new(), executor);
    let runtime = runtime.with_max_cost_usd(Some(0.001));
    let err = run_refused(&runtime, &wf).await;
    assert_eq!(err.spec_code(), "NIKA-1709");
    assert!(
        probe.captured_calls().is_empty(),
        "no executor call may be recorded"
    );
    let msg = err.to_string();
    assert!(!msg.contains("cost_usd"));
    assert!(!msg.contains("spent $0.02"));
}

#[tokio::test]
async fn priced_image_builtin_under_generous_cap_reaches_the_stub() {
    let wf = parse(&image_generate_wf("xai"));
    let executor = MockToolExecutor::new().enqueue_ok(ToolResult::success("og", r#"{"ok":true}"#));
    let (runtime, probe) = runtime_with_tools(MockShell::new(), executor);
    let runtime = runtime.with_max_cost_usd(Some(1.00));
    let outcome = run(&runtime, &wf).await;
    assert!(outcome.ok, "cap 1.00 admits a $0.02 floor");
    assert_eq!(probe.captured_calls().len(), 1, "the stub must run once");
    assert_eq!(probe.captured_calls()[0].name, "nika:image_generate");
}

/// The gates' order: trust → required inputs → budget floor (a run
/// that would fail the input gate never reaches the budget verdict).
#[test]
fn gates_fire_the_budget_floor_after_the_input_gate() {
    let wf = parse(
        "nika: m\ninputs:\n  needed: { type: string, required: true }\ntasks:\n  \
         a:\n    infer: { prompt: hi, max_tokens: 1000000, model: \"anthropic/claude-sonnet-5\" }\n",
    );
    let report = nika_check::check(&wf);
    let err = gates(
        &wf,
        &report,
        &BTreeMap::new(),
        Some(0.000_001),
        None,
        (None, &[], None),
    )
    .expect_err("both gates could fire — the input gate speaks first");
    assert!(
        matches!(err, RuntimeError::MissingRequiredInputs { .. }),
        "the input gate must precede"
    );
    // With the input satisfied, the budget floor is the word.
    let mut overrides = BTreeMap::new();
    overrides.insert("needed".to_owned(), Value::String("x".to_owned()));
    let err = gates(
        &wf,
        &report,
        &overrides,
        Some(0.000_001),
        None,
        (None, &[], None),
    )
    .expect_err("the budget gate fires once inputs are satisfied");
    assert!(
        matches!(err, RuntimeError::BudgetFloor { .. }),
        "the budget gate must then own the refusal"
    );
}

/// A hermetic probe row — hand-built, zero env reads (the gate must
/// be judgeable without ambient state · instrument law).
fn access_probe(
    id: &str,
    requires_key: bool,
    key_present: bool,
    access: nika_types::access::AccessClass,
) -> ProviderProbe {
    use nika_providers::probe::{ExecutionLocus, ProviderReadiness};
    ProviderProbe::new(
        id,
        requires_key,
        key_present,
        format!("{}_API_KEY", id.to_uppercase()),
        false,
        ProviderReadiness::new(
            true,
            !requires_key || key_present,
            None,
            None,
            true,
            ExecutionLocus::classify(None, "https://api.example.com"),
            access,
        ),
        "https://api.example.com",
    )
}

fn mistral_wf() -> (RawWorkflow, CheckReport) {
    let wf = parse(
        "nika: t\ntasks:\n  s:\n    infer: { prompt: \"x\", model: \"mistral/mistral-small-latest\" }\n",
    );
    let report = nika_check::check(&wf);
    (wf, report)
}

#[test]
fn no_pin_never_gates_access() {
    let (wf, report) = mistral_wf();
    // Even a machine with ZERO configured paths admits without a
    // pin — P2 changes nothing for single-access installs.
    let (pin, probes) = (
        None,
        &[access_probe(
            "mistral",
            true,
            false,
            nika_types::access::AccessClass::Api,
        )],
    );
    assert!(access_pin_refusal(&wf, &report, probes, pin, None).is_none());
}

#[test]
fn an_unknown_access_token_teaches_before_resolution() {
    let (wf, report) = mistral_wf();
    let (pin, probes) = (Some("locale"), &[]);
    let err = access_pin_refusal(&wf, &report, probes, pin, None).expect("typo refused");
    assert!(matches!(err, RuntimeError::AccessUnknownToken { .. }));
    assert!(err.to_string().contains("locale"));
    assert!(err.to_string().contains("NIKA-1802"));
}

#[test]
fn a_class_pin_no_candidate_matches_refuses_1801() {
    let (wf, report) = mistral_wf();
    let (pin, probes) = (
        Some("local"),
        &[access_probe(
            "mistral",
            true,
            true,
            nika_types::access::AccessClass::Api,
        )],
    );
    let err = access_pin_refusal(&wf, &report, probes, pin, None).expect("pin unsatisfied");
    assert!(matches!(err, RuntimeError::AccessPinUnsatisfied { .. }));
    assert!(err.to_string().contains("never a substitute"));
}

#[test]
fn a_known_cli_token_without_the_binary_is_1803_not_1802() {
    let (wf, report) = mistral_wf();
    let err = access_pin_refusal(&wf, &report, &[], Some("claude-code"), None)
        .expect("known token refused");
    assert!(matches!(err, RuntimeError::AccessUnavailable { .. }));
    assert!(err.to_string().contains("NIKA-1803"));
    assert!(!err.to_string().contains("NIKA-1802"));
}

#[cfg(feature = "access-harness")]
#[test]
fn an_acp_only_seat_refuses_infer_with_the_attestation_witness() {
    // Gemini CLI speaks ACP and has no one-shot contract the engine measured: the four
    // conjuncts stay unproven and the witness names each of them. (Claude Code, Grok Build and
    // Copilot prove them by their measured `-p` contracts since 076a2a91 — see the next test.)
    let wf = parse(
        "nika: t\nmodel: gemini/gemini-2.5-pro\ntasks:\n  s:\n    infer: { prompt: \"x\" }\n",
    );
    let report = nika_check::check(&wf);
    let probe = access_probe(
        "gemini-cli",
        false,
        true,
        nika_types::access::AccessClass::Harness,
    )
    .with_serves(vec!["gemini".to_owned()]);
    let err = access_pin_refusal(&wf, &report, &[probe], Some("gemini-cli"), None)
        .expect("ACP alone is not an infer-grade proof");
    assert!(matches!(err, RuntimeError::AccessNoPath { .. }));
    let witness = err.to_string();
    for term in [
        "gemini-cli",
        "single_turn",
        "no_implicit_tools",
        "structured_output",
        "model_identity",
    ] {
        assert!(witness.contains(term), "missing witness term: {term}");
    }
}

#[cfg(feature = "access-harness")]
#[test]
fn a_ready_claude_code_seat_is_infer_grade_by_its_one_shot_contract() {
    let wf = parse(
        "nika: t\nmodel: anthropic/claude-sonnet-4-6\ntasks:\n  s:\n    infer: { prompt: \"x\" }\n",
    );
    let report = nika_check::check(&wf);
    let probe = access_probe(
        "claude-code",
        false,
        true,
        nika_types::access::AccessClass::Harness,
    )
    .with_serves(vec!["anthropic".to_owned()]);
    assert!(
        access_pin_refusal(&wf, &report, &[probe], Some("claude-code"), None).is_none(),
        "claude -p --tools \"\" --max-turns 1 --json-schema proves the four conjuncts"
    );
}

#[cfg(feature = "access-harness")]
#[test]
fn codex_infer_grade_pin_admits_an_infer_only_workflow() {
    let (wf, report) = mistral_wf();
    let probe = access_probe(
        "codex",
        false,
        true,
        nika_types::access::AccessClass::Harness,
    )
    .with_serves(vec!["mistral".to_owned()]);
    assert!(access_pin_refusal(&wf, &report, &[probe], Some("codex"), None).is_none());
}

#[test]
fn access_harness_with_only_api_probes_is_1803_not_api_keys() {
    let (wf, report) = mistral_wf();
    let (pin, probes) = (
        Some("harness"),
        &[access_probe(
            "mistral",
            true,
            true,
            nika_types::access::AccessClass::Api,
        )],
    );
    let err = access_pin_refusal(&wf, &report, probes, pin, None).expect("no runtime");
    assert!(matches!(err, RuntimeError::AccessUnavailable { .. }));
    assert!(err.to_string().contains("NIKA-1803"));
    assert!(!err.to_string().contains("API_KEY"));
}

#[test]
fn a_pinned_path_failing_admission_refuses_1800_with_the_fix_var() {
    let (wf, report) = mistral_wf();
    let (pin, probes) = (
        Some("api"),
        &[access_probe(
            "mistral",
            true,
            false,
            nika_types::access::AccessClass::Api,
        )],
    );
    let err = access_pin_refusal(&wf, &report, probes, pin, None).expect("path inadmissible");
    assert!(matches!(err, RuntimeError::AccessNoPath { .. }));
    assert!(err.to_string().contains("MISTRAL_API_KEY unset"));
}

#[test]
fn a_satisfied_pin_admits() {
    let wf =
        parse("nika: t\ntasks:\n  s:\n    infer: { prompt: \"x\", model: \"ollama/llama3.2\" }\n");
    let report = nika_check::check(&wf);
    let (pin, probes) = (
        Some("local"),
        &[access_probe(
            "ollama",
            false,
            false,
            nika_types::access::AccessClass::Local,
        )],
    );
    assert!(access_pin_refusal(&wf, &report, probes, pin, None).is_none());
}

#[test]
fn a_mock_pin_keeps_the_rehearsal_runnable() {
    let wf = parse("nika: t\ntasks:\n  s:\n    infer: { prompt: \"x\", model: \"mock/echo\" }\n");
    let report = nika_check::check(&wf);
    // Probes exclude the mock backend by design — the gate must
    // synthesize its keyless candidate, or the rehearsal dies.
    let (pin, probes) = (Some("mock"), &[]);
    assert!(access_pin_refusal(&wf, &report, probes, pin, None).is_none());
}

#[cfg(feature = "access-harness")]
#[test]
fn a_mock_model_refuses_a_harness_pin_before_any_live_seat() {
    let wf = parse("nika: t\ntasks:\n  s:\n    infer: { prompt: \"x\", model: \"mock/echo\" }\n");
    let report = nika_check::check(&wf);
    let probe = access_probe(
        "codex",
        false,
        true,
        nika_types::access::AccessClass::Harness,
    );
    let err = access_pin_refusal(&wf, &report, &[probe], Some("harness"), None)
        .expect("mock must never substitute a live harness");
    assert!(matches!(err, RuntimeError::AccessPinUnsatisfied { .. }));
    let witness = err.to_string();
    assert!(witness.contains("mock/echo"));
    assert!(witness.contains("--access mock"));
    assert!(witness.contains("never a live substitute"));
}

#[test]
fn the_model_override_is_what_the_pin_judges() {
    // The ENVELOPE model is mistral (api) — the override sends the
    // run to ollama (local): a `local` pin must judge the OVERRIDE
    // (#342's law), so it admits. A per-task `model:` would keep
    // winning over the override — hence the envelope-form fixture.
    let wf = parse(
        "nika: t\nmodel: mistral/mistral-small-latest\ntasks:\n  s:\n    infer: { prompt: \"x\" }\n",
    );
    let report = nika_check::check(&wf);
    let (pin, probes) = (
        Some("local"),
        &[
            access_probe("mistral", true, true, nika_types::access::AccessClass::Api),
            access_probe(
                "ollama",
                false,
                false,
                nika_types::access::AccessClass::Local,
            ),
        ],
    );
    assert!(
        access_pin_refusal(&wf, &report, probes, pin, Some("ollama/llama3.2")).is_none(),
        "the override's provider satisfies the pin"
    );
    // And WITHOUT the override the same pin refuses (mistral is api).
    assert!(access_pin_refusal(&wf, &report, probes, pin, None).is_some());
}

fn breakdown_of(yaml: &str) -> String {
    let wf = parse(yaml);
    unbounded_breakdown(&nika_check::check(&wf).cost)
}

#[test]
fn breakdown_names_each_reason_not_the_fixed_disjunction() {
    // A priced-but-unbounded task must read « no max_tokens », not
    // « unpriced model » — the operator sees which is FIXABLE. The
    // unpriced specimen is a sovereign local (unpriced-never-free);
    // mock stopped qualifying when it became a proven zero (A-02).
    let msg = breakdown_of(
        "nika: m\ntasks:\n  \
         a:\n    infer: { prompt: hi, model: \"anthropic/claude-sonnet-5\" }\n  \
         b:\n    infer: { prompt: hi, max_tokens: 100, model: \"ollama/llama3\" }\n",
    );
    assert!(msg.contains("2 task(s)"));
    assert!(
        msg.contains("1 with no `max_tokens`"),
        "the priced-unbounded task must be classified"
    );
    assert!(
        msg.contains("1 on an unpriced model"),
        "the local task must be classified"
    );
}

#[test]
fn breakdown_counts_only_the_unbounded_tasks() {
    // Bounded tasks are never in the tally — and the mock plane is
    // bounded BY CONSTRUCTION now (a proven zero · A-02), so it
    // rides along as a second exclusion specimen. id b carries
    // max_tokens so its reason is NoPrice (sovereign local), not
    // NoTokenLimit — proving the unpriced bucket AND both
    // exclusions in one shot.
    let msg = breakdown_of(
        "nika: m\ntasks:\n  \
         a:\n    infer: { prompt: hi, max_tokens: 100, model: \"anthropic/claude-sonnet-5\" }\n  \
         b:\n    infer: { prompt: hi, max_tokens: 100, model: \"ollama/llama3\" }\n  \
         c:\n    infer: { prompt: hi, max_tokens: 100, model: \"mock/echo\" }\n",
    );
    assert!(
        msg.contains("1 task(s)"),
        "only the unpriced local task must count"
    );
    assert!(msg.contains("unpriced model"));
}

/// Spec 02 §Project files: a run never starts on a project file no compose step read; the
/// launch refuses before the prologue, and the composed workflow clears that gate.
#[test]
fn a_run_never_starts_on_an_unread_project_file() {
    let wf = parse(
        "nika: f\npermits: { fs: { read: [\"prompts/**\"] } }\ntasks:\n  a:\n    infer: { prompt: { file: \"prompts/a.md\" }, model: \"anthropic/claude-sonnet-5\" }\n",
    );
    let report = nika_check::check(&wf);
    let err = gates(
        &wf,
        &report,
        &BTreeMap::new(),
        None,
        None,
        (None, &[], None),
    )
    .expect_err("an unread project file refuses the launch");
    let RuntimeError::ReportMismatch { detail } = &err else {
        panic!("the launch refusal names the unread file: {err:?}");
    };
    assert!(
        detail.contains("project files") && detail.contains("prompts/a.md"),
        "{detail}"
    );
    let mut read =
        |_: &str| -> Result<Vec<u8>, nika_schema::ProjectFileRead> { Ok(b"Summarize".to_vec()) };
    let composed = nika_schema::compose_project_files(&wf, ".", &mut read);
    assert!(composed.findings.is_empty(), "{:?}", composed.findings);
    assert!(unread_project_files_refusal(&composed.workflow).is_none());
}
