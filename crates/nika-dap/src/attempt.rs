// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! What a host reads before it runs a workflow a second time (ADR-156).
//!
//! A resident that lost a run may start it again. Three facts decide
//! whether it may do so on its own, and each is read here from what the
//! engine already wrote, never from a second model of the run:
//!
//! - [`effects_class`]: whether running the workflow twice can repeat an
//!   effect outside the engine, from the boundary `nika check` derives;
//! - [`rode_harness`]: whether the attempt's models were served by an agent
//!   application, from the access plan its journal states;
//! - [`interrupted_spend`]: what an attempt without a terminal record may
//!   have spent, from its journal and the check's cost rows.
//!
//! The journal of a killed run shows no call in flight: a task's frames are
//! written when it settles. So the spend counts every task that MAY have
//! been dispatched at its static bound. That bound is the one the engine
//! floors a run with: it prices the declared output, so a ceiling held with
//! it is a metered cap, as a single run's ceiling is.

use std::collections::{BTreeMap, BTreeSet};

use nika_cap::ExecPermit;
use nika_check::{CheckReport, EndpointLocus, UnboundedReason};
use nika_event::{Event, EventKind};
use nika_schema::raw::{RawAction, RawInvokeTarget, RawWorkflow};
use nika_types::resource::Value as FieldValue;

/// Whether a second run of a workflow can repeat an effect outside the
/// engine. A model call is not one: it costs money, and the spend bounds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum EffectsClass {
    /// Nothing the workflow does reaches outside: no program, no file
    /// written, no tool beyond those that read or compute, and every
    /// network use a literal `GET` or `HEAD` fetch.
    Contained,
    /// The same, provided no `agent` task is served by an agent
    /// application, whose own tools the engine does not see. Only the
    /// route a run rode decides it ([`rode_harness`]).
    ContainedOffHarness,
    /// Everything else, and every workflow whose boundary the check could
    /// not complete.
    External,
}

impl EffectsClass {
    /// The stored word.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Contained => "none",
            Self::ContainedOffHarness => "none_off_harness",
            Self::External => "external",
        }
    }

    /// The class a stored word names.
    #[must_use]
    pub fn parse(word: &str) -> Option<Self> {
        [Self::Contained, Self::ContainedOffHarness, Self::External]
            .into_iter()
            .find(|class| class.as_str() == word)
    }
}

/// Builtins that read or compute and are not in the pure-internal class.
/// `nika:fetch` is judged apart, on its method.
const READING_TOOLS: &[&str] = &["nika:glob", "nika:grep", "nika:read"];

const FETCH: &str = "nika:fetch";

fn reads_or_computes(tool: &str) -> bool {
    nika_cap::is_pure_internal(tool) || READING_TOOLS.contains(&tool)
}

/// A fetch whose method is absent, `GET` or `HEAD`, written as a literal.
fn fetch_only_reads(args: Option<&serde_json::Value>) -> bool {
    match args.and_then(|args| args.get("method")) {
        None => true,
        Some(serde_json::Value::String(method)) => {
            method.eq_ignore_ascii_case("GET") || method.eq_ignore_ascii_case("HEAD")
        }
        Some(_) => false,
    }
}

/// Class `wf` from its own check report (ADR-156 §5).
#[must_use]
pub fn effects_class(wf: &RawWorkflow, report: &CheckReport) -> EffectsClass {
    let permits = &report.permits;
    let needed = &permits.needed;
    let runs_programs = !matches!(needed.exec, None | Some(ExecPermit::No));
    let writes_files = needed.fs.as_ref().is_some_and(|fs| !fs.write.is_empty());
    if permits.partial.any() || runs_programs || writes_files {
        return EffectsClass::External;
    }
    let mut agent = false;
    for task in &wf.tasks {
        let contained = match &task.value.action {
            RawAction::Infer(_) => true,
            RawAction::Invoke(invoke) => match &invoke.target {
                RawInvokeTarget::Tool(tool) if tool.value == FETCH => {
                    fetch_only_reads(invoke.args.as_ref().map(|args| &args.value))
                }
                RawInvokeTarget::Tool(tool) => reads_or_computes(&tool.value),
                // A child workflow's effects are its own: this check does
                // not derive them.
                RawInvokeTarget::Workflow(_) => false,
            },
            RawAction::Agent(action) => {
                agent = true;
                // A model chooses a fetch's method at run time: no literal
                // exists to read, so an agent holding the tool is not contained.
                action
                    .tools
                    .iter()
                    .filter(|tool| !tool.value.starts_with('!'))
                    .all(|tool| reads_or_computes(&tool.value))
            }
            _ => false,
        };
        if !contained {
            return EffectsClass::External;
        }
    }
    if agent {
        EffectsClass::ContainedOffHarness
    } else {
        EffectsClass::Contained
    }
}

