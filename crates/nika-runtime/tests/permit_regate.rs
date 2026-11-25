// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>
#![allow(clippy::expect_used, clippy::panic)]

//! The F-O1 PR-2 runtime re-gate, end to end (NEP-0004 law 2 ·
//! `NIKA-SEC-004`): an UNTRUSTED value reaching a permitted verb's
//! argument is matched against the step's `permits:` on its RESOLVED,
//! canonical form. The untrusted source is a mocked `nika:fetch` (born
//! untrusted by the shared ingress law — no catalog consult); every
//! effect seam is mocked, so the refusal is the production code path
//! with zero I/O.
//!
//! The fixtures are ENGINE.md's own: the traversal under `fs.read`, the
//! option-injection under an argv allowlist, the in-permit pivot (the
//! re-gate is not a blind deny), and the `mcp:*` border both ways. Every
//! task hoists its tainted read into `with:` (NIKA-VAR-021's idiom — the
//! verb field then reads `${{ with.<name> }}`).

use std::num::NonZeroUsize;
use std::sync::Arc;

use nika_cap::Integrity;
use nika_check::check;
use nika_event::{Event, EventKind};
use nika_kernel::tool_executor::{ToolExecError, ToolResult};
use nika_kernel_mock::{
    MockClock, MockProvider, MockShell, MockToolDefinitionProvider, MockToolExecutor,
};
use nika_providers::{ProviderRegistry, ProvidersConfig};
use nika_runtime::{DeterministicStamper, RunOutcome, Runtime, RuntimeConfig, TaskStatus, VecSink};
use nika_schema::{FileId, ParseMode, parse};
use nika_types::resource::Value as FieldValue;
use nika_verb_agent::AgentVerb;
use nika_verb_exec::ExecVerb;
use nika_verb_infer::InferVerb;
use nika_verb_invoke::InvokeVerb;

/// Run one workflow whose untrusted ingress is ONE mocked fetch result.
/// Returns the outcome plus the mock handles (side-effect counts prove
/// the refusal is pre-spawn / pre-dispatch).
async fn run_with_ingress(
    yaml: &str,
    ingress_payload: &str,
    shell: MockShell,
) -> (RunOutcome, Arc<MockToolExecutor>, MockShell) {
    let tools =
        MockToolExecutor::new().enqueue_ok(ToolResult::success("r1", ingress_payload.to_owned()));
    let run = run_with_mocks(yaml, tools, shell, RuntimeConfig::default()).await;
    (run.outcome, run.tools, run.shell)
}

/// One mocked run: the outcome, the event stream and the mock handles.
struct MockRun {
    outcome: RunOutcome,
    events: Vec<Event>,
    tools: Arc<MockToolExecutor>,
    shell: MockShell,
}

/// Run one checked-clean workflow against the given mocks, keeping the
/// event stream (a cleanup's refusal and a parked recovery are visible
/// there and nowhere else).
async fn run_with_mocks(
    yaml: &str,
    tools: MockToolExecutor,
    shell: MockShell,
    config: RuntimeConfig,
) -> MockRun {
    let wf = parse(yaml, FileId::new(0), ParseMode::Strict).expect("fixture parses");
    let report = check(&wf);
    assert!(
        report.is_clean(),
        "the re-gate is a RUNTIME law — the static ladder defers the dynamic value: {:?}",
        report.findings
    );
    let tools = Arc::new(tools);
    let registry = Arc::new(ProviderRegistry::without_http(ProvidersConfig::default()));
    let invoke = Arc::new(InvokeVerb::new(Arc::clone(&tools)));
    let runtime = Runtime::new(
        ExecVerb::new(Arc::new(shell.clone())),
        Arc::clone(&invoke),
        InferVerb::new(registry, "mock/echo"),
        AgentVerb::new(
            Arc::new(MockProvider::new("mock")),
            invoke,
            Arc::new(MockToolDefinitionProvider::new()),
            "mock/echo",
        ),
        MockClock::new(),
        config,
    );
    let mut stamper = DeterministicStamper::new();
    let mut sink = VecSink::new();
    let outcome = runtime
        .run(&wf, &report, &mut stamper, &mut sink)
        .await
        .expect("clean run");
    MockRun {
        outcome,
        events: sink.into_events(),
        tools,
        shell,
    }
}

