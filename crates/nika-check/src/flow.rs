// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! Information-flow control — the taint engine (ADR-092 · the keystone).
//!
//! A confidentiality lattice (Denning · *A Lattice Model of Secure
//! Information Flow* · CACM 1976) traces `secrets.X` taint through the
//! workflow ·
//!
//! - **sources** — every `${{ secrets.X }}` reference is `Secret`.
//! - **propagation** — a `with:` binding that references a secret makes
//!   that `with.K` slot secret WITHIN its task; under a `for_each:`, a
//!   binding that reads `item` (or `index`) renders once per iteration with
//!   the item bound (spec 03, `for_each` semantics), so it carries what the
//!   item carries, while the `for_each:` collection, evaluated before the
//!   fan-out, reads only the other bindings; an `exec`/`invoke` task
//!   whose effect-carrying fields reference a secret (directly, via a
//!   `with:` alias, or via a tainted upstream record) makes its OUTPUT
//!   secret (the captured stdout/tool-result can embed the value);
//!   `infer`/`agent` outputs are NOT tainted from a prompt secret — the
//!   provider is operator-chosen and trusted by that choice, and the model
//!   response is not a verbatim echo (ADR-092 carve-out). A task's
//!   `on_error.recover:` value replaces its output whatever the verb, so the
//!   record also carries what that value reads; a `group.<name>` fold
//!   carries what each member record carries.
//! - **sinks** — a secret reaching an `exec`/`invoke` effect is a LEAK; a
//!   secret reaching the workflow `outputs:` is an EGRESS (the value leaves
//!   the run as the return). A secret reaching a ROUTE ([`Route`]: an
//!   `infer:`/`agent:` `model:`, an `exec:`'s `cwd:`, an `agent:`'s
//!   `images.save:`, a media builtin's `args.model`) is refused: the run
//!   renders each with secrets resolved, as the provider and model it calls,
//!   the directory it spawns the program in or the file it writes, and
//!   neither a model name nor a path is ever a secret, so no `egress:` rule
//!   clears it (the run refuses such a task before the field renders). A task
//!   model carries nothing into the record (the output carve-out above); the
//!   other routes do: a program can print its working directory, an agent's
//!   output names the path of each image it saved, and a media builtin names
//!   its model in the file it saves and in its output.
//!
//! Every slot holds one trace PER SECRET, the most direct one found (fewest
//! hops; a tie keeps the trace read first), so a record that carries two
//! secrets passes both on and each sink judges each of them.
//!
//! **Soundness.** The scheduling graph is acyclic BY CONSTRUCTION (ADR-002 ·
//! cycle = `NIKA-DAG-001`), but it does not order every read: a `recover:`
//! read is no execution-order edge (spec 05 §recover · the run awaits the
//! source), so a recover can read a task later in its own wave, in a later
//! wave, or one that recovers from it in turn. One pass in topological order
//! would judge such a record before the record it reads is known. The pass
//! therefore repeats its sweep until a sweep moves no record: no new
//! (task, secret) pair and no shorter trace. Propagation only adds pairs and
//! only shortens traces, so it settles, and it settles within
//! `tasks × secrets` sweeps (a most direct trace crosses each (task, secret)
//! pair at most once, and each sweep settles one more pair of it); the loop is
//! bounded by that count plus one. Findings are read once, from the settled
//! facts. Should the loop ever stop at the bound with work left, the facts
//! fail closed: every task is judged as if every declared secret reached its
//! effect and its record, so only a send each secret's `egress:` allows can
//! stay clean.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use nika_schema::Spanned;
use nika_schema::expression::{NamespaceRef, expr_refs, scan_templates};
use nika_schema::raw::{
    ForEachValue, RawAction, RawInvokeAction, RawInvokeTarget, RawTask, RawWorkflow,
};
use nika_schema::types::{EgressRule, OnErrorAction, OutputDecl, Permits};

use super::declass;

/// Secret name → the most direct trace of that secret reaching one slot.
type Taints = BTreeMap<String, TaintTrace>;
type WithTaints<'a> = BTreeMap<&'a str, Taints>;
/// Task index → every secret its record carries.
type Records = BTreeMap<usize, Taints>;
/// Task index → route → every secret that route reads.
type Routes = BTreeMap<usize, BTreeMap<Route, Taints>>;

/// One link in a [`TaintTrace`] hop chain — a structurally-shared cons-list.
/// `via` prepends a link in O(1) (an `Arc` bump of the shared tail), never a
/// copy, so carrying a length-k trace through k propagation hops stays O(k)
/// total instead of O(k²). Materialized to a string only when a diagnostic
/// renders a real leak (rare) — never on the propagation hot path.
#[derive(Debug, PartialEq, Eq)]
struct Hop {
    label: Arc<str>,
    /// The earlier hop (one step closer to the source); `None` at the root.
    prev: Option<Arc<Hop>>,
}

/// How a value became `Secret` — the chain from the originating secret to
/// the current slot, for auditable diagnostics (« via `with.tok` ← via
/// `tasks.a.output` ← `secrets.api_key` »).
///
/// The hop chain is an `Arc` cons-list (the private `Hop`): extending or cloning a
/// trace is O(1), so the IFC pass can carry one trace per secret per slot across
/// up to `MAX_TASKS` waves without the O(n²) memory + copy blow-up a
/// per-trace `Vec<String>` caused (a 0.89 MiB workflow → 5.2 GB · 2026-06-16
/// Gate-11 finding). The reachable hops are unchanged; only the storage is.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct TaintTrace {
    /// The originating `secrets.<name>`.
    pub secret: String,
    /// The most-recent hop; walk `prev` to reach the source.
    head: Arc<Hop>,
    /// The hops between the source and `head`: what « most direct » compares.
    hops: usize,
}