/// Whether the run a journal records was served, for any model, by an
/// agent application. A journal that started a workflow and states no
/// access plan answers `true`: an unknown route is never read as a plain
/// one. A journal with no boot record ran nothing, and answers `false`.
#[must_use]
pub fn rode_harness(events: &[Event]) -> bool {
    let Some(started) = events
        .iter()
        .find(|event| matches!(event.kind, EventKind::WorkflowStarted))
    else {
        return false;
    };
    let plan = str_field(started, "access_plan")
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok());
    // The current shape is a list of rows; the 0.117 manifests kept a map
    // keyed by model.
    let rows: Vec<&serde_json::Value> = match &plan {
        Some(serde_json::Value::Array(rows)) => rows.iter().collect(),
        Some(serde_json::Value::Object(rows)) => rows.values().collect(),
        _ => return true,
    };
    !rows.into_iter().all(plain_lane)
}

/// Routes that reach a model without an agent application in between.
const PLAIN_ROUTES: &[&str] = &["api", "local", "mock", "oauth"];

/// A lane its row states plain. A row that names no chosen route (an
/// older journal) says nothing about it and is not read as plain, the mock
/// access excepted.
fn plain_lane(row: &serde_json::Value) -> bool {
    match row.get("chosen").and_then(serde_json::Value::as_str) {
        Some(chosen) => PLAIN_ROUTES.contains(&chosen),
        None => row.get("access").and_then(serde_json::Value::as_str) == Some("mock"),
    }
}

/// The events of a lost attempt's journal at `path`, when its chain holds.
/// A torn tail is what a kill leaves, and reads. A journal that cannot be
/// read, has no chain, or whose chain is broken anywhere before its tail is
/// `None`: what it shows of the run's spend is not evidence.
#[must_use]
pub fn lost_journal(path: &std::path::Path) -> Option<Vec<Event>> {
    let raw = std::fs::read_to_string(path).ok()?; // seam-bypass-ok: L4 reading a lost run's journal, as `recover::load_events` does
    if crate::resume::judge_trust(&raw) != crate::resume::TrustVerdict::Verified {
        return None;
    }
    let label = path.display().to_string();
    Some(crate::recover::recover_events(&raw, &label).ok()?.events)
}

/// What one attempt may have committed against a USD ceiling.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct AttemptSpend {
    /// The journaled spend of settled tasks, plus the static bound of each
    /// task that may have been dispatched and left no terminal record.
    pub usd: f64,
    /// Some spend is real and cannot be bounded: a call the provider did
    /// not price or meter, or a task in flight with no bound. Never zero.
    pub unknown: bool,
}

/// Unpriced reasons a USD ceiling does not meter, the two sparing arms of
/// the run's own admission: the mock spends nothing and local compute is
/// not billed in USD.
const UNMETERED: &[&str] = &["mock_provider", "local_model"];

/// What an attempt whose journal has no terminal record may have spent
/// (ADR-156 §6). `wf` is the attempt's own workflow and `inputs` the
/// values its job bound, so the cost rows are those of the run as seated.
#[must_use]
pub fn interrupted_spend(
    wf: &RawWorkflow,
    inputs: &BTreeMap<String, serde_json::Value>,
    events: &[Event],
) -> AttemptSpend {
    let mut spend = AttemptSpend {
        usd: 0.0,
        unknown: false,
    };
    let mut settled = BTreeSet::new();
    let mut scheduled = false;
    for event in events {
        match event.kind {
            EventKind::TaskScheduled => scheduled = true,
            EventKind::TaskCompleted
            | EventKind::TaskFailed
            | EventKind::TaskSkipped
            | EventKind::TaskCancelled
            | EventKind::TaskCacheHit => {
                if let Some(task) = str_field(event, "task") {
                    settled.insert(task.to_owned());
                }
                journaled(event, &mut spend);
            }
            _ => {}
        }
    }
    // Every task is scheduled before the first one runs: a journal without
    // that record dispatched nothing.
    if !scheduled {
        return spend;
    }
    let seated = nika_runtime::effective_workflow(wf, None, inputs);
    let seated = seated.as_ref().unwrap_or(wf);
    let report = nika_check::check(seated);
    let unsettled = possibly_dispatched(seated, &report.waves, &settled);
    bound_unsettled(&report, &unsettled, &mut spend);
    spend.unknown |= child_in_flight(seated, &unsettled);
    // A catalog price is not what a call through an agent application
    // bills: that lane draws its subscription's quota. Which task rode it
    // is not in the journal, so any model call in flight is unknown spend.
    spend.unknown |= model_in_flight(&report, &unsettled) && rode_harness(events);
    spend
}

