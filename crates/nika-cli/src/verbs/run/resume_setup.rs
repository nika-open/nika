// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! The `--resume`/`--answer` surface (ADR-099 · F4) — validated and
//! folded BEFORE the runtime composition. Extracted from `mod.rs`
//! 2026-07-30 (the 1500-LOC cap · the same descent pressure as
//! `budget.rs`); the semantics are unchanged.

// The run verb streams live output (the render IS the run) — the same
// sanctioned exemption `mod.rs` carries.
#![allow(clippy::disallowed_macros, clippy::print_stdout, clippy::print_stderr)]

use std::collections::BTreeMap;

use nika_dap::continuation::{ResumeAsk, ResumeNotice, ResumeRefusal};
use nika_dap::resume::ResumeRequest;
use nika_runtime::approval::PausedApproval;
use nika_runtime::resume::{ResumePlan, ResumeUnverified};
use nika_schema::raw::RawWorkflow;
use serde_json::Value;

use super::epilogue;
use crate::verbs::exit;

/// The validated `--resume`/`--answer` inputs the composition consumes.
pub(super) struct ResumeSetup {
    /// The folded skip plan (`None` = no `--resume` requested).
    pub plan: Option<ResumePlan>,
    /// Facts recorded by the continued run; absent on a fresh or older trace.
    pub run_facts: Option<nika_runtime::RunFacts>,
    /// The validated `--answer task=value` map (empty without answers).
    pub answers: BTreeMap<String, Value>,
    /// The F-P4 resume authority (NEP-0013) — the approval ticket folded
    /// from the paused trace (`None` on a fresh run or a pre-F-P4 trace).
    pub paused: Option<PausedApproval>,
    /// The F-P21 declared compat (NEP-0014 law 4) — the recorded engine
    /// version the operator allowed the crossing from (`Some` only when
    /// a cross-version resume proceeds under `--resume-compat`).
    pub compat: Option<String>,
    /// The ADR-099 trust attestation (2026-08-08) — `Some` when the run
    /// proceeds WITHOUT a verified chain (the declared opt-out · the
    /// chainless compat), journaled on the boot manifest so no unverified
    /// ancestor launders silently. `None` = the chain verified (or no resume).
    pub unverified: Option<ResumeUnverified>,
    /// The trace this run CONTINUES (#1462) — the resumed journal's own
    /// trace id, attested on the boot manifest and the settlement so a
    /// continuation renders as one. `None` = a fresh run.
    pub resumed_from: Option<String>,
    /// The resumed journal, whose failed run may have moved files aside:
    /// they go back before this run starts (consumed by
    /// [`Self::bind_durable`]). `None` = a fresh run.
    pub restore_from: Option<std::path::PathBuf>,
    /// The files that restore put back: the run's ledger adopts them, so a
    /// failure of this run quarantines them again.
    pub restored: Vec<String>,
}

impl ResumeSetup {
    /// Attach durable replay protection only after Run monetary admission,
    /// then settle what a crashed run left in this project, then put back
    /// what the resumed run's teardown moved aside: this run's cache hits
    /// name those files as written. A task whose file stays in quarantine
    /// is not completed: it runs again.
    pub(super) fn bind_durable(mut self, wf: &RawWorkflow, output_json: bool) -> Result<Self, u8> {
        self.paused = durable_paused(self.paused, output_json)?;
        // First, so a resume of the crashed journal itself puts its moved
        // files back like a failed run's.
        say_settled(&nika_dap::quarantine::settle_crashed(std::path::Path::new(
            "",
        )));
        if let Some(journal) = self.restore_from.take() {
            let restored = nika_dap::quarantine::restore(&journal);
            say_restored(restored.as_ref());
            if let Some(restored) = restored.as_ref() {
                self.restored = restored_paths(restored);
                if let Some(plan) = self.plan.as_mut() {
                    rerun_unrestored(plan, wf, restored);
                }
            }
        }
        Ok(self)
    }

    /// The tasks the resumed journal completed: each is served from it as a
    /// cache hit, so the budget gate prices only the rest (NIK-245). Empty on
    /// a fresh run.
    pub(super) fn settled(&self) -> std::collections::BTreeSet<String> {
        (self.plan.iter())
            .flat_map(|plan| plan.keys().cloned())
            .collect()
    }

    /// A fresh run (no trace to fold) — the answers ride in later.
    const fn fresh() -> Self {
        Self {
            plan: None,
            run_facts: None,
            answers: BTreeMap::new(),
            paused: None,
            compat: None,
            unverified: None,
            resumed_from: None,
            restore_from: None,
            restored: Vec::new(),
        }
    }
}