impl TaintTrace {
    /// A fresh trace rooted at a secret reference.
    fn source(secret: &str) -> Self {
        Self {
            secret: secret.to_owned(),
            head: Arc::new(Hop {
                label: format!("secrets.{secret}").into(),
                prev: None,
            }),
            hops: 0,
        }
    }

    /// Extend the chain by one hop, preserving the origin — O(1): the existing
    /// chain is shared by `Arc`, not copied.
    fn via(&self, hop: String) -> Self {
        Self {
            secret: self.secret.clone(),
            head: Arc::new(Hop {
                label: hop.into(),
                prev: Some(Arc::clone(&self.head)),
            }),
            // One allocated hop each: the count cannot outgrow memory, let alone `usize`.
            hops: self.hops.saturating_add(1),
        }
    }

    /// The chain rendered for a diagnostic (`secrets.x → with.t → ...`),
    /// source-first. Walks the shared cons-list — O(depth), only on a leak.
    #[must_use]
    pub fn render(&self) -> String {
        let mut labels: Vec<&str> = Vec::new();
        let mut cur = Some(&self.head);
        while let Some(node) = cur {
            labels.push(&node.label);
            cur = node.prev.as_ref();
        }
        labels.reverse();
        labels.join(" → ")
    }
}

/// The fact base produced by the settled pass — the taint slice of the
/// unified flow IR (ADR-092 #3). Reports read from this; they never
/// re-walk the AST for taint.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct FlowFacts {
    /// Task index → every secret reaching its EFFECT, one most direct trace
    /// each (absent = clean). This is the PROPAGATION fact — an exec/invoke
    /// output carries each of them regardless of any egress sanction (a
    /// sanctioned egress still lets the captured output embed the secret ·
    /// the sanction clears the SEND, not the capture). The REPORTABLE leaks
    /// are `effect_leaks` (sanction-filtered).
    effect_taint: BTreeMap<usize, Taints>,
    /// Task index → every UNSANCTIONED secret edge reaching that effect.
    effect_leaks: BTreeMap<usize, Taints>,
    /// Task index → every secret each of its routes ([`Route`]) reads, one
    /// most direct trace each, ending on the route's `<field> @ <task>` hop.
    /// Every one is reportable: no `egress:` rule clears a route.
    route_taint: Routes,
    /// Task index → every secret its RECORD carries (`tasks.<id>.output`, and
    /// each `group.<name>` fold it joins): what an exec/invoke effect saw and
    /// a route the output names reads (an exec's `cwd:`, an agent's
    /// `images.save:`, a media builtin's `args.model`), and what its
    /// `on_error.recover:` value reads
    /// (the recovered value replaces the output whatever the verb · spec 05
    /// §recovery).
    output_taint: Records,
    /// Workflow `outputs:` entry name → every secret reaching it unsanctioned
    /// (egress).
    egress: BTreeMap<String, Taints>,
}

impl FlowFacts {
    /// One secret reaching `task`'s effect, if any — the first by name, with
    /// its most direct trace. The PROPAGATION fact (independent of any egress
    /// sanction).
    #[must_use]
    pub fn effect_taint(&self, task: usize) -> Option<&TaintTrace> {
        self.effect_taint.get(&task)?.values().next()
    }

    /// Every secret reaching `task`'s effect, one most direct trace each, by
    /// secret name.
    pub(crate) fn effect_taints(&self, task: usize) -> impl Iterator<Item = &TaintTrace> {
        self.effect_taint
            .get(&task)
            .into_iter()
            .flat_map(BTreeMap::values)
    }

    /// The first UNSANCTIONED effect leak of `task` by secret name, if any.
    #[must_use]
    pub fn effect_leak(&self, task: usize) -> Option<&TaintTrace> {
        self.effect_leaks.get(&task)?.values().next()
    }

    /// Every independently judged UNSANCTIONED edge at `task`'s effect.
    pub(crate) fn effect_leaks(&self, task: usize) -> impl Iterator<Item = &TaintTrace> {
        self.effect_leaks
            .get(&task)
            .into_iter()
            .flat_map(BTreeMap::values)
    }

    /// Every secret reaching one of `task`'s routes ([`Route`]), one most
    /// direct trace each, route by route and by secret name within a route.
    pub(crate) fn route_taints(&self, task: usize) -> impl Iterator<Item = (Route, &TaintTrace)> {
        self.route_taint
            .get(&task)
            .into_iter()
            .flatten()
            .flat_map(|(&route, taints)| taints.values().map(move |trace| (route, trace)))
    }

    /// Every workflow-output egress, one row per (entry, secret), sorted by
    /// entry name then secret name.
    #[must_use]
    pub fn egresses(&self) -> Vec<(&str, &TaintTrace)> {
        self.egress
            .iter()
            .flat_map(|(name, taints)| taints.values().map(move |trace| (name.as_str(), trace)))
            .collect()
    }
}

/// Run the IFC analysis over every task of a workflow. The analyzer's
/// `topo_waves` only order the sweeps (a task absent from them, an unwind
/// task or every task of a workflow whose order was refused, follows in
/// declaration order): the pass repeats until the records settle, so the
/// order decides how fast, never what is found.
#[must_use]
pub(super) fn analyze_flow(wf: &RawWorkflow, waves: &[Vec<usize>]) -> FlowFacts {
    analyze_flow_within(wf, waves, sweep_bound).0
}

