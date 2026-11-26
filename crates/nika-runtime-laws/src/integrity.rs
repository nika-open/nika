// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! The F-O1 runtime integrity walk (PR-1 · additive — no gate consumes
//! the label yet). Computes one task's coarse [`Integrity`] from its
//! STATIC reference surface + the already-settled upstream records, and
//! the settle spine stamps it on the record.
//!
//! The algorithm MIRRORS `nika_check`'s content-taint pass
//! (`content_flow.rs`) deliberately — the same extractor
//! (`nika_schema::expression::{scan_templates, expr_refs}`), the same
//! effect-surface table ([`nika_check::action_effect_fields`]), the same
//! born-source predicates ([`nika_cap::tool_grant_admits_ingress`] ·
//! [`nika_cap::invoke_tool_is_ingress`]) — so the runtime label and the
//! static witness CANNOT drift (check≡run by construction, ENGINE.md
//! step 1). Differences from the check, all declared:
//!
//! - it reads the LIVE records (the run's own truth), not a projected
//!   index — same least-fixpoint shape, since a task's refs only ever
//!   point at earlier waves (checker law · the wave-frozen view);
//! - `${{ inputs.X }}` reads are untrusted (the caller boundary — the
//!   mission's v1 law; the static twin lands in PR-3);
//! - a `${{ group.<name> }}` fold joins its members' record labels (spec
//!   03 §group: the fold holds each member's record), over the membership
//!   the `with:` render binds; the first untrusted member in declaration
//!   order is the witness, and a fold the membership does not name fails
//!   closed, labelled `group.<name>`;
//! - NO file channel (a tainted writer → an exec reader under `fs.read`
//!   is the declared v1 residual, ENGINE.md risk (d));
//! - `mcp:*` outputs are untrusted WITHOUT consulting the catalog's
//!   `content_trust` mark (fail-closed — the check's absent-mark
//!   default; a catalog-trusted server is over-tainted at runtime, the
//!   safe direction).
//!
//! `infer:`/`agent:` are NOT born sources: their outputs carry the join
//! of what their prompt sees (RS-06 · « un LLM n'élève jamais
//! l'intégrité ») — which falls out of the effect-field walk, since the
//! prompt+system ARE effect fields of those verbs.
//!
//! Why a settle-time static walk and not ENGINE.md's step-4 render
//! out-param: identical precision (the extractor is static on both
//! sides — a dynamic CEL path is out of the subset for BOTH), zero
//! `Scope`/`render` call-site churn, and parity by import instead of
//! parity by convention. PR-2 consumes the SAME walk per template
//! through [`ValueTaint`] — the dispatch re-gate's per-element oracle
//! (an argv element · an invoke arg leaf · the exec `cwd`).

use std::collections::BTreeMap;

use nika_cap::Integrity;
use nika_schema::expression::{NamespaceRef, expr_refs, scan_templates};
use nika_schema::raw::{ForEachValue, RawAction, RawTask};
use nika_schema::types::OnErrorAction;
use serde_json::Value;

use crate::record::TaskRecord;

/// One task's coarse output integrity: born-source first (the witness is
/// the task's own id), else the join of every taint its effect surface
/// reads, else its `on_error: recover` reads (the content-flow
/// priority — the witness named is the earliest taint origin).
///
/// `records` is the wave-frozen view (same-wave tasks never reference
/// each other — checker law), so every `tasks.X` read resolves against
/// a FINAL label. `groups` is the fold membership the `with:` render binds
/// (group name → member ids in declaration order): a `group.<name>` read
/// joins the labels of those members' records.
pub fn task_integrity(
    task: &RawTask,
    records: &BTreeMap<String, TaskRecord>,
    groups: &BTreeMap<String, Vec<String>>,
) -> Integrity {
    let taint = ValueTaint::of_task(task, records, groups);

    // Effect taint: the verb's effect-carrying fields (exec argv/shell
    //    · invoke args · infer/agent prompt+system — the infer/agent
    //    prompt-taint inversion falls out of THIS walk).
    let effect = join_refs(
        &nika_check::action_effect_fields(&task.action)
            .into_iter()
            .flat_map(refs_in_str)
            .collect::<Vec<_>>(),
        &taint.with_taint,
        taint.item_taint.as_ref(),
        records,
        groups,
        &taint.declassified,
    );

    // Born-source wins the witness; else the effect flows out; else the
    //    recover reads (the content-flow output priority).
    if born_untrusted(&task.action) {
        return Integrity::untrusted(task.id.value.clone());
    }
    if effect.is_untrusted() {
        return effect;
    }
    recover_taint(
        task,
        &taint.with_taint,
        taint.item_taint.as_ref(),
        records,
        groups,
        &taint.declassified,
    )
}

/// The per-template taint oracle (F-O1 PR-2 · the dispatch re-gate's
/// lookup). Precomputes the task-local taints ONCE per task — the
/// progressive `with:` walk and the `for_each` item taint, exactly the
/// steps [`task_integrity`] opens with, so a per-element verdict and the
/// task's own label can never drift — then labels ANY template string
/// the dispatch is about to render (an argv element · an invoke arg
/// leaf · the exec `cwd`) against the wave-frozen records.
///
/// The oracle is iteration-agnostic: a fan-out's `with:` is re-rendered
/// per item but its TAINT is a static function of the templates (an
/// `item` read joins the collection's taint, not the item's value).
///
/// F-O1 PR-3 (NEP-0004 law 5): the task's `declassify:` declarations ride
/// the oracle — a `from:` binding reads TRUSTED here (scoped to THIS
/// task, the only door through the re-gate); the receipt event is the
/// settle spine's. A door names a `tasks.<id>.output` or `inputs.<name>`
/// binding, never a `group.<name>` fold: a fold reads each member's record
/// label as it settled, and a `tasks.<id>.output` door of this task opens
/// the direct read of that record alone, never the fold holding it.
pub struct ValueTaint<'a> {
    /// `with:` slot taints, progressive (a with-value may reference an
    /// EARLIER with key — declaration order, the content-flow walk). A
    /// binding that reads `item` carries the item's taint: the run renders
    /// it once per iteration, with the item bound.
    with_taint: BTreeMap<&'a str, Integrity>,
    /// `for_each` item taint: the collection's refs taint the loop-local
    /// `item` within the task (an expression, or the template strings of a
    /// literal list: the run renders the list as a template tree).
    item_taint: Option<Integrity>,
    /// The fold membership the `with:` render binds (group name → member
    /// ids in declaration order).
    groups: &'a BTreeMap<String, Vec<String>>,
    /// The bindings THIS task declassifies (`declassify.from` verbatim ·
    /// the dotted canonical form `inputs.p` · `tasks.dl.output`).
    declassified: std::collections::BTreeSet<String>,
}

