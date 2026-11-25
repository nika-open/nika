// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! The JOURNEY rung of `nika check` — the data voyage made readable
//! (P0-18 · CORE-18): class + counts, the named secret flows, and one
//! dim disclosure row per external endpoint (locus · retention ·
//! training in the provider's own sourced words). Split from
//! `check_render.rs` at the 1500-line file wall — same seams, no
//! behavior change.

use std::fmt::Write as _;

use nika_check::CheckReport;

use crate::check_render::mark;
use crate::theme::{Role, Theme};

/// The SECRETS headline names observed flows without re-judging consent.
/// Findings own that verdict; JOURNEY owns the complete visible projection.
pub(crate) fn declared_egress_summary(report: &CheckReport) -> String {
    let declared_flows = report
        .data_journey
        .secrets_used
        .iter()
        .map(|secret| secret.flows_to.len())
        .sum::<usize>();
    if declared_flows == 0 {
        if report.data_journey.secrets_used.is_empty() {
            "no declared secret reaches an effect · model echo untracked".to_owned()
        } else {
            // flows_to names external destinations, not every local effect.
            // An exec sanction must never turn into a claim of no effect.
            format!(
                "declared secrets used · {} declared · model echo untracked",
                crate::vocab::count(report.data_journey.consents.len(), "egress rule")
            )
        }
    } else {
        format!(
            "{} shown in JOURNEY · consent findings above · model echo untracked",
            crate::vocab::count(declared_flows, "declared-secret flow")
        )
    }
}

/// One sanctioned flow, named: `(secret, destination, pinned)`. A
/// secret with a leak finding is not sanctioned — its rows are the
/// findings'. Destinations are the journey's (a host · a provider id ·
/// an MCP tool id); `pinned` is whether an `egress:` rule of that secret
/// carries this destination as its `host:` clause.
fn sanctioned_flows(report: &CheckReport) -> Vec<(String, String, bool)> {
    let leaking: std::collections::BTreeSet<&str> = report
        .secret_leaks
        .iter()
        .map(|l| l.secret.as_str())
        .collect();
    let j = &report.data_journey;
    let mut out = Vec::new();
    for s in &j.secrets_used {
        if leaking.contains(s.name.as_str()) {
            continue;
        }
        for dest in &s.flows_to {
            let pinned = j
                .consents
                .iter()
                .any(|c| c.secret == s.name && c.host.as_deref() == Some(dest.as_str()));
            out.push((s.name.clone(), dest.clone(), pinned));
        }
    }
    out
}

/// The count the audited line carries (#1393 — the one line an operator
/// reads before running must say a secret leaves).
pub(crate) fn sanctioned_flow_count(report: &CheckReport) -> usize {
    sanctioned_flows(report).len()
}

/// The SECRETS rung: the leak findings' rows when there are any; else,
/// when a declared secret reaches an external destination by SANCTION,
/// the warn posture with one named row per flow (#1393 — the sanction is
/// stated, never erased: the repair `check` prints for the unsanctioned
/// form used to turn an exfiltration into « no declared secret reaches an
/// effect »); else the narrowed green sentence.
pub(crate) fn secrets_rung(out: &mut String, report: &CheckReport, t: Theme) {
    let leak_rows: Vec<String> = report
        .findings
        .iter()
        .filter(|f| {
            matches!(
                f.kind,
                "secret_leak" | "secret_egress" | "literal_credential"
            )
        })
        // The wire code rides the row like every other lane's: without it
        // `nika explain` has nothing to be pointed at (wave 3 · personas 02
        // and 07 both hit a SECRETS refusal with no code on the screen).
        .map(|f| match f.code.as_deref() {
            Some(code) => format!("[{code}] {}", f.message),
            None => f.message.clone(),
        })
        .collect();
    let flows = sanctioned_flows(report);
    if !leak_rows.is_empty() || flows.is_empty() || !report.conformance.is_empty() {
        crate::check_render::section_or_skip(
            out,
            report,
            t,
            "SECRETS",
            &declared_egress_summary(report),
            leak_rows,
        );
        return;
    }
    let warn = t.paint(Role::Warn, if t.ascii { "!" } else { "⚠" });
    let label = t.paint(Role::Strong, "SECRETS ");
    let _ = writeln!(
        out,
        " {warn} {label} {}",
        t.paint(
            Role::Dim,
            &format!(
                "{} by declaration · review consent before the run · model echo untracked",
                crate::vocab::count(flows.len(), "sanctioned secret flow")
            )
        )
    );
    let hosts: std::collections::BTreeSet<&str> = report
        .data_journey
        .destinations
        .iter()
        .filter(|d| d.kind == "net.http")
        .map(|d| d.target.as_str())
        .collect();
    for (name, dest, pinned) in &flows {
        // Each sink once, in the file's order: a sink pinned to one host
        // and left open beside it is one sink.
        let mut clauses: Vec<&str> = Vec::new();
        for c in report
            .data_journey
            .consents
            .iter()
            .filter(|c| &c.secret == name)
        {
            if !clauses.contains(&c.to.as_str()) {
                clauses.push(&c.to);
            }
        }
        let via = clauses.join(" · ");
        let tail = if hosts.contains(dest.as_str()) {
            host_clause(&report.data_journey.consents, name, dest, &via, *pinned)
        } else {
            format!("egress.to {via}")
        };
        let _ = writeln!(
            out,
            " {warn} {label} sanctioned · secret `{name}` → {dest} · {tail}"
        );
    }
}

