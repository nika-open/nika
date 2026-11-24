// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! How the trifecta lane reads a human gate whose answer is text rather than a typed yes
//! (`mode: choice` · `mode: input`): the effects whose own `when:` tests that answer
//! ([`answer_tested_by`]), the `nika:jq` stages that carry it on ([`answer_derivations`]), and
//! the effects that send nothing but the typed answer ([`answer_sent_by`], which the caller reads
//! for an input gate only).
//!
//! What is NOT judged: what a jq stage computes from the answer it reads. The stage is read
//! syntactically (its `args.input` carries the answer, its `expression` reads that value), never
//! run, so a program that reads the answer and then ignores it is credited.

use std::collections::BTreeSet;

use nika_schema::expression::{Expr, Literal, NamespaceRef, RelOp, expr_refs};
use nika_schema::raw::{RawAction, RawCommand, RawTask, RawWorkflow};
use nika_schema::types::WhenGate;

use super::{
    Gate, K3, RefusalEnv, eval_consent, exact_carrier, gate_under, parse_gate, reads_answer,
    record_field, refs_of, sole_island,
};

/// The tasks of `wf` whose own `when:` tests the answer of the human gate `prompt`, for a gate
/// whose answer is text rather than a typed yes (`mode: choice` · `mode: input`), as indices
/// into `wf.tasks`. A guard tests the answer in one of two ways:
///
/// - **directly** — it reads an exact `with:` carrier of `tasks.<prompt>.output` (the shape
///   [`super::affirmed_by`] reads), and it is NOT proven true under every answer `choices`
///   lists, each substituted in turn (Kleene over the consent fragment): a guard true whatever
///   the person picks (`with.pick != null` · `with.pick == 'send' || true` · a membership of
///   every choice) lets the task run on any answer. A guard the fragment cannot decide under
///   some choice (`with.pick.startsWith('s')`) counts: the read is there, and the check claims
///   nothing it cannot evaluate. `None` (an input · choices that are not a literal list of
///   strings) cannot enumerate the answers, so the read counts unless the guard is proven true
///   with nothing substituted (`… || true`) or the only read is a `null` test of the binding
///   itself (`with.pick != null`);
/// - **through a derived stage** — it reads a `with:` binding of `${{ tasks.<T>.output }}` or a
///   field of it, where `T` is one of [`answer_derivations`], under the same two exclusions: the
///   guard is not proven true with the derived binding unknown, and a `null` test of the binding
///   itself (`with.plan != null`) is no read (a jq stage always yields a value).
#[must_use]
pub fn answer_tested_by(wf: &RawWorkflow, prompt: &str, choices: Option<&[String]>) -> Vec<usize> {
    let derived = answer_derivations(wf, prompt);
    indices(wf, |task| tests_answer(task, prompt, choices, &derived))
}

/// The `nika:jq` tasks of `wf` whose value derives from the answer of `prompt`, to a fixpoint.
/// Each one is an `invoke:` of `nika:jq` with no `on_error:` (a recovered value is nobody's
/// answer) and no `for_each:` (a list of results is not one answer), and:
///
/// 1. **the answer reaches the program** — a `with:` key binds exactly
///    `${{ tasks.<prompt>.output }}`, or `${{ tasks.<T>.output }}` (or a field of it) for a task
///    `T` already derived, and `args.input` reads that key, directly or inside its object or
///    array literal. A binding `args.input` does not read carries nothing;
/// 2. **the program reads it** — the `expression` is literal and, read syntactically (string
///    literals, `\( … )` interpolations and `#` comments told apart, never parsed by jq), starts
///    a `.` path: any one when `args.input` is the answer itself, `.<field>` (or `."<field>"` ·
///    `.["<field>"]`) for the field that carries it when `args.input` is an object literal. A
///    path read after the program rebinds `.` is not told apart.
///
/// An `infer:`, an `agent:`, an `exec:` or any other tool is never a derived stage.
#[must_use]
pub fn answer_derivations(wf: &RawWorkflow, prompt: &str) -> BTreeSet<String> {
    let mut derived = BTreeSet::new();
    loop {
        let found: Vec<String> = wf
            .tasks
            .iter()
            .map(|t| &t.value)
            .filter(|t| !derived.contains(&t.id.value) && jq_reads_answer(t, prompt, &derived))
            .map(|t| t.id.value.clone())
            .collect();
        if found.is_empty() {
            return derived;
        }
        derived.extend(found);
    }
}