/// [`analyze_flow`] with the sweep bound as a parameter, and how many task
/// evaluations the sweeps took: the tests reach the fail-closed path through
/// a bound smaller than the proof's, and measure the work.
fn analyze_flow_within(
    wf: &RawWorkflow,
    waves: &[Vec<usize>],
    bound: fn(usize, usize) -> u128,
) -> (FlowFacts, usize) {
    let scope = Scope::of(wf);
    let mut facts = FlowFacts::default();
    if scope.declared.is_empty() {
        return (facts, 0); // no secrets declared → nothing can be tainted
    }
    // The per-secret egress policy (declass · ADR-092) — looked up by name
    // at every leak edge. Empty list / absent = default-deny.
    let egress_of: BTreeMap<&str, &[EgressRule]> = wf
        .secrets
        .iter()
        .map(|(n, s)| (n.value.as_str(), s.value.egress.as_slice()))
        .collect();
    let permits = wf.permits.as_ref().map(|p| &p.value);
    let reads: Vec<Reads<'_>> = wf
        .tasks
        .iter()
        .map(|t| Reads::of(&t.value, &scope))
        .collect();

    // The sweep order: the waves, then the UNWIND tasks, which carry effects
    // but never enter wave assignment (spec 03 · E_f never schedules). The
    // waves are this pass's ORDERING device, not its census: a task missing
    // from them is a task no judge sees, and an unseen effect carrier is
    // exactly the hole `unwind` was minted to close. They run after their
    // producer settles, so appending them here IS their order.
    let seen: BTreeSet<usize> = waves.iter().flatten().copied().collect();
    let order: Vec<usize> = waves
        .iter()
        .flatten()
        .copied()
        .chain((0..wf.tasks.len()).filter(|idx| !seen.contains(idx)))
        .collect();
    let bound = bound(wf.tasks.len(), scope.declared.len());
    let settled = settle(&wf.tasks, &order, &reads, bound);
    let reached = if settled.unsettled {
        worst_case(&wf.tasks, &reads, &scope.declared, bound)
    } else {
        settled.reached
    };

    // The findings read the settled facts, once.
    for (idx, effect) in reached.effects {
        record_effect_leaks(
            idx,
            &wf.tasks[idx].value,
            &effect,
            &egress_of,
            permits,
            &mut facts,
        );
        facts.effect_taint.insert(idx, effect);
    }
    // A route is refused whatever the secret's `egress:`: no sanction filter.
    facts.route_taint = reached.routes;
    facts.output_taint = reached.records;
    judge_outputs(wf, &scope, &egress_of, &mut facts);
    (facts, settled.evaluations)
}

/// The facts when the sweeps stop at `bound` with work left (unreachable by
/// the bound's argument, so a bug if it ever happens): every task's effect and
/// record carry every declared secret, each traced to this hop, and so does
/// every route ([`Route`]) that reads anything a secret could ride
/// (a `with.*` slot, the `item`, a record or a secret). The usual judges then
/// read them, so every send and `outputs:` entry a secret's `egress:` does not
/// allow refuses, every such route refuses, and the trace says why.
fn worst_case(
    tasks: &[Spanned<RawTask>],
    reads: &[Reads<'_>],
    declared: &BTreeSet<&str>,
    bound: u128,
) -> Reached {
    let hop = format!(
        "the secret flow stopped at its sweep bound ({bound}) before settling: judged as if \
         every declared secret reached this task"
    );
    let every: Taints = declared
        .iter()
        .map(|&secret| {
            (
                secret.to_owned(),
                TaintTrace::source(secret).via(hop.clone()),
            )
        })
        .collect();
    let all: Records = (0..tasks.len()).map(|idx| (idx, every.clone())).collect();
    let routes: Routes = reads
        .iter()
        .enumerate()
        .filter_map(|(idx, task_reads)| {
            let reached: BTreeMap<Route, Taints> = task_reads
                .route
                .iter()
                .filter(|(_, route_reads)| !route_reads.is_empty())
                .map(|(route, _)| {
                    (
                        *route,
                        via_each(every.clone(), &route.hop(&tasks[idx].value)),
                    )
                })
                .collect();
            (!reached.is_empty()).then_some((idx, reached))
        })
        .collect();
    Reached {
        records: all.clone(),
        effects: all,
        routes,
    }
}

/// Egress: every secret a workflow `outputs:` entry carries —
/// UNLESS that secret declassifies the workflow boundary itself
/// (`egress: [{ to: "outputs" }]` · spec 01-envelope §egress · the DLM
/// owner's act, same as every sink; absent = default-deny, the report
/// stands). Sink-only and secret-SPECIFIC: it clears nothing but THIS
/// secret's taints reaching `outputs:` — never a send, never another secret
/// the same entry carries. A secret the entry reads ITSELF is reported
/// whatever the rule, rooted at THAT secret: the engine masks a secret's own
/// value on every surface it writes, the outputs export included, so the rule
/// declassifies what is DERIVED from a secret, never the value (E39 N7). One
/// row per secret, as a sink with two unsanctioned secrets is two leaks.
fn judge_outputs(
    wf: &RawWorkflow,
    scope: &Scope<'_>,
    egress_of: &BTreeMap<&str, &[EgressRule]>,
    facts: &mut FlowFacts,
) {
    for (name, decl) in &wf.outputs {
        let reads = scope.resolve(refs_in_str(decl_value(decl)));
        let mut reached = Taints::new();
        for (secret, trace) in carried(&reads, None, None, &facts.output_taint) {
            let own = reads
                .iter()
                .any(|r| matches!(r, Read::Secret(s) if *s == secret));
            let cleared = || {
                egress_of
                    .get(secret.as_str())
                    .is_some_and(|rules| rules.iter().any(|rule| rule.to == "outputs"))
            };
            if own {
                let trace = TaintTrace::source(&secret).via(OWN_VALUE.to_owned());
                reached.insert(secret, trace);
            } else if !cleared() {
                reached.insert(secret, trace.via("outputs".to_owned()));
            }
        }
        if !reached.is_empty() {
            facts.egress.insert(name.value.clone(), reached);
        }
    }
}

/// The last hop of an outputs entry that reads a secret's own value (E39 N7).
const OWN_VALUE: &str = "outputs (the secret's own value: every export masks it, so no \
                         egress rule clears it)";

/// Whether a rendered outputs trace ends on the own-value hop: removal is its only repair.
pub(crate) fn is_own_value(rendered: &str) -> bool {
    rendered.ends_with(OWN_VALUE)
}

/// What a reference can name in this workflow: the declared secrets, each
/// task by id, and each group's members in declaration order (spec 03 §group
/// · the fold holds each member's record, so it carries whatever a member's
/// record carries).
struct Scope<'a> {
    declared: BTreeSet<&'a str>,
    id_of: BTreeMap<&'a str, usize>,
    groups: BTreeMap<&'a str, Vec<usize>>,
}

