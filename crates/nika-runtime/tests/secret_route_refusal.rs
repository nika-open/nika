// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>
#![allow(clippy::expect_used, clippy::panic)]

//! The run never renders a secret into a task's `model:`, an `exec:`'s `cwd:` or a media
//! builtin's `args.model` (an `agent:`'s `images.save:` is pinned on a harness seat, in the
//! crate's `harness_image_values` tests).
//!
//! `nika check` refuses each (`NIKA-SEC-006`), and the run refuses a dirty report, so every run
//! below holds the report an older check gave for the same bytes: one that did not judge these
//! fields and passed them clean. On that report the run refuses the task itself, before it
//! renders the field: no provider request, no spawn, no inference call, no ledger line. The
//! refusal names the task and the field, never a value, and nothing the run writes (an event,
//! the trace line of each event, the settlement, the ledger the inspect cell reads, the records)
//! carries the value, whatever its length. A model the run never renders from a secret is
//! rendered exactly as written beside a secret of any length.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use nika_builtin::{LiveInspect, WorkflowIntrospect as _};
use nika_check::check;
use nika_event::{Event, EventKind};
use nika_kernel::http::{HttpError, HttpPostDyn, HttpRequest, HttpResponse, HttpStreamResponse};
use nika_kernel::secret::Secret;
use nika_kernel_mock::{MockProvider, MockShell, MockToolDefinitionProvider, MockToolExecutor};
use nika_providers::{ProviderRegistry, ProvidersConfig};
use nika_runtime::{
    DeterministicStamper, RunOutcome, Runtime, RuntimeConfig, SecretResolveError, VecSink,
    WorkflowSecretResolver,
};
use nika_schema::types::SecretRef;
use nika_schema::{FileId, ParseMode, parse};
use nika_types::resource::Value as FieldValue;
use nika_verb_agent::AgentVerb;
use nika_verb_exec::ExecVerb;
use nika_verb_infer::InferVerb;
use nika_verb_invoke::InvokeVerb;

/// Maps a secret name to its value, as the composer's store would.
struct MapSecrets(BTreeMap<String, String>);

impl WorkflowSecretResolver for MapSecrets {
    fn resolve(&self, name: &str, _reference: &SecretRef) -> Result<String, SecretResolveError> {
        self.0.get(name).cloned().ok_or_else(|| SecretResolveError {
            name: name.to_owned(),
            reason: "absent".to_owned(),
        })
    }
}

/// A provider that answers every chat completion for the model it was sent, and keeps the
/// models it was sent and how many requests reached it. No network: the registry's HTTP seam is
/// this value.
#[derive(Default)]
struct Wire {
    models: Mutex<Vec<String>>,
    requests: AtomicUsize,
}

impl Wire {
    /// How many requests reached the provider, streamed or not.
    fn requests(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }
}