/// One stderr line per file put back or left in quarantine (stdout stays
/// the machine lanes' own).
fn say_restored(restored: Option<&Value>) {
    let Some(restored) = restored else {
        return;
    };
    let dir = restored["dir"].as_str().unwrap_or_default();
    for entry in restored["restored"].as_array().into_iter().flatten() {
        let path = entry["path"].as_str().unwrap_or_default();
        match entry["reason"].as_str() {
            None => eprintln!("nika run: --resume: put back {path} from {dir}"),
            Some(reason) => eprintln!("nika run: --resume: {path} stays in {dir}: {reason}"),
        }
    }
}

/// The paths a restore put back in place.
fn restored_paths(restored: &Value) -> Vec<String> {
    restored["restored"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|entry| entry.get("restored_from").is_some())
        .filter_map(|entry| entry["path"].as_str().map(str::to_owned))
        .collect()
}

/// One stderr line per action taken on a crashed run's leftovers: a file
/// it created moved to its quarantine, one kept in place and why, a
/// person's file it changed with its original, a staging left as it is.
fn say_settled(folds: &[Value]) {
    for fold in folds {
        let run = format!(
            "a crashed run ({})",
            fold["crashed"].as_str().unwrap_or("?")
        );
        if let Some(reason) = fold["reason"].as_str() {
            let staging = fold["staging_left"].as_str().unwrap_or_default();
            eprintln!("nika run: {run} left {staging} as it is: {reason}");
            continue;
        }
        for entry in fold["outputs"].as_array().into_iter().flatten() {
            let path = entry["path"].as_str().unwrap_or_default();
            match entry["quarantined_to"].as_str() {
                Some(to) => eprintln!("nika run: {run} left {path}: moved to {to}"),
                None => eprintln!(
                    "nika run: {run} left {path}: it stays in place, the move failed: {}",
                    entry["error"].as_str().unwrap_or("unknown")
                ),
            }
        }
        for entry in fold["kept"].as_array().into_iter().flatten() {
            say_kept(&run, entry);
        }
        if let Some(staging) = fold["staging_left"].as_str() {
            eprintln!("nika run: {run} left copies in {staging}: they stay as they are");
        }
    }
}

/// The line for a file a crashed run's settlement left in place.
fn say_kept(run: &str, entry: &Value) {
    let path = entry["path"].as_str().unwrap_or_default();
    if let Some(original) = entry["original"].as_str() {
        eprintln!(
            "nika run: {run} changed {path}: it stays in place; its original is at {original}"
        );
    } else if let Some(error) = entry["original_error"].as_str() {
        eprintln!(
            "nika run: {run} changed {path}: it stays in place; its original was not kept: {error}"
        );
    } else {
        let reason = entry["reason"].as_str().unwrap_or("its bytes are unproven");
        eprintln!("nika run: {run} left {path} in place: {reason}");
    }
}

/// Drop from the plan every writer whose file stayed in quarantine, with
/// every task after it (the `--from` walk): they read what it wrote, and a
/// cache hit would call it written while its bytes sit in quarantine.
fn rerun_unrestored(plan: &mut ResumePlan, wf: &RawWorkflow, restored: &Value) {
    let stays = nika_dap::quarantine::left_quarantined(restored);
    for task in nika_dap::quarantine::tasks_naming(plan, wf, &stays) {
        if nika_dap::resume::apply_from(plan, wf, &task).is_ok() {
            eprintln!(
                "nika run: --resume: {task} runs again, with the tasks after it: its file did not come back"
            );
        }
    }
}

/// The ENV-class refusal, one voice: said on stderr, enveloped on the
/// machine face, and the exit code returned for `?`.
fn refuse_env(message: &str, output_json: bool) -> u8 {
    eprintln!("nika run: {message}");
    epilogue::emit_error_envelope(message, output_json);
    exit::ENV
}

/// Validate + fold the whole `--resume` surface (plan · `--from` ·
/// `--answer`) BEFORE composing — every refusal is the ENV class,
/// already printed + enveloped.
///
/// # Errors
///
/// The exit code to return unchanged.
pub(super) fn resume_setup(
    resume: Option<&ResumeRequest>,
    wf: &RawWorkflow,
    source: &str,
    model_override: Option<&str>,
    access: (&nika_providers::ExecutionAccessPlan, Option<&str>),
    output_json: bool,
) -> Result<ResumeSetup, u8> {
    validate_resume(resume, wf, source, model_override, access, output_json)?
        .bind_durable(wf, output_json)
}

