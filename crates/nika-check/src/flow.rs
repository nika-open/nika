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
//! - **sinks** — a secret reaching an `exec`/`invoke` effect is a LEAK;
//!   a secret reaching the workflow `outputs:` is an EGRESS (the value
//!   leaves the run as the return).
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
use nika_schema::raw::{ForEachValue, RawAction, RawTask, RawWorkflow};
use nika_schema::types::{EgressRule, OnErrorAction, OutputDecl, Permits};

use super::declass;

/// Secret name → the most direct trace of that secret reaching one slot.
type Taints = BTreeMap<String, TaintTrace>;
type WithTaints<'a> = BTreeMap<&'a str, Taints>;
/// Task index → every secret its record carries.
type Records = BTreeMap<usize, Taints>;

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
    /// Task index → every secret its RECORD carries (`tasks.<id>.output`, and
    /// each `group.<name>` fold it joins): what an exec/invoke effect saw, and
    /// what its `on_error.recover:` value reads (the recovered value replaces
    /// the output whatever the verb · spec 05 §recovery).
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
    let (records, effects) = if settled.unsettled {
        worst_case(wf.tasks.len(), &scope.declared, bound)
    } else {
        (settled.records, settled.effects)
    };

    // The findings read the settled facts, once.
    for (idx, effect) in effects {
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
    facts.output_taint = records;
    judge_outputs(wf, &scope, &egress_of, &mut facts);
    (facts, settled.evaluations)
}

/// The facts when the sweeps stop at `bound` with work left (unreachable by
/// the bound's argument, so a bug if it ever happens): every task's effect and
/// record carry every declared secret, each traced to this hop. The usual
/// judges then read them, so every send and `outputs:` entry a secret's
/// `egress:` does not allow refuses, and the trace says why.
fn worst_case(tasks: usize, declared: &BTreeSet<&str>, bound: u128) -> (Records, Records) {
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
    let all: Records = (0..tasks).map(|idx| (idx, every.clone())).collect();
    (all.clone(), all)
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
    /// What the effect-carrying fields read ([`action_effect_fields`]).
    effect: Vec<Read>,
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
            action_effect_fields(&task.action)
                .into_iter()
                .flat_map(refs_in_str)
                .collect(),
        );
        let recover = match task.on_error.as_ref().map(|e| &e.value.action) {
            Some(OnErrorAction::Recover(value)) => Some(scope.resolve(refs_in_json(&value.value))),
            _ => None,
        };
        Self {
            with,
            item,
            effect,
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

/// What [`settle`] leaves.
struct Settled {
    /// Task index → every secret its record carries.
    records: Records,
    /// Task index → every secret its effect sees (absent = none).
    effects: Records,
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
    let (mut records, mut effects) = (Records::new(), Records::new());
    let mut due: BTreeSet<usize> = (0..order.len()).collect();
    let (mut sweeps, mut evaluations) = (0_u128, 0_usize);
    while !due.is_empty() && sweeps < bound {
        sweeps += 1;
        let mut next = BTreeSet::new();
        while let Some(at) = due.pop_first() {
            let idx = order[at];
            evaluations += 1;
            let (effect, record) = evaluate(&tasks[idx].value, &reads[idx], &records);
            if !effect.is_empty() {
                effects.insert(idx, effect);
            }
            if !merge_record(&mut records, idx, record) {
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
        records,
        effects,
        evaluations,
        unsettled: !due.is_empty(),
    }
}

/// What `task`'s effect sees and what its record carries, from the records
/// known so far.
fn evaluate(task: &RawTask, reads: &Reads<'_>, records: &Records) -> (Taints, Taints) {
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
    // 5. The record: an exec/invoke whose effect saw a secret can embed it in
    //    the captured output (infer/agent are provider-bound: no output
    //    taint), and a recovered value replaces the output whatever the verb.
    //    Written to the record, not sent: no sink here, each later reader is
    //    judged as usual.
    let output = format!("tasks.{id}.output");
    let mut record = Taints::new();
    if matches!(task.action, RawAction::Exec(_) | RawAction::Invoke(_)) {
        for trace in effect.values() {
            keep_most_direct(&mut record, trace.via(output.clone()));
        }
    }
    if let Some(value) = &reads.recover {
        let recover = format!("on_error.recover @ {id}");
        for trace in carried(value, Some(&with), Some(&item), records).into_values() {
            keep_most_direct(&mut record, trace.via(recover.clone()).via(output.clone()));
        }
    }
    (effect, record)
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
/// `pub` (F-O1 PR-1): `nika-runtime`'s integrity walk reads the SAME
/// effect-surface table — check≡run by construction, never a duplicated
/// field list.
#[must_use]
pub fn action_effect_fields(action: &RawAction) -> Vec<&str> {
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
        RawAction::Invoke(a) => a
            .args
            .as_ref()
            .map(|args| collect_json_strings(&args.value))
            .unwrap_or_default(),
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
mod tests {
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
    fn facts_within(
        yaml: &str,
        bound: fn(usize, usize) -> u128,
    ) -> (RawWorkflow, FlowFacts, usize) {
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
}
