// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! The content-taint pass (NEP-0002 v2.0 · the INTEGRITY half of the flow
//! lattice — [`super::flow`] tracks secret CONFIDENTIALITY): does untrusted
//! content actually REACH a task's effect surface, and from which ingress?
//! Sources: invoked `nika:fetch` · `mcp:*` (fail-closed) · `agent:` with an
//! ingress whitelist · `exec:`. Propagation mirrors [`super::flow`] with three
//! inversions: `infer:`/`agent:` outputs ARE tainted when their prompt sees
//! it · under a declared `fs.read`, a task that reads a file a tainted
//! WRITER left earlier is tainted (the file channel argv and bindings cannot
//! see · [`super::file_channel`]): an `exec:` whatever the file, a builtin
//! read (nika-cap's read slots) when its path may be one the writer wrote ·
//! recover reads propagate · a `group.<name>` fold carries what its members'
//! records carry (spec 03 §group: the fold holds each member's record), the
//! first member found tainted as the origin: the first in declaration order
//! within a sweep, though a member a late recover taints on a later sweep
//! leaves the origin found before it, where the run names the first in
//! declaration order (the same taint under another name) · a `with:`
//! binding that reads `item` carries what the item carries (the run renders
//! it once per iteration).
//!
//! **Settling.** The sweep walks every task once, producers first: the waves,
//! then the unwind cleanup tasks, which never enter a wave (spec 03) but run
//! after their producer settles ([`judge_order`]). One sweep is not enough: a
//! `recover:` read is no scheduling edge (spec 05 §recover · the run awaits
//! the source), so it may read a task the sweep reaches later, and a builtin
//! read meets a located write in any order. The pass repeats the sweep until
//! a sweep moves no fact. A task holds two facts, the origin reaching its
//! effect and the origin its record carries; a fact moves from clean to
//! tainted once and keeps the origin it was first given, so the facts settle
//! within `2 × tasks` moving sweeps ([`sweep_bound`]). A pass that reaches the
//! bound with a fact still moving stops on the fail-closed reading
//! ([`ContentFlow::unsettled`]), never on the facts it had so far.
//!
//! An `invoke: workflow:` call is judged through what the composed check
//! learned of its child closure ([`crate::ComposedCall`] · spec 14 ·
//! NIK-273): content born in the closure may reach the call's output, an
//! egress task in the closure, or a file; the call's arguments and the
//! files an earlier writer left may reach an egress task or a file there.
//! The pass follows one [`Entry`] at a time, so the same table judges a
//! child for each kind of content that can enter it. A call no composed
//! check listed is judged child-blind: its arguments flow to its output,
//! and nothing else.

use std::collections::BTreeMap;

use nika_cap::TaintWitness;

use super::file_channel::{self, Written};
use super::flow::{action_effect_fields, collect_json_strings, refs_in_str};
use crate::ComposedCall;
use crate::analyzer::Edge;
use crate::analyzer::edges::group_members;
use nika_schema::expression::NamespaceRef;
use nika_schema::raw::{RawAction, RawInvokeTarget, RawTask, RawWorkflow};
use nika_schema::types::OnErrorAction;

/// The origin `inputs.*` reads carry in an [`Entry::Args`] pass.
pub(super) const ARGS_ORIGIN: &str = "<the call's arguments>";
/// The origin the pre-armed file channel carries in an [`Entry::Files`] pass.
pub(super) const FILES_ORIGIN: &str = "<a file written before the call>";
/// The origin every fact carries when the sweeps did not settle within their
/// bound ([`ContentFlow::unsettled`]).
pub(super) const UNSETTLED_ORIGIN: &str = "<content the check could not follow to a fixed point>";

/// Which untrusted content one pass follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Entry {
    /// The workflow's own born sources (the root's pass · a child's content).
    Born,
    /// Only what `inputs.*` reads carry: the arguments a caller passes.
    Args,
    /// Only what the file channel carries when a tainted writer ran before
    /// the first task: the files a caller's writers left, at any path.
    Files,
}