/// Whether a task in `unsettled` calls a model. A tool that calls none
/// bills no route, whichever one the run took.
fn model_in_flight(report: &CheckReport, unsettled: &BTreeSet<String>) -> bool {
    report
        .data_journey
        .model_endpoints
        .iter()
        .any(|endpoint| unsettled.contains(&endpoint.task))
}

/// Whether a task in `unsettled` calls a child workflow. Only the composed
/// check bounds a child's calls, and this reading holds the parent's own
/// check: a child in flight is unknown spend, never zero.
fn child_in_flight(wf: &RawWorkflow, unsettled: &BTreeSet<String>) -> bool {
    wf.tasks.iter().any(|task| {
        unsettled.contains(&task.value.id.value)
            && matches!(
                &task.value.action,
                RawAction::Invoke(invoke) if matches!(invoke.target, RawInvokeTarget::Workflow(_))
            )
    })
}

/// Add what a settled task's terminal frame states.
fn journaled(event: &Event, spend: &mut AttemptSpend) {
    if let Some(usd) = float_field(event, "cost_usd").filter(|usd| usd.is_finite() && *usd > 0.0) {
        spend.usd += usd;
    }
    match str_field(event, "cost_unpriced") {
        Some(reason) if UNMETERED.contains(&reason) => {}
        Some(_) => spend.unknown = true,
        // Calls excluded from the known subtotal with no reason named.
        None if int_field(event, "cost_unknown_calls").is_some_and(|calls| calls > 0) => {
            spend.unknown = true;
        }
        None => {}
    }
}

/// The tasks with no terminal record that the run may have dispatched.
///
/// Waves run one after the other and a wave's records are written before
/// the next wave exists, so the first wave with a task left unsettled is
/// where the run stopped. One case defers records past that point: a
/// recovery that waits on a task not yet terminal. A workflow that
/// declares one counts every unsettled task.
fn possibly_dispatched(
    wf: &RawWorkflow,
    waves: &[Vec<usize>],
    settled: &BTreeSet<String>,
) -> BTreeSet<String> {
    let id = |index: &usize| wf.tasks.get(*index).map(|task| task.value.id.value.clone());
    let unsettled = |wave: &Vec<usize>| -> Vec<String> {
        wave.iter()
            .filter_map(id)
            .filter(|task| !settled.contains(task))
            .collect()
    };
    let defers = wf.tasks.iter().any(|task| {
        task.value.on_error.as_ref().is_some_and(|policy| {
            matches!(policy.value.action, nika_vocab::OnErrorAction::Recover(_))
        })
    });
    if defers {
        return waves.iter().flat_map(unsettled).collect();
    }
    waves
        .iter()
        .map(unsettled)
        .find(|tasks| !tasks.is_empty())
        .map(|tasks| tasks.into_iter().collect())
        .unwrap_or_default()
}

/// Add the static bound of each task in `unsettled`.
fn bound_unsettled(report: &CheckReport, unsettled: &BTreeSet<String>, spend: &mut AttemptSpend) {
    let cost = &report.cost;
    for row in cost
        .tasks
        .iter()
        .filter(|row| unsettled.contains(&row.task))
    {
        match row.usd {
            // A route that reasons beyond its token bound bills that
            // reasoning as output: its bound covers the visible part only.
            Some(usd) if row.reasoning_beyond => {
                spend.usd += usd;
                spend.unknown = true;
            }
            Some(usd) => spend.usd += usd,
            None if unmetered_seat(report, &row.task, row.unbounded_reason) => {}
            None => spend.unknown = true,
        }
    }
    // A paid media call states the least it costs, never the most.
    spend.unknown |= cost.media.iter().any(|call| unsettled.contains(&call.task));
}

/// A task with no price because every model it names runs on this machine.
fn unmetered_seat(report: &CheckReport, task: &str, reason: Option<UnboundedReason>) -> bool {
    let mut seats = report
        .data_journey
        .model_endpoints
        .iter()
        .filter(|endpoint| endpoint.task == task)
        .peekable();
    reason == Some(UnboundedReason::NoPrice)
        && seats.peek().is_some()
        && seats.all(|endpoint| endpoint.locus == EndpointLocus::Local)
}

fn field<'a>(event: &'a Event, key: &str) -> Option<&'a FieldValue> {
    event
        .fields
        .iter()
        .find(|field| field.key == key)
        .map(|field| &field.value)
}

fn str_field<'a>(event: &'a Event, key: &str) -> Option<&'a str> {
    match field(event, key)? {
        FieldValue::String(text) => Some(text.as_str()),
        _ => None,
    }
}

fn int_field(event: &Event, key: &str) -> Option<i64> {
    match field(event, key)? {
        FieldValue::Int(number) => Some(*number),
        _ => None,
    }
}

fn float_field(event: &Event, key: &str) -> Option<f64> {
    match field(event, key)? {
        FieldValue::Float(number) => Some(*number),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