/// No fold membership: the shape of a lane that binds no `with:`.
static NO_GROUPS: BTreeMap<String, Vec<String>> = BTreeMap::new();

impl<'a> ValueTaint<'a> {
    /// The task-local taints (the first two steps of the integrity
    /// walk), borrowed from the task's own templates. `groups` is the fold
    /// membership the task's `with:` render binds.
    ///
    /// The order is the run's (spec 03 §`for_each` · the boundary render):
    /// the bindings that read no loop local first, rendered once before the
    /// fan-out; then the item, from the collection, which reads those
    /// bindings alone (an expression, or a literal list whose strings may
    /// hold templates); then the bindings that read `item` or `index`,
    /// rendered once per iteration with the item bound, so they carry the
    /// item's taint. A task without `for_each:` renders every binding at
    /// the boundary.
    #[must_use]
    pub fn of_task(
        task: &'a RawTask,
        records: &BTreeMap<String, TaskRecord>,
        groups: &'a BTreeMap<String, Vec<String>>,
    ) -> Self {
        // the `taint` doors only — `data-as-code` raises no binding
        let declassified: std::collections::BTreeSet<String> = task
            .taint_lifts()
            .filter_map(|entry| entry.from.as_ref())
            .map(|from| from.value.clone())
            .collect();
        let fans_out = task.for_each.is_some();
        let boundary = with_taints(
            task,
            BTreeMap::new(),
            |value| !(fans_out && reads_a_loop_local(value)),
            None,
            (records, groups, &declassified),
        );
        let item_taint: Option<Integrity> = task.for_each.as_ref().and_then(|f| {
            let refs = match &f.value {
                ForEachValue::Expression(src) => refs_in_str(src),
                ForEachValue::List(list) => refs_in_json(list),
                #[allow(
                    clippy::unreachable,
                    reason = "non_exhaustive future variant — enum and checker ship together; fail loud beats silently-wrong output"
                )]
                other => unreachable!("unknown for_each form: {other:?}"),
            };
            let taint = join_refs(&refs, &boundary, None, records, groups, &declassified);
            taint.is_untrusted().then_some(taint)
        });
        let with_taint = if fans_out {
            with_taints(
                task,
                boundary,
                reads_a_loop_local,
                item_taint.as_ref(),
                (records, groups, &declassified),
            )
        } else {
            boundary
        };
        Self {
            with_taint,
            item_taint,
            groups,
            declassified,
        }
    }

    /// No task-local taints — the `on_finally:` mini-task shape (no
    /// `with:` · no `for_each` · no `declassify:` — a cleanup never
    /// declares the door); the records/inputs lookups of
    /// [`ValueTaint::label`] still apply.
    #[must_use]
    pub fn bare() -> ValueTaint<'static> {
        ValueTaint {
            with_taint: BTreeMap::new(),
            item_taint: None,
            groups: &NO_GROUPS,
            declassified: std::collections::BTreeSet::new(),
        }
    }

    /// The label of ONE template's rendered value: the join of every
    /// reference the template reads (the SAME `join_refs` + `source_of`
    /// law as the task-level walk — `inputs.X` is the caller boundary,
    /// `tasks.X` reads the settled record's label, a declassified binding
    /// reads trusted HERE).
    #[must_use]
    pub fn label(&self, template: &str, records: &BTreeMap<String, TaskRecord>) -> Integrity {
        join_refs(
            &refs_in_str(template),
            &self.with_taint,
            self.item_taint.as_ref(),
            records,
            self.groups,
            &self.declassified,
        )
    }
}

/// What a `with:` value reads, the labels it joins: the records, the
/// fold membership and the task's doors.
type Reads<'r> = (
    &'r BTreeMap<String, TaskRecord>,
    &'r BTreeMap<String, Vec<String>>,
    &'r std::collections::BTreeSet<String>,
);

/// Add the slot taints of the `with:` bindings `admit` selects to
/// `with_taint`, in declaration order (a value may read an earlier key),
/// `item` read as `item_taint`.
fn with_taints<'t>(
    task: &'t RawTask,
    mut with_taint: BTreeMap<&'t str, Integrity>,
    admit: impl Fn(&Value) -> bool,
    item_taint: Option<&Integrity>,
    (records, groups, declassified): Reads<'_>,
) -> BTreeMap<&'t str, Integrity> {
    for (key, value) in task.with.iter().filter(|(_, value)| admit(&value.value)) {
        let taint = join_refs(
            &refs_in_json(&value.value),
            &with_taint,
            item_taint,
            records,
            groups,
            declassified,
        );
        if taint.is_untrusted() {
            with_taint.insert(key.value.as_str(), taint);
        }
    }
    with_taint
}

/// Whether a `with:` value reads a `for_each` loop local (`item` or
/// `index`): the run's own test (`references_loop_locals`) for the bindings
/// it renders once per iteration rather than at the boundary.
fn reads_a_loop_local(value: &Value) -> bool {
    refs_in_json(value)
        .iter()
        .any(|r| matches!(r, NamespaceRef::Item | NamespaceRef::Index))
}