/// The tasks of `wf` that send nothing but the typed answer of the human gate `prompt`, as
/// indices into `wf.tasks`: what such a task sends or writes is what the person typed, so no
/// content reaches it without passing through a human's hands. The caller reads it for a
/// `mode: input` gate only (a choice's answer is one of the author's strings, not content).
///
/// The task binds `${{ tasks.<prompt>.output }}` (or a field of it) in `with:`, every other
/// `with:` value reads nothing but `const.*` and `secrets.*`, and every template its verb and its
/// `when:` hold reads nothing but those keys, `const.*` and `secrets.*`: no other task's output,
/// no `inputs.*`, no loop-local, no `run.*` fact. A `for_each:` task, an `infer:` or an `agent:`
/// (a model writes what they send) and a template that does not scan never qualify.
#[must_use]
pub fn answer_sent_by(wf: &RawWorkflow, prompt: &str) -> Vec<usize> {
    indices(wf, |task| sends_only_answer(task, prompt))
}

fn indices(wf: &RawWorkflow, keep: impl Fn(&RawTask) -> bool) -> Vec<usize> {
    wf.tasks
        .iter()
        .enumerate()
        .filter(|(_, t)| keep(&t.value))
        .map(|(i, _)| i)
        .collect()
}

/// One task's guard tests the answer of `prompt` — see [`answer_tested_by`].
fn tests_answer(
    task: &RawTask,
    prompt: &str,
    choices: Option<&[String]>,
    derived: &BTreeSet<String>,
) -> bool {
    let Some(WhenGate::Expr(src)) = task.when.as_ref().map(|w| &w.value) else {
        return false;
    };
    let Some(expr) = parse_gate(src) else {
        return false;
    };
    let mut read = BTreeSet::new();
    guard_reads(&expr, &mut read);
    let open = proven_open(task, &expr);
    let reads_any = |keys: BTreeSet<String>| keys.iter().any(|k| read.contains(k));
    let direct = reads_answer(task, &expr, prompt)
        && match choices {
            Some(choices) => choices.iter().any(|choice| {
                gate_under(task, prompt, false, Some(Literal::Str(choice.clone()))) != Gate::Open
            }),
            None => {
                !open
                    && reads_any(bound_keys(task, |s| {
                        exact_carrier(s, "output").is_some_and(|id| id == prompt)
                    }))
            }
        };
    direct
        || (!open
            && reads_any(bound_keys(task, |s| {
                output_binding(s).is_some_and(|id| derived.contains(&id))
            })))
}

/// The guard is true whatever any binding holds: Kleene over the consent fragment with nothing
/// substituted (no task id is empty, so no read resolves).
fn proven_open(task: &RawTask, expr: &Expr) -> bool {
    eval_consent(expr, "", &RefusalEnv::of(task, None), false) == K3::True
}

/// The `with:` keys a guard reads, a `null` test of a binding itself (`with.k != null` ·
/// `null == with.k`) aside: a value that is always there passes it.
fn guard_reads(e: &Expr, out: &mut BTreeSet<String>) {
    match e {
        Expr::Relation {
            op: RelOp::Eq | RelOp::Ne,
            lhs,
            rhs,
        } if null_test(lhs, rhs) || null_test(rhs, lhs) => {}
        Expr::Or(x, y) | Expr::And(x, y) => {
            guard_reads(x, out);
            guard_reads(y, out);
        }
        Expr::Relation { lhs, rhs, .. } => {
            guard_reads(lhs, out);
            guard_reads(rhs, out);
        }
        Expr::Not(x) | Expr::SizeCall(x) | Expr::HasCall(x) | Expr::SizeMethod(x) => {
            guard_reads(x, out);
        }
        Expr::Ternary { cond, then, else_ } => {
            for x in [cond, then, else_] {
                guard_reads(x, out);
            }
        }
        Expr::StringMethod { base, arg, .. } => {
            guard_reads(base, out);
            guard_reads(arg, out);
        }
        Expr::List(items) => items.iter().for_each(|x| guard_reads(x, out)),
        Expr::Member { .. } | Expr::Index { .. } | Expr::Ident(_) => {
            out.extend(expr_refs(e).into_iter().filter_map(|r| match r {
                NamespaceRef::With(key) => Some(key),
                _ => None,
            }));
        }
        Expr::Lit(_) => {}
    }
}