/// The workflow spine of the exec fixtures: one fetch (the untrusted
/// ingress), then a `tar`/`make` task whose argv or cwd reads it through
/// a `with:` slot (NIKA-VAR-021's idiom).
/// The untrusted value arrives at the CALLER boundary, not over the net.
///
/// It arrived over the net until 2026-08-13, when the order law
/// (`NIKA-SEC-015`) made a fetch→exec route a static refusal: the file
/// would never run, so a runtime fixture written that way proves nothing
/// about the runtime. The caller boundary is the same taint by the
/// Perl-taint slot rule (NEP-0004 law 2 — the sibling fixture
/// `untrusted_traversal_under_fs_read_is_refused` has always used it),
/// and the order law does not reach it: `inputs:` is not a net ingress.
fn exec_workflow(permits: &str, payload: &str, task: &str) -> String {
    format!(
        "nika: regate\ninputs:\n  payload: {{ type: string, default: \"{payload}\" }}\n{permits}\ntasks:\n{task}\n"
    )
}

/// ENGINE.md fixture 1 (the ledger row): a caller-supplied value
/// (`inputs.p` — the caller boundary, untrusted by the Perl-taint slot
/// rule; the fetch-driven twin is the f3-03 adversarial fixture, which
/// declares no fs and so stays clear of the NEP-0002 trifecta) resolving
/// to `datasets/../../../etc/passwd` escapes `fs.read: [datasets/**]` —
/// the canonical fold refuses the argv element pre-spawn.
#[tokio::test]
async fn untrusted_traversal_under_fs_read_is_refused() {
    let yaml = "nika: regate\ninputs:\n  p: { type: string, default: \"datasets/../../../etc/passwd\" }\npermits:\n  exec: [\"tar\"]\n  fs: { read: [\"datasets/**\"] }\ntasks:\n  untar:\n    with: { p: \"${{ inputs.p }}\" }\n    exec: { command: [\"tar\", \"-xf\", \"${{ with.p }}\"] }\n";
    let (outcome, _, shell) = run_with_ingress(yaml, "unused", MockShell::new()).await;
    assert!(!outcome.ok, "a re-gate refusal fails the run");
    let rec = &outcome.records["untar"];
    assert_eq!(rec.status, TaskStatus::Failure);
    let err = rec.error.as_ref().expect("the refusal record");
    assert_eq!(err.code, "NIKA-SEC-004");
    assert!(
        err.message.contains("taint path: inputs.p -> exec.argv[2]")
            && err.message.contains("../../etc/passwd"),
        "source-first taint path + the canonical form: {}",
        err.message
    );
    assert!(
        shell.executed_commands().is_empty(),
        "the refusal is pre-spawn — the runner is never reached"
    );
}

/// ENGINE.md fixture 2: the untrusted argv tail resolves to a re-entry
/// option token (`--checkpoint-action=…`) where the author wrote a data
/// slot — option-injection, refused even under an argv allowlist.
#[tokio::test]
async fn untrusted_option_injection_is_refused() {
    let yaml = exec_workflow(
        "permits:\n  exec: [\"tar\"]",
        "--checkpoint-action=exec=sh id",
        "  untar:\n    with: { p: \"${{ inputs.payload }}\" }\n    exec: { command: [\"tar\", \"-xf\", \"${{ with.p }}\"] }",
    );
    let (outcome, _, shell) = run_with_ingress(&yaml, "unused", MockShell::new()).await;
    assert!(!outcome.ok);
    let err = outcome.records["untar"]
        .error
        .as_ref()
        .expect("the refusal");
    assert_eq!(err.code, "NIKA-SEC-004");
    assert!(
        err.message.contains("option token"),
        "the refusal names the class: {}",
        err.message
    );
    assert!(shell.executed_commands().is_empty());
}

/// ENGINE.md fixture 4 (the pivot): the SAME slot canonicalizes INSIDE
/// the permit (`datasets/2026/report.csv` under `datasets/**`) — the
/// re-gate is not a blind deny, the step RUNS.
#[tokio::test]
async fn untrusted_value_inside_the_permit_runs() {
    let yaml = "nika: regate\ninputs:\n  p: { type: string, default: \"datasets/2026/report.csv\" }\npermits:\n  exec: [\"tar\"]\n  fs: { read: [\"datasets/**\"] }\ntasks:\n  untar:\n    with: { p: \"${{ inputs.p }}\" }\n    exec: { command: [\"tar\", \"-xf\", \"${{ with.p }}\"] }\n";
    let (outcome, _, shell) =
        run_with_ingress(yaml, "unused", MockShell::new().enqueue_ok("extracted\n")).await;
    assert!(outcome.ok, "a covered value runs: {:?}", outcome.records);
    assert_eq!(outcome.records["untar"].status, TaskStatus::Success);
    assert_eq!(
        shell.executed_commands().len(),
        1,
        "the in-permit exec reached the runner exactly once"
    );
}