/// Where untrusted content is born, beyond the verb tables: the judgments
/// this pass cannot make from the workflow's own bytes.
pub(super) struct Sources<'a> {
    /// `mcp_trusted(server_id)` answers the catalog's `content_trust` mark
    /// (`true` = trusted content).
    pub(super) mcp_trusted: &'a dyn Fn(&str) -> bool,
    /// The `invoke: workflow:` calls a composed check resolved. A
    /// reader-less check lists none.
    pub(super) calls: &'a [ComposedCall],
    /// The content this pass follows.
    pub(super) entry: Entry,
    /// Whether a task reads the file channel. `None`: when the workflow
    /// declares `fs.read` (the root). A child is judged with `Some(true)`:
    /// the trifecta needs the root's `fs.read` (leg ①), and under it the
    /// child's subprocesses and builtin reads read what earlier writers left.
    pub(super) exec_reads_files: Option<bool>,
}

impl Sources<'_> {
    /// The call `task_id` stands for, when it is a listed workflow call.
    fn call(&self, task_id: &str, action: &RawAction) -> Option<&ComposedCall> {
        let RawAction::Invoke(inv) = action else {
            return None;
        };
        if !matches!(inv.target, RawInvokeTarget::Workflow(_)) {
            return None;
        }
        self.calls.iter().find(|c| c.task == task_id)
    }
}

/// What one pass found.
pub(super) struct ContentFlow {
    /// Per task: the origin reaching its EFFECT (the judge's witness). For a
    /// listed workflow call: the origin reaching an egress task of its
    /// closure (the call is a sink exactly when this is set).
    pub(super) witnesses: Vec<TaintWitness>,
    /// Per task: the origin its OUTPUT carries.
    pub(super) outputs: Vec<Option<String>>,
    /// The first task that wrote a file with untrusted content in it, as
    /// `(writer, origin)`: the file channel's arming.
    pub(super) writer: Option<(String, String)>,
}

impl ContentFlow {
    /// The fail-closed reading of a pass whose sweeps did not settle within
    /// [`sweep_bound`]: every effect and every output carries untrusted
    /// content, and the first task armed the file channel. The bound is a
    /// proof (its doc), so this reading is never reached while the proof
    /// holds; it exists so that a broken bound refuses, never judges on the
    /// facts a stopped sweep happened to hold.
    fn unsettled(wf: &RawWorkflow) -> Self {
        let origin = || Some(UNSETTLED_ORIGIN.to_owned());
        Self {
            witnesses: wf
                .tasks
                .iter()
                .map(|_| TaintWitness::new(true, origin()))
                .collect(),
            outputs: wf.tasks.iter().map(|_| origin()).collect(),
            writer: wf
                .tasks
                .first()
                .map(|t| (t.value.id.value.clone(), UNSETTLED_ORIGIN.to_owned())),
        }
    }
}

/// Every task once, producers first: the waves in run order, then the tasks
/// no wave holds, ordered over every derived edge. Those are the unwind
/// cleanup tasks (spec 03: an unwind edge never enters wave assignment, and
/// neither does the task it reaches), which run once their producer has
/// settled, and whatever waits on one. No edge leaves such a task for a
/// task a wave holds (that task would never enter a wave either), so the
/// whole order is topological. The waves are the ordering device, never the
/// census: a task they leave out is still judged.
pub(super) fn judge_order(task_count: usize, waves: &[Vec<usize>], edges: &[Edge]) -> Vec<usize> {
    let mut placed = vec![false; task_count];
    let mut order = Vec::with_capacity(task_count);
    for &i in waves.iter().flatten() {
        if i < task_count && !placed[i] {
            placed[i] = true;
            order.push(i);
        }
    }
    let mut parents: Vec<Vec<usize>> = vec![Vec::new(); task_count];
    for e in edges
        .iter()
        .filter(|e| e.from < task_count && e.to < task_count)
    {
        parents[e.to].push(e.from);
    }
    loop {
        let before = order.len();
        for i in 0..task_count {
            if !placed[i] && parents[i].iter().all(|&p| placed[p]) {
                placed[i] = true;
                order.push(i);
            }
        }
        if order.len() == before {
            break;
        }
    }
    // A cycle the DAG layer refuses first: declaration order, so that no task
    // goes unjudged even then.
    order.extend((0..task_count).filter(|&i| !placed[i]));
    order
}