impl<'a> Scope<'a> {
    fn of(wf: &'a RawWorkflow) -> Self {
        Self {
            declared: wf.secrets.iter().map(|(n, _)| n.value.as_str()).collect(),
            id_of: wf
                .tasks
                .iter()
                .enumerate()
                .map(|(i, t)| (t.value.id.value.as_str(), i))
                .collect(),
            groups: crate::analyzer::edges::group_members(&wf.tasks),
        }
    }

    /// Each reference resolved to where a secret it reads would come from; a
    /// reference that can carry none (an undeclared secret, a constant, an
    /// input) is dropped.
    fn resolve(&self, refs: Vec<NamespaceRef>) -> Vec<Read> {
        refs.into_iter()
            .filter_map(|r| match r {
                NamespaceRef::Secrets(name) if self.declared.contains(name.as_str()) => {
                    Some(Read::Secret(name))
                }
                NamespaceRef::With(key) => Some(Read::With(key)),
                NamespaceRef::Item => Some(Read::Item),
                r @ (NamespaceRef::Tasks { .. } | NamespaceRef::Group(_)) => {
                    let producers = self.producers(&r);
                    (!producers.is_empty()).then(|| Read::Records(producers.to_vec()))
                }
                _ => None,
            })
            .collect()
    }

    /// The tasks whose records `r` reads: `tasks.<id>` one, `group.<name>`
    /// each member; empty for any other reference.
    fn producers(&self, r: &NamespaceRef) -> &[usize] {
        match r {
            NamespaceRef::Tasks { id, .. } => self
                .id_of
                .get(id.as_str())
                .map_or(&[], std::slice::from_ref),
            NamespaceRef::Group(name) => self.groups.get(name.as_str()).map_or(&[], Vec::as_slice),
            _ => &[],
        }
    }
}

/// One reference, resolved once: where a secret it reads would come from.
enum Read {
    /// A declared `secrets.<name>`: the source itself.
    Secret(String),
    /// A task-local `with.<key>` slot.
    With(String),
    /// The `for_each` loop's `item`.
    Item,
    /// The records a `tasks.<id>` read names (one) or a `group.<name>` fold
    /// holds (each member, in declaration order).
    Records(Vec<usize>),
}

/// One `with:` binding, resolved once.
struct Binding<'a> {
    key: &'a str,
    /// What its value reads.
    reads: Vec<Read>,
    /// Whether the run renders it once per iteration: it reads a loop local
    /// of the task's `for_each:` (spec 03, `for_each` semantics), so it sees the
    /// item, and the collection, evaluated before the fan-out, does not see it.
    per_iteration: bool,
}

/// What each surface of one task reads, resolved once: the sweep repeats,
/// the template scan does not.
struct Reads<'a> {
    /// Each `with:` binding, in declaration order.
    with: Vec<Binding<'a>>,
    /// What the `for_each:` collection reads, expression or literal list.
    item: Option<Vec<Read>>,
    /// What the fields a secret is SENT through read ([`sink_fields`]).
    effect: Vec<Read>,
    /// What each route reads ([`route_fields`]).
    route: Vec<(Route, Vec<Read>)>,
    /// What the `on_error.recover:` value reads.
    recover: Option<Vec<Read>>,
}

impl<'a> Reads<'a> {
    fn of(task: &'a RawTask, scope: &Scope<'_>) -> Self {
        let fans_out = task.for_each.is_some();
        let with = task
            .with
            .iter()
            .map(|(key, value)| {
                let refs = refs_in_json(&value.value);
                // The run's own test (`references_loop_locals`): `index` makes a
                // binding per-iteration too, though it carries no secret.
                let per_iteration = fans_out
                    && refs
                        .iter()
                        .any(|r| matches!(r, NamespaceRef::Item | NamespaceRef::Index));
                Binding {
                    key: key.value.as_str(),
                    reads: scope.resolve(refs),
                    per_iteration,
                }
            })
            .collect();
        let item = task.for_each.as_ref().map(|f| {
            scope.resolve(match &f.value {
                ForEachValue::Expression(src) => refs_in_str(src),
                ForEachValue::List(list) => refs_in_json(list),
                #[allow(
                    clippy::unreachable,
                    reason = "non_exhaustive future variant — enum and checker ship together; fail loud beats silently-wrong output"
                )]
                _ => unreachable!("unknown for_each form"),
            })
        });
        let effect = scope.resolve(
            sink_fields(&task.action)
                .into_iter()
                .flat_map(refs_in_str)
                .collect(),
        );
        let route = route_fields(&task.action)
            .into_iter()
            .map(|(route, text)| (route, scope.resolve(refs_in_str(text))))
            .collect();
        let recover = match task.on_error.as_ref().map(|e| &e.value.action) {
            Some(OnErrorAction::Recover(value)) => Some(scope.resolve(refs_in_json(&value.value))),
            _ => None,
        };
        Self {
            with,
            item,
            effect,
            route,
            recover,
        }
    }

    /// Every task whose record this task reads, on any surface.
    fn producers(&self) -> impl Iterator<Item = usize> + '_ {
        self.with
            .iter()
            .flat_map(|binding| &binding.reads)
            .chain(self.item.iter().flatten())
            .chain(&self.effect)
            .chain(self.route.iter().flat_map(|(_, route_reads)| route_reads))
            .chain(self.recover.iter().flatten())
            .filter_map(|read| match read {
                Read::Records(producers) => Some(producers),
                _ => None,
            })
            .flatten()
            .copied()
    }
}

