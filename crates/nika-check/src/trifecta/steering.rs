// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! The branch-steering hint: the control-flow side of the lethal trifecta.
//!
//! `NIKA-SEC-009` follows untrusted content into a task's EFFECT (its
//! arguments, its prompt · [`crate::content_flow`]). A `when:` guard is not
//! an effect field: content a guard reads never enters the payload, and
//! still decides WHETHER the task runs. With the three legs armed, a file
//! can read private notes, fetch a page, and POST the notes only when the
//! page says so: the body is clean, the trifecta lane is silent, and
//! whoever writes the page chooses whether the notes are sent.
//!
//! **The rule (version 1, narrow on purpose).** One hint per task `T` when
//! all four hold:
//!
//! 1. the trifecta's legs are armed, read off the verdict that lane already
//!    reached ([`nika_cap::TrifectaVerdict::armed`] · a declared `permits:`,
//!    a graph that conforms, private read ∧ realized untrusted ingress ∧
//!    external egress);
//! 2. `T`'s `when:` reads a `with:` key carrying the CONTENT of a task whose
//!    output the content-taint pass marks (`ContentFlow::outputs`, the same
//!    reading the refusal judges · [`super::born_flow`]). A guard reads a
//!    task's content through `with:` and nowhere else (`NIKA-VAR-021`);
//! 3. `T`, or a task downstream of it over any derived edge, acts: the
//!    consent lane's [`Sink`] (the effect table's egress-capable tasks, and
//!    a composed call whose closure reaches one);
//! 4. the trifecta did not already refuse `T` as a sink: the refusal owns
//!    that task and its repair.
//!
//! **Three reads that are not steering.**
//!
//! - The answer of a blocking human gate ([`super::human_gate`]), even one
//!   whose message showed the content: a person chose. A prompt carrying
//!   `default:` answers itself unattended, so it is read like any task.
//! - A guard a human confirm closes on « no » ([`human_confirm`] ∧
//!   [`affirmed_by`] · the refusal substitution of the affirmative-consent
//!   law): `with.go == true && with.news != null` runs only once a person
//!   said yes, so the content can cancel an approved action and never start
//!   one. The same guard with `||`, or under a prompt that approves by
//!   default, is steered.
//! - A record field the engine authors (`status` · `error` · timings):
//!   [`crate::analyzer::EdgeKind::carries_value`] is the one predicate.
//!
//! The hint fires whether or not a gate sits upstream. The trifecta lane
//! credits a gate that dominates the sink for content reaching an
//! argument; this lane credits a gate only where the guard is PROVEN to
//! need its yes (the second read above). A confirm gate scheduled before
//! the task the guard reads is named in the advice, with the binding that
//! makes its approval provable; a gate in another mode (a choice, an input)
//! gives no typed yes to test and is not named. A confirm gate that
//! dominates `T` is always upstream of that task (the guard's binding is a
//! direct edge from it, so a gate on every route to `T` sits before it), so
//! a dominating confirm never goes unmentioned.
//!
//! **What this lane does not judge.** Each line is a narrowing of version
//! 1, never a claim that the shape is safe:
//!
//! - a guard that reads only `inputs.*`: a root check does not know who
//!   calls, and this lane does not follow a call's arguments into the
//!   guards of its child;
//! - a guard INSIDE a child workflow: a composed check reads a call's
//!   closure for where content goes ([`ComposedCall`]), never its guards.
//!   The call itself is judged here, as a producer and as a task that acts;
//! - the other ways content picks a branch: a `for_each:` collection (how
//!   many times, zero included), an `after:` predicate and a status read
//!   (the remote party can make a fetch fail);
//! - whether the downstream task's run depends on `T`'s fate: rule 3 is
//!   reachability, so an `after: { T: terminal }` consumer counts;
//! - a refusal that closes the guard through a skipped producer: `P` runs
//!   only on a person's yes and `T` tests `with.p != null`, so a « no »
//!   skips `P`, its output reads null and `T` skips too. The content can
//!   only cancel there, and the gate substrate proves one guard against one
//!   answer, never that chain: `T` is flagged (an over-approximation), and
//!   the advice's repair, the answer tested by `T` itself, is the provable
//!   form;
//! - the other steering reads of one guard: one hint per task, naming the
//!   first such read in source order.
//!
//! A HINT, never a finding: `is_clean` ignores it, nothing is refused, and
//! no spec code is minted (a new `NIKA-SEC-*` is the spec's to rule first).

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use nika_schema::expression::NamespaceRef;
use nika_schema::raw::{RawTask, RawWorkflow};
use nika_schema::types::WhenGate;