/// The `mcp:*` border: an untrusted URL in a permitted mcp tool's args
/// escapes `net.http` — refused BEFORE the tool dispatch (the grant of
/// the tool is the category, never the resolved value).
#[tokio::test]
async fn untrusted_mcp_arg_escaping_net_is_refused() {
    let yaml = "nika: regate-mcp\npermits:\n  net: { http: [\"news.example\", \"api.example.com\"] }\n  tools: [\"nika:fetch\", \"mcp:store/put\"]\ntasks:\n  dl:\n    invoke:\n      tool: \"nika:fetch\"\n      args: { url: \"https://news.example/payload.txt\" }\n  put:\n    after: { dl: success }\n    with: { u: \"${{ tasks.dl.output }}\" }\n    invoke:\n      tool: \"mcp:store/put\"\n      args: { url: \"${{ with.u }}\" }\n";
    let (outcome, tools, _) =
        run_with_ingress(yaml, "https://evil.example/x", MockShell::new()).await;
    assert!(!outcome.ok);
    let err = outcome.records["put"].error.as_ref().expect("the refusal");
    assert_eq!(err.code, "NIKA-SEC-004");
    assert!(
        err.message.contains("dl -> mcp:store/put.args.url")
            && err.message.contains("host \"evil.example\""),
        "the mcp sink + the escaped host: {}",
        err.message
    );
    assert_eq!(
        tools.captured_calls().len(),
        1,
        "only the fetch dispatched — the mcp call never reached the executor"
    );
}

/// The `mcp:*` border, in-permit: the untrusted path arg canonicalizes
/// inside `fs.read` — the tool call runs. (Caller-supplied here: a fetch
/// source would complete the NEP-0002 trifecta with `fs.read` + mcp
/// egress — the static human-gate, a different law.)
#[tokio::test]
async fn untrusted_mcp_path_arg_inside_the_permit_runs() {
    let yaml = "nika: regate-mcp-ok\ninputs:\n  p: { type: string, default: \"datasets/report.csv\" }\npermits:\n  fs: { read: [\"datasets/**\"] }\n  tools: [\"mcp:fs/read\"]\ntasks:\n  read:\n    with: { p: \"${{ inputs.p }}\" }\n    invoke:\n      tool: \"mcp:fs/read\"\n      args: { path: \"${{ with.p }}\" }\n";
    let tools =
        Arc::new(MockToolExecutor::new().enqueue_ok(ToolResult::success("r1", "file contents")));
    let registry = Arc::new(ProviderRegistry::without_http(ProvidersConfig::default()));
    let invoke = Arc::new(InvokeVerb::new(Arc::clone(&tools)));
    let wf = parse(yaml, FileId::new(0), ParseMode::Strict).expect("fixture parses");
    let report = check(&wf);
    assert!(report.is_clean(), "static ladder: {:?}", report.findings);
    let runtime = Runtime::new(
        ExecVerb::new(Arc::new(MockShell::new())),
        Arc::clone(&invoke),
        InferVerb::new(registry, "mock/echo"),
        AgentVerb::new(
            Arc::new(MockProvider::new("mock")),
            invoke,
            Arc::new(MockToolDefinitionProvider::new()),
            "mock/echo",
        ),
        MockClock::new(),
        RuntimeConfig::default(),
    );
    let mut stamper = DeterministicStamper::new();
    let mut sink = VecSink::new();
    let outcome = runtime
        .run(&wf, &report, &mut stamper, &mut sink)
        .await
        .expect("clean run");
    assert!(
        outcome.ok,
        "the covered mcp arg runs: {:?}",
        outcome.records
    );
    assert_eq!(outcome.records["read"].status, TaskStatus::Success);
    assert_eq!(tools.captured_calls().len(), 1, "the mcp call dispatched");
}