/// Is the task's verb a born untrusted-ingress source (v1 · the shared
/// nika-cap predicates): an invoked `nika:fetch` · any `mcp:*` tool
/// (fail-closed — no catalog consult at runtime) · an `agent:` whose
/// whitelist admits ingress · an `exec:` (F2 · 2026-07-30 · operator
/// decision A: a subprocess is a trust boundary — its stdout is content
/// the workflow did not author, the same class `nika:fetch` is judged
/// for; the check twin is `content_flow::classify`, check≡run by
/// construction). A child `workflow:` call is not (spec 14 owns its
/// boundary — `tool()` is `None`).
fn born_untrusted(action: &RawAction) -> bool {
    match action {
        RawAction::Exec(_) => true,
        RawAction::Invoke(inv) => inv
            .tool()
            .is_some_and(|t| nika_cap::invoke_tool_is_ingress(t.value.as_str(), false)),
        RawAction::Agent(a) => a
            .tools
            .iter()
            .any(|t| nika_cap::tool_grant_admits_ingress(t.value.as_str())),
        _ => false,
    }
}

/// The `on_error: recover` value's taint (recover reads propagate — the
/// recovered output embeds what the template saw).
fn recover_taint(
    task: &RawTask,
    with_taint: &BTreeMap<&str, Integrity>,
    item_taint: Option<&Integrity>,
    records: &BTreeMap<String, TaskRecord>,
    groups: &BTreeMap<String, Vec<String>>,
    declassified: &std::collections::BTreeSet<String>,
) -> Integrity {
    let Some(on_error) = &task.on_error else {
        return Integrity::trusted();
    };
    let OnErrorAction::Recover(value) = &on_error.value.action else {
        return Integrity::trusted();
    };
    join_refs(
        &refs_in_json(&value.value),
        with_taint,
        item_taint,
        records,
        groups,
        declassified,
    )
}

/// The join of a ref set's taints — the FIRST untrusted witness sticks
/// (deterministic · the earliest origin a fix must address).
fn join_refs(
    refs: &[NamespaceRef],
    with_taint: &BTreeMap<&str, Integrity>,
    item_taint: Option<&Integrity>,
    records: &BTreeMap<String, TaskRecord>,
    groups: &BTreeMap<String, Vec<String>>,
    declassified: &std::collections::BTreeSet<String>,
) -> Integrity {
    refs.iter().fold(Integrity::trusted(), |acc, r| {
        acc.join(source_of(
            r,
            with_taint,
            item_taint,
            records,
            groups,
            declassified,
        ))
    })
}

/// One reference's taint (the content-flow `source_of`, plus the v1
/// caller-boundary law): `tasks.X` reads the settled record's label ·
/// `with.K` / `item` read the task-local taints · `inputs.X` is the
/// external boundary (Perl-taint slot rule — untrusted whether or not
/// THIS run overrode the default). `config:`/`const:` are the operator's
/// authorities · `secrets:` is the confidentiality axis's business — all
/// trusted on THIS lattice. F-O1 PR-3 (NEP-0004 law 5): a binding the
/// task's `declassify:` names reads TRUSTED here — the only door,
/// scoped to this task (the receipt event is the settle spine's). A
/// `group.<name>` fold joins its members' record labels in declaration
/// order, and one the membership does not name is untrusted; no door
/// names a fold, so none applies to it.
fn source_of(
    r: &NamespaceRef,
    with_taint: &BTreeMap<&str, Integrity>,
    item_taint: Option<&Integrity>,
    records: &BTreeMap<String, TaskRecord>,
    groups: &BTreeMap<String, Vec<String>>,
    declassified: &std::collections::BTreeSet<String>,
) -> Integrity {
    let label = |id: &str| {
        records
            .get(id)
            .map_or_else(Integrity::default, |rec| rec.integrity.clone())
    };
    match r {
        NamespaceRef::Tasks { id, .. } => {
            if declassified.contains(&format!("tasks.{id}.output")) {
                return Integrity::trusted();
            }
            label(id)
        }
        // A fold the membership does not name fails closed: nothing was
        // handed to join, so nothing proves the read clean.
        NamespaceRef::Group(name) => groups.get(name).map_or_else(
            || Integrity::untrusted(format!("group.{name}")),
            |members| {
                members
                    .iter()
                    .fold(Integrity::trusted(), |acc, id| acc.join(label(id)))
            },
        ),
        NamespaceRef::With(key) => with_taint.get(key.as_str()).cloned().unwrap_or_default(),
        NamespaceRef::Item => item_taint.cloned().unwrap_or_default(),
        NamespaceRef::Inputs(name) => {
            if declassified.contains(&format!("inputs.{name}")) {
                return Integrity::trusted();
            }
            Integrity::untrusted(format!("inputs.{name}"))
        }
        _ => Integrity::trusted(),
    }
}

/// The `${{ … }}` references inside a string — the real extractor, the
/// SAME path the check's flow pass uses (so the runtime label and
/// `NIKA-VAR-001` agree by construction).
fn refs_in_str(text: &str) -> Vec<NamespaceRef> {
    let Ok(islands) = scan_templates(text) else {
        return Vec::new();
    };
    islands.iter().flat_map(|i| expr_refs(&i.expr)).collect()
}

/// References inside any string within a JSON value (with-values ·
/// recover templates).
fn refs_in_json(value: &Value) -> Vec<NamespaceRef> {
    let mut strings = Vec::new();
    collect_json_strings(value, &mut strings);
    strings.into_iter().flat_map(refs_in_str).collect()
}