/// Compute the per-task content-taint witness over `sources`, sweeping the
/// tasks in [`judge_order`] until the facts settle (module doc).
///
/// A file outlives the run order: a reader in its writer's wave runs
/// beside it (a retry waits for the file), and a reader the order puts
/// first reads what the previous run left. So a builtin read meets every
/// tainted write the check can locate (a literal path), earlier or not
/// (NIK-273 review): each sweep reads the located writes the one before
/// found. A write the check cannot locate (a templated path · a subprocess ·
/// a server · a child closure) meets only later reads, as before: whatever
/// the order, it would meet every read of the workflow. An `exec:` reads
/// what it likes, so only a writer earlier in the sweep arms it, and so for
/// a call whose closure reads files (its reads are not told apart).
pub(super) fn analyze_content_flow(
    wf: &RawWorkflow,
    waves: &[Vec<usize>],
    edges: &[Edge],
    sources: &Sources<'_>,
) -> ContentFlow {
    let order = judge_order(wf.tasks.len(), waves, edges);
    settle(wf, &order, sources, sweep_bound(wf.tasks.len()))
}

/// The most sweeps the facts can take to settle. A task holds two facts, the
/// origin reaching its effect and the origin its record carries. A fact moves
/// from clean to tainted at most once and never back (it keeps the origin it
/// was first given), and a sweep that moves no fact is the last one: the
/// next would read exactly what it read (the same records, the same writes,
/// so the same located writes). So at most `2 × tasks` sweeps move a fact,
/// and one more finds none left to move. A task-count bound alone would not
/// hold: a recover can taint a record before anything reaches the task's
/// effect, and a file the task reads can taint that effect sweeps later, so
/// one task can move twice. Saturating: no workflow holds `usize::MAX / 2`
/// tasks.
fn sweep_bound(tasks: usize) -> usize {
    tasks.saturating_mul(2).saturating_add(1)
}

/// The two facts of every task, as the sweeps found them.
struct Facts {
    /// The origin reaching each task's effect (`None` = clean).
    effect: Vec<Option<String>>,
    /// The origin each task's record carries (`None` = clean).
    output: Vec<Option<String>>,
}

/// What one sweep found beside the facts.
struct Swept {
    /// Whether a fact moved from clean to tainted.
    moved: bool,
    /// Every tainted write of the tasks, in sweep order.
    written: Vec<Written>,
    /// The first task that wrote untrusted content, with its origin.
    writer: Option<(String, String)>,
}

/// Sweep until no fact moves, at most `bound` times.
fn settle(wf: &RawWorkflow, order: &[usize], sources: &Sources<'_>, bound: usize) -> ContentFlow {
    let n = wf.tasks.len();
    let scope = Scope::of(wf, sources);
    let mut facts = Facts {
        effect: vec![None; n],
        output: vec![None; n],
    };
    let mut known: Vec<Written> = Vec::new();
    for _ in 0..bound {
        let swept = sweep(wf, order, sources, &scope, &mut facts, &known);
        if !swept.moved {
            return ContentFlow {
                witnesses: facts
                    .effect
                    .iter()
                    .map(|e| TaintWitness::new(e.is_some(), e.clone()))
                    .collect(),
                outputs: facts.output,
                writer: swept.writer,
            };
        }
        known = swept.written.iter().filter_map(Written::located).collect();
    }
    ContentFlow::unsettled(wf)
}