/// A tainted `cwd:` re-gates as a path (ENGINE.md step 5 · « cwd idem »):
/// the payload climbing out of `src/**` is refused pre-spawn.
#[tokio::test]
async fn untrusted_cwd_escaping_is_refused() {
    let yaml = exec_workflow(
        "permits:\n  exec: [\"make\"]\n  fs: { read: [\"src/**\"] }",
        "src/../../etc",
        "  build:\n    with: { dir: \"${{ inputs.payload }}\" }\n    exec: { command: [\"make\"], cwd: \"${{ with.dir }}\" }",
    );
    let (outcome, _, shell) = run_with_ingress(&yaml, "unused", MockShell::new()).await;
    assert!(!outcome.ok);
    let err = outcome.records["build"]
        .error
        .as_ref()
        .expect("the refusal");
    assert_eq!(err.code, "NIKA-SEC-004");
    assert!(
        err.message.contains("-> exec.cwd"),
        "the cwd sink speaks: {}",
        err.message
    );
    assert!(shell.executed_commands().is_empty());
}

/// The unwind cleanup lane re-gates too: the overlay record carries the
/// PRODUCER's label, so a cleanup argv reading its producer's tainted
/// output is refused pre-spawn. The cleanup lane is best-effort (its
/// outcome is swallowed) — the proof is the shell mock staying silent.
/// Without the label riding the overlay this fixture is RED: the
/// cleanup's exec would reach the (enqueue-less) mock and panic.
///
/// The unwind cleanup lane's integrity overlay — a cleanup argv reading
/// its producer's tainted output — is now behind a STATIC wall, and
/// this fixture pins the wall.
///
/// A net ingress (`nika_cap::invoke_tool_is_ingress` · `nika:fetch` ∪
/// untrusted `mcp:`) reaching an `exec:` is exactly what the order law
/// (`NIKA-SEC-015`) refuses, so a fetched producer gives the overlay no
/// fixture that passes `check` — and the runtime will not run a dirty
/// report, so there is no « run it anyway » arm to write either.
///
/// An `exec:` output is untrusted too, and the order law covers only
/// net-effecting producers, so an exec producer reaches the overlay from
/// a file that passes `check`:
/// `an_unwind_cleanup_reading_a_fold_of_its_producer_is_refused` pins
/// that lane through a fold.
#[tokio::test]
async fn an_unwind_cleanup_reading_the_producers_taint_is_refused_statically() {
    let yaml = "nika: regate-finally\npermits:\n  exec: [\"tar\"]\n  net: { http: [\"news.example\"] }\n  tools: [\"nika:fetch\"]\ntasks:\n  dl:\n    invoke:\n      tool: \"nika:fetch\"\n      args: { url: \"https://news.example/payload.txt\" }\n  dl_cleanup:\n    after: { dl: unwind }\n    exec: { command: [\"tar\", \"-xf\", \"${{ tasks.dl.output }}\"] }\n";
    let wf = parse(yaml, FileId::new(0), ParseMode::Strict).expect("fixture parses");
    let report = check(&wf);
    let order = report
        .findings
        .iter()
        .find(|f| f.code.as_deref() == Some("NIKA-SEC-015"))
        .expect("the order law refuses the cleanup route");
    assert_eq!(
        order.task.as_deref(),
        Some("dl_cleanup"),
        "the cleanup IS the sink — an unwind edge leaves the precedence \
         graph, never the content graph: {order:?}"
    );
}