/// The ceiling a resumed run continues under when the resume states none: what the ceiling its
/// paused leg was admitted under leaves after that leg's spend (live gauntlet r04 B8, 2026-10-17:
/// the leg after a gate ran unbounded, « budget: none journaled »). `None` for a fresh run, a
/// trace that cannot be read, or a leg admitted under no ceiling. Said on stderr unless `machine`.
pub(super) fn resumed_ceiling(resume: Option<&ResumeRequest>, machine: bool) -> Option<f64> {
    let trace = resume?.trace.as_deref()?;
    let facts = nika_trace::run_view::RunFacts::read(trace)?;
    let left = facts.ceiling_left()?;
    if !machine {
        eprintln!("resumed under the run's own ceiling: ${left:.4} left");
    }
    Some(left)
}

/// Read, validate and fold the resume without opening or pruning durable claims.
/// Run can reject a changed access lane before asking for spending authority.
/// The returned setup must bind its durable replay protection before execution.
pub(super) fn validate_resume(
    resume: Option<&ResumeRequest>,
    wf: &RawWorkflow,
    source: &str,
    model_override: Option<&str>,
    access: (&nika_providers::ExecutionAccessPlan, Option<&str>),
    output_json: bool,
) -> Result<ResumeSetup, u8> {
    // The answers-only form (F4): no trace, no plan — the answers below
    // ride into the gate map and wait for the ask.
    let mut setup = match resume.and_then(|req| req.trace.as_deref().map(|t| (req, t))) {
        None => ResumeSetup::fresh(),
        Some((req, trace)) => {
            load_resume_plan(req, trace, wf, source, model_override, access, output_json)?
        }
    };
    let pairs = resume.map_or(&[][..], |r| r.answers.as_slice());
    setup.answers = nika_dap::resume::parse_answers(pairs, wf)
        .map_err(|message| refuse_env(&message, output_json))?;
    // #1067 · a journaled success is a decision. `--answer` on resume
    // used to force the prompt to re-run (ADR-099 F4 "operator intent");
    // that turned a recorded NO into a shipment. Paused gates are not in
    // the plan (they never completed), so they still accept answers.
    if let Some(plan) = &setup.plan {
        nika_dap::resume::refuse_reopened_settled_gates(plan, &setup.answers)
            .map_err(|message| refuse_env(&message, output_json))?;
    }
    Ok(setup)
}

/// Read + fold the `--resume` trace into the runtime skip plan (ADR-099)
/// plus the F-P4 paused ticket (NEP-0013) plus the F-P21 version verdict
/// (NEP-0014 law 4). The TRUST judgment comes FIRST (ADR-099 trust
/// amendment · 2026-08-08): the tamper-evidence chain is verified BEFORE
/// anything is folded — the forgery class refuses (FILE, one voice with
/// `trace verify`), rides the NAMED `--resume-unverified` opt-out, or
/// proceeds under the chainless compat — both attested on the boot
/// manifest (`resume_unverified`), never a silent default. The
/// cross-version judgment follows: a resume under an engine different
/// from the recording one is an explicit refusal naming both versions —
/// or rides a declared compat (`--resume-compat`). Honest degradation
/// stays the contract for the KEYS: a keyless trace (older engine)
/// yields an EMPTY plan + a notice — never an error; an unreadable file
/// or an unknown `--from` id is refused loudly (environment class).
///
/// # Errors
///
/// The exit code (already printed + enveloped) — FILE for the tamper
/// class, ENV for every other refusal.
fn load_resume_plan(
    req: &ResumeRequest,
    trace: &std::path::Path,
    wf: &RawWorkflow,
    source: &str,
    model_override: Option<&str>,
    access: (&nika_providers::ExecutionAccessPlan, Option<&str>),
    output_json: bool,
) -> Result<ResumeSetup, u8> {
    let label = trace.display().to_string();
    let raw = read_trace(trace, &label, output_json)?;
    // The same fingerprint the composition root stamps (#1367): the
    // process cwd for a local run.
    let here = std::env::current_dir()
        .ok()
        .and_then(|cwd| nika_runtime::project_root_fingerprint(&cwd));
    let live = nika_dap::continuation::live_lanes(access.0);
    let ask = ResumeAsk::new(trace, &label, wf, source, env!("CARGO_PKG_VERSION"))
        .with_project(here.as_deref())
        .with_compat(req.compat.as_deref())
        .waiving_a_broken_chain(req.allow_unverified)
        .with_model_override(model_override)
        .with_access(&live, access.1)
        .from_task(req.from.as_deref());
    let continuation =
        nika_dap::continuation::continue_from(&ask, &raw, &mut |notice| say(&notice, &label))
            .map_err(|refusal| refuse_resume(refusal, &label, output_json))?;
    Ok(ResumeSetup {
        plan: Some(continuation.plan),
        run_facts: continuation.run_facts,
        answers: BTreeMap::new(),
        paused: continuation.paused,
        compat: continuation.compat,
        unverified: continuation.unverified,
        // #1462 · the continuation link: this leg names the trace it folded.
        resumed_from: continuation.resumed_from,
        restore_from: Some(trace.to_path_buf()),
        restored: Vec::new(),
    })
}