impl HttpPostDyn for Wire {
    fn supports_single_attempt(&self) -> bool {
        false
    }
    async fn post(&self, req: HttpRequest) -> Result<HttpResponse, HttpError> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        let sent: serde_json::Value =
            serde_json::from_slice(req.body.as_ref().expect("a body")).expect("json");
        let model = sent["model"].as_str().expect("a model").to_owned();
        self.models.lock().expect("models").push(model.clone());
        let body = serde_json::json!({
            "id": "route-fixture", "model": model,
            "choices": [{"message": {"content": "observed"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
        });
        Ok(HttpResponse::new(
            200,
            BTreeMap::new(),
            body.to_string().into(),
            req.url.clone(),
        ))
    }
    async fn send_streaming(&self, _: HttpRequest) -> Result<HttpStreamResponse, HttpError> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        panic!("no route here streams");
    }
}

/// Everything one run left behind.
struct Ran {
    outcome: RunOutcome,
    events: Vec<Event>,
    /// The inspect cell the run published its records and ledger to.
    cell: Arc<LiveInspect>,
}

/// The seams of one run: the provider's HTTP seam, the agent verb's provider and the shell. An
/// `agent:` calls its provider, never the HTTP seam, so a refused agent leaves both untouched.
struct Seams {
    wire: Arc<Wire>,
    agent: Arc<MockProvider>,
    shell: MockShell,
}

impl Seams {
    fn new(shell: MockShell) -> Self {
        Self {
            wire: Arc::new(Wire::default()),
            agent: Arc::new(MockProvider::new("mock")),
            shell,
        }
    }
}

/// Run `wf` on `report` with `secrets` bound, an `openai` key configured over [`Wire`], and an
/// inspect cell attached.
async fn run_on(
    wf: &nika_schema::raw::RawWorkflow,
    report: &nika_check::CheckReport,
    secrets: &[(&str, &str)],
    seams: &Seams,
) -> Ran {
    let config = ProvidersConfig::new().with_key("openai", Secret::new("fixture"));
    let registry = ProviderRegistry::new(Arc::clone(&seams.wire), config);
    let invoke = Arc::new(InvokeVerb::new(Arc::new(MockToolExecutor::new())));
    let cell = Arc::new(LiveInspect::new());
    let runtime = Runtime::new(
        ExecVerb::new(Arc::new(seams.shell.clone())),
        Arc::clone(&invoke),
        InferVerb::new(Arc::new(registry), "mock/echo"),
        AgentVerb::new(
            Arc::clone(&seams.agent),
            invoke,
            Arc::new(MockToolDefinitionProvider::new()),
            "mock/echo",
        ),
        nika_clock::DeclaredClock::system(),
        RuntimeConfig::default(),
    )
    .with_secret_resolver(Arc::new(MapSecrets(
        secrets
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect(),
    )))
    .with_inspect(Arc::clone(&cell));
    let mut stamper = DeterministicStamper::new();
    let mut sink = VecSink::new();
    let outcome = runtime
        .run(wf, report, &mut stamper, &mut sink)
        .await
        .expect("the launch cannot judge a field only the run renders");
    Ran {
        outcome,
        events: sink.into_events(),
        cell,
    }
}

/// `yaml` run on the report a check that did not judge `model:` or `cwd:` gave: today's check
/// refuses the file on the tasks `refused` names, and that older one passed it clean.
async fn run_on_a_stale_report(
    yaml: &str,
    refused: &[&str],
    secrets: &[(&str, &str)],
    seams: &Seams,
) -> Ran {
    let wf = parse(yaml, FileId::new(0), ParseMode::Strict).expect("fixture parses");
    let mut report = check(&wf);
    assert_eq!(
        report
            .secret_leaks
            .iter()
            .map(|l| l.task.as_str())
            .collect::<BTreeSet<_>>(),
        refused.iter().copied().collect::<BTreeSet<_>>(),
        "check refuses the file on those tasks: {:?}",
        report.secret_leaks
    );
    report.secret_leaks.clear();
    assert!(report.is_clean(), "{report:?}");
    run_on(&wf, &report, secrets, seams).await
}

/// The tariff table every run journals as it starts, the same whatever the workflow: it names
/// every priced model, `gpt-4o` among them, so the scan below leaves it out (and the model
/// witness proves it does not depend on the secret).
const TARIFF_TABLE: &str = "admission_pricing";

/// The tariff table `ran` journaled.
fn tariff_table(ran: &Ran) -> Option<&str> {
    ran.events
        .iter()
        .find(|e| e.kind == EventKind::WorkflowStarted)?
        .str_field(TARIFF_TABLE)
}

/// Every place `value` reached: a string field of an event, the event's trace line (its JSON
/// serialization, the line the trace file holds), the settlement, the ledger and records the
/// inspect cell reads, and the run's records. The tariff table of `WorkflowStarted` is left out,
/// and only there: the same field on any other event is scanned.
fn places_carrying(ran: &Ran, value: &str) -> Vec<String> {
    let mut places = Vec::new();
    for event in &ran.events {
        let mut line = event.clone();
        if event.kind == EventKind::WorkflowStarted {
            line.fields.retain(|f| f.key != TARIFF_TABLE);
        }
        for f in &line.fields {
            if let FieldValue::String(s) = &f.value
                && s.contains(value)
            {
                places.push(format!("{:?}.{}", event.kind, f.key));
            }
        }
        if serde_json::to_string(&line)
            .expect("an event serializes")
            .contains(value)
        {
            places.push(format!("{:?} trace line", event.kind));
        }
    }
    let settled = nika_event::settlement::RunSettlement::from_events(&ran.events);
    for (place, text) in [
        ("settlement", format!("{:?}", ran.outcome.settlement)),
        ("settlement from the trace", format!("{settled:?}")),
        ("ledger", ran.cell.cost().to_string()),
        ("inspect records", ran.cell.records().to_string()),
        ("records", format!("{:?}", ran.outcome.records)),
    ] {
        if text.contains(value) {
            places.push(place.to_owned());
        }
    }
    places
}

/// The string field `key` of the first `kind` event of `task`.
fn field_of<'e>(events: &'e [Event], kind: EventKind, task: &str, key: &str) -> Option<&'e str> {
    events
        .iter()
        .find(|e| e.kind == kind && e.str_field("task") == Some(task))?
        .str_field(key)
}

