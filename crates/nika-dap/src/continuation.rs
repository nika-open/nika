// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! One resume, judged once for every door (ADR-099; ADR-156 section 3).
//!
//! `nika run --resume` and a resident's `resume` attempt judge a journal the
//! same way and in the same order: its chain first, then what it records of
//! its project, its workflow, its engine, its model seat and its access, then
//! the facts the run continues and the plan the journal folds to. Each door
//! brings what only it knows (the journal's text, this project's
//! fingerprint, the live access lanes) and says the notices and refusals in
//! its own voice. A notice is handed over as the judgment meets it, so a door
//! that prints them keeps their order even when a refusal follows.

use std::collections::BTreeMap;
use std::path::Path;

use nika_event::{Event, EventKind};
use nika_runtime::approval::PausedApproval;
use nika_runtime::resume::{ResumePlan, ResumeUnverified};
use nika_runtime::{MissedSlots, RunFacts, ScheduledForSource};
use nika_schema::raw::RawWorkflow;
use nika_types::resource::Value as FieldValue;
use nika_types::timestamp::Timestamp;

use crate::resume::{
    AccessVerdict, CompatVerdict, LaneCarry, ModelVerdict, ProjectVerdict, SourceVerdict,
    TrustVerdict,
};

/// The attestation a run resumed from a journal with no chain carries.
const UNCHAINED: &str = "the trace carries no tamper-evidence chain (a stream capture or a \
                         pre-0.96 journal) — the records were trusted without verification";

/// What a door knows about the resume it asks for.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct ResumeAsk<'a> {
    journal: &'a Path,
    label: &'a str,
    workflow: &'a RawWorkflow,
    source: &'a str,
    engine: &'a str,
    project: Option<&'a str>,
    compat: Option<&'a str>,
    waive_broken_chain: bool,
    chain_required: bool,
    model_override: Option<&'a str>,
    access: Option<(&'a BTreeMap<String, LaneCarry>, Option<&'a str>)>,
    from: Option<&'a str>,
}

impl<'a> ResumeAsk<'a> {
    /// A resume of the journal at `journal` (named `label` in what is said
    /// about it), for `workflow` as `source` holds it, under engine `engine`.
    /// Nothing else is declared: no compatibility, no waiver, no model
    /// override, and no access judgment until [`Self::with_access`].
    #[must_use]
    pub const fn new(
        journal: &'a Path,
        label: &'a str,
        workflow: &'a RawWorkflow,
        source: &'a str,
        engine: &'a str,
    ) -> Self {
        Self {
            journal,
            label,
            workflow,
            source,
            engine,
            project: None,
            compat: None,
            waive_broken_chain: false,
            chain_required: false,
            model_override: None,
            access: None,
            from: None,
        }
    }

    /// This project's root fingerprint; `None` when it cannot be read.
    #[must_use]
    pub const fn with_project(mut self, fingerprint: Option<&'a str>) -> Self {
        self.project = fingerprint;
        self
    }

    /// The engine version a cross-version resume declares (`--resume-compat`).
    #[must_use]
    pub const fn with_compat(mut self, compat: Option<&'a str>) -> Self {
        self.compat = compat;
        self
    }

    /// Proceed over a broken chain (`--resume-unverified`), attested.
    #[must_use]
    pub const fn waiving_a_broken_chain(mut self, waive: bool) -> Self {
        self.waive_broken_chain = waive;
        self
    }

    /// Refuse a journal with no chain instead of trusting it unverified.
    #[must_use]
    pub const fn requiring_a_chain(mut self) -> Self {
        self.chain_required = true;
        self
    }

    /// The model override the resumed run declares (`--model`).
    #[must_use]
    pub const fn with_model_override(mut self, model: Option<&'a str>) -> Self {
        self.model_override = model;
        self
    }

    /// The access lanes the resumed run resolved, and the pin it declares.
    #[must_use]
    pub const fn with_access(
        mut self,
        live: &'a BTreeMap<String, LaneCarry>,
        pin: Option<&'a str>,
    ) -> Self {
        self.access = Some((live, pin));
        self
    }