/// `binding` is `with.<k>` itself (no field) and `other` the literal `null`.
fn null_test(binding: &Expr, other: &Expr) -> bool {
    let bare = match binding {
        Expr::Member { base, .. } => matches!(base.as_ref(), Expr::Ident(root) if root == "with"),
        Expr::Index { base, index } => matches!(
            (base.as_ref(), index.as_ref()),
            (Expr::Ident(root), Expr::Lit(Literal::Str(_))) if root == "with"
        ),
        _ => false,
    };
    bare && matches!(other, Expr::Lit(Literal::Null))
}

/// The `with:` keys whose value is a string `binds` accepts.
fn bound_keys(task: &RawTask, binds: impl Fn(&str) -> bool) -> BTreeSet<String> {
    task.with
        .iter()
        .filter(|(_, v)| v.value.as_str().is_some_and(&binds))
        .map(|(k, _)| k.value.clone())
        .collect()
}

/// A derived stage — see [`answer_derivations`].
fn jq_reads_answer(task: &RawTask, prompt: &str, derived: &BTreeSet<String>) -> bool {
    let RawAction::Invoke(inv) = &task.action else {
        return false;
    };
    let jq = inv.tool().is_some_and(|tool| tool.value == "nika:jq");
    if !jq || task.on_error.is_some() || task.for_each.is_some() {
        return false;
    }
    let answer_keys = bound_keys(task, |s| {
        exact_carrier(s, "output").is_some_and(|id| id == prompt)
            || output_binding(s).is_some_and(|id| derived.contains(&id))
    });
    let args = inv.args.as_ref().map(|a| &a.value);
    let input = args.and_then(|a| a.get("input"));
    let program = args
        .and_then(|a| a.get("expression"))
        .and_then(serde_json::Value::as_str)
        .filter(|p| !p.contains("${{"));
    let (Some(input), Some(program)) = (input, program) else {
        return false;
    };
    let carries = |v: &serde_json::Value| {
        refs_of(v).is_some_and(|refs| {
            refs.iter()
                .any(|r| matches!(r, NamespaceRef::With(k) if answer_keys.contains(k)))
        })
    };
    let reads = jq_reads(program);
    match input {
        serde_json::Value::Object(fields) => fields
            .iter()
            .any(|(name, v)| carries(v) && reads.fields.contains(name)),
        other => carries(other) && reads.any_path,
    }
}

/// What a jq program reads of its input, syntactically: whether it starts any `.` path, and the
/// fields its paths start with.
#[derive(Debug, Default)]
struct JqReads {
    any_path: bool,
    fields: BTreeSet<String>,
}