/// The frames of a task refused before it starts: scheduled, then failed, never started.
const REFUSED_FRAMES: [EventKind; 2] = [EventKind::TaskScheduled, EventKind::TaskFailed];

/// The kinds of the events of `task`, in order.
fn kinds_of(events: &[Event], task: &str) -> Vec<EventKind> {
    events
        .iter()
        .filter(|e| e.str_field("task") == Some(task))
        .map(|e| e.kind)
        .collect()
}

/// The refusal a task records when `secrets.<secret>` reaches its `field` along `path`.
fn refusal(task: &str, field: &str, path: &str) -> (String, String) {
    let remedy = if field == "model" {
        "name the model there, or read it from `inputs:` or `const:`"
    } else {
        "name the directory there, or read it from `inputs:` or `const:`"
    };
    (
        "NIKA-SEC-006".to_owned(),
        format!(
            "refused before the task ran: a secret reaches the `{field}:` of task `{task}` \
             ({path}), and the run renders no secret there · {remedy}"
        ),
    )
}

/// The code and message of `task`'s error record.
fn error_of(ran: &Ran, task: &str) -> (String, String) {
    let err = ran.outcome.records[task]
        .error
        .as_ref()
        .expect("the refusal is on record");
    (err.code.clone(), err.message.clone())
}

/// The reviewer's witness: a model route rendered from a six-byte secret. Without the refusal,
/// the provider received `gpt-4o`, and the task frame's `inference_calls`, the ledger's sources,
/// the settlement and the inspect cell carried it, the event lane's scrub sparing a value that
/// short outside its payload fields.
#[tokio::test]
async fn a_secret_in_a_model_route_is_refused_before_any_call() {
    let yaml = r#"nika: secret-route
permits: {}
secrets:
  k: { source: env, key: MY_K }
tasks:
  ask:
    infer: { prompt: hi, model: "openai/${{ secrets.k }}", max_tokens: 64 }
"#;
    let seams = Seams::new(MockShell::new());
    let ran = run_on_a_stale_report(yaml, &["ask"], &[("k", "gpt-4o")], &seams).await;

    assert_eq!(places_carrying(&ran, "gpt-4o"), Vec::<String>::new());
    // The table the scan leaves out is the same under another value of the secret.
    let other = run_on_a_stale_report(
        yaml,
        &["ask"],
        &[("k", "q7Zpx")],
        &Seams::new(MockShell::new()),
    )
    .await;
    assert_eq!(
        (tariff_table(&ran).is_some(), tariff_table(&ran)),
        (true, tariff_table(&other))
    );
    assert_eq!(
        *seams.wire.models.lock().expect("models"),
        Vec::<String>::new(),
        "no provider request"
    );
    assert!(!ran.outcome.ok);
    assert_eq!(
        error_of(&ran, "ask"),
        refusal("ask", "model", "secrets.k → model @ ask")
    );
    assert_eq!(kinds_of(&ran.events, "ask"), REFUSED_FRAMES);
    let (code, message) = refusal("ask", "model", "secrets.k → model @ ask");
    assert_eq!(
        (
            field_of(&ran.events, EventKind::TaskFailed, "ask", "note"),
            field_of(&ran.events, EventKind::TaskFailed, "ask", "detail"),
        ),
        (Some("secret"), Some(format!("{code} · {message}").as_str())),
        "the frame names the stage and the refusal"
    );
    assert_eq!(
        field_of(&ran.events, EventKind::TaskFailed, "ask", "inference_calls"),
        None
    );
    assert_eq!(
        (ran.outcome.priced_calls, ran.outcome.unpriced_calls),
        (0, 0),
        "no call reached the ledger"
    );
}