use crate::ComposedCall;
use crate::analyzer::edges::{group_members, producer_ids};
use crate::analyzer::gates::{affirmed_by, human_confirm};
use crate::analyzer::{Edge, role_of_field};
use crate::consent::Sink;
use crate::flow::{collect_json_strings, refs_in_str};
use crate::hints::Hint;

/// The hint kind (`nika explain branch-steering` · the catalog row).
pub(crate) const KIND: &str = "branch-steering";

/// The hints over a checked graph. None unless the trifecta lane found its
/// legs armed: the default verdict (no `permits:` · a graph that did not
/// conform · a missing leg) is never armed, so this lane inherits that
/// lane's gating and judges nothing on its own authority.
pub(crate) fn hints(
    wf: &RawWorkflow,
    edges: &[Edge],
    topo_waves: &[Vec<usize>],
    calls: &[ComposedCall],
    verdict: &nika_cap::TrifectaVerdict,
) -> Vec<Hint> {
    if !verdict.armed {
        return Vec::new();
    }
    let guards: Vec<(usize, &str)> = wf
        .tasks
        .iter()
        .enumerate()
        .filter_map(|(at, t)| match t.value.when.as_ref().map(|w| &w.value) {
            Some(WhenGate::Expr(src)) => Some((at, src.as_str())),
            _ => None,
        })
        .collect();
    if guards.is_empty() {
        return Vec::new();
    }
    let graph = Graph::of(wf, edges, topo_waves, calls);
    guards
        .into_iter()
        .filter_map(|(at, src)| graph.steered(at, src, verdict))
        .collect()
}

/// One read of a guard: the `with:` key it names, the task whose content
/// the key carries, and the ingress that content was born at.
struct Read {
    key: String,
    producer: usize,
    origin: String,
}

/// What every guard's judgment reads, derived once per check.
struct Graph<'a> {
    wf: &'a RawWorkflow,
    ids: BTreeMap<&'a str, usize>,
    members: BTreeMap<&'a str, Vec<usize>>,
    /// Per task: the untrusted origin its output carries.
    outputs: Vec<Option<String>>,
    /// Per task: the nearest task that acts, itself or downstream.
    acts_below: Vec<Option<usize>>,
    /// Per task: a confirm gate scheduled before it.
    confirm_above: Vec<Option<usize>>,
}

impl<'a> Graph<'a> {
    fn of(
        wf: &'a RawWorkflow,
        edges: &[Edge],
        topo_waves: &[Vec<usize>],
        calls: &[ComposedCall],
    ) -> Self {
        let n = wf.tasks.len();
        let task = |i: usize| &wf.tasks[i].value;
        // Downstream is EVERY derived edge (a skipped task unwinds nothing,
        // so its cleanup is a consequence too); « settles before » is
        // ORDER, which an unwind attachment does not carry.
        let (mut up, mut down) = (vec![Vec::new(); n], vec![Vec::new(); n]);
        for e in edges.iter().filter(|e| e.from < n && e.to < n) {
            up[e.to].push(e.from);
            if e.kind.is_scheduling() {
                down[e.from].push(e.to);
            }
        }
        let acting: Vec<usize> = (0..n)
            .filter(|&i| Sink::of_task(task(i), calls) != Sink::Inert)
            .collect();
        // The gates whose answer a guard can test as a typed yes: the ones
        // `approved_by_a_person` credits once bound.
        let confirms: Vec<usize> = (0..n).filter(|&i| human_confirm(task(i))).collect();
        Self {
            wf,
            ids: wf
                .tasks
                .iter()
                .enumerate()
                .map(|(i, t)| (t.value.id.value.as_str(), i))
                .collect(),
            members: group_members(&wf.tasks),
            outputs: super::born_flow(wf, topo_waves, calls).outputs,
            acts_below: nearest(&up, &acting, true),
            confirm_above: nearest(&down, &confirms, false),
        }
    }