/// Every string leaf of a JSON value (the check's `collect_json_strings`
/// shape — trivial enough to own locally; the LAW that must stay shared,
/// the extraction itself, lives in `nika-schema`).
fn collect_json_strings<'a>(value: &'a Value, out: &mut Vec<&'a str>) {
    match value {
        Value::String(s) => out.push(s.as_str()),
        Value::Array(items) => {
            for item in items {
                collect_json_strings(item, out);
            }
        }
        Value::Object(map) => {
            for v in map.values() {
                collect_json_strings(v, out);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::record::{TaskStatus, TerminalCause};

    fn parse_task(yaml: &str) -> RawTask {
        let wf = nika_schema::parse(
            yaml,
            nika_schema::FileId::new(0),
            nika_schema::ParseMode::Strict,
        )
        .expect("fixture parses");
        wf.tasks.into_iter().next().expect("one task").value
    }

    fn settled(id: &str, integrity: Integrity) -> (String, TaskRecord) {
        let mut rec = TaskRecord::unran(TaskStatus::Success, TerminalCause::Normal);
        rec.attempts = Some(1);
        rec.output = Value::String(format!("{id} content"));
        rec.integrity = integrity;
        (id.to_owned(), rec)
    }

    fn records(entries: Vec<(String, TaskRecord)>) -> BTreeMap<String, TaskRecord> {
        entries.into_iter().collect()
    }

    const HEAD: &str = "nika: w\ntasks:\n";

    #[test]
    fn fetch_output_is_born_untrusted_with_its_own_id_as_witness() {
        let task = parse_task(&format!(
            "{HEAD}  dl:\n    invoke: {{ tool: \"nika:fetch\", args: {{ url: \"https://x\" }} }}\n"
        ));
        let label = task_integrity(&task, &records(Vec::new()), &NO_GROUPS);
        assert_eq!(label, Integrity::untrusted("dl"));
    }

    #[test]
    fn mcp_output_is_born_untrusted_fail_closed() {
        let task = parse_task(&format!(
            "{HEAD}  page:\n    invoke: {{ tool: \"mcp:browser/open\", args: {{ url: \"x\" }} }}\n"
        ));
        assert_eq!(
            task_integrity(&task, &records(Vec::new()), &NO_GROUPS),
            Integrity::untrusted("page")
        );
    }

    #[test]
    fn a_first_party_builtin_is_not_ingress() {
        let task = parse_task(&format!(
            "{HEAD}  r:\n    invoke: {{ tool: \"nika:read\", args: {{ path: \"x\" }} }}\n"
        ));
        assert_eq!(
            task_integrity(&task, &records(Vec::new()), &NO_GROUPS),
            Integrity::trusted()
        );
    }

    #[test]
    fn an_agent_is_born_untrusted_only_when_its_whitelist_admits_ingress() {
        let browsing = parse_task(&format!(
            "{HEAD}  a:\n    agent: {{ prompt: \"go\", tools: [\"mcp:browser/*\"] }}\n"
        ));
        assert_eq!(
            task_integrity(&browsing, &records(Vec::new()), &NO_GROUPS),
            Integrity::untrusted("a")
        );
        let local = parse_task(&format!(
            "{HEAD}  a:\n    agent: {{ prompt: \"go\", tools: [\"nika:read\"] }}\n"
        ));
        assert_eq!(
            task_integrity(&local, &records(Vec::new()), &NO_GROUPS),
            Integrity::trusted()
        );
        let negated = parse_task(&format!(
            "{HEAD}  a:\n    agent: {{ prompt: \"go\", tools: [\"!mcp:x\"] }}\n"
        ));
        assert_eq!(
            task_integrity(&negated, &records(Vec::new()), &NO_GROUPS),
            Integrity::trusted()
        );
    }

    #[test]
    fn taint_flows_from_an_upstream_output_through_with_into_the_effect() {
        // The consumer is an INFER (never a born source): the output
        // label is the pure propagation story. (It used to ride an exec —
        // since F2 2026-07-30 exec is born-untrusted, and the born
        // witness would mask the propagation this test is about.)
        let task = parse_task(&format!(
            "{HEAD}  use:\n    with: {{ page: \"${{{{ tasks.dl.output }}}}\" }}\n    infer: {{ prompt: \"${{{{ with.page }}}}\", max_tokens: 5 }}\n"
        ));
        let recs = records(vec![settled("dl", Integrity::untrusted("dl"))]);
        assert_eq!(
            task_integrity(&task, &recs, &NO_GROUPS),
            Integrity::untrusted("dl")
        );
        let clean = records(vec![settled("dl", Integrity::trusted())]);
        assert_eq!(
            task_integrity(&task, &clean, &NO_GROUPS),
            Integrity::trusted()
        );
    }

    #[test]
    fn with_values_reference_earlier_with_keys() {
        let task = parse_task(&format!(
            "{HEAD}  use:\n    with:\n      a: \"${{{{ tasks.dl.output }}}}\"\n      b: \"${{{{ with.a }}}}\"\n    infer: {{ prompt: \"${{{{ with.b }}}}\", max_tokens: 5 }}\n"
        ));
        let recs = records(vec![settled("dl", Integrity::untrusted("dl"))]);
        assert_eq!(
            task_integrity(&task, &recs, &NO_GROUPS),
            Integrity::untrusted("dl")
        );
    }

    #[test]
    fn inputs_reads_are_the_caller_boundary() {
        let task = parse_task(&format!(
            "{HEAD}  t:\n    infer: {{ prompt: \"${{{{ inputs.q }}}}\", max_tokens: 5 }}\n"
        ));
        assert_eq!(
            task_integrity(&task, &records(Vec::new()), &NO_GROUPS),
            Integrity::untrusted("inputs.q")
        );
    }

    /// An `agent:`'s `images.save:` is part of its effect: its output names
    /// each saved path, so a path the caller or untrusted content picks
    /// labels that output as a prompt reading it would, and a path the author
    /// names leaves it trusted.
    #[test]
    fn an_image_save_path_labels_the_agent_output_it_names() {
        let agent = |save: &str| {
            parse_task(&format!(
                "{HEAD}  draw:\n    agent: {{ prompt: \"draw\", images: {{ save: \"{save}\" }} }}\n"
            ))
        };
        let recs = records(vec![settled("dl", Integrity::untrusted("dl"))]);
        for (save, label) in [
            (
                "./out/${{ inputs.name }}.png",
                Integrity::untrusted("inputs.name"),
            ),
            (
                "./out/${{ tasks.dl.output }}.png",
                Integrity::untrusted("dl"),
            ),
            ("./out/hero.png", Integrity::trusted()),
        ] {
            assert_eq!(
                task_integrity(&agent(save), &recs, &NO_GROUPS),
                label,
                "{save}"
            );
        }
    }

    #[test]
    fn an_infer_never_launders_taint_but_a_clean_prompt_stays_trusted() {
        // RS-06 · « un LLM n'élève jamais l'intégrité »: the model output
        // carries the join of what its prompt sees — no more, no less.
        let tainted_prompt = parse_task(&format!(
            "{HEAD}  sum:\n    infer: {{ prompt: \"summarize ${{{{ tasks.dl.output }}}}\", max_tokens: 5 }}\n"
        ));
        let recs = records(vec![settled("dl", Integrity::untrusted("dl"))]);
        assert_eq!(
            task_integrity(&tainted_prompt, &recs, &NO_GROUPS),
            Integrity::untrusted("dl")
        );
        let clean_prompt = parse_task(&format!(
            "{HEAD}  sum:\n    infer: {{ prompt: \"summarize the plan\", max_tokens: 5 }}\n"
        ));
        assert_eq!(
            task_integrity(&clean_prompt, &recs, &NO_GROUPS),
            Integrity::trusted()
        );
    }

    /// Spec 02 §Vision inputs · the `vision:` entries are effect fields of the ONE table the
    /// check reads too: content that picks the file (or the URL) picks what the model sees, so
    /// the answer carries its label, exactly as a prompt that reads it would.
    #[test]
    fn a_vision_entry_carries_the_label_of_what_picks_it() {
        let recs = records(vec![settled("dl", Integrity::untrusted("dl"))]);
        for entry in [
            "{ source: file, path: \"${{ tasks.dl.output }}\" }",
            "{ source: url, url: \"${{ tasks.dl.output }}\" }",
        ] {
            let task = parse_task(&format!(
                "{HEAD}  look:\n    infer: {{ prompt: \"describe\", max_tokens: 5, vision: [{entry}] }}\n"
            ));
            assert_eq!(
                task_integrity(&task, &recs, &NO_GROUPS),
                Integrity::untrusted("dl"),
                "{entry}"
            );
        }
        let authored = parse_task(&format!(
            "{HEAD}  look:\n    infer: {{ prompt: \"describe\", max_tokens: 5, vision: [{{ source: file, path: \"./diagrams/flow.png\" }}] }}\n"
        ));
        assert_eq!(
            task_integrity(&authored, &recs, &NO_GROUPS),
            Integrity::trusted()
        );
    }

    #[test]
    fn exec_is_a_born_source() {
        // F2 · 2026-07-30 (operator decision A, run 5): a subprocess is a
        // trust boundary — its stdout is content the workflow did not
        // author, judged exactly like `nika:fetch`'s. The check twin is
        // `content_flow::classify` (check≡run). The file channel (a
        // tainted writer → this reader) stays the declared v1 residual
        // (ENGINE.md risk (d)) — orthogonal to this born law.
        let task = parse_task(&format!(
            "{HEAD}  t:\n    exec: {{ shell: \"cat out.txt\" }}\n"
        ));
        assert_eq!(
            task_integrity(&task, &records(Vec::new()), &NO_GROUPS),
            Integrity::untrusted("t")
        );
    }

    #[test]
    fn recover_reads_propagate_when_the_effect_is_clean() {
        let task = parse_task(&format!(
            "{HEAD}  t:\n    infer: {{ prompt: \"hi\", max_tokens: 5 }}\n    on_error: {{ recover: \"${{{{ tasks.dl.output }}}}\" }}\n"
        ));
        let recs = records(vec![settled("dl", Integrity::untrusted("dl"))]);
        assert_eq!(
            task_integrity(&task, &recs, &NO_GROUPS),
            Integrity::untrusted("dl")
        );
    }

    #[test]
    fn for_each_item_carries_the_collections_taint() {
        let task = parse_task(&format!(
            "{HEAD}  t:\n    for_each: {{ items: \"${{{{ tasks.dl.output }}}}\" }}\n    infer: {{ prompt: \"${{{{ item }}}}\", max_tokens: 5 }}\n"
        ));
        let recs = records(vec![settled("dl", Integrity::untrusted("dl"))]);
        assert_eq!(
            task_integrity(&task, &recs, &NO_GROUPS),
            Integrity::untrusted("dl")
        );
        // A literal list of plain strings is authored — clean even with an item read.
        let literal = parse_task(&format!(
            "{HEAD}  t:\n    for_each: {{ items: [\"a\", \"b\"] }}\n    infer: {{ prompt: \"${{{{ item }}}}\", max_tokens: 5 }}\n"
        ));
        assert_eq!(
            task_integrity(&literal, &records(Vec::new()), &NO_GROUPS),
            Integrity::trusted()
        );
    }

    #[test]
    fn born_source_wins_the_witness_over_propagated_taint() {
        // A fetch whose args read ANOTHER tainted task: the witness is the
        // task itself (the content-flow priority — born first).
        let task = parse_task(&format!(
            "{HEAD}  dl2:\n    invoke: {{ tool: \"nika:fetch\", args: {{ url: \"${{{{ tasks.dl1.output }}}}\" }} }}\n"
        ));
        let recs = records(vec![settled("dl1", Integrity::untrusted("dl1"))]);
        assert_eq!(
            task_integrity(&task, &recs, &NO_GROUPS),
            Integrity::untrusted("dl2")
        );
    }

    #[test]
    fn value_taint_labels_one_template_with_the_same_law() {
        // The re-gate's per-element oracle: a with-chain, a literal, an
        // inputs read, and a mixed template — the walk is the task's own.
        let task = parse_task(&format!(
            "{HEAD}  use:\n    with: {{ page: \"${{{{ tasks.dl.output }}}}\" }}\n    exec: {{ command: [\"echo\", \"${{{{ with.page }}}}\"] }}\n"
        ));
        let recs = records(vec![settled("dl", Integrity::untrusted("dl"))]);
        let oracle = ValueTaint::of_task(&task, &recs, &NO_GROUPS);
        assert_eq!(
            oracle.label("${{ with.page }}", &recs),
            Integrity::untrusted("dl"),
            "a with-slot tainted by an upstream fetch"
        );
        assert_eq!(
            oracle.label("prefix-${{ with.page }}-suffix", &recs),
            Integrity::untrusted("dl"),
            "a mixed template joins every island it reads"
        );
        assert_eq!(
            oracle.label("${{ inputs.q }}", &recs),
            Integrity::untrusted("inputs.q"),
            "the caller boundary"
        );
        assert_eq!(
            oracle.label("a literal element", &recs),
            Integrity::trusted()
        );
        assert_eq!(
            oracle.label("${{ const.home }}", &recs),
            Integrity::trusted(),
            "the authored constant stays trusted"
        );
    }

    #[test]
    fn value_taint_item_reads_the_collections_taint() {
        let task = parse_task(&format!(
            "{HEAD}  t:\n    for_each: {{ items: \"${{{{ tasks.dl.output }}}}\" }}\n    exec: {{ command: [\"echo\", \"${{{{ item }}}}\"] }}\n"
        ));
        let recs = records(vec![settled("dl", Integrity::untrusted("dl"))]);
        let oracle = ValueTaint::of_task(&task, &recs, &NO_GROUPS);
        assert_eq!(
            oracle.label("${{ item }}", &recs),
            Integrity::untrusted("dl")
        );
        // A literal list of plain strings is authored — the item stays clean.
        let literal = parse_task(&format!(
            "{HEAD}  t:\n    for_each: {{ items: [\"a\", \"b\"] }}\n    exec: {{ command: [\"echo\", \"${{{{ item }}}}\"] }}\n"
        ));
        assert_eq!(
            ValueTaint::of_task(&literal, &recs, &NO_GROUPS).label("${{ item }}", &recs),
            Integrity::trusted()
        );
    }

    #[test]
    fn value_taint_bare_keeps_the_records_and_inputs_lookups() {
        // The `on_finally:` mini-task shape: no with/for_each, but a
        // `tasks.X` or `inputs.X` read still taints (the cleanup re-gate).
        let oracle = ValueTaint::bare();
        let recs = records(vec![settled("dl", Integrity::untrusted("dl"))]);
        assert_eq!(
            oracle.label("${{ tasks.dl.output }}", &recs),
            Integrity::untrusted("dl")
        );
        assert_eq!(
            oracle.label("${{ inputs.q }}", &recs),
            Integrity::untrusted("inputs.q")
        );
        assert_eq!(
            oracle.label("${{ with.page }}", &recs),
            Integrity::trusted(),
            "a mini-task has no with: to taint"
        );
    }

    #[test]
    fn declassify_lifts_the_named_binding_scoped_to_the_task() {
        // NEP-0004 law 5 — the ONLY door: a task-level `declassify:` entry
        // raises its `from:` binding to trusted HERE (the re-gate oracle
        // AND the task's own output label), and leaves every OTHER
        // binding tainted (never a blanket lift).
        let task = parse_task(&format!(
            "{HEAD}  load:\n    invoke:\n      tool: \"nika:read\"\n      args: {{ path: \"${{{{ inputs.p }}}}\" }}\n    lift:\n      - {{ law: taint, from: inputs.p, because: \"reviewed vendor path\" }}\n"
        ));
        let recs = records(Vec::new());
        let oracle = ValueTaint::of_task(&task, &recs, &NO_GROUPS);
        assert_eq!(
            oracle.label("${{ inputs.p }}", &recs),
            Integrity::trusted(),
            "the declassified binding reads trusted"
        );
        assert_eq!(
            oracle.label("${{ inputs.other }}", &recs),
            Integrity::untrusted("inputs.other"),
            "an unnamed binding stays tainted — the lift is scoped"
        );
        // …and the task's own output label follows (the author vouched).
        assert_eq!(
            task_integrity(&task, &recs, &NO_GROUPS),
            Integrity::trusted()
        );

        // A tasks.<id>.output door lifts the record's label at the
        // CONSUMER (the record itself keeps its provenance). (The consumer
        // rides an invoke, never a born source — since F2 an exec's own
        // output is untrusted regardless of what it read, which would
        // mask the door this test is about.)
        let consumer = parse_task(&format!(
            "{HEAD}  use:\n    invoke:\n      tool: \"nika:read\"\n      args: {{ path: \"${{{{ tasks.dl.output }}}}\" }}\n    lift:\n      - {{ law: taint, from: tasks.dl.output, because: \"pinned artifact, hash-reviewed\" }}\n"
        ));
        let recs = records(vec![settled("dl", Integrity::untrusted("dl"))]);
        assert_eq!(
            ValueTaint::of_task(&consumer, &recs, &NO_GROUPS)
                .label("${{ tasks.dl.output }}", &recs),
            Integrity::trusted(),
            "the tasks.* door lifts the consumer's read"
        );
        assert_eq!(
            task_integrity(&consumer, &recs, &NO_GROUPS),
            Integrity::trusted()
        );
        // …but a DIFFERENT tainted record still taints the same task.
        let recs2 = records(vec![
            settled("dl", Integrity::untrusted("dl")),
            settled("page", Integrity::untrusted("page")),
        ]);
        assert_eq!(
            ValueTaint::of_task(&consumer, &recs2, &NO_GROUPS)
                .label("${{ tasks.page.output }}", &recs2),
            Integrity::untrusted("page"),
            "the door names ONE binding — page stays tainted"
        );
    }

    /// The fold membership the `with:` render binds, from literal pairs.
    fn groups(entries: &[(&str, &[&str])]) -> BTreeMap<String, Vec<String>> {
        entries
            .iter()
            .map(|(name, ids)| {
                (
                    (*name).to_owned(),
                    ids.iter().map(|id| (*id).to_owned()).collect(),
                )
            })
            .collect()
    }

    /// `notes` is clean, `dl` and `dl2` are untrusted, each its own witness.
    fn mixed_members() -> BTreeMap<String, TaskRecord> {
        records(vec![
            settled("notes", Integrity::trusted()),
            settled("dl", Integrity::untrusted("dl")),
            settled("dl2", Integrity::untrusted("dl2")),
        ])
    }

    #[test]
    fn a_fold_over_an_untrusted_member_labels_that_member() {
        // The fold holds each member's record (spec 03 §group): it carries
        // the members' labels, the first untrusted one in declaration order
        // as the witness — never the trusted default of an unknown root.
        let task = parse_task(&format!(
            "{HEAD}  use:\n    with: {{ got: \"${{{{ group.logs }}}}\" }}\n    exec: {{ command: [\"echo\", \"${{{{ with.got }}}}\"] }}\n"
        ));
        let recs = mixed_members();
        let logs = groups(&[("logs", &["notes", "dl", "dl2"])]);
        let oracle = ValueTaint::of_task(&task, &recs, &logs);
        assert_eq!(
            oracle.label("${{ group.logs }}", &recs),
            Integrity::untrusted("dl"),
            "the first untrusted member in declaration order is the witness"
        );
        let reversed = groups(&[("logs", &["dl2", "notes", "dl"])]);
        assert_eq!(
            ValueTaint::of_task(&task, &recs, &reversed).label("${{ group.logs }}", &recs),
            Integrity::untrusted("dl2"),
            "the witness follows the declaration order, not the id order"
        );
        assert_eq!(
            ValueTaint::of_task(&task, &recs, &NO_GROUPS).label("${{ group.logs }}", &recs),
            Integrity::untrusted("group.logs"),
            "without the membership the fold fails closed, labelled by itself"
        );
    }

    #[test]
    fn a_fold_over_clean_members_stays_trusted() {
        let task = parse_task(&format!(
            "{HEAD}  use:\n    with: {{ got: \"${{{{ group.logs }}}}\" }}\n    infer: {{ prompt: \"${{{{ with.got }}}}\", max_tokens: 5 }}\n"
        ));
        let recs = records(vec![
            settled("notes", Integrity::trusted()),
            settled("more", Integrity::trusted()),
        ]);
        let logs = groups(&[("logs", &["notes", "more"])]);
        assert_eq!(task_integrity(&task, &recs, &logs), Integrity::trusted());
        assert_eq!(
            ValueTaint::of_task(&task, &recs, &logs).label("${{ with.got }}", &recs),
            Integrity::trusted()
        );
    }

    #[test]
    fn a_fold_the_membership_does_not_name_fails_closed() {
        // A caller that hands no membership (or the wrong one) must never
        // make a fold read clean: the read is labelled by the fold itself.
        let task = parse_task(&format!(
            "{HEAD}  use:\n    with: {{ got: \"${{{{ group.logs }}}}\" }}\n    infer: {{ prompt: \"${{{{ with.got }}}}\", max_tokens: 5 }}\n"
        ));
        let recs = records(vec![settled("notes", Integrity::trusted())]);
        for membership in [NO_GROUPS.clone(), groups(&[("pages", &["notes"])])] {
            assert_eq!(
                task_integrity(&task, &recs, &membership),
                Integrity::untrusted("group.logs"),
                "{membership:?}"
            );
            assert_eq!(
                ValueTaint::of_task(&task, &recs, &membership).label("${{ with.got }}", &recs),
                Integrity::untrusted("group.logs"),
                "{membership:?}"
            );
        }
        let named = groups(&[("logs", &["notes"])]);
        assert_eq!(task_integrity(&task, &recs, &named), Integrity::trusted());
    }

    #[test]
    fn a_with_alias_of_a_fold_carries_the_member_label() {
        // `group.*` is legal in a `with:` value alone (NIKA-VAR-021), so the
        // alias is the shape every fold reaches an effect through.
        let task = parse_task(&format!(
            "{HEAD}  use:\n    with: {{ got: \"${{{{ group.logs }}}}\" }}\n    infer: {{ prompt: \"${{{{ with.got }}}}\", max_tokens: 5 }}\n"
        ));
        let recs = mixed_members();
        let logs = groups(&[("logs", &["notes", "dl"])]);
        assert_eq!(
            task_integrity(&task, &recs, &logs),
            Integrity::untrusted("dl"),
            "an infer reading the alias carries the member's label"
        );
        assert_eq!(
            ValueTaint::of_task(&task, &recs, &logs).label("prefix ${{ with.got }}", &recs),
            Integrity::untrusted("dl"),
            "the re-gate oracle reads the alias the same way"
        );
    }

    #[test]
    fn a_for_each_over_the_alias_carries_the_member_label() {
        let task = parse_task(&format!(
            "{HEAD}  use:\n    with: {{ got: \"${{{{ group.logs }}}}\" }}\n    for_each: {{ items: \"${{{{ with.got }}}}\" }}\n    infer: {{ prompt: \"${{{{ item }}}}\", max_tokens: 5 }}\n"
        ));
        let recs = mixed_members();
        let logs = groups(&[("logs", &["notes", "dl"])]);
        assert_eq!(
            task_integrity(&task, &recs, &logs),
            Integrity::untrusted("dl")
        );
        assert_eq!(
            ValueTaint::of_task(&task, &recs, &logs).label("${{ item }}", &recs),
            Integrity::untrusted("dl")
        );
    }

    #[test]
    fn a_recover_through_the_alias_carries_the_member_label() {
        // The effect is clean (an authored prompt); the recovered value
        // embeds the fold, so the record carries the member's label.
        let task = parse_task(&format!(
            "{HEAD}  use:\n    with: {{ got: \"${{{{ group.logs }}}}\" }}\n    infer: {{ prompt: \"hi\", max_tokens: 5 }}\n    on_error: {{ recover: \"${{{{ with.got }}}}\" }}\n"
        ));
        let recs = mixed_members();
        let logs = groups(&[("logs", &["notes", "dl"])]);
        assert_eq!(
            task_integrity(&task, &recs, &logs),
            Integrity::untrusted("dl")
        );
        let clean = groups(&[("logs", &["notes"])]);
        assert_eq!(task_integrity(&task, &recs, &clean), Integrity::trusted());
    }

    #[test]
    fn a_tasks_door_does_not_open_through_a_fold() {
        // A `tasks.dl.output` door lifts the direct read of `dl` alone: the
        // fold holding `dl` keeps the member's label (no door names a fold).
        let task = parse_task(&format!(
            "{HEAD}  use:\n    with: {{ got: \"${{{{ group.logs }}}}\" }}\n    invoke:\n      tool: \"nika:read\"\n      args: {{ path: \"${{{{ with.got }}}}\" }}\n    lift:\n      - {{ law: taint, from: tasks.dl.output, because: \"pinned artifact\" }}\n"
        ));
        let recs = mixed_members();
        let logs = groups(&[("logs", &["dl"])]);
        let oracle = ValueTaint::of_task(&task, &recs, &logs);
        assert_eq!(
            oracle.label("${{ tasks.dl.output }}", &recs),
            Integrity::trusted()
        );
        assert_eq!(
            oracle.label("${{ with.got }}", &recs),
            Integrity::untrusted("dl")
        );
        assert_eq!(
            task_integrity(&task, &recs, &logs),
            Integrity::untrusted("dl")
        );
    }

    #[test]
    fn a_with_binding_that_reads_the_item_carries_the_items_label() {
        // The run renders a binding that reads `item` once per iteration,
        // with the item bound (spec 03 §`for_each`): it carries the item's
        // label, while the collection itself reads the item-free bindings.
        let task = parse_task(&format!(
            "{HEAD}  relay:\n    for_each: {{ items: \"${{{{ tasks.pull.output }}}}\", max_items: 10 }}\n    with: {{ one: \"${{{{ item }}}}\" }}\n    infer: {{ prompt: \"${{{{ with.one }}}}\", max_tokens: 5 }}\n"
        ));
        let recs = records(vec![settled("pull", Integrity::untrusted("pull"))]);
        assert_eq!(
            task_integrity(&task, &recs, &NO_GROUPS),
            Integrity::untrusted("pull")
        );
        assert_eq!(
            ValueTaint::of_task(&task, &recs, &NO_GROUPS).label("${{ with.one }}", &recs),
            Integrity::untrusted("pull")
        );
        let clean = records(vec![settled("pull", Integrity::trusted())]);
        assert_eq!(
            task_integrity(&task, &clean, &NO_GROUPS),
            Integrity::trusted()
        );

        // The fold twin: the collection reads the alias of a fold, and the
        // per-iteration binding reads the item.
        let folded = parse_task(&format!(
            "{HEAD}  relay:\n    with: {{ all: \"${{{{ group.logs }}}}\", one: \"${{{{ item }}}}\" }}\n    for_each: {{ items: \"${{{{ with.all }}}}\", max_items: 10 }}\n    infer: {{ prompt: \"${{{{ with.one }}}}\", max_tokens: 5 }}\n"
        ));
        let recs = mixed_members();
        let logs = groups(&[("logs", &["notes", "dl"])]);
        assert_eq!(
            ValueTaint::of_task(&folded, &recs, &logs).label("${{ with.one }}", &recs),
            Integrity::untrusted("dl")
        );
    }

    #[test]
    fn a_literal_list_carries_what_its_templates_read() {
        // The run renders a literal `for_each:` list as a template tree, so
        // a string in it that reads a tainted binding taints the item.
        let task = parse_task(&format!(
            "{HEAD}  t:\n    with: {{ got: \"${{{{ tasks.dl.output }}}}\" }}\n    for_each: {{ items: [\"${{{{ with.got }}}}\", \"fixed\"] }}\n    exec: {{ command: [\"echo\", \"${{{{ item }}}}\"] }}\n"
        ));
        let recs = records(vec![settled("dl", Integrity::untrusted("dl"))]);
        assert_eq!(
            ValueTaint::of_task(&task, &recs, &NO_GROUPS).label("${{ item }}", &recs),
            Integrity::untrusted("dl")
        );
        let clean = records(vec![settled("dl", Integrity::trusted())]);
        assert_eq!(
            ValueTaint::of_task(&task, &clean, &NO_GROUPS).label("${{ item }}", &clean),
            Integrity::trusted(),
            "a list of clean strings stays clean"
        );
    }

    #[test]
    fn the_collection_never_reads_a_per_iteration_binding() {
        // `one` reads `item`, so the run renders it per iteration and drops it
        // from the boundary render the collection reads: the collection sees
        // no `with.one`, while the binding itself carries `dl`'s label.
        let task = parse_task(&format!(
            "{HEAD}  t:\n    with: {{ one: \"${{{{ item }}}} ${{{{ tasks.dl.output }}}}\" }}\n    for_each: {{ items: \"${{{{ with.one }}}}\" }}\n    exec: {{ command: [\"echo\", \"${{{{ with.one }}}}\"] }}\n"
        ));
        let recs = records(vec![settled("dl", Integrity::untrusted("dl"))]);
        let oracle = ValueTaint::of_task(&task, &recs, &NO_GROUPS);
        assert_eq!(oracle.label("${{ item }}", &recs), Integrity::trusted());
        assert_eq!(
            oracle.label("${{ with.one }}", &recs),
            Integrity::untrusted("dl")
        );
        // An `index` read makes a binding per-iteration too: the collection
        // does not see it, though it carries no taint of its own.
        let indexed = parse_task(&format!(
            "{HEAD}  t:\n    with: {{ at: \"${{{{ index }}}} ${{{{ tasks.dl.output }}}}\" }}\n    for_each: {{ items: \"${{{{ with.at }}}}\" }}\n    exec: {{ command: [\"echo\", \"${{{{ with.at }}}}\"] }}\n"
        ));
        let oracle = ValueTaint::of_task(&indexed, &recs, &NO_GROUPS);
        assert_eq!(oracle.label("${{ item }}", &recs), Integrity::trusted());
        assert_eq!(
            oracle.label("${{ with.at }}", &recs),
            Integrity::untrusted("dl")
        );
    }
}