/// One stderr line per notice of the resume judgment, in its order (stdout
/// stays the machine lanes' own).
fn say(notice: &ResumeNotice, label: &str) {
    match notice {
        ResumeNotice::Unchained => eprintln!(
            "nika run: --resume: {label} carries no tamper-evidence chain (a stream \
             capture or a pre-0.96 journal) — the records are trusted WITHOUT \
             verification (attested on the run's boot manifest)"
        ),
        ResumeNotice::BrokenChainWaived { finding } => eprintln!(
            "nika run: --resume: {finding} — proceeding under --resume-unverified: \
             the records are trusted WITHOUT chain verification (attested on the \
             run's boot manifest)"
        ),
        ResumeNotice::Truncated { note } => eprintln!("nika run: {note}"),
        // NIK-131 · never served as a cache hit, the tasks they belong to run again.
        ResumeNotice::Unresolved { values } => eprintln!(
            "nika run: --resume: {} value(s) kept beside {label} are missing or altered ({}) — \
             the tasks they belong to run live",
            values.len(),
            values.join(" · ")
        ),
        ResumeNotice::UnprotectedProject => eprintln!(
            "nika run: --resume: unverified — the trace's project binding is a recorded field the \
             waived chain no longer protects; resume only a trace you wrote"
        ),
        ResumeNotice::SourceChanged => eprintln!(
            "nika run: --resume: the workflow file CHANGED since {label} recorded it — the \
             current bytes are what runs (an edited `model:` moves the seat, and edited tasks \
             re-run instead of serving the recorded output)"
        ),
        ResumeNotice::CrossVersion { recorded } => eprintln!(
            "nika run: --resume: cross-version compat declared — the trace was \
             recorded under engine {recorded}, this engine is {} (attested on \
             the run's boot manifest)",
            env!("CARGO_PKG_VERSION")
        ),
        ResumeNotice::SeatChanged { recorded, declared } => eprintln!(
            "nika run: --resume: model change declared — the trace was recorded \
             under --model {recorded}, this resume runs --model {declared}"
        ),
        ResumeNotice::AccessChanged { model, was, now } => eprintln!(
            "nika run: --resume: access change declared — {label} ran `{model}` on \
             `{was}`, this resume runs it on `{now}`"
        ),
        ResumeNotice::NoKeys => eprintln!(
            "nika run: --resume: {label} carries no resume keys — running everything live"
        ),
        ResumeNotice::Keyless { count } => eprintln!(
            "nika run: --resume: {count} record(s) without a usable resume key — those tasks run live"
        ),
        ResumeNotice::GatesAskAgain { tasks } => eprintln!(
            "nika run: --resume: unverified — {} recorded human decision(s) re-ask ({}) · a \
             decision is a credential and the chain that bound it is waived",
            tasks.len(),
            tasks.join(" · ")
        ),
        // ResumeNotice is #[non_exhaustive]: a notice newer than this CLI is
        // still said, never dropped.
        other => eprintln!("nika run: --resume: {other:?}"),
    }
}

/// The exit class of a refused resume, said and enveloped: the forgery class
/// is FILE with the teaching `nika trace verify` shares (ADR-099 trust
/// amendment), every other refusal ENV.
fn refuse_resume(refusal: ResumeRefusal, label: &str, output_json: bool) -> u8 {
    match refusal {
        ResumeRefusal::BrokenChain { finding } => {
            let message = format!(
                "--resume: {finding}\n  a resume trusts the trace's recorded successes — \
                 the chain verdict is the resume's precondition (ADR-099)\n  verify: \
                 nika trace verify {label} · or re-run fresh · or resume under \
                 --resume-unverified (attested on the run's boot manifest)"
            );
            eprintln!("nika run: {message}");
            epilogue::emit_error_envelope(&message, output_json);
            exit::FILE
        }
        ResumeRefusal::AccessMoved(message) => refuse_env(
            &format!("--resume: {} · {message}", nika_error::codes::NIKA_1807),
            output_json,
        ),
        ResumeRefusal::Refused(message) => refuse_env(&format!("--resume: {message}"), output_json),
        // ResumeRefusal is #[non_exhaustive]: a class newer than this CLI
        // refuses, fail closed.
        other => refuse_env(&format!("--resume: {other:?}"), output_json),
    }
}