/// What the `egress:` rules of `name` say about `dest`, a host of the net
/// grant. They pin it; or they pin no host, which clears any host the tool
/// reaches, and the row prints the pinned form; or they pin other hosts,
/// which the row names (« pins no host » would contradict the file), and
/// says when a rule beside them pins none.
fn host_clause(
    consents: &[nika_check::JourneyConsent],
    name: &str,
    dest: &str,
    via: &str,
    pinned: bool,
) -> String {
    if pinned {
        return format!("egress.to {via} · host pinned");
    }
    let exact = format!("exact form: egress: [{{ to: \"{via}\", host: \"{dest}\" }}]");
    let rules = || consents.iter().filter(|c| c.secret == name);
    let pins: std::collections::BTreeSet<&str> =
        rules().filter_map(|c| c.host.as_deref()).collect();
    if pins.is_empty() {
        return format!("egress.to {via} pins no host (any host the tool reaches) · {exact}");
    }
    let pins = pins.into_iter().collect::<Vec<_>>().join(", ");
    if rules().any(|c| c.host.is_none() && reaches_hosts(&c.to)) {
        format!(
            "egress.to {via} pins {pins}, not {dest}, and a rule with no host clears any host the tool reaches · {exact}"
        )
    } else {
        format!("egress.to {via} pins {pins}, not {dest}")
    }
}

/// Whether a rule's sink can address a host: a network builtin, or an
/// agent (its tools reach the net grant). `exec`, `outputs` and the other
/// sinks carry no host, so a rule naming one pins nothing about a host.
fn reaches_hosts(to: &str) -> bool {
    to == "agent"
        || matches!(
            nika_cap::builtin_effect(to, None),
            Some(nika_cap::BuiltinEffect::Net { .. })
        )
}

/// JOURNEY rung (P0-18 · audit UX 2026-07-30) — the data voyage made
/// visible BEFORE the run: the derived class and the counts, then one ⚠
/// row per secret reaching an external destination, NAMED. Advisory by
/// design — the blocking refusal of an UNSANCTIONED flow lives in the
/// SECRETS lane (the IFC leak finding); a sanctioned flow still has to
/// be SEEN, so the row asks the operator to review consent before the
/// run. Names and classes only — never a value (law 13).
pub(crate) fn journey_rung(out: &mut String, report: &CheckReport, t: Theme) {
    let j = &report.data_journey;
    let flows: Vec<(&str, &str)> = j
        .secrets_used
        .iter()
        .flat_map(|s| {
            s.flows_to
                .iter()
                .map(move |d| (s.name.as_str(), d.as_str()))
        })
        .collect();
    let summary = format!(
        "{} · {} · {} · {}",
        j.classification.as_str(),
        crate::vocab::count(j.sources.len(), "source"),
        crate::vocab::count(j.destinations.len(), "destination"),
        crate::vocab::count(j.model_endpoints.len(), "model endpoint"),
    );
    // The local→cloud flip must be READABLE, not a JSON-only fact
    // (gauntlet 08-01, Aïcha: the --plain line was byte-identical for
    // mock and mistral while the machine lane knew locus, retention
    // and training). One dim row per distinct CLOUD provider, its
    // sourced facts beside it — an unknown stays the word `unknown`.
    let mut cloud_rows: Vec<String> = Vec::new();
    for e in &j.model_endpoints {
        if e.locus != nika_check::EndpointLocus::Cloud {
            continue;
        }
        let retention = e.retention.as_deref().unwrap_or("unknown");
        let trains = e.trains.as_deref().unwrap_or("unknown");
        let row = format!(
            "cloud endpoint {} · task data leaves this machine · retention {retention} · training {trains}",
            e.provider
        );
        if !cloud_rows.contains(&row) {
            cloud_rows.push(row);
        }
    }
    if flows.is_empty() {
        let _ = writeln!(
            out,
            " {} {} {}",
            mark(t, true),
            t.paint(Role::Strong, "JOURNEY"),
            t.paint(
                Role::Dim,
                &format!("{summary} · no secret reaches an external destination")
            )
        );
        for row in &cloud_rows {
            let _ = writeln!(
                out,
                " {} {} {}",
                mark(t, true),
                t.paint(Role::Strong, "JOURNEY"),
                t.paint(Role::Dim, row)
            );
        }
        return;
    }
    // A flow exists: the headline takes the warn posture (the audit
    // completed, the voyage carries a receipt obligation), and every
    // (secret, destination) pair gets its own named row.
    let _ = writeln!(
        out,
        " {} {} {}",
        t.paint(Role::Warn, if t.ascii { "!" } else { "⚠" }),
        t.paint(Role::Strong, "JOURNEY"),
        t.paint(Role::Dim, &summary)
    );
    for (name, dest) in flows {
        let _ = writeln!(
            out,
            " {} {} secret `{name}` flows to {dest} · review consent before the run",
            t.paint(Role::Warn, if t.ascii { "!" } else { "⚠" }),
            t.paint(Role::Strong, "JOURNEY"),
        );
    }
    for row in &cloud_rows {
        let _ = writeln!(
            out,
            " {} {} {}",
            mark(t, true),
            t.paint(Role::Strong, "JOURNEY"),
            t.paint(Role::Dim, row)
        );
    }
}

#[cfg(test)]
mod tests;