    /// Run again from this task, and every task after it (`--from`).
    #[must_use]
    pub const fn from_task(mut self, from: Option<&'a str>) -> Self {
        self.from = from;
        self
    }
}

/// What the judgment says on its way, in the order it meets it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ResumeNotice {
    /// The journal has no chain: its records are trusted unverified.
    Unchained,
    /// The chain is broken and the resume was told to proceed anyway.
    BrokenChainWaived {
        /// The chain walk's finding.
        finding: String,
    },
    /// A torn tail was cut; what precedes it was recovered.
    Truncated {
        /// Where the journal tore, and what was recovered.
        note: String,
    },
    /// Values kept beside the journal are missing or altered: the tasks
    /// they belong to run again.
    Unresolved {
        /// One `<kind>.<field> sha256:<16 hex>` per value.
        values: Vec<String>,
    },
    /// The journal's project binding is a field no verified chain protects.
    UnprotectedProject,
    /// The workflow changed since the journal recorded it.
    SourceChanged,
    /// A cross-version resume proceeds under a declared compatibility.
    CrossVersion {
        /// The engine version the journal recorded.
        recorded: String,
    },
    /// The model seat changed, as the resume declared.
    SeatChanged {
        /// The override the journal recorded.
        recorded: String,
        /// The override this resume declares.
        declared: String,
    },
    /// One model's access lane changed, as the resume's pin declared.
    AccessChanged {
        /// The model.
        model: String,
        /// The lane the journal recorded.
        was: String,
        /// The lane this resume runs it on.
        now: String,
    },
    /// The journal carries no resume key: everything runs.
    NoKeys,
    /// Records without a usable resume key: those tasks run.
    Keyless {
        /// How many.
        count: usize,
    },
    /// Recorded human decisions ask again: the chain that bound them is
    /// waived.
    GatesAskAgain {
        /// The gate tasks.
        tasks: Vec<String>,
    },
}

/// Why a resume does not proceed.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ResumeRefusal {
    /// The chain is broken: the journal may have been edited, and the
    /// resume was not told to proceed over it.
    BrokenChain {
        /// The chain walk's finding.
        finding: String,
    },
    /// The recorded access lanes moved and no pin declared it.
    AccessMoved(String),
    /// Any other judgment: the journal cannot be read, or it belongs to
    /// another project, workflow, engine or model seat, or it has no chain
    /// where one is required, or `--from` names no task.
    Refused(String),
}

/// A resume that may proceed: what the resumed run is composed with.
#[non_exhaustive]
pub struct Continuation {
    /// The tasks served from the journal.
    pub plan: ResumePlan,
    /// The facts the continued run recorded; `None` on an older journal.
    pub run_facts: Option<RunFacts>,
    /// The approval ticket of a paused run.
    pub paused: Option<PausedApproval>,
    /// The engine version a declared compatibility crossed.
    pub compat: Option<String>,
    /// Set when the run proceeds without a verified chain.
    pub unverified: Option<ResumeUnverified>,
    /// The trace the resumed run continues.
    pub resumed_from: Option<String>,
}

impl std::fmt::Debug for Continuation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Continuation")
            .field("plan", &self.plan.keys().collect::<Vec<_>>())
            .field("compat", &self.compat)
            .field("unverified", &self.unverified.is_some())
            .field("resumed_from", &self.resumed_from)
            .finish_non_exhaustive()
    }
}