/// Scan `program` for path starts outside string literals and `#` comments (inside a `\( … )`
/// interpolation is code again). A `.` right after a name, a variable, a number or a closing
/// bracket continues a path or a number (`$ENV.HOME` · `1.5` · `.a.b`), and starts none.
fn jq_reads(program: &str) -> JqReads {
    let chars: Vec<char> = program.chars().collect();
    let mut reads = JqReads::default();
    // The paren depth to resume when each open interpolation closes.
    let mut resume: Vec<usize> = Vec::new();
    let (mut in_str, mut depth, mut i) = (false, 0usize, 0usize);
    while let Some(&c) = chars.get(i) {
        let prev = i.checked_sub(1).and_then(|p| chars.get(p)).copied();
        i += 1;
        if in_str {
            match c {
                '\\' if chars.get(i) == Some(&'(') => {
                    resume.push(depth);
                    (depth, in_str) = (0, false);
                    i += 1;
                }
                '\\' => i += 1,
                '"' => in_str = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '#' => {
                while chars.get(i).is_some_and(|c| *c != '\n') {
                    i += 1;
                }
            }
            '(' => depth += 1,
            ')' if depth > 0 => depth -= 1,
            ')' => {
                if let Some(d) = resume.pop() {
                    (depth, in_str) = (d, true);
                }
            }
            '.' if prev.is_none_or(|p| {
                !(p.is_ascii_alphanumeric() || matches!(p, '_' | '$' | ')' | ']' | '"' | '?' | '.'))
            }) =>
            {
                reads.any_path = true;
                if let Some(name) = field_after(&chars, i) {
                    reads.fields.insert(name);
                }
            }
            _ => {}
        }
    }
    reads
}

/// The field a path started at `at` names: `.name`, `."name"` or `.["name"]`.
fn field_after(chars: &[char], at: usize) -> Option<String> {
    let rest = chars.get(at..)?;
    match rest.first()? {
        c if c.is_ascii_alphabetic() || *c == '_' => Some(
            rest.iter()
                .take_while(|c| c.is_ascii_alphanumeric() || **c == '_')
                .collect(),
        ),
        '"' => quoted(rest.get(1..)?),
        '[' if rest.get(1) == Some(&'"') => quoted(rest.get(2..)?),
        _ => None,
    }
}

/// The plain text of a string literal whose opening quote precedes `rest` (an escape: `None`).
fn quoted(rest: &[char]) -> Option<String> {
    let end = rest.iter().position(|c| *c == '"' || *c == '\\')?;
    (rest.get(end) == Some(&'"')).then(|| rest.iter().take(end).collect())
}

/// One task sends nothing but the typed answer of `prompt` — see [`answer_sent_by`].
fn sends_only_answer(task: &RawTask, prompt: &str) -> bool {
    if task.for_each.is_some() {
        return false;
    }
    let answer = bound_keys(task, |s| output_binding(s).is_some_and(|id| id == prompt));
    let fixed = |r: &NamespaceRef| matches!(r, NamespaceRef::Const(_) | NamespaceRef::Secrets(_));
    let with_ok = task.with.iter().all(|(k, v)| {
        answer.contains(&k.value) || refs_of(&v.value).is_some_and(|refs| refs.iter().all(fixed))
    });
    let reads_ok = |refs: Option<Vec<NamespaceRef>>| {
        refs.is_some_and(|refs| {
            refs.iter().all(|r| {
                fixed(r)
                    || matches!(r, NamespaceRef::With(key)
                        if task.with.iter().any(|(k, _)| k.value == *key))
            })
        })
    };
    let when_ok = match task.when.as_ref().map(|w| &w.value) {
        Some(WhenGate::Expr(src)) => reads_ok(refs_of(&serde_json::Value::String(src.clone()))),
        _ => true,
    };
    !answer.is_empty() && with_ok && when_ok && reads_ok(action_refs(&task.action))
}

/// Every root the verb's own templates read: `None` for a model verb (what it sends is what a
/// model writes) or a template that does not scan.
fn action_refs(action: &RawAction) -> Option<Vec<NamespaceRef>> {
    let texts = |items: Vec<&str>| {
        items.into_iter().try_fold(Vec::new(), |mut all, s| {
            all.extend(refs_of(&serde_json::Value::String(s.to_owned()))?);
            Some(all)
        })
    };
    match action {
        RawAction::Invoke(inv) => inv
            .args
            .as_ref()
            .map_or_else(|| Some(Vec::new()), |a| refs_of(&a.value)),
        RawAction::Exec(ex) => {
            let mut items: Vec<&str> = match &ex.command {
                RawCommand::Shell(s) => vec![s.value.as_str()],
                RawCommand::Argv(argv) => argv.iter().map(|s| s.value.as_str()).collect(),
                _ => return None,
            };
            items.extend(ex.cwd.iter().map(|s| s.value.as_str()));
            items.extend(ex.env.iter().map(|(_, v)| v.value.as_str()));
            items.extend(ex.stdin.iter().map(|s| s.value.as_str()));
            texts(items)
        }
        _ => None,
    }
}

/// The with-value IS one bare island reading `tasks.<id>.output` or a field of it (member or
/// literal-index steps): the id.
fn output_binding(value: &str) -> Option<String> {
    let mut e = sole_island(value)?;
    loop {
        if let Some(id) = record_field(&e, "output") {
            return Some(id);
        }
        e = match e {
            Expr::Member { base, .. } => *base,
            Expr::Index { base, index } if matches!(index.as_ref(), Expr::Lit(_)) => *base,
            _ => return None,
        };
    }
}

#[cfg(test)]
mod tests;