/// One sweep in `order`. `known`: every located tainted write the previous
/// sweep found. A fact already tainted keeps its origin; a clean one takes
/// the origin this sweep finds.
fn sweep(
    wf: &RawWorkflow,
    order: &[usize],
    sources: &Sources<'_>,
    scope: &Scope<'_>,
    facts: &mut Facts,
    known: &[Written],
) -> Swept {
    // Every tainted write so far, in run order (the first arms an `exec:`).
    let mut channel: Vec<Written> = Vec::new();
    if sources.entry == Entry::Files {
        channel.push(Written::anywhere(FILES_ORIGIN.to_owned()));
    }
    // The writes of the tasks alone (the entry's seed is not one).
    let mut swept = Swept {
        moved: false,
        written: Vec::new(),
        writer: None,
    };
    for &i in order {
        let Some(task) = wf.tasks.get(i).map(|t| &t.value) else {
            continue;
        };
        let refs = scope.refs(&facts.output);
        let (with_src, item_src) = refs.bindings(task);
        let arrived: Option<String> = action_effect_fields(&task.action)
            .into_iter()
            .flat_map(refs_in_str)
            .find_map(|r| refs.source_of(&r, &with_src, item_src.as_ref()));
        let lens = Lens {
            follows_born: scope.follows_born,
            fs_read_declared: scope.fs_read_declared,
            channel: &channel,
            known,
        };
        let held = facts.effect[i].clone();
        let reached = match sources.call(&task.id.value, &task.action) {
            Some(call) => lens.call(&task.id.value, call, arrived),
            None => lens.task(task, held.clone().or(arrived), sources.mcp_trusted),
        };
        let effect = held.or(reached.effect);
        let output = facts.output[i]
            .clone()
            .or(reached.output)
            .or_else(|| refs.recovered(task, &with_src, item_src.as_ref()));
        if let Some(write) = reached.written {
            if channel.is_empty() {
                swept.writer = Some((task.id.value.clone(), write.origin.clone()));
            }
            swept.written.push(write.clone());
            channel.push(write);
        }
        swept.moved |= effect != facts.effect[i] || output != facts.output[i];
        facts.effect[i] = effect;
        facts.output[i] = output;
    }
    swept
}

/// What one task does with the untrusted content reaching it.
struct Reached {
    /// The origin reaching its effect (the judge's witness).
    effect: Option<String>,
    /// The origin its output carries (before a recover read).
    output: Option<String>,
    /// The origin it writes to a file and where (the channel's arming).
    written: Option<Written>,
}

/// The pass's view at one task: which content it follows, the file
/// channel so far, and every located tainted write the previous sweep
/// found.
struct Lens<'a> {
    follows_born: bool,
    fs_read_declared: bool,
    channel: &'a [Written],
    known: &'a [Written],
}

impl Lens<'_> {
    /// An inline task, through the ONE table ([`classify`]).
    fn task(
        &self,
        task: &RawTask,
        arrived: Option<String>,
        mcp_trusted: &dyn Fn(&str) -> bool,
    ) -> Reached {
        let effect = arrived.or_else(|| self.file_read(&task.action));
        // Output taint: born-source wins; else effect flows out (inversion).
        let (born, writes) = classify(&task.action, mcp_trusted);
        let output = if born && self.follows_born {
            Some(task.id.value.clone())
        } else {
            effect.clone()
        };
        // The file channel arms on ARRIVED taint (`effect`), never on born
        // output — the full story and its mutation-proven pin:
        // `trifecta::tests::two_execs_with_no_flow_between_them_pass`.
        let written = effect.clone().filter(|_| writes).map(|origin| Written {
            origin,
            spots: file_channel::written_spots(&task.action),
        });
        Reached {
            effect,
            output,
            written,
        }
    }

    /// The origin a task reads through the file channel: an `exec:` reads
    /// what the first earlier tainted writer left, a builtin read what an
    /// earlier writer of a path it reads left, or a located writer of it
    /// in any order.
    fn file_read(&self, action: &RawAction) -> Option<String> {
        if !self.fs_read_declared {
            return None;
        }
        if matches!(action, RawAction::Exec(_)) {
            return self.channel.first().map(|w| w.origin.clone());
        }
        let reads = file_channel::read_spots(action);
        file_channel::first_met(self.channel, &reads)
            .or_else(|| file_channel::first_met(self.known, &reads))
            .cloned()
    }

    /// A listed workflow call, through what its closure does with each
    /// entry: the call's arguments (`arrived`), the channel an earlier
    /// writer armed (when a task of the closure reads files, whatever the
    /// path: the closure's reads, an `exec:` among them, are compared
    /// neither by path nor by order across the call), and what the closure
    /// brings in itself (the call's own id is that origin).
    fn call(&self, task_id: &str, call: &ComposedCall, arrived: Option<String>) -> Reached {
        let file = self
            .channel
            .first()
            .filter(|_| self.fs_read_declared && !call.files.is_empty())
            .map(|w| w.origin.clone());
        let own = || Some(task_id.to_owned());
        let output = if self.follows_born && call.born.output.is_some() {
            own()
        } else {
            // Arguments pass through (the child-blind reading).
            arrived
                .clone()
                .or_else(|| file.clone().filter(|_| call.files.output.is_some()))
        };
        // Arrived content names its origin first (the caller's own task),
        // then what the closure brings in itself.
        let effect = arrived
            .clone()
            .filter(|_| call.args.egress.is_some())
            .or_else(|| file.filter(|_| call.files.egress.is_some()))
            .or_else(|| own().filter(|_| self.follows_born && call.born.egress.is_some()));
        let written = if self.follows_born && call.born.file.is_some() {
            own()
        } else {
            arrived.filter(|_| call.args.file.is_some())
        }
        .map(Written::anywhere);
        Reached {
            effect,
            output,
            written,
        }
    }
}