/// Judge a resume of the journal whose text is `raw`, and fold it.
///
/// # Errors
/// [`ResumeRefusal`]: the first judgment that refuses. The notices said
/// before it were handed to `notice` already.
pub fn continue_from(
    ask: &ResumeAsk<'_>,
    raw: &str,
    notice: &mut dyn FnMut(ResumeNotice),
) -> Result<Continuation, ResumeRefusal> {
    let unverified = trust(ask, raw, notice)?;
    let mut recovered = crate::recover::recover_events(raw, ask.label)
        .map_err(|error| ResumeRefusal::Refused(error.to_string()))?;
    if let Some(note) = recovered.truncated_note.take() {
        notice(ResumeNotice::Truncated { note });
    }
    let kept = crate::spill::rehydrate(&mut recovered.events, ask.journal);
    if !kept.unresolved.is_empty() {
        notice(ResumeNotice::Unresolved {
            values: kept.unresolved,
        });
    }
    let events = recovered.events;
    let compat = judge_recording(ask, &events, unverified.is_some(), notice)?;
    // Facts fold only after the trust, project, source and access judgments.
    let run_facts = recorded_facts(&events).map_err(ResumeRefusal::Refused)?;
    let fold = crate::resume::fold_plan(&events);
    if fold.plan.is_empty() {
        notice(ResumeNotice::NoKeys);
    } else if fold.keyless + fold.unreadable > 0 {
        notice(ResumeNotice::Keyless {
            count: fold.keyless + fold.unreadable,
        });
    }
    let mut plan = fold.plan;
    if unverified.is_some() {
        // A decision is a credential, and the chain that bound it is waived.
        let asked = crate::resume::strip_gate_records(&mut plan, ask.workflow);
        if !asked.is_empty() {
            notice(ResumeNotice::GatesAskAgain { tasks: asked });
        }
    }
    if let Some(from) = ask.from {
        crate::resume::apply_from(&mut plan, ask.workflow, from).map_err(ResumeRefusal::Refused)?;
    }
    Ok(Continuation {
        plan,
        run_facts,
        paused: fold.paused,
        compat,
        unverified,
        resumed_from: crate::resume::trace_run_id(&events),
    })
}

/// The chain verdict, before anything is folded: `None` when it holds.
fn trust(
    ask: &ResumeAsk<'_>,
    raw: &str,
    notice: &mut dyn FnMut(ResumeNotice),
) -> Result<Option<ResumeUnverified>, ResumeRefusal> {
    match crate::resume::judge_trust(raw) {
        TrustVerdict::Verified => Ok(None),
        TrustVerdict::Unverifiable if ask.chain_required => Err(ResumeRefusal::Refused(format!(
            "{} carries no tamper-evidence chain: its records cannot be verified",
            ask.label
        ))),
        TrustVerdict::Unverifiable => {
            notice(ResumeNotice::Unchained);
            Ok(Some(ResumeUnverified::Unchained(UNCHAINED.to_owned())))
        }
        TrustVerdict::Tampered { finding } if ask.waive_broken_chain => {
            notice(ResumeNotice::BrokenChainWaived {
                finding: finding.clone(),
            });
            Ok(Some(ResumeUnverified::Declared(finding)))
        }
        TrustVerdict::Tampered { finding } => Err(ResumeRefusal::BrokenChain { finding }),
    }
}

/// What the journal recorded of its project, workflow, engine, model seat
/// and access, against this resume. Returns the declared compatibility a
/// cross-version resume rides.
fn judge_recording(
    ask: &ResumeAsk<'_>,
    events: &[Event],
    unverified: bool,
    notice: &mut dyn FnMut(ResumeNotice),
) -> Result<Option<String>, ResumeRefusal> {
    // The project first: a journal of another project has nothing else to
    // judge, and no notice may describe a run that never happens.
    if unverified {
        notice(ResumeNotice::UnprotectedProject);
    }
    if let ProjectVerdict::Refuse(message) = crate::resume::judge_project(events, ask.project) {
        return Err(ResumeRefusal::Refused(message));
    }
    let id = ask.workflow.workflow.as_ref().map(|w| w.value.as_str());
    match crate::resume::judge_source(events, id, ask.source) {
        SourceVerdict::Foreign(message) => return Err(ResumeRefusal::Refused(message)),
        SourceVerdict::Changed => notice(ResumeNotice::SourceChanged),
        SourceVerdict::Unbound | SourceVerdict::Same => {}
    }
    let version = crate::resume::judge_version(events, ask.engine);
    let compat = match crate::resume::judge_resume(&version, ask.compat) {
        CompatVerdict::Proceed { compat_with } => compat_with,
        CompatVerdict::Refuse(message) => return Err(ResumeRefusal::Refused(message)),
    };
    if let Some(recorded) = &compat {
        notice(ResumeNotice::CrossVersion {
            recorded: recorded.clone(),
        });
    }
    let recorded = crate::resume::trace_model_override(events);
    match crate::resume::judge_model(recorded.as_deref(), ask.model_override) {
        ModelVerdict::Proceed { changed } => {
            if let Some((recorded, declared)) = changed {
                notice(ResumeNotice::SeatChanged { recorded, declared });
            }
        }
        ModelVerdict::Refuse(message) => return Err(ResumeRefusal::Refused(message)),
    }
    if let Some((live, pin)) = ask.access {
        let recorded = crate::resume::trace_access_lanes(events);
        match crate::resume::judge_access(recorded.as_ref(), live, pin) {
            AccessVerdict::Proceed { changed } => {
                for (model, was, now) in changed {
                    notice(ResumeNotice::AccessChanged { model, was, now });
                }
            }
            AccessVerdict::Refuse(message) => return Err(ResumeRefusal::AccessMoved(message)),
        }
    }
    Ok(compat)
}