/// NEP-0004 law 5 (F-O1 PR-3) — the ONLY door: a task-level `declassify:`
/// entry admits the named binding at the re-gate (the traversal below
/// would refuse NIKA-SEC-004 without it — the first fixture of this
/// file), SCOPED to the task, and the receipt records the lift: one
/// `declassify` event between `task_started` and the terminal frame,
/// carrying `from` · `because` · the admitted value's digest.
#[tokio::test]
async fn declassify_admits_the_binding_and_the_receipt_records_it() {
    let yaml = "nika: regate-declassify\ninputs:\n  p: { type: string, default: \"datasets/../../../etc/passwd\" }\npermits:\n  exec: [\"tar\"]\n  fs: { read: [\"datasets/**\"] }\ntasks:\n  untar:\n    exec: { command: [\"tar\", \"-xf\", \"${{ inputs.p }}\"] }\n    lift:\n      - law: taint\n        from: inputs.p\n        because: \"pinned vendor bundle, hash-reviewed at release time\"\n";
    let (outcome, _, shell) =
        run_with_ingress(yaml, "unused", MockShell::new().enqueue_ok("extracted\n")).await;
    assert!(
        outcome.ok,
        "the declassified binding is admitted at the re-gate: {:?}",
        outcome.records
    );
    assert_eq!(outcome.records["untar"].status, TaskStatus::Success);
    assert_eq!(
        shell.executed_commands().len(),
        1,
        "the admitted exec reached the runner"
    );
    // The receipt: one `declassify` event, between task_started and the
    // terminal, with the law-5 evidence (from · because · value digest).
    let sink_events = run_with_ingress_sink(yaml).await;
    let kinds: Vec<EventKind> = sink_events.iter().map(|e| e.kind).collect();
    let started = kinds
        .iter()
        .position(|k| *k == EventKind::TaskStarted)
        .expect("task_started");
    let lift = kinds
        .iter()
        .position(|k| *k == EventKind::Declassify)
        .expect("the declassify event rides the receipt");
    let terminal = kinds
        .iter()
        .rposition(|k| *k == EventKind::TaskCompleted)
        .expect("task_completed");
    assert!(
        started < lift && lift < terminal,
        "task_started < declassify < task_completed: {kinds:?}"
    );
    let event = &sink_events[lift];
    let field = |key: &str| {
        event
            .fields
            .iter()
            .find(|kv| kv.key == key)
            .and_then(|kv| match &kv.value {
                FieldValue::String(s) => Some(s.as_str()),
                _ => None,
            })
    };
    assert_eq!(field("task"), Some("untar"));
    assert_eq!(field("from"), Some("inputs.p"));
    assert!(
        field("because").is_some_and(|b| b.contains("hash-reviewed")),
        "the justification rides: {:?}",
        event.fields
    );
    assert!(
        field("value_digest").is_some_and(|d| d.len() == 64),
        "the blake3 hex digest of the admitted value: {:?}",
        event.fields
    );
}

/// Re-run the declassify workflow keeping the sink (the event assertions
/// need the stream, not just the outcome).
async fn run_with_ingress_sink(yaml: &str) -> Vec<Event> {
    let wf = parse(yaml, FileId::new(0), ParseMode::Strict).expect("fixture parses");
    let report = check(&wf);
    assert!(
        report.is_clean(),
        "the door is check-visible, never a finding: {:?}",
        report.findings
    );
    let tools = Arc::new(MockToolExecutor::new());
    let registry = Arc::new(ProviderRegistry::without_http(ProvidersConfig::default()));
    let invoke = Arc::new(InvokeVerb::new(Arc::clone(&tools)));
    let runtime = Runtime::new(
        ExecVerb::new(Arc::new(MockShell::new().enqueue_ok("extracted\n"))),
        Arc::clone(&invoke),
        InferVerb::new(registry, "mock/echo"),
        AgentVerb::new(
            Arc::new(MockProvider::new("mock")),
            invoke,
            Arc::new(MockToolDefinitionProvider::new()),
            "mock/echo",
        ),
        MockClock::new(),
        RuntimeConfig::default(),
    );
    let mut stamper = DeterministicStamper::new();
    let mut sink = VecSink::new();
    let outcome = runtime
        .run(&wf, &report, &mut stamper, &mut sink)
        .await
        .expect("clean run");
    assert!(outcome.ok, "{:?}", outcome.records);
    sink.into_events()
}

/// The exec spine of the fold fixtures: a `git` member of group `logs`
/// whose subject line (an exec output is untrusted content) reaches a `tar`
/// argv through `{read}`. No `fs.read` and no net: neither the trifecta nor
/// the order law has a leg here, so the refusal is the run's re-gate alone.
const FOLD_EXEC: &str = "nika: regate-fold\npermits:\n  exec: [\"git\", \"tar\"]\ntasks:\n  subjects:\n    group: logs\n    exec: { command: [\"git\", \"log\", \"--format=%s\", \"-n\", \"1\"] }\n  pack:\n    with: { got: \"{read}\" }\n    exec: { command: [\"tar\", \"-cf\", \"out.tar\", \"${{ with.got }}\"] }\n";

/// A subject line that is a re-entry option token where `tar` expects a
/// file name.
const HOSTILE_SUBJECT: &str = "--checkpoint-action=exec=sh id\n";