/// The most sweeps the records can take to settle: a most direct trace
/// crosses each (task, secret) pair at most once and each sweep settles one
/// more pair of it, so `tasks × secrets` sweeps settle every record, and one
/// more is the margin. Counted in `u128`, where the product of two `usize`
/// plus one cannot overflow.
fn sweep_bound(tasks: usize, secrets: usize) -> u128 {
    (tasks as u128) * (secrets as u128) + 1
}

/// What the judges read, per task: what its record carries, what its effect
/// sees and what each of its routes reads (absent = none).
struct Reached {
    /// Task index → every secret its record carries.
    records: Records,
    /// Task index → every secret its effect sees.
    effects: Records,
    /// Task index → every secret each of its routes reads.
    routes: Routes,
}

/// What [`settle`] leaves.
struct Settled {
    /// The settled facts.
    reached: Reached,
    /// How many task evaluations the sweeps took.
    evaluations: usize,
    /// Whether the sweeps stopped at the bound with work left: the facts are
    /// then incomplete, and the caller fails closed ([`worst_case`]).
    unsettled: bool,
}

/// Evaluate the tasks until every record settles, within `bound` sweeps;
/// returns the settled records and what each effect sees, read from them.
///
/// A sweep evaluates the tasks in `order`, and the sweeps repeat while one
/// moves a record (a recover read is no scheduling edge, so a record can be
/// read before it is known). A task is evaluated again only when a record it
/// reads moved since its last evaluation: later in the same sweep when it
/// comes after the task that moved, in the next sweep otherwise. Its result
/// is a function of those records alone, so an evaluation skipped would move
/// nothing: the sweeps are those of the full pass, without the idle work.
fn settle(
    tasks: &[Spanned<RawTask>],
    order: &[usize],
    reads: &[Reads<'_>],
    bound: u128,
) -> Settled {
    let mut at_of = vec![0; tasks.len()];
    for (at, &idx) in order.iter().enumerate() {
        at_of[idx] = at;
    }
    let mut readers: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); tasks.len()];
    for (reader, task_reads) in reads.iter().enumerate() {
        for producer in task_reads.producers() {
            readers[producer].insert(reader);
        }
    }
    let mut reached = Reached {
        records: Records::new(),
        effects: Records::new(),
        routes: Routes::new(),
    };
    let mut due: BTreeSet<usize> = (0..order.len()).collect();
    let (mut sweeps, mut evaluations) = (0_u128, 0_usize);
    while !due.is_empty() && sweeps < bound {
        sweeps += 1;
        let mut next = BTreeSet::new();
        while let Some(at) = due.pop_first() {
            let idx = order[at];
            evaluations += 1;
            let seen = evaluate(&tasks[idx].value, &reads[idx], &reached.records);
            if !seen.effect.is_empty() {
                reached.effects.insert(idx, seen.effect);
            }
            if !seen.route.is_empty() {
                reached.routes.insert(idx, seen.route);
            }
            if !merge_record(&mut reached.records, idx, seen.record) {
                continue;
            }
            for &reader in &readers[idx] {
                let reader_at = at_of[reader];
                if reader_at > at {
                    due.insert(reader_at);
                } else {
                    next.insert(reader_at);
                }
            }
        }
        due = next;
    }
    Settled {
        reached,
        evaluations,
        unsettled: !due.is_empty(),
    }
}

/// What one evaluation of a task finds.
struct Evaluation {
    /// Every secret its effect sees.
    effect: Taints,
    /// Every secret each route reads, each trace ending on [`Route::hop`]
    /// (a route that reads none is absent).
    route: BTreeMap<Route, Taints>,
    /// Every secret its record carries.
    record: Taints,
}