/// The lane a resolved access plan runs each model on, as a journal's boot
/// manifest records it: the access and the pin that names it. What
/// [`ResumeAsk::with_access`] judges the recorded lanes against.
#[must_use]
pub fn live_lanes(plan: &nika_providers::ExecutionAccessPlan) -> BTreeMap<String, LaneCarry> {
    plan.admitted()
        .map(|(model, lane)| {
            let flag = crate::resume::pin_flag(&lane.plan.access, Some(lane.plan.chosen.as_str()));
            (model.to_owned(), (lane.plan.access.clone(), flag))
        })
        .collect()
}

/// The facts the continued run recorded on its first manifest (spec 04 and
/// 17): value facts, never execution, cost or approval authority. A journal
/// that records none answers `None`; a partial or malformed fact refuses
/// rather than change silently.
///
/// # Errors
/// The message naming the malformed field.
pub fn recorded_facts(events: &[Event]) -> Result<Option<RunFacts>, String> {
    let Some(started) = events
        .iter()
        .find(|event| event.kind == EventKind::WorkflowStarted)
    else {
        return Ok(None);
    };
    let at = field(started, "scheduled_for")?;
    let source = field(started, "scheduled_for_source")?;
    let missed = field(started, "missed")?;
    if at.is_none() && source.is_none() && missed.is_none() {
        return Ok(None);
    }
    let at = instant(
        at.ok_or_else(|| malformed("scheduled_for"))?,
        "scheduled_for",
    )?;
    let source = match source.ok_or_else(|| malformed("scheduled_for_source"))? {
        "slot" => ScheduledForSource::Slot,
        "at" => ScheduledForSource::At,
        "start" => ScheduledForSource::Start,
        _ => return Err(malformed("scheduled_for_source")),
    };
    let mut facts = RunFacts::default();
    facts.scheduled_for = Some((at, source));
    facts.missed = missed.map(silence).transpose()?;
    Ok(Some(facts))
}

fn malformed(field: &str) -> String {
    format!("recorded run facts contain a missing, duplicate or malformed `{field}`")
}

fn field<'a>(event: &'a Event, key: &str) -> Result<Option<&'a str>, String> {
    let mut values = event.fields.iter().filter(|entry| entry.key == key);
    let Some(entry) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(malformed(key));
    }
    match &entry.value {
        FieldValue::String(value) => Ok(Some(value)),
        _ => Err(malformed(key)),
    }
}

/// Use the same Jiff instant parser and checked run-clock conversion as `test --at`.
fn instant(text: &str, key: &str) -> Result<Timestamp, String> {
    let at = text
        .parse::<jiff::Timestamp>()
        .map_err(|_| malformed(key))?;
    i64::try_from(at.as_nanosecond())
        .map(Timestamp::from_unix_ns)
        .map_err(|_| malformed(key))
}

fn silence(text: &str) -> Result<MissedSlots, String> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|_| malformed("missed"))?;
    let first = value
        .get("first")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| malformed("missed.first"))?;
    let last = value
        .get("last")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| malformed("missed.last"))?;
    let count = value
        .get("count")
        .and_then(serde_json::Value::as_u64)
        .and_then(|count| u32::try_from(count).ok())
        .ok_or_else(|| malformed("missed.count"))?;
    Ok(MissedSlots::new(
        instant(first, "missed.first")?,
        instant(last, "missed.last")?,
        count,
    ))
}

#[cfg(test)]
mod tests;