/// The same for an `exec:` `cwd:`: the program is never spawned in a directory a secret names.
#[tokio::test]
async fn a_secret_in_an_exec_cwd_is_refused_before_the_spawn() {
    let yaml = r#"nika: secret-cwd
permits: { exec: ["true"] }
secrets:
  d: { source: env, key: MY_D }
tasks:
  run:
    exec: { command: ["true"], cwd: "${{ secrets.d }}" }
"#;
    let seams = Seams::new(MockShell::new().enqueue_ok(""));
    let ran = run_on_a_stale_report(yaml, &["run"], &[("d", "q7Zp")], &seams).await;

    assert_eq!(places_carrying(&ran, "q7Zp"), Vec::<String>::new());
    assert_eq!(
        seams
            .shell
            .executed_commands()
            .iter()
            .map(|c| c.cwd.clone())
            .collect::<Vec<_>>(),
        Vec::<Option<std::path::PathBuf>>::new(),
        "no spawn"
    );
    assert!(!ran.outcome.ok);
    assert_eq!(
        error_of(&ran, "run"),
        refusal("run", "cwd", "secrets.d → cwd @ run")
    );
    assert_eq!(kinds_of(&ran.events, "run"), REFUSED_FRAMES);
}

/// `pick` prints the token (the token may reach `exec`) and joins `picks`; `ask` binds `WITH`
/// and calls `VERB` on `MODEL`.
fn routed(verb: &str, with: &str, model: &str) -> String {
    let limit = if verb == "agent" {
        "max_tokens_total"
    } else {
        "max_tokens"
    };
    format!(
        r#"nika: secret-model
secrets:
  token: {{ source: env, key: T, egress: [{{ to: "exec" }}] }}
permits:
  exec: ["printf", "false"]
tasks:
  pick:
    group: picks
    exec: {{ command: [printf, "${{{{ secrets.token }}}}"] }}
  ask:
{with}    {verb}: {{ model: "{model}", prompt: hi, {limit}: 10 }}
"#
    )
}

/// Every way the check follows a secret into a `model:` is refused at run, on the task the check
/// names and with the path it gives.
#[tokio::test]
async fn every_way_a_secret_reaches_a_model_is_refused_at_run() {
    let alias = "    with: { m: \"${{ secrets.token }}\" }\n";
    let output = "    with: { m: \"${{ tasks.pick.output }}\" }\n";
    let group = "    with: { m: \"${{ group.picks }}\" }\n";
    let item = "    for_each: { items: [\"${{ secrets.token }}\"] }\n";
    let cases = [
        (
            "infer",
            "",
            "${{ secrets.token }}",
            "secrets.token → model @ ask",
        ),
        (
            "agent",
            "",
            "${{ secrets.token }}",
            "secrets.token → model @ ask",
        ),
        (
            "infer",
            "",
            "openai/${{ secrets.token }}",
            "secrets.token → model @ ask",
        ),
        (
            "infer",
            alias,
            "${{ with.m }}",
            "secrets.token → with.m @ ask → model @ ask",
        ),
        (
            "infer",
            output,
            "${{ with.m }}",
            "secrets.token → tasks.pick.output → with.m @ ask → model @ ask",
        ),
        (
            "infer",
            group,
            "${{ with.m }}",
            "secrets.token → tasks.pick.output → with.m @ ask → model @ ask",
        ),
        (
            "infer",
            item,
            "${{ item }}",
            "secrets.token → item @ ask → model @ ask",
        ),
    ];
    for (verb, with, model, path) in cases {
        let seams = Seams::new(MockShell::new().enqueue_ok("printed"));
        let yaml = routed(verb, with, model);
        let ran = run_on_a_stale_report(&yaml, &["ask"], &[("token", "q7")], &seams).await;
        assert_eq!(
            error_of(&ran, "ask"),
            refusal("ask", "model", path),
            "{verb} {with} {model}"
        );
        assert_eq!(kinds_of(&ran.events, "ask"), REFUSED_FRAMES, "{yaml}");
        assert_eq!(
            *seams.wire.models.lock().expect("models"),
            Vec::<String>::new()
        );
        assert_eq!(
            seams.agent.captured_requests().len(),
            0,
            "no request reaches the agent's provider: {verb} {with} {model}"
        );
    }
}