/// What `task`'s effect sees, what its routes read and what its record
/// carries, from the records known so far.
fn evaluate(task: &RawTask, reads: &Reads<'_>, records: &Records) -> Evaluation {
    let id = task.id.value.as_str();
    // A binding's slot (scoped to this task): a value reading a secret, a
    // tainted record or a tainted `item` taints that key. Bindings never read
    // each other: the run renders each one in a scope without `with:`.
    let slot = |binding: &Binding<'_>, item: Option<&Taints>| {
        let taints = carried(&binding.reads, None, item, records);
        via_each(taints, &format!("with.{} @ {id}", binding.key))
    };
    // 1. The bindings the run renders before any fan-out: every binding of a
    //    single-lane task, the loop-local-free ones of a `for_each:` task.
    let mut with = WithTaints::new();
    for binding in reads.with.iter().filter(|b| !b.per_iteration) {
        with.insert(binding.key, slot(binding, None));
    }
    // 2. `for_each` item taint: what the collection reads (an expression or a
    //    literal list, which may hold `with.*` reads of the bindings above),
    //    the loop-local `item` carries within the task.
    let item = match &reads.item {
        Some(value) => via_each(
            carried(value, Some(&with), None, records),
            &format!("item @ {id}"),
        ),
        None => Taints::new(),
    };
    // 3. The bindings the run renders once per iteration, with the item bound
    //    (spec 03, `for_each` semantics): they carry what the item carries.
    for binding in reads.with.iter().filter(|b| b.per_iteration) {
        with.insert(binding.key, slot(binding, Some(&item)));
    }
    // 4. Effect taint: do this task's effect-carrying fields see a secret?
    let effect = carried(&reads.effect, Some(&with), Some(&item), records);
    //    And the routes: does one read a secret? Each is rendered in the same
    //    scope as the effect, so it reads the same slots and item.
    let routed: Vec<(Route, Taints)> = reads
        .route
        .iter()
        .map(|(route, route_reads)| {
            let taints = carried(route_reads, Some(&with), Some(&item), records);
            (*route, taints)
        })
        .collect();
    // 5. The record: an exec/invoke whose effect saw a secret can embed it in
    //    the captured output (infer/agent are provider-bound: no output
    //    taint), and a route of the effect names its value in the output
    //    whatever the verb (a program can print its working directory, an
    //    agent's output names each image it saved, a media builtin its model);
    //    a recovered value replaces the output
    //    whatever the verb. Written to the record, not sent: no sink here,
    //    each later reader is judged as usual.
    let output = format!("tasks.{id}.output");
    let mut record = Taints::new();
    if matches!(task.action, RawAction::Exec(_) | RawAction::Invoke(_)) {
        for trace in effect.values() {
            keep_most_direct(&mut record, trace.via(output.clone()));
        }
    }
    for (_, taints) in routed.iter().filter(|(route, _)| route.is_effect_field()) {
        for trace in taints.values() {
            keep_most_direct(&mut record, trace.via(output.clone()));
        }
    }
    let route = routed
        .into_iter()
        .filter(|(_, taints)| !taints.is_empty())
        .map(|(route, taints)| (route, via_each(taints, &route.hop(task))))
        .collect();
    if let Some(value) = &reads.recover {
        let recover = format!("on_error.recover @ {id}");
        for trace in carried(value, Some(&with), Some(&item), records).into_values() {
            keep_most_direct(&mut record, trace.via(recover.clone()).via(output.clone()));
        }
    }
    Evaluation {
        effect,
        route,
        record,
    }
}

/// Each trace extended by one more hop.
fn via_each(taints: Taints, hop: &str) -> Taints {
    taints
        .into_iter()
        .map(|(secret, trace)| (secret, trace.via(hop.to_owned())))
        .collect()
}

/// Merge one evaluation's record into the held one; whether it moved.
fn merge_record(records: &mut Records, idx: usize, record: Taints) -> bool {
    if record.is_empty() {
        return false;
    }
    let held = records.entry(idx).or_default();
    let mut moved = false;
    for trace in record.into_values() {
        moved |= keep_most_direct(held, trace);
    }
    moved
}

/// Keep `trace` when no trace of its secret is held or when it has fewer hops
/// than the one held. A tie keeps the one held: the first read, in reading
/// order (references as written, a fold's members as declared), or from an
/// earlier sweep. Whether it was kept.
fn keep_most_direct(held: &mut Taints, trace: TaintTrace) -> bool {
    match held.get_mut(trace.secret.as_str()) {
        Some(kept) if kept.hops <= trace.hops => false,
        Some(kept) => {
            *kept = trace;
            true
        }
        None => {
            held.insert(trace.secret.clone(), trace);
            true
        }
    }
}

/// Every secret `reads` carries, one most direct trace each: a declared
/// secret read is its own source; a `with.*` slot or the `item` carries what
/// the task bound to it; a `tasks.*` read carries what that record carries,
/// and a `group.*` fold what each member record carries. Each slot and
/// record is read once however often it is named, so the work is bounded by
/// the reads plus the distinct secrets, not their product.
fn carried(
    reads: &[Read],
    with: Option<&WithTaints<'_>>,
    item: Option<&Taints>,
    records: &Records,
) -> Taints {
    let mut out = Taints::new();
    let mut seen_with = BTreeSet::new();
    let mut seen_records = BTreeSet::new();
    let mut saw_item = false;
    let keep_all = |out: &mut Taints, taints: Option<&Taints>| {
        for trace in taints.into_iter().flat_map(BTreeMap::values) {
            keep_most_direct(out, trace.clone());
        }
    };
    for read in reads {
        match read {
            Read::Secret(name) => {
                keep_most_direct(&mut out, TaintTrace::source(name));
            }
            Read::With(key) if seen_with.insert(key.as_str()) => {
                keep_all(&mut out, with.and_then(|slots| slots.get(key.as_str())));
            }
            Read::Item if !saw_item => {
                saw_item = true;
                keep_all(&mut out, item);
            }
            Read::Records(producers) => {
                for &producer in producers {
                    if seen_records.insert(producer) {
                        keep_all(&mut out, records.get(&producer));
                    }
                }
            }
            Read::With(_) | Read::Item => {}
        }
    }
    out
}

fn record_effect_leaks(
    idx: usize,
    task: &RawTask,
    effect_taints: &Taints,
    egress_of: &BTreeMap<&str, &[EgressRule]>,
    permits: Option<&Permits>,
    facts: &mut FlowFacts,
) {
    let leaks: Taints = effect_taints
        .iter()
        .filter(|(_, trace)| {
            let egress = egress_of.get(trace.secret.as_str()).copied().unwrap_or(&[]);
            !declass::is_sanctioned(&trace.secret, egress, &task.action, permits)
        })
        .map(|(secret, trace)| (secret.clone(), trace.clone()))
        .collect();
    if !leaks.is_empty() {
        facts.effect_leaks.insert(idx, leaks);
    }
}