/// The `group.<name>` fold holds each member's record (spec 03 §group), so
/// it carries the member's untrusted label into the argv exactly as the
/// direct read does: the option token is refused pre-spawn, and the refusal
/// names the member as the taint source.
#[tokio::test]
async fn an_exec_argv_reading_a_fold_of_an_untrusted_member_is_refused() {
    for read in [
        "${{ tasks.subjects.output }}",
        "${{ group.logs[0].output }}",
    ] {
        let yaml = FOLD_EXEC.replace("{read}", read);
        let shell = MockShell::new().enqueue_ok(HOSTILE_SUBJECT);
        let (outcome, _, shell) = run_with_ingress(&yaml, "unused", shell).await;
        assert!(!outcome.ok, "{read}: a re-gate refusal fails the run");
        assert_eq!(outcome.records["subjects"].status, TaskStatus::Success);
        let rec = &outcome.records["pack"];
        assert_eq!(rec.status, TaskStatus::Failure, "{read}");
        let err = rec.error.as_ref().expect("the refusal record");
        assert_eq!(err.code, "NIKA-SEC-004", "{read}: {}", err.message);
        assert!(
            err.message.contains("taint path: subjects -> exec.argv[3]")
                && err.message.contains("option token"),
            "{read}: the member is the source, the class is named: {}",
            err.message
        );
        assert_eq!(
            shell.executed_commands().len(),
            1,
            "{read}: only `git` ran — `tar` never reached the runner"
        );
    }
}

/// The `mcp:*` border through a fold: the fetched member's URL escapes
/// `net.http`, refused before the tool dispatch, the member named.
#[tokio::test]
async fn an_mcp_arg_reading_a_fold_of_an_untrusted_member_is_refused() {
    let yaml = "nika: regate-mcp-fold\npermits:\n  net: { http: [\"news.example\", \"api.example.com\"] }\n  tools: [\"nika:fetch\", \"mcp:store/put\"]\ntasks:\n  dl:\n    group: pages\n    invoke:\n      tool: \"nika:fetch\"\n      args: { url: \"https://news.example/payload.txt\" }\n  put:\n    with: { u: \"${{ group.pages[0].output }}\" }\n    invoke:\n      tool: \"mcp:store/put\"\n      args: { url: \"${{ with.u }}\" }\n";
    let (outcome, tools, _) =
        run_with_ingress(yaml, "https://evil.example/x", MockShell::new()).await;
    assert!(!outcome.ok);
    let err = outcome.records["put"].error.as_ref().expect("the refusal");
    assert_eq!(err.code, "NIKA-SEC-004");
    assert!(
        err.message.contains("dl -> mcp:store/put.args.url")
            && err.message.contains("host \"evil.example\""),
        "the mcp sink, the member as source, the escaped host: {}",
        err.message
    );
    assert_eq!(
        tools.captured_calls().len(),
        1,
        "only the fetch dispatched — the mcp call never reached the executor"
    );
}

/// A `with:` binding that reads the `for_each` item renders once per
/// iteration with the item bound (spec 03 §`for_each`), so it carries the
/// item's label: the caller-supplied option token is refused through
/// `with.one` exactly as it is through `item`.
#[tokio::test]
async fn an_exec_argv_reading_an_item_binding_is_refused_like_the_item() {
    for read in ["${{ item }}", "${{ with.one }}"] {
        let yaml = format!(
            "nika: regate-item\ninputs:\n  names: {{ type: {{ array: string }}, default: [\"--checkpoint-action=exec=sh id\"] }}\npermits:\n  exec: [\"tar\"]\ntasks:\n  untar:\n    for_each: {{ items: \"${{{{ inputs.names }}}}\", max_items: 10 }}\n    with: {{ one: \"${{{{ item }}}}\" }}\n    exec: {{ command: [\"tar\", \"-xf\", \"{read}\"] }}\n"
        );
        let (outcome, _, shell) = run_with_ingress(&yaml, "unused", MockShell::new()).await;
        assert!(!outcome.ok, "{read}: a re-gate refusal fails the run");
        let err = outcome.records["untar"]
            .error
            .as_ref()
            .expect("the refusal record");
        assert_eq!(err.code, "NIKA-SEC-004", "{read}: {}", err.message);
        assert!(
            err.message.contains("option token"),
            "{read}: the refusal names the class: {}",
            err.message
        );
        assert!(
            shell.executed_commands().is_empty(),
            "{read}: the refusal is pre-spawn"
        );
    }
}