/// A recovered value reaches the route as the check says: `pick` fails and recovers to the
/// token, and `ask` calls on the model `pick`'s record then names.
#[tokio::test]
async fn a_recovered_secret_in_a_model_is_refused_at_run() {
    let yaml = r#"nika: recover-model
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
    infer: { model: "${{ with.m }}", prompt: hi, max_tokens: 10 }
"#;
    let seams = Seams::new(MockShell::new().enqueue_fail(1, "no"));
    let ran = run_on_a_stale_report(yaml, &["ask"], &[("token", "q7")], &seams).await;
    assert_eq!(
        error_of(&ran, "ask"),
        refusal(
            "ask",
            "model",
            "secrets.token → on_error.recover @ pick → tasks.pick.output → with.m @ ask → \
             model @ ask"
        )
    );
    assert_eq!(kinds_of(&ran.events, "ask"), REFUSED_FRAMES);
    assert_eq!(seams.wire.requests(), 0);
    assert_eq!(
        *seams.wire.models.lock().expect("models"),
        Vec::<String>::new()
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

/// On a tie between two paths of one length, the run's refusal names the path the check
/// finding names, never the other one.
#[tokio::test]
async fn a_tie_between_two_paths_is_refused_on_the_checks_path() {
    let wf = parse(TIE_MODEL, FileId::new(0), ParseMode::Strict).expect("fixture parses");
    let checked: Vec<String> = check(&wf)
        .secret_leaks
        .iter()
        .filter(|l| l.task == "ask")
        .map(|l| l.trace.clone())
        .collect();
    assert_eq!(
        checked,
        [
            "secrets.token → tasks.c.output → with.v @ a → tasks.a.output → with.x @ m → \
         tasks.m.output → with.m @ ask → model @ ask"
        ]
    );
    let seams = Seams::new(
        MockShell::new()
            .enqueue_ok("printed")
            .enqueue_ok("printed")
            .enqueue_ok("printed")
            .enqueue_ok("printed")
            .enqueue_ok("printed"),
    );
    let ran = run_on_a_stale_report(TIE_MODEL, &["ask"], &[("token", "q7")], &seams).await;
    assert_eq!(error_of(&ran, "ask"), refusal("ask", "model", &checked[0]));
    assert_eq!(kinds_of(&ran.events, "ask"), REFUSED_FRAMES);
    assert_eq!(seams.wire.requests(), 0);
}

/// Every way into a `cwd:`: a `with:` alias, a task output, a `for_each` item.
#[tokio::test]
async fn every_way_a_secret_reaches_a_cwd_is_refused_at_run() {
    let head = r#"nika: secret-cwd
secrets:
  token: { source: env, key: T, egress: [{ to: "exec" }] }
permits:
  exec: ["printf", "true"]
tasks:
  pick:
    exec: { command: [printf, "${{ secrets.token }}"] }
  run:
"#;
    let cases = [
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
            "    for_each: { items: [\"${{ secrets.token }}\"] }\n",
            "${{ item }}",
            "secrets.token → item @ run → cwd @ run",
        ),
    ];
    for (with, dir, path) in cases {
        let yaml = format!("{head}{with}    exec: {{ command: [\"true\"], cwd: \"{dir}\" }}\n");
        let seams = Seams::new(MockShell::new().enqueue_ok("printed").enqueue_ok(""));
        let ran = run_on_a_stale_report(&yaml, &["run"], &[("token", "q7")], &seams).await;
        assert_eq!(error_of(&ran, "run"), refusal("run", "cwd", path), "{yaml}");
        assert_eq!(kinds_of(&ran.events, "run"), REFUSED_FRAMES, "{yaml}");
        // Only `pick` was spawned, in the directory the run inherits.
        assert_eq!(
            seams
                .shell
                .executed_commands()
                .iter()
                .map(|c| c.cwd.clone())
                .collect::<Vec<_>>(),
            [None],
            "{yaml}"
        );
    }
}

/// The `on_finally` lane's journal as `(decision, why)`, in order.
fn cleanup_journal(events: &[Event]) -> Vec<(String, String)> {
    events
        .iter()
        .filter(|e| {
            e.kind == EventKind::PermitChecked && e.str_field("plane") == Some("on_finally")
        })
        .map(|e| {
            let text = |key| e.str_field(key).unwrap_or_default().to_owned();
            (text("decision"), text("why"))
        })
        .collect()
}

/// An `unwind` cleanup is a task: a secret in its `cwd:` or `model:` refuses it before anything
/// renders, journaled on the cleanup lane like any cleanup failure, and nothing is spawned in that
/// directory or sent on that route.
#[tokio::test]
async fn a_cleanup_with_a_secret_route_is_refused_before_it_renders() {
    for (cleanup, field) in [
        (
            r#"exec: { command: ["true"], cwd: "${{ secrets.token }}" }"#,
            "cwd",
        ),
        (
            r#"infer: { prompt: bye, model: "openai/${{ secrets.token }}", max_tokens: 10 }"#,
            "model",
        ),
    ] {
        let yaml = format!(
            "nika: cleanup-route\nsecrets:\n  token: {{ source: env, key: T }}\npermits: {{ exec: \
             [\"true\"] }}\ntasks:\n  work:\n    exec: {{ command: [\"true\"] }}\n  tidy:\n    \
             after: {{ work: unwind }}\n    {cleanup}\n"
        );
        let seams = Seams::new(MockShell::new().enqueue_ok("done").enqueue_ok(""));
        let ran = run_on_a_stale_report(&yaml, &["tidy"], &[("token", "q7Zp")], &seams).await;
        // `work` ran in the directory the run inherits; `tidy` never started.
        assert_eq!(
            seams
                .shell
                .executed_commands()
                .iter()
                .map(|c| c.cwd.clone())
                .collect::<Vec<_>>(),
            [None],
            "{field}"
        );
        assert_eq!(
            *seams.wire.models.lock().expect("models"),
            Vec::<String>::new()
        );
        let (code, message) = refusal("tidy", field, &format!("secrets.token → {field} @ tidy"));
        assert_eq!(
            cleanup_journal(&ran.events),
            [(
                "failure".to_owned(),
                format!(
                    "cleanup failed — the error does not propagate but is journaled (spec 03 \
                     §unwind guarantee 3) · {code}: {message}"
                )
            )],
            "{field}"
        );
        assert_eq!(
            places_carrying(&ran, "q7Zp"),
            Vec::<String>::new(),
            "{field}"
        );
    }
}

/// The task frame's single inference call of `task`.
fn call_of(events: &[Event], task: &str) -> serde_json::Value {
    let calls: serde_json::Value = serde_json::from_str(
        field_of(events, EventKind::TaskCompleted, task, "inference_calls")
            .expect("the calls ride"),
    )
    .expect("JSON");
    let [call] = calls.as_array().expect("an array").as_slice() else {
        panic!("{task}: {calls}");
    };
    call.clone()
}

/// A model the run never renders from a secret is rendered as written, beside a one-byte secret
/// that occurs in it: the literal and the input both reach the provider as `gpt-4.1`, the frame
/// names `openai/gpt-4.1`, and the pricing projection parses.
#[tokio::test]
async fn a_literal_or_an_input_model_renders_as_written_beside_a_short_secret() {
    let yaml = r#"nika: literal-and-input
permits: { exec: ["printf"] }
inputs:
  m: { type: string, required: false, default: "openai/gpt-4.1" }
secrets:
  k: { source: env, key: MY_K, egress: [{ to: "exec" }] }
tasks:
  use_it:
    exec: { command: [printf, "${{ secrets.k }}"] }
  lit:
    infer: { prompt: hi, model: "openai/gpt-4.1", max_tokens: 64 }
  inp:
    infer: { prompt: hi, model: "${{ inputs.m }}", max_tokens: 64 }
"#;
    let wf = parse(yaml, FileId::new(0), ParseMode::Strict).expect("fixture parses");
    let report = check(&wf);
    assert!(report.is_clean(), "{report:?}");
    let seams = Seams::new(MockShell::new().enqueue_ok("4"));
    let ran = run_on(&wf, &report, &[("k", "4")], &seams).await;

    assert!(ran.outcome.ok, "{:?}", ran.outcome.records);
    assert_eq!(
        *seams.wire.models.lock().expect("models"),
        ["gpt-4.1", "gpt-4.1"]
    );
    let mut rendered = Vec::new();
    for task in ["lit", "inp"] {
        assert_eq!(
            field_of(&ran.events, EventKind::TaskCompleted, task, "model"),
            Some("openai/gpt-4.1"),
            "{task}"
        );
        let call = call_of(&ran.events, task);
        assert_eq!(
            (&call["route"]["provider"], &call["route"]["model"]),
            (&serde_json::json!("openai"), &serde_json::json!("gpt-4.1")),
            "{task}"
        );
        let pricing: serde_json::Value = serde_json::from_str(
            field_of(&ran.events, EventKind::TaskCompleted, task, "pricing_route")
                .expect("the pricing rides"),
        )
        .expect("the pricing parses");
        assert_eq!(pricing, call["pricing"], "{task}");
        rendered.push((call["route"].clone(), pricing));
    }
    assert_eq!(
        rendered[0], rendered[1],
        "the input renders as the literal does"
    );
}

/// A media builtin's `args.model` is the model the tool calls, and it names the file the tool
/// saves: a secret there, under the rule that lets it reach the tool, is refused before the call,
/// and nothing the run writes carries it (a lowercase value, as the saved file's name would).
#[tokio::test]
async fn a_secret_in_a_media_model_is_refused_before_the_call() {
    for (tool, text) in [
        ("nika:tts_generate", "text"),
        ("nika:image_generate", "prompt"),
    ] {
        let yaml = format!(
            r#"nika: secret-tool-model
secrets:
  k: {{ source: env, key: MY_K, egress: [{{ to: "{tool}" }}] }}
permits:
  tools: ["{tool}"]
  fs: {{ write: ["./out/**"] }}
tasks:
  say:
    invoke: {{ tool: "{tool}", args: {{ provider: mock, model: "${{{{ secrets.k }}}}", {text}: hi, output_dir: "./out" }} }}
"#
        );
        let seams = Seams::new(MockShell::new());
        let ran = run_on_a_stale_report(&yaml, &["say"], &[("k", "canary-q7z")], &seams).await;
        assert!(!ran.outcome.ok, "{tool}");
        assert_eq!(
            error_of(&ran, "say"),
            (
                "NIKA-SEC-006".to_owned(),
                "refused before the task ran: a secret reaches the `args.model:` of task `say` \
                 (secrets.k → args.model @ say), and the run renders no secret there · name the \
                 model there, or read it from `inputs:` or `const:`"
                    .to_owned()
            ),
            "{tool}"
        );
        assert_eq!(kinds_of(&ran.events, "say"), REFUSED_FRAMES, "{tool}");
        assert_eq!(
            places_carrying(&ran, "canary-q7z"),
            Vec::<String>::new(),
            "{tool}"
        );
    }
}