/// The origin a workflow `outputs:` entry carries after `flow`, as
/// `(output name, origin)`: the first entry, in declaration order, that
/// reads a tainted task (or, in an [`Entry::Args`] pass, an input).
pub(super) fn output_origin(
    wf: &RawWorkflow,
    flow: &ContentFlow,
    entry: Entry,
) -> Option<(String, String)> {
    let id_of = ids(wf);
    let groups = group_members(&wf.tasks);
    let inputs = (entry == Entry::Args).then(|| ARGS_ORIGIN.to_owned());
    let refs = Refs {
        output: &flow.outputs,
        id_of: &id_of,
        groups: &groups,
        inputs: inputs.as_ref(),
    };
    wf.outputs.iter().find_map(|(name, decl)| {
        refs_in_str(decl.value().value.as_str())
            .into_iter()
            .find_map(|r| refs.source_of(&r, &BTreeMap::new(), None))
            .map(|origin| (name.value.clone(), origin))
    })
}

/// Each task by id.
fn ids(wf: &RawWorkflow) -> BTreeMap<&str, usize> {
    wf.tasks
        .iter()
        .enumerate()
        .map(|(i, t)| (t.value.id.value.as_str(), i))
        .collect()
}

/// What every sweep of one pass reads alike: the tasks by id, each group's
/// members in declaration order (spec 03 §group), the origin `inputs.*`
/// reads carry, which content the pass follows, and whether a task reads
/// the file channel.
struct Scope<'a> {
    id_of: BTreeMap<&'a str, usize>,
    groups: BTreeMap<&'a str, Vec<usize>>,
    inputs: Option<String>,
    follows_born: bool,
    fs_read_declared: bool,
}

impl<'a> Scope<'a> {
    fn of(wf: &'a RawWorkflow, sources: &Sources<'_>) -> Self {
        let fs_read_declared = sources.exec_reads_files.unwrap_or_else(|| {
            wf.permits
                .as_ref()
                .and_then(|p| p.value.fs.as_ref())
                .is_some_and(|fs| !fs.read.is_empty())
        });
        Self {
            id_of: ids(wf),
            groups: group_members(&wf.tasks),
            inputs: (sources.entry == Entry::Args).then(|| ARGS_ORIGIN.to_owned()),
            follows_born: sources.entry == Entry::Born,
            fs_read_declared,
        }
    }

    /// The references over the records as they stand.
    fn refs<'s>(&'s self, output: &'s [Option<String>]) -> Refs<'s> {
        Refs {
            output,
            id_of: &self.id_of,
            groups: &self.groups,
            inputs: self.inputs.as_ref(),
        }
    }
}

/// What a reference can read: the records as they stand, each group's
/// members, and the inputs' origin.
struct Refs<'a> {
    output: &'a [Option<String>],
    id_of: &'a BTreeMap<&'a str, usize>,
    groups: &'a BTreeMap<&'a str, Vec<usize>>,
    inputs: Option<&'a String>,
}