    /// The hint for the guard `src` of task `at`, when rules 2 to 4 hold
    /// (the caller checked rule 1).
    fn steered(&self, at: usize, src: &str, verdict: &nika_cap::TrifectaVerdict) -> Option<Hint> {
        let task = &self.wf.tasks.get(at)?.value;
        let id = task.id.value.as_str();
        if verdict.violations.iter().any(|v| v.task == id) {
            return None;
        }
        let read = self.steering_read(task, src)?;
        if self.approved_by_a_person(task) {
            return None;
        }
        let acts = self.acts_below.get(at).copied().flatten()?;
        Some(Hint {
            kind: KIND,
            code: None,
            task: id.to_owned(),
            advice: self.advice(at, &read, acts),
        })
    }

    /// The first read of the guard, in source order, that reaches a marked
    /// producer no person stands for.
    fn steering_read(&self, task: &RawTask, src: &str) -> Option<Read> {
        refs_in_str(src).into_iter().find_map(|r| {
            let NamespaceRef::With(key) = r else {
                return None;
            };
            let mut seen = BTreeSet::new();
            self.producers(task, &key, &mut seen)
                .into_iter()
                .find_map(|producer| {
                    let origin = self.outputs.get(producer)?.clone()?;
                    let gate = super::human_gate(&self.wf.tasks.get(producer)?.value.action);
                    (!gate).then(|| Read {
                        key: key.clone(),
                        producer,
                        origin,
                    })
                })
        })
    }

    /// The tasks whose CONTENT `with.<key>` carries: a value reference (the
    /// output or a named binding, never a field the engine authors), every
    /// member of a group fold, and what another key of the same task
    /// carries.
    fn producers(&self, task: &RawTask, key: &str, seen: &mut BTreeSet<String>) -> Vec<usize> {
        if !seen.insert(key.to_owned()) {
            return Vec::new();
        }
        let Some((_, value)) = task.with.iter().find(|(k, _)| k.value == key) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for r in collect_json_strings(&value.value)
            .into_iter()
            .flat_map(refs_in_str)
        {
            match r {
                NamespaceRef::Tasks { id, field }
                    if role_of_field(field.as_deref()).carries_value() =>
                {
                    out.extend(self.ids.get(id.as_str()).copied());
                }
                NamespaceRef::Group(name) => {
                    out.extend(
                        self.members
                            .get(name.as_str())
                            .into_iter()
                            .flatten()
                            .copied(),
                    );
                }
                NamespaceRef::With(other) => out.extend(self.producers(task, &other, seen)),
                _ => {}
            }
        }
        out
    }

    /// Whether the guard runs the task only once a person said yes: a human
    /// confirm the task binds, whose « no » (and whose skip) proves the
    /// guard false. Only the task's own producers can be that gate.
    fn approved_by_a_person(&self, task: &RawTask) -> bool {
        producer_ids(task).iter().any(|gate| {
            self.ids
                .get(gate.as_str())
                .and_then(|&g| self.wf.tasks.get(g))
                .is_some_and(|g| human_confirm(&g.value) && affirmed_by(task, gate))
        })
    }

    fn advice(&self, at: usize, read: &Read, acts: usize) -> String {
        let name = |i: usize| {
            self.wf
                .tasks
                .get(i)
                .map_or("?", |t| t.value.id.value.as_str())
        };
        let (id, key, producer) = (name(at), read.key.as_str(), name(read.producer));
        let carried = if read.origin == producer {
            String::new()
        } else {
            format!(", which carries untrusted content from `{}`", read.origin)
        };
        let egress = if acts == at {
            format!("`{id}` is an egress task")
        } else {
            format!("`{}`, downstream of `{id}`, is an egress task", name(acts))
        };
        let confirm = self.confirm_above.get(read.producer).copied().flatten();
        let repair = match confirm.map(name) {
            Some(gate) => format!(
                "The confirm gate `{gate}` settles before `{producer}` runs, and this guard is \
                 not proven closed by its refusal: bind its answer and test it first (`with: {{ \
                 go: \"${{{{ tasks.{gate}.output }}}}\" }}` + `when: ${{{{ with.go == true && \
                 <content test> }}}}`)"
            ),
            None => "Bind the answer of a confirm gate (a blocking `nika:prompt`) and test it \
                     first (`with: { go: \"${{ tasks.<gate>.output }}\" }` + `when: ${{ with.go \
                     == true && <content test> }}`)"
                .to_owned(),
        };
        format!(
            "untrusted content chooses whether `{id}` runs: its `when:` reads `with.{key}`, the \
             output of `{producer}`{carried}, while private read, untrusted ingress and external \
             egress are all permitted and {egress}. NIKA-SEC-009 judges content that reaches a \
             task's arguments, and a guard is not an argument: that law is silent on this read, \
             and whoever writes the content picks the branch. {repair}, so the content can only \
             cancel what a person approved, or drop one of the three grants from `permits:`. \
             Advisory (`is_clean` ignores it)"
        )
    }
}