/// Bind the folded ticket to the durable claim stores (`<root>/.nika/
/// approval-claims` · the replay guard): the operator's HOME and the run's
/// project, the working directory the project judgment already bound the
/// trace to (#1367), so a consumed ticket is refused from any HOME (E39
/// N5). A trace resumed under the waived chain (`--resume-unverified`) or
/// one older than the project binding can still reach a fresh store from
/// another project and HOME; its ticket TTL bounds it. Then prune the
/// claims that outlived every trace on the way in (#1466): the stores are
/// touched only here, so this is where they stay bounded. Pruning is
/// fail-open and speaks exactly one line when anything was removed (D2).
fn durable_paused(
    paused: Option<PausedApproval>,
    output_json: bool,
) -> Result<Option<PausedApproval>, u8> {
    let Some(mut approval) = paused else {
        return Ok(None);
    };
    let home = std::env::home_dir().ok_or_else(|| {
        refuse_env(
            "--resume: HOME is unavailable; the durable approval claim store cannot be opened",
            output_json,
        )
    })?;
    let project = std::env::current_dir().map_err(|error| {
        refuse_env(
            &format!("--resume: the project directory is unavailable: {error}"),
            output_json,
        )
    })?;
    let one = |a: &std::path::Path, b: &std::path::Path| {
        std::fs::canonicalize(a)
            .ok()
            .is_some_and(|a| std::fs::canonicalize(b).is_ok_and(|b| a == b))
    };
    let roots = if one(&home, &project) {
        vec![home]
    } else {
        vec![home, project]
    };
    let (cfg, _notes) = nika_dap::retention::RetentionConfig::from_env();
    for root in &roots {
        if let Some(n) = nika_dap::retention::prune_claims(root, &cfg, std::time::SystemTime::now())
        {
            eprintln!("nika run: approval claims gc · removed {n} expired claim(s)");
        }
        approval = approval.with_durable_claim_root(root).map_err(|error| {
            refuse_env(
                &format!("--resume: cannot open the durable approval claim store: {error}"),
                output_json,
            )
        })?;
    }
    Ok(Some(approval))
}

fn read_trace(trace: &std::path::Path, label: &str, output_json: bool) -> Result<String, u8> {
    let refuse = |message: String| refuse_env(&message, output_json);
    // The freeze audit · a run in flight cannot be resumed: its writer
    // holds the journal's lease, and a second execution over a partial
    // journal would re-run and re-spend its in-flight tasks (ADR-129 · the
    // lease is the liveness truth, never a guess).
    if let nika_dap::liveness::Liveness::Alive { pid } = nika_dap::liveness::probe(trace) {
        return Err(refuse(format!(
            "--resume: the writer of {label} is alive (pid {pid} on this host) — a run in flight cannot be resumed: wait for its terminal, or cancel it"
        )));
    }
    let trace_parent = trace
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    let trace_name = trace
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .ok_or_else(|| refuse(format!("--resume: invalid trace path {label}")))?;
    let trace_dir =
        nika_fs::OwnedDir::open(trace_parent.unwrap_or_else(|| std::path::Path::new(".")))
            .map_err(|error| {
                refuse(format!(
                    "--resume: cannot open the trace directory for {label}: {error}"
                ))
            })?;
    trace_dir
        .read(trace_name)
        .map_err(|error| refuse(format!("--resume: cannot read {label}: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The CLI face of a refused resume (#1586 · ADR-099): the forgery class
    /// is FILE, a foreign journal or a moved lane is ENV (exit 3 · the
    /// cross-project posture).
    #[test]
    fn a_broken_chain_is_the_file_class_and_every_other_refusal_env() {
        let broken = ResumeRefusal::BrokenChain {
            finding: "chain BROKEN at line 2".to_owned(),
        };
        assert_eq!(refuse_resume(broken, "t", true), exit::FILE);
        let foreign = ResumeRefusal::Refused("the trace was written by `compose-brief`".to_owned());
        assert_eq!(refuse_resume(foreign, "t", true), exit::ENV);
        let moved = ResumeRefusal::AccessMoved("`mock/echo` moved".to_owned());
        assert_eq!(refuse_resume(moved, "t", true), exit::ENV);
    }
}