impl Refs<'_> {
    /// The origin each `with:` binding carries, and the `for_each` item's.
    ///
    /// The order is the run's (spec 03 §`for_each` · the boundary render): the
    /// bindings that read no loop local first, rendered once before the
    /// fan-out; then the item, from the collection, which reads those
    /// bindings alone (an expression, or a literal list whose strings may
    /// hold templates); then the bindings that read `item` or `index`,
    /// rendered once per iteration with the item bound, so they carry what
    /// the item carries. A task without `for_each:` renders every binding at
    /// the boundary.
    fn bindings<'t>(&self, task: &'t RawTask) -> (BTreeMap<&'t str, String>, Option<String>) {
        let fans_out = task.for_each.is_some();
        let boundary = self.with_bindings(
            task,
            BTreeMap::new(),
            |value| !(fans_out && reads_a_loop_local(value)),
            None,
        );
        let item_src: Option<String> = task.for_each.as_ref().and_then(|f| {
            let refs = match &f.value {
                nika_schema::raw::ForEachValue::Expression(src) => refs_in_str(src),
                nika_schema::raw::ForEachValue::List(list) => collect_json_strings(list)
                    .into_iter()
                    .flat_map(refs_in_str)
                    .collect(),
                #[allow(
                    clippy::unreachable,
                    reason = "non_exhaustive future variant — enum and checker ship together; fail loud beats silently-wrong output"
                )]
                other => unreachable!("unknown for_each form: {other:?}"),
            };
            refs.into_iter()
                .find_map(|r| self.source_of(&r, &boundary, None))
        });
        if !fans_out {
            return (boundary, item_src);
        }
        let with_src = self.with_bindings(task, boundary, reads_a_loop_local, item_src.as_ref());
        (with_src, item_src)
    }

    /// Add the origin each `with:` binding `admit` selects carries to `with_src`,
    /// in declaration order (a value may read an earlier key), `item` read as
    /// `item_src`.
    fn with_bindings<'t>(
        &self,
        task: &'t RawTask,
        mut with_src: BTreeMap<&'t str, String>,
        admit: impl Fn(&serde_json::Value) -> bool,
        item_src: Option<&String>,
    ) -> BTreeMap<&'t str, String> {
        for (key, value) in task.with.iter().filter(|(_, value)| admit(&value.value)) {
            let src = collect_json_strings(&value.value)
                .into_iter()
                .flat_map(refs_in_str)
                .find_map(|r| self.source_of(&r, &with_src, item_src));
            if let Some(src) = src {
                with_src.insert(key.value.as_str(), src);
            }
        }
        with_src
    }

    /// The origin a task's `on_error.recover:` value reads: the recovered
    /// value replaces the output whatever the verb (spec 05 §recovery).
    fn recovered(
        &self,
        task: &RawTask,
        with_src: &BTreeMap<&str, String>,
        item_src: Option<&String>,
    ) -> Option<String> {
        let OnErrorAction::Recover(value) = &task.on_error.as_ref()?.value.action else {
            return None;
        };
        collect_json_strings(&value.value)
            .into_iter()
            .flat_map(refs_in_str)
            .find_map(|r| self.source_of(&r, with_src, item_src))
    }

    /// The origin one reference reads. A `group.<name>` fold holds each
    /// member's record (spec 03 §group), so it carries the first tainted
    /// member's origin, in declaration order.
    fn source_of(
        &self,
        r: &NamespaceRef,
        with_src: &BTreeMap<&str, String>,
        item_src: Option<&String>,
    ) -> Option<String> {
        match r {
            NamespaceRef::Tasks { id, .. } => self
                .id_of
                .get(id.as_str())
                .and_then(|&u| self.output.get(u).cloned().flatten()),
            // A group with no members reads clean here: NIKA-DAG-008 already
            // refuses an unknown group. The run fails closed instead.
            NamespaceRef::Group(name) => self
                .groups
                .get(name.as_str())
                .into_iter()
                .flatten()
                .find_map(|&m| self.output.get(m).cloned().flatten()),
            NamespaceRef::With(key) => with_src.get(key.as_str()).cloned(),
            NamespaceRef::Item => item_src.cloned(),
            NamespaceRef::Inputs(_) => self.inputs.cloned(),
            _ => None,
        }
    }
}

/// Whether a `with:` value reads a `for_each` loop local (`item` or
/// `index`): the run's own test (`references_loop_locals`) for the bindings
/// it renders once per iteration rather than at the boundary.
fn reads_a_loop_local(value: &serde_json::Value) -> bool {
    collect_json_strings(value)
        .into_iter()
        .flat_map(refs_in_str)
        .any(|r| matches!(r, NamespaceRef::Item | NamespaceRef::Index))
}