/// A literal `for_each:` list is rendered as a template tree: a string in it
/// that reads a caller-supplied binding carries the caller boundary into the
/// item, and the re-gate refuses the option token it resolves to.
#[tokio::test]
async fn an_exec_argv_reading_an_item_of_a_templated_literal_list_is_refused() {
    let yaml = exec_workflow(
        "permits:\n  exec: [\"tar\"]",
        "--checkpoint-action=exec=sh id",
        "  untar:\n    with: { p: \"${{ inputs.payload }}\" }\n    for_each: { items: [\"${{ with.p }}\"] }\n    exec: { command: [\"tar\", \"-xf\", \"${{ item }}\"] }",
    );
    let (outcome, _, shell) = run_with_ingress(&yaml, "unused", MockShell::new()).await;
    assert!(!outcome.ok, "a re-gate refusal fails the run");
    let err = outcome.records["untar"]
        .error
        .as_ref()
        .expect("the refusal record");
    assert_eq!(err.code, "NIKA-SEC-004", "{}", err.message);
    assert!(
        err.message
            .contains("taint path: inputs.payload -> exec.argv[2]")
            && err.message.contains("option token"),
        "the caller boundary is the source, the class is named: {}",
        err.message
    );
    assert!(
        shell.executed_commands().is_empty(),
        "the refusal is pre-spawn"
    );
}

/// A wave parallelism of one: the FIFO mock queues stay aligned with the
/// declaration order inside a wave.
fn sequential() -> RuntimeConfig {
    RuntimeConfig::new(NonZeroUsize::new(1), 0)
}

/// The position of the first `kind` event of `task` in the stream.
fn position_of(events: &[Event], kind: EventKind, task: &str) -> Option<usize> {
    events.iter().position(|event| {
        event.kind == kind
            && event.fields.iter().any(|kv| {
                kv.key == "task" && matches!(&kv.value, FieldValue::String(s) if s == task)
            })
    })
}

/// The laundering twin of [`FOLD_EXEC`]: the fold reaches the `tar` argv
/// through a `nika:jq` hop, so the label crosses the hop on the hop's
/// RECORD (stamped before its dispatch), never on a binding of `pack`.
const FOLD_LAUNDER: &str = "nika: regate-fold-launder\npermits:\n  exec: [\"git\", \"tar\"]\n  tools: [\"nika:jq\"]\ntasks:\n  subjects:\n    group: logs\n    exec: { command: [\"git\", \"log\", \"--format=%s\", \"-n\", \"1\"] }\n  merge:\n    with: { got: \"${{ group.logs }}\" }\n    invoke: { tool: \"nika:jq\", args: { expression: \".[0].output\", input: \"${{ with.got }}\" } }\n  pack:\n    with: { got: \"${{ tasks.merge.output }}\" }\n    exec: { command: [\"tar\", \"-cf\", \"out.tar\", \"${{ with.got }}\"] }\n";

/// The hop's record carries the member's label (the record label the task
/// lane stamps reads the fold through the membership), so the option token
/// the hop hands on is refused at the `tar` argv, the member named.
#[tokio::test]
async fn a_fold_laundered_through_a_jq_hop_is_refused_at_the_later_argv() {
    // The mocked hop returns what `.[0].output` selects: the subject line.
    let tools = MockToolExecutor::new().enqueue_ok(ToolResult::success("jq", HOSTILE_SUBJECT));
    let shell = MockShell::new().enqueue_ok(HOSTILE_SUBJECT);
    let run = run_with_mocks(FOLD_LAUNDER, tools, shell, sequential()).await;
    assert!(!run.outcome.ok, "a re-gate refusal fails the run");
    let merge = &run.outcome.records["merge"];
    assert_eq!(merge.status, TaskStatus::Success);
    assert_eq!(
        merge.integrity,
        Integrity::untrusted("subjects"),
        "the hop's record carries the member's label, the member named"
    );
    let rec = &run.outcome.records["pack"];
    assert_eq!(rec.status, TaskStatus::Failure);
    let err = rec.error.as_ref().expect("the refusal record");
    assert_eq!(err.code, "NIKA-SEC-004", "{}", err.message);
    assert!(
        err.message.contains("taint path: subjects -> exec.argv[3]")
            && err.message.contains("option token"),
        "the member is the source across the hop: {}",
        err.message
    );
    assert_eq!(
        run.tools.captured_calls().len(),
        1,
        "the hop dispatched once"
    );
    assert_eq!(
        run.shell.executed_commands().len(),
        1,
        "only `git` ran — `tar` never reached the runner"
    );
}