/// A ROUTE: a field the run renders with secrets resolved to decide where an
/// effect goes. Neither a model name nor a path is ever a secret, so a secret
/// reaching a route is refused whatever its `egress:` (`NIKA-SEC-006`), and the
/// run refuses that task before the field renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Route {
    /// An `infer:`/`agent:` `model:` (spec 02), sent as the provider and
    /// model the run calls. The envelope `model:` is never rendered at run.
    Model,
    /// An `exec:` `cwd:`, the directory the run spawns the program in (and a
    /// literal of the sandbox profile).
    WorkingDirectory,
    /// An `agent:` `images.save:` (spec 02 §images), the file the run writes
    /// each image the turn delivered to, and the path the output names.
    ImageSave,
    /// A media builtin's `args.model` ([`MODEL_TOOLS`]), the model the tool
    /// calls and, without a `provider:`, the provider it infers from it. The
    /// file the tool saves and its output name it.
    ToolModel,
}

/// The builtins whose `args.model` names the model they call.
const MODEL_TOOLS: [&str; 2] = ["nika:image_generate", "nika:tts_generate"];

impl Route {
    /// The field, as the last hop of a trace names it.
    pub(crate) const fn field(self) -> &'static str {
        match self {
            Self::Model => "model",
            Self::WorkingDirectory => "cwd",
            Self::ImageSave => "images.save",
            Self::ToolModel => "args.model",
        }
    }

    /// The sink a leak into this route names: `<verb>.<field>`.
    pub(crate) fn sink(self, action: &RawAction) -> &'static str {
        match self {
            Self::Model if matches!(action, RawAction::Agent(_)) => "agent.model",
            Self::Model => "infer.model",
            Self::WorkingDirectory => "exec.cwd",
            Self::ImageSave => "agent.images.save",
            Self::ToolModel => "invoke.args.model",
        }
    }

    /// Why no `egress:` rule clears a secret here.
    pub(crate) const fn reason(self) -> declass::LeakReason {
        match self {
            Self::Model | Self::ToolModel => declass::LeakReason::ModelRoute,
            Self::WorkingDirectory => declass::LeakReason::WorkingDirectory,
            Self::ImageSave => declass::LeakReason::ImageSavePath,
        }
    }

    /// Whether the route is a field of the effect too, which the shared
    /// effect table lists ([`action_effect_fields`]) and which the task's
    /// output names (a program can print its working directory, an agent's
    /// output names each image it saved, a media builtin its model). A task's
    /// `model:` is neither.
    const fn is_effect_field(self) -> bool {
        !matches!(self, Self::Model)
    }

    /// The last hop of a trace into this route of `task`: `<field> @ <task>`.
    fn hop(self, task: &RawTask) -> String {
        format!("{} @ {}", self.field(), task.id.value)
    }
}

/// Every route a task names ([`Route`]), with its text, in [`Route`] order:
/// none for a verb that has none, and none for a field the task leaves out.
fn route_fields(action: &RawAction) -> Vec<(Route, &str)> {
    let mut routes = Vec::new();
    match action {
        RawAction::Infer(a) => {
            routes.extend(a.model.as_ref().map(|m| (Route::Model, m.value.as_str())));
        }
        RawAction::Agent(a) => {
            routes.extend(a.model.as_ref().map(|m| (Route::Model, m.value.as_str())));
            let save = a.images.as_ref().and_then(|i| i.value.save.as_ref());
            routes.extend(save.map(|s| (Route::ImageSave, s.value.as_str())));
        }
        RawAction::Exec(a) => {
            routes.extend(
                a.cwd
                    .as_ref()
                    .map(|d| (Route::WorkingDirectory, d.value.as_str())),
            );
        }
        RawAction::Invoke(a) => routes.extend(tool_model(a).map(|m| (Route::ToolModel, m))),
        _ => {}
    }
    routes
}

/// The `args.model` text of a media builtin call ([`Route::ToolModel`]),
/// when the call names one.
fn tool_model(a: &RawInvokeAction) -> Option<&str> {
    let RawInvokeTarget::Tool(tool) = &a.target else {
        return None;
    };
    if !MODEL_TOOLS.contains(&tool.value.as_str()) {
        return None;
    }
    a.args.as_ref()?.value.get("model")?.as_str()
}

/// The text fields of an action's EFFECT (the sink surface): one table for
/// every task's verb, an `unwind` cleanup task's included (cleanup is a task,
/// spec 03 §unwind).
///
/// The `infer:`/`agent:` `prompt:` + `system:` ARE sinks: a secret in a
/// prompt LEAVES the run to a third-party provider (the same third-party-
/// network exposure as an `mcp:` tool, which is already flagged). So are an
/// `infer:`'s `vision:` entries, which ride the same request (spec 02
/// §Vision inputs). A secret
/// reaching one with no `egress:` sanction is a leak (BUG#3 · now CANONICAL ·
/// spec `01-envelope.md` §egress aligned to this strict behavior · F-03).
/// The OUTPUT carve-out is SEPARATE and PRESERVED (see `evaluate`
/// step 5 · the model response is not a verbatim echo, so infer/agent OUTPUT
/// is never tainted from a prompt secret · ADR-092). To send a secret to a
/// provider, the author sanctions it explicitly (`egress: [{ to: "infer" }]`
/// / `{ to: "agent" }`).
///
/// Every route but a task's `model:` is part of the effect too: an
/// `exec:`'s `cwd:`, which the run renders, labels and re-gates as a path
/// (`regate_exec_cwd`) and which chooses what the program reads; an
/// `agent:`'s `images.save:`, which chooses the files the run writes; a media
/// builtin's `args.model`, an argument of its call. Every reader of this table
/// sees them as it sees an argument. The secret flow alone reads the table
/// without them: a secret there is a route, refused whatever its `egress:`,
/// never a send an `egress:` rule could clear.
///
/// `pub` (F-O1 PR-1): `nika-runtime`'s integrity walk reads the SAME
/// effect-surface table — check≡run by construction, never a duplicated
/// field list.
#[must_use]
pub fn action_effect_fields(action: &RawAction) -> Vec<&str> {
    // An invoke's effect is its whole `args`, in their own order, its model
    // route included: a reader that names the first field carrying a value
    // names the same one whether or not the tool takes a model.
    if let RawAction::Invoke(a) = action {
        return a
            .args
            .as_ref()
            .map_or_else(Vec::new, |args| collect_json_strings(&args.value));
    }
    let mut fields = sink_fields(action);
    fields.extend(
        route_fields(action)
            .into_iter()
            .filter(|(route, _)| route.is_effect_field())
            .map(|(_, text)| text),
    );
    fields
}