/// The invoke-side classifications, ONE effect-table walk: `(born_ingress,
/// writes_fs)` — the module doc's source table, and the writer clause.
/// The ingress predicates are nika-cap's shared ones (check≡run — the
/// runtime's F-O1 label reads the same law).
///
/// `exec:` is born-ingress (F2 · 2026-07-30 · operator decision A, run 5):
/// a subprocess is a trust boundary — its stdout is content the workflow
/// did not author (a curled page · a git log with hostile subjects · a
/// compromised binary), exactly the class `nika:fetch` is judged for.
/// Until this row read `(false, true)` the SAME flow drew SEC-009 over
/// `nika:fetch` and a ✔ over `exec curl` — a perverse incentive to leave
/// the native path (measured live). Every exec workflow's posture moves
/// with this row: its outputs taint downstream effects like any other
/// ingress, and the trifecta's leg ② counts the exec permit as an
/// ingress channel (`nika_cap::trifecta` — the grant twin of this law).
///
/// An `invoke: workflow:` call reads `(false, false)` here: the table
/// judges one task's own bytes, and a call's are its child's. The composed
/// check folds the child closure through this same table
/// ([`crate::ComposedCall`] · NIK-273), and [`analyze_content_flow`] reads
/// what it found.
pub(super) fn classify(action: &RawAction, mcp_trusted: &dyn Fn(&str) -> bool) -> (bool, bool) {
    match action {
        RawAction::Exec(_) => (true, true),
        RawAction::Agent(a) => (
            a.tools
                .iter()
                .any(|t| nika_cap::tool_grant_admits_ingress(t.value.as_str())),
            false,
        ),
        RawAction::Invoke(inv) => {
            let Some(tool) = inv.tool() else {
                return (false, false); // a child call — folded by the composed check
            };
            let id = tool.value.as_str();
            if let Some(server) = id.strip_prefix("mcp:") {
                // Server content untrusted UNLESS the catalog marks it
                // (absent/unknown = untrusted) · server writes are opaque.
                let trusted = mcp_trusted(server.split('/').next().unwrap_or(server));
                return (nika_cap::invoke_tool_is_ingress(id, trusted), true);
            }
            let args = inv.args.as_ref().map(|a| &a.value);
            let writes = match nika_cap::builtin_effect(id, args) {
                Some(nika_cap::BuiltinEffect::Fs { writes, .. }) => writes,
                _ => false,
            };
            (nika_cap::invoke_tool_is_ingress(id, false), writes)
        }
        _ => (false, false),
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "fixture construction is a test assertion"
)]
mod tests {
    use super::{Entry, Sources, UNSETTLED_ORIGIN, judge_order, settle, sweep_bound};
    use crate::analyzer::{AnalyzedWorkflow, analyze};
    use nika_schema::parser::{ParseMode, parse};
    use nika_schema::raw::RawWorkflow;
    use nika_schema::source::FileId;

    fn parsed(yaml: &str) -> (RawWorkflow, AnalyzedWorkflow) {
        let wf = parse(yaml, FileId::new(0), ParseMode::Strict).expect("fixture parses");
        let analyzed = analyze(&wf).expect("fixture analyzes");
        (wf, analyzed)
    }