/// A recovery parked on a later referent (spec 05 §recover): `stage` fails
/// in wave 2 and its recover reads `later`, which settles in wave 3, so the
/// recovery resolves over the final view, and its alias of the fold keeps
/// the member's label there.
const FOLD_PARKED_RECOVER: &str = "nika: regate-fold-recover\npermits:\n  exec: [\"git\"]\n  tools: [\"nika:jq\"]\ntasks:\n  subjects:\n    group: logs\n    exec: { command: [\"git\", \"log\", \"--format=%s\", \"-n\", \"1\"] }\n  stage:\n    with: { got: \"${{ group.logs[0].output }}\" }\n    invoke: { tool: \"nika:jq\", args: { expression: \".\", input: \"fixed\" } }\n    on_error: { recover: \"${{ with.got }} ${{ tasks.later.output }}\" }\n  mid:\n    after: { subjects: success }\n    exec: { command: [\"git\", \"status\", \"--short\"] }\n  later:\n    after: { mid: success }\n    invoke: { tool: \"nika:jq\", args: { expression: \".\", input: \"tail\" } }\n";

#[tokio::test]
async fn a_parked_recovery_over_a_fold_alias_carries_the_member_label() {
    // One consumer per mock per wave: `stage` takes the tool error in wave
    // 2 while `mid` takes the shell, and `later` takes the tool in wave 3.
    let tools = MockToolExecutor::new()
        .enqueue_err(ToolExecError::NotFound {
            name: "nika:jq".to_owned(),
        })
        .enqueue_ok(ToolResult::success("jq", "tail"));
    let shell = MockShell::new().enqueue_ok(HOSTILE_SUBJECT).enqueue_ok("");
    let run = run_with_mocks(FOLD_PARKED_RECOVER, tools, shell, sequential()).await;
    let stage = &run.outcome.records["stage"];
    assert_eq!(stage.status, TaskStatus::Success, "{:?}", stage.error);
    assert!(
        stage.recovered_from.is_some(),
        "the value is the recovery's"
    );
    assert_eq!(
        stage.integrity,
        Integrity::untrusted("subjects"),
        "the parked recovery is labelled over the final view, the member named"
    );
    let recovered =
        position_of(&run.events, EventKind::TaskRecovered, "stage").expect("stage recovered");
    let later =
        position_of(&run.events, EventKind::TaskCompleted, "later").expect("later completed");
    assert!(
        later < recovered,
        "the recovery parked until its referent settled: later #{later}, stage #{recovered}"
    );
}

/// An unwind cleanup whose argv reads a fold of its producer (the cleanup
/// lane binds the membership over the producer's fresh record): the option
/// token is refused before the spawn, and the refusal, journaled on the
/// `on_finally` plane since a cleanup failure never propagates, names the
/// member.
const FOLD_CLEANUP: &str = "nika: regate-fold-cleanup\npermits:\n  exec: [\"git\", \"tar\"]\ntasks:\n  subjects:\n    group: logs\n    exec: { command: [\"git\", \"log\", \"--format=%s\", \"-n\", \"1\"] }\n  pack:\n    after: { subjects: unwind }\n    with: { got: \"${{ group.logs[0].output }}\" }\n    exec: { command: [\"tar\", \"-cf\", \"out.tar\", \"${{ with.got }}\"] }\n";

#[tokio::test]
async fn an_unwind_cleanup_reading_a_fold_of_its_producer_is_refused() {
    let shell = MockShell::new().enqueue_ok(HOSTILE_SUBJECT);
    let run = run_with_mocks(FOLD_CLEANUP, MockToolExecutor::new(), shell, sequential()).await;
    assert_eq!(run.outcome.records["subjects"].status, TaskStatus::Success);
    assert_eq!(
        run.shell.executed_commands().len(),
        1,
        "only `git` ran — the cleanup's `tar` never reached the runner"
    );
    let journaled: Vec<String> = run
        .events
        .iter()
        .filter(|event| event.kind == EventKind::PermitChecked)
        .map(|event| format!("{:?}", event.fields))
        .filter(|fields| fields.contains("on_finally") && fields.contains("NIKA-SEC-004"))
        .collect();
    assert_eq!(
        journaled.len(),
        1,
        "one journaled cleanup refusal: {journaled:?}"
    );
    assert!(
        journaled[0].contains("taint path: subjects -> exec.argv[3]")
            && journaled[0].contains("option token"),
        "the member is the source in the cleanup lane: {}",
        journaled[0]
    );
}