/// Per node, the nearest of `seeds` that reaches it over `next` (fewest
/// hops, then seed order). A seed reaches itself only when `own`.
fn nearest(next: &[Vec<usize>], seeds: &[usize], own: bool) -> Vec<Option<usize>> {
    let mut found: Vec<Option<usize>> = vec![None; next.len()];
    let mut queue: VecDeque<(usize, usize)> = VecDeque::new();
    for &seed in seeds {
        if own {
            found[seed] = Some(seed);
        }
        queue.push_back((seed, seed));
    }
    while let Some((node, seed)) = queue.pop_front() {
        for &to in &next[node] {
            if found[to].is_none() {
                found[to] = Some(seed);
                queue.push_back((to, seed));
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::KIND;
    use crate::{CheckReport, Hint, check, check_composed};
    use nika_schema::parser::{ParseMode, parse};
    use nika_schema::source::FileId;

    /// The witness: private notes, a fetched page, and a POST whose BODY is
    /// the notes alone. Only the guard reads the page, so the data-flow lane
    /// (`NIKA-SEC-009`) is silent, and whoever writes the page still chooses
    /// whether the notes are sent.
    const WITNESS: &str = r#"nika: steer-probe
model: mock/echo
permits:
  tools: ["nika:read", "nika:fetch"]
  net: { http: ["hnrss.org", "hooks.slack.com"] }
  fs:
    read: ["notes/week.md"]
tasks:
  notes:
    invoke: { tool: "nika:read", args: { path: "./notes/week.md" } }
  news:
    invoke: { tool: "nika:fetch", args: { url: "https://hnrss.org/frontpage" } }
  post:
    with:
      notes: ${{ tasks.notes.output }}
      news: ${{ tasks.news.output }}
    when: ${{ with.news.contains('urgent') }}
    invoke:
      tool: "nika:fetch"
      args: { url: "https://hooks.slack.com/services/T000/B000/XXXX", method: POST, body: "${{ with.notes }}" }
"#;

    const TOOLS: &str = r#"tools: ["nika:read", "nika:fetch"]"#;
    const TOOLS_AND_PROMPT: &str = r#"tools: ["nika:read", "nika:fetch", "nika:prompt"]"#;
    const GUARD: &str = "when: ${{ with.news.contains('urgent') }}";
    const NEWS_BINDING: &str = "      news: ${{ tasks.news.output }}\n";
    const POST: &str = "  post:\n";
    /// A blocking confirm gate, declared before `post`.
    const ASK: &str = "  ask:\n    invoke: { tool: \"nika:prompt\", args: { message: \"Post the weekly notes?\" } }\n  post:\n";

    fn report(yaml: &str) -> CheckReport {
        check(&parse(yaml, FileId::new(0), ParseMode::Strict).expect("fixture parses"))
    }

    fn steered(report: &CheckReport) -> Vec<&Hint> {
        report.hints.iter().filter(|h| h.kind == KIND).collect()
    }

    /// One edit of a fixture, refused when the text it replaces is gone: a
    /// variant that silently equals its base proves nothing.
    fn swap(yaml: &str, from: &str, to: &str) -> String {
        assert_eq!(yaml.matches(from).count(), 1, "fixture drift on `{from}`");
        yaml.replacen(from, to, 1)
    }

    /// The witness with a blocking confirm gate whose answer `post` binds
    /// as `with.go`, under `guard`.
    fn gated(guard: &str) -> String {
        let y = swap(WITNESS, TOOLS, TOOLS_AND_PROMPT);
        let y = swap(&y, POST, ASK);
        let y = swap(
            &y,
            NEWS_BINDING,
            "      news: ${{ tasks.news.output }}\n      go: ${{ tasks.ask.output }}\n",
        );
        swap(&y, GUARD, guard)
    }

    #[test]
    fn the_witness_gets_one_hint_naming_the_task_and_the_producer() {
        let r = report(WITNESS);
        assert!(
            r.trifecta_findings.is_empty() && r.trifecta_mitigations.is_empty(),
            "the body carries no untrusted content, so the data-flow lane is silent: {:?}",
            r.trifecta_findings
        );
        let hints = steered(&r);
        assert_eq!(hints.len(), 1, "{:?}", r.hints);
        assert_eq!(hints[0].task, "post");
        assert_eq!(
            hints[0].code, None,
            "no spec code is minted for an advisory"
        );
        let advice = &hints[0].advice;
        for part in [
            "its `when:` reads `with.news`, the output of `news`",
            "whether `post` runs",
            "`post` is an egress task",
            "NIKA-SEC-009",
            "`is_clean` ignores it",
        ] {
            assert!(advice.contains(part), "missing `{part}`: {advice}");
        }
        assert!(
            !advice.contains("The confirm gate") && advice.contains("tasks.<gate>.output"),
            "no gate exists, so none is named: {advice}"
        );
    }

    #[test]
    fn the_hint_never_changes_is_clean() {
        let r = report(WITNESS);
        assert_eq!(steered(&r).len(), 1);
        assert!(r.is_clean(), "a hint is never a refusal: {:?}", r.findings);
        assert!(r.findings.is_empty(), "no finding row: {:?}", r.findings);
        assert!(r.extra_conformance_codes().is_empty());
        assert!(
            !steered(&r)[0].is_paid_run(),
            "an advisory, not a paid-run blocker"
        );
    }

    #[test]
    fn the_printed_kind_has_a_catalog_row() {
        let help = crate::hint_help(KIND).expect("`nika explain branch-steering` has a row");
        for part in ["`when:`", "`inputs.*`", "child workflow", "is_clean"] {
            assert!(help.contains(part), "missing `{part}`: {help}");
        }
    }

    #[test]
    fn a_guard_on_a_human_answer_alone_is_not_steered() {
        let r = report(&gated("when: ${{ with.go == true }}"));
        assert!(steered(&r).is_empty(), "{:?}", steered(&r));
    }

    /// A gate that SHOWS the page and is then read by the guard: the content
    /// pass marks the gate's output (the page reached its message), and the
    /// answer is still a person's. The same prompt with a `default:` answers
    /// itself unattended: it is no blocking gate, and is read like any task.
    #[test]
    fn a_blocking_gate_that_shows_the_content_still_answers_for_a_person() {
        let shown = "  ask:\n    with: { news: \"${{ tasks.news.output }}\" }\n    invoke: { tool: \"nika:prompt\", args: { mode: choice, message: \"${{ with.news }}\", choices: [\"skip\", \"send\"] } }\n  post:\n";
        let y = swap(WITNESS, TOOLS, TOOLS_AND_PROMPT);
        let y = swap(&y, POST, shown);
        let y = swap(&y, NEWS_BINDING, "      pick: ${{ tasks.ask.output }}\n");
        let y = swap(&y, GUARD, "when: ${{ with.pick == 'send' }}");
        assert!(steered(&report(&y)).is_empty(), "{:?}", report(&y).hints);

        let defaulted = swap(&y, "mode: choice,", "mode: choice, default: \"skip\",");
        let r = report(&defaulted);
        let hints = steered(&r);
        assert_eq!(hints.len(), 1, "{:?}", r.hints);
        assert!(
            hints[0].advice.contains(
                "`with.pick`, the output of `ask`, which carries untrusted content from `news`"
            ),
            "{}",
            hints[0].advice
        );
    }

    /// A guard a human confirm closes on « no » is not steered: the content
    /// can only cancel what a person approved. An `||` lets the content open
    /// the branch alone, and a `default: true` approves with nobody there.
    #[test]
    fn a_mixed_guard_is_steered_unless_a_human_no_closes_it() {
        let both = "when: ${{ with.go == true && with.news.contains('urgent') }}";
        assert!(steered(&report(&gated(both))).is_empty());

        let either = "when: ${{ with.go == true || with.news.contains('urgent') }}";
        let r = report(&gated(either));
        assert_eq!(steered(&r).len(), 1, "{:?}", r.hints);
        assert_eq!(steered(&r)[0].task, "post");

        let unattended = swap(
            &gated(both),
            "message: \"Post the weekly notes?\"",
            "message: \"Post the weekly notes?\", default: true",
        );
        let r = report(&unattended);
        assert_eq!(steered(&r).len(), 1, "{:?}", r.hints);
    }

    /// The hint fires whether or not a gate sits upstream: a confirm answered
    /// before the page is fetched is named, with the binding that makes its
    /// approval provable. Applied as printed, that repair silences the hint.
    #[test]
    fn an_upstream_confirm_gate_is_named_with_the_binding_that_proves_it() {
        let y = swap(WITNESS, TOOLS, TOOLS_AND_PROMPT);
        let y = swap(
            &y,
            "  news:\n",
            "  ask:\n    invoke: { tool: \"nika:prompt\", args: { message: \"Run the weekly post?\" } }\n  news:\n    after: { ask: success }\n",
        );
        let r = report(&y);
        let hints = steered(&r);
        assert_eq!(hints.len(), 1, "{:?}", r.hints);
        assert_eq!(hints[0].task, "post");
        for part in [
            "The confirm gate `ask` settles before `news` runs, and this guard is not proven closed by its refusal",
            "`with: { go: \"${{ tasks.ask.output }}\" }` + `when: ${{ with.go == true && <content test> }}`",
        ] {
            assert!(
                hints[0].advice.contains(part),
                "missing `{part}`: {}",
                hints[0].advice
            );
        }

        let repaired = swap(
            &y,
            NEWS_BINDING,
            "      news: ${{ tasks.news.output }}\n      go: \"${{ tasks.ask.output }}\"\n",
        );
        let repaired = swap(
            &repaired,
            GUARD,
            "when: ${{ with.go == true && with.news.contains('urgent') }}",
        );
        let r = report(&repaired);
        assert!(r.conformance.is_empty(), "{:?}", r.conformance);
        assert!(steered(&r).is_empty(), "{:?}", steered(&r));
    }

    /// Rule 3, both sides: a steered `infer:` with nothing after it sends
    /// nothing; the same task feeding a POST decides whether the POST has
    /// anything to send, and the hint names that task.
    #[test]
    fn no_egress_at_or_below_the_guard_means_no_hint() {
        let digest = "    infer: { prompt: \"Summarize: ${{ with.notes }}\", max_tokens: 64 }\n";
        let (head, _) = WITNESS
            .split_once("    invoke:\n      tool: \"nika:fetch\"\n")
            .expect("the POST");
        let quiet = swap(&format!("{head}{digest}"), POST, "  digest:\n");
        let r = report(&quiet);
        assert!(r.trifecta_findings.is_empty(), "{:?}", r.trifecta_findings);
        assert!(steered(&r).is_empty(), "{:?}", steered(&r));

        let sent = format!(
            "{quiet}  post:\n    with: {{ digest: \"${{{{ tasks.digest.output }}}}\" }}\n    invoke:\n      tool: \"nika:fetch\"\n      args: {{ url: \"https://hooks.slack.com/services/T000/B000/XXXX\", method: POST, body: \"${{{{ with.digest }}}}\" }}\n"
        );
        let r = report(&sent);
        assert!(r.trifecta_findings.is_empty(), "{:?}", r.trifecta_findings);
        let hints = steered(&r);
        assert_eq!(hints.len(), 1, "{:?}", r.hints);
        assert_eq!(hints[0].task, "digest");
        assert!(
            hints[0]
                .advice
                .contains("`post`, downstream of `digest`, is an egress task"),
            "{}",
            hints[0].advice
        );
    }

    /// Rule 1: the same guard with no private read declared is two legs of
    /// three. Declaring `fs.read` alone arms the third.
    #[test]
    fn a_missing_trifecta_leg_means_no_hint() {
        let two_legs = r#"nika: steer-probe
permits:
  tools: ["nika:fetch"]
  net: { http: ["hnrss.org", "hooks.slack.com"] }
tasks:
  news:
    invoke: { tool: "nika:fetch", args: { url: "https://hnrss.org/frontpage" } }
  post:
    with:
      news: ${{ tasks.news.output }}
    when: ${{ with.news.contains('urgent') }}
    invoke:
      tool: "nika:fetch"
      args: { url: "https://hooks.slack.com/services/T000/B000/XXXX", method: POST, body: "weekly ping" }
"#;
        let r = report(two_legs);
        assert!(r.is_clean(), "{:?}", r.findings);
        assert!(steered(&r).is_empty(), "{:?}", steered(&r));

        let armed = swap(
            two_legs,
            "  net: { http: [\"hnrss.org\", \"hooks.slack.com\"] }\n",
            "  net: { http: [\"hnrss.org\", \"hooks.slack.com\"] }\n  fs: { read: [\"notes/week.md\"] }\n",
        );
        let r = report(&armed);
        assert_eq!(steered(&r).len(), 1, "{:?}", r.hints);
    }

    /// Rule 4: once the page reaches the POST body, `NIKA-SEC-009` owns the
    /// sink and its repair. No advisory twin.
    #[test]
    fn a_sink_the_trifecta_refuses_keeps_no_twin_hint() {
        let y = swap(
            WITNESS,
            "body: \"${{ with.notes }}\"",
            "body: \"${{ with.news }}\"",
        );
        let r = report(&y);
        assert_eq!(r.trifecta_findings.len(), 1, "{:?}", r.trifecta_findings);
        assert_eq!(r.trifecta_findings[0].task, "post");
        assert_eq!(r.trifecta_findings[0].source.as_deref(), Some("news"));
        assert!(steered(&r).is_empty(), "{:?}", steered(&r));
    }

    /// The untrusted value reaches the guard through an `infer:` in between:
    /// the model's answer carries what its prompt saw. The hint names the
    /// task the guard reads and the ingress the content was born at.
    #[test]
    fn a_guard_on_an_infer_output_tainted_by_propagation_is_steered() {
        let triage = "  triage:\n    with: { news: \"${{ tasks.news.output }}\" }\n    infer: { prompt: \"Anything urgent? ${{ with.news }}\", max_tokens: 16 }\n  post:\n";
        let y = swap(WITNESS, POST, triage);
        let y = swap(
            &y,
            NEWS_BINDING,
            "      label: ${{ tasks.triage.output }}\n",
        );
        let y = swap(&y, GUARD, "when: ${{ with.label == 'urgent' }}");
        let r = report(&y);
        assert!(r.trifecta_findings.is_empty(), "{:?}", r.trifecta_findings);
        let hints = steered(&r);
        assert_eq!(hints.len(), 1, "{:?}", r.hints);
        assert_eq!(hints[0].task, "post");
        assert!(
            hints[0].advice.contains(
                "its `when:` reads `with.label`, the output of `triage`, which carries untrusted content from `news`"
            ),
            "{}",
            hints[0].advice
        );
    }

    /// What version 1 leaves alone: the engine's own record of an untrusted
    /// task (its status), and a caller's input.
    #[test]
    fn a_status_read_and_an_input_read_are_not_steered() {
        let status = swap(
            WITNESS,
            NEWS_BINDING,
            "      state: ${{ tasks.news.status }}\n",
        );
        let status = swap(&status, GUARD, "when: ${{ with.state == 'success' }}");
        let r = report(&status);
        assert!(r.conformance.is_empty(), "{:?}", r.conformance);
        assert!(steered(&r).is_empty(), "{:?}", steered(&r));

        let input = swap(
            WITNESS,
            "permits:\n",
            "inputs:\n  urgent: { type: bool, default: false }\npermits:\n",
        );
        let input = swap(&input, GUARD, "when: ${{ inputs.urgent == true }}");
        let r = report(&input);
        assert!(r.conformance.is_empty(), "{:?}", r.conformance);
        assert!(steered(&r).is_empty(), "{:?}", steered(&r));
    }

    /// A child workflow that fetches a page is an untrusted producer once a
    /// composed check resolved it; a plain check reads no child and makes no
    /// claim about the call.
    #[test]
    fn a_guard_on_a_composed_call_output_is_steered_once_the_child_is_resolved() {
        let child = r#"nika: child-fetch
permits:
  tools: ["nika:fetch"]
  net: { http: ["hnrss.org"] }
tasks:
  page:
    invoke: { tool: "nika:fetch", args: { url: "https://hnrss.org/frontpage", mode: text } }
outputs:
  text: { value: "${{ tasks.page.output }}", type: string }
"#;
        let parent = swap(
            WITNESS,
            "    invoke: { tool: \"nika:fetch\", args: { url: \"https://hnrss.org/frontpage\" } }\n",
            "    invoke: { workflow: \"./child.nika\" }\n    returns: { object: { text: string } }\n",
        );
        let parent = swap(
            &parent,
            NEWS_BINDING,
            "      news: ${{ tasks.news.output.text }}\n",
        );
        let wf = parse(&parent, FileId::new(0), ParseMode::Strict).expect("fixture parses");
        let mut read = |path: &str| {
            if path.ends_with("child.nika") {
                Ok(child.to_owned())
            } else {
                Err(format!("no such file: {path}"))
            }
        };
        let composed = check_composed(&wf, "parent.nika", &mut read);
        assert!(
            composed.composition.is_empty(),
            "{:?}",
            composed.composition
        );
        let hints = steered(&composed);
        assert_eq!(hints.len(), 1, "{:?}", composed.hints);
        assert_eq!(hints[0].task, "post");
        assert!(
            hints[0]
                .advice
                .contains("`with.news`, the output of `news`"),
            "{}",
            hints[0].advice
        );
        assert!(
            steered(&check(&wf)).is_empty(),
            "a plain check resolves no child"
        );
    }

    /// Every example and template the engine embeds, as `(file, source)`.
    fn embedded_corpus() -> Vec<(String, &'static str)> {
        let examples = nika_pack::example_slugs().into_iter().map(|slug| {
            let source = nika_pack::example(&slug).expect("a listed example resolves");
            (format!("examples/{slug}.nika"), source)
        });
        let templates = nika_pack::template_names().into_iter().map(|name| {
            let source = nika_pack::template(&name).expect("a listed template resolves");
            (format!("templates/{name}.nika"), source)
        });
        examples.chain(templates).collect()
    }

    /// The noise census: the hint over every embedded example and template,
    /// each judged by the composed check with its sibling files readable
    /// (a child the pack does not hold stays unresolved, which is the
    /// fail-closed reading: it can only ADD hints). Each row is `file ·
    /// task · guard`.
    fn census() -> (usize, Vec<String>) {
        let corpus = embedded_corpus();
        let mut rows = Vec::new();
        for (file, source) in &corpus {
            let wf = parse(source, FileId::new(0), ParseMode::Strict)
                .unwrap_or_else(|e| panic!("{file} parses: {e}"));
            let mut read = |path: &str| {
                corpus
                    .iter()
                    .find(|(name, _)| name == path)
                    .map(|(_, source)| (*source).to_owned())
                    .ok_or_else(|| format!("not in the embedded corpus: {path}"))
            };
            let report = check_composed(&wf, file, &mut read);
            for hint in steered(&report) {
                let guard = wf
                    .tasks
                    .iter()
                    .find(|t| t.value.id.value == hint.task)
                    .and_then(|t| t.value.when.as_ref())
                    .and_then(|w| w.value.as_expr().map(str::to_owned))
                    .unwrap_or_default();
                rows.push(format!("{file} · {} · {guard}", hint.task));
            }
        }
        (corpus.len(), rows)
    }

    /// The census, pinned: which embedded examples and templates carry the
    /// hint (the walked count is a floor, so a new example breaks nothing).
    /// A file that gains the hint shows up here by name, with its guard.
    /// Read today: one monitor whose fetched config decides whether the
    /// page goes out, and four guards under a confirm gate that a refusal
    /// closes only through a skipped producer (the module doc's stated
    /// over-approximation).
    #[test]
    fn the_embedded_corpus_census_is_pinned() {
        let (files, rows) = census();
        assert!(
            files >= 80,
            "only {files} embedded files walked: layout drift?"
        );
        assert_eq!(
            rows,
            [
                "examples/config-drift-sentinel.nika · alert · ${{ size(with.paging) > 0 }}",
                "examples/pr-review-fanout.nika · files · ${{ with.changed != null && size(with.changed) > 0 }}",
                "examples/pr-review-fanout.nika · reviews · ${{ with.files != null }}",
                "examples/pr-review-fanout.nika · save · ${{ with.summary != null && size(with.summary) > 0 }}",
                "examples/release-notes.nika · notes · ${{ with.log != null && size(with.log) > 0 && !with.log.startsWith('git history unavailable') }}",
            ],
            "the embedded files carrying the hint, of {files} walked"
        );
    }
}