    fn born() -> Sources<'static> {
        Sources {
            mcp_trusted: &|_| false,
            calls: &[],
            entry: Entry::Born,
            exec_reads_files: None,
        }
    }

    /// The recover source is declared after the recovering task: one sweep
    /// reads `page` before it is known, the second finds it, the third moves
    /// nothing.
    const SOURCE_AFTER: &str = "nika: h\npermits:\n  tools: [\"nika:fetch\", \"nika:jq\"]\n  fs: { read: [\"./notes/**\"] }\n  net: { http: [\"feed.example.com\", \"sink.example.com\"] }\ntasks:\n  stage:\n    invoke: { tool: \"nika:jq\", args: { expression: \".\", input: \"fixed\" } }\n    on_error: { recover: \"${{ tasks.page.output }}\" }\n  page:\n    invoke: { tool: \"nika:fetch\", args: { url: \"https://feed.example.com/a\" } }\n  send:\n    with: { body: \"${{ tasks.stage.output }}\" }\n    invoke: { tool: \"nika:fetch\", args: { url: \"https://sink.example.com/in\", method: POST, body: \"${{ with.body }}\" } }\n";

    #[test]
    fn the_sweeps_settle_on_a_recover_source_declared_later() {
        let (wf, a) = parsed(SOURCE_AFTER);
        let order = judge_order(wf.tasks.len(), &a.topo_waves, &a.edges);
        let flow = settle(&wf, &order, &born(), 3);
        assert_eq!(
            flow.outputs,
            vec![
                Some("page".to_owned()),
                Some("page".to_owned()),
                Some("send".to_owned())
            ],
            "stage recovers page's content; send's own fetch is born untrusted"
        );
        let effects: Vec<Option<String>> =
            flow.witnesses.iter().map(|w| w.source.clone()).collect();
        assert_eq!(effects, vec![None, None, Some("page".to_owned())]);
    }

    #[test]
    fn a_bound_reached_with_a_fact_still_moving_refuses_every_effect() {
        // Two sweeps move a fact here, so a bound of two stops with work
        // left: the pass returns the fail-closed reading, never the facts
        // the second sweep happened to hold.
        let (wf, a) = parsed(SOURCE_AFTER);
        let order = judge_order(wf.tasks.len(), &a.topo_waves, &a.edges);
        let flow = settle(&wf, &order, &born(), 2);
        let unsettled = Some(UNSETTLED_ORIGIN.to_owned());
        assert!(
            flow.witnesses
                .iter()
                .all(|w| w.tainted && w.source == unsettled),
            "every effect carries the unsettled origin"
        );
        assert!(flow.outputs.iter().all(|o| *o == unsettled));
        assert_eq!(
            flow.writer,
            Some(("stage".to_owned(), UNSETTLED_ORIGIN.to_owned())),
            "the file channel is armed from the first task"
        );
        // The real bound leaves room for every moving sweep.
        assert!(sweep_bound(wf.tasks.len()) >= 3);
        assert_eq!(sweep_bound(3), 7);
        assert_eq!(sweep_bound(usize::MAX), usize::MAX);
    }

    #[test]
    fn the_judge_order_appends_unwind_tasks_after_their_producers() {
        // `clean` reaches `page` by an unwind edge (no wave holds it), and
        // `after_clean` waits on `clean`: both follow the waves, in order.
        let (wf, a) = parsed(
            "nika: u\npermits:\n  tools: [\"nika:jq\"]\ntasks:\n  page:\n    invoke: { tool: \"nika:jq\", args: { expression: \".\", input: \"a\" } }\n  clean:\n    after: { page: unwind }\n    invoke: { tool: \"nika:jq\", args: { expression: \".\", input: \"b\" } }\n  after_clean:\n    after: { clean: success }\n    invoke: { tool: \"nika:jq\", args: { expression: \".\", input: \"c\" } }\n  tail:\n    after: { page: success }\n    invoke: { tool: \"nika:jq\", args: { expression: \".\", input: \"d\" } }\n",
        );
        let waved: Vec<usize> = a.topo_waves.iter().flatten().copied().collect();
        assert_eq!(waved, vec![0, 3], "the waves leave the unwind task out");
        assert_eq!(
            judge_order(wf.tasks.len(), &a.topo_waves, &a.edges),
            vec![0, 3, 1, 2],
            "every task once, producers first"
        );
    }

    #[test]
    fn the_judge_order_keeps_every_task_when_the_edges_cycle() {
        // The DAG layer refuses a cycle first; the order still judges every
        // task once (declaration order for the ones no producer releases).
        let (_, a) = parsed(
            "nika: c\npermits:\n  tools: [\"nika:jq\"]\ntasks:\n  a:\n    invoke: { tool: \"nika:jq\", args: { expression: \".\", input: \"a\" } }\n  b:\n    after: { a: success }\n    invoke: { tool: \"nika:jq\", args: { expression: \".\", input: \"b\" } }\n",
        );
        let mut back = a.edges[0].clone();
        (back.from, back.to) = (1, 1);
        let cycle = [a.edges[0].clone(), back];
        assert_eq!(judge_order(3, &[], &cycle), vec![0, 2, 1]);
        assert_eq!(judge_order(2, &[vec![0, 1, 1, 7]], &[]), vec![0, 1]);
    }
}