/// The fields a secret is judged at as a SEND, which an `egress:` rule can
/// clear: the effect table ([`action_effect_fields`]) without the routes it
/// shares.
fn sink_fields(action: &RawAction) -> Vec<&str> {
    match action {
        RawAction::Exec(a) => {
            let mut fields = a.command.text_fragments();
            if let Some(stdin) = &a.stdin {
                fields.push(stdin.value.as_str());
            }
            for (_, v) in &a.env {
                fields.push(v.value.as_str());
            }
            fields
        }
        RawAction::Invoke(a) => match (a.args.as_ref().map(|args| &args.value), tool_model(a)) {
            // A media builtin's model is its route, judged there alone.
            (Some(serde_json::Value::Object(args)), Some(_)) => args
                .iter()
                .filter(|(key, _)| key.as_str() != "model")
                .flat_map(|(_, value)| collect_json_strings(value))
                .collect(),
            (Some(args), _) => collect_json_strings(args),
            (None, _) => Vec::new(),
        },
        // The prompt (+ system) is a third-party egress sink — a secret
        // reaching it is a leak unless the author sanctions `to: "infer"`/
        // `"agent"` (BUG#3). Output taint stays excluded (evaluate §5).
        // An `infer:`'s `vision:` entries ride the same request (spec 02
        // §Vision inputs): a URL the provider fetches, and a file path that
        // chooses the bytes the provider receives. Both are sink fields, so a
        // secret in either is the `infer` leak, and untrusted content in
        // either picks what the model sees (its answer carries it).
        RawAction::Infer(a) => {
            let mut fields = prompt_system_fields(a.prompt.value.as_str(), a.system.as_ref());
            fields.extend(vision_fields(&a.vision));
            fields
        }
        RawAction::Agent(a) => prompt_system_fields(a.prompt.value.as_str(), a.system.as_ref()),
        // The message never formats the action: it carries prompts, arguments and secret
        // references.
        #[allow(
            clippy::unreachable,
            reason = "non_exhaustive future variant — enum and checker ship together; fail loud beats silently-wrong output"
        )]
        _ => unreachable!("an action kind this checker does not know"),
    }
}

/// The text of every `vision:` entry of an `infer:` action (spec 02 §Vision
/// inputs): a `source: url`'s URL and a `source: file`'s path. A source form
/// this build does not know carries no text here (the run refuses it unwired).
fn vision_fields(
    items: &[nika_schema::Spanned<nika_schema::raw::VisionInput>],
) -> impl Iterator<Item = &str> {
    items.iter().filter_map(|item| match &item.value {
        nika_schema::raw::VisionInput::File { path } => Some(path.value.as_str()),
        nika_schema::raw::VisionInput::Url { url } => Some(url.value.as_str()),
        _ => None,
    })
}

/// The `prompt:` + optional `system:` text of an `infer:`/`agent:` action —
/// the provider-egress sink surface (BUG#3).
pub(crate) fn prompt_system_fields<'a>(
    prompt: &'a str,
    system: Option<&'a nika_schema::Spanned<String>>,
) -> Vec<&'a str> {
    let mut fields = vec![prompt];
    if let Some(system) = system {
        fields.push(system.value.as_str());
    }
    fields
}

/// The `${{ … }}` references inside a string (via the real extractor — the
/// same path the analyzer uses, so taint and `NIKA-VAR-001` agree).
pub(crate) fn refs_in_str(text: &str) -> Vec<NamespaceRef> {
    let Ok(islands) = scan_templates(text) else {
        return Vec::new();
    };
    islands.iter().flat_map(|i| expr_refs(&i.expr)).collect()
}

/// References inside any string within a JSON value (with-values, args).
fn refs_in_json(value: &serde_json::Value) -> Vec<NamespaceRef> {
    collect_json_strings(value)
        .into_iter()
        .flat_map(refs_in_str)
        .collect()
}

/// Every string leaf of a JSON value (for the ref/taint scan).
pub(crate) fn collect_json_strings(value: &serde_json::Value) -> Vec<&str> {
    let mut out = Vec::new();
    collect_json_strings_into(value, &mut out);
    out
}

fn collect_json_strings_into<'a>(value: &'a serde_json::Value, out: &mut Vec<&'a str>) {
    match value {
        serde_json::Value::String(s) => out.push(s.as_str()),
        serde_json::Value::Array(items) => {
            for it in items {
                collect_json_strings_into(it, out);
            }
        }
        serde_json::Value::Object(map) => {
            for v in map.values() {
                collect_json_strings_into(v, out);
            }
        }
        _ => {}
    }
}

/// The `${{ }}` reference expression of an `outputs:` declaration.
fn decl_value(decl: &OutputDecl) -> &str {
    decl.value().value.as_str()
}

#[cfg(test)]
mod tests;
