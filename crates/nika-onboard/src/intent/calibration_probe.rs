//! The one-shot calibration probe of the catalog router: for each phrase a
//! routing law names, the scores after the contract gate and the verdicts.
//! Run it after any corpus change:
//! `cargo test -p nika-onboard --lib print_catalog_scores -- --ignored --nocapture`.

use super::*;

#[test]
#[ignore = "one-shot calibration probe · run with --ignored --nocapture"]
fn print_catalog_scores() {
    let names = catalog_names();
    let index = build_template_index(&names);
    let skeleton_names = nika_pack::template_names();
    let skeleton_index = build_template_index(&skeleton_names);
    // Every utterance a catalog routing law of this crate or of the
    // MCP oracle names, plus the vague probes the floor must refuse.
    let cases = [
        "extract action items from meeting notes with owners and deadlines",
        "translate my docs folder to spanish",
        "chase unpaid invoices every friday",
        "chase unpaid invoices",
        "write release notes from git commits",
        "Chaque lundi compare three competitors and write a French brief",
        "Relance les factures impayées par email après validation humaine",
        "please answer my emails",
        "summarize every item in parallel",
        "an agentic loop that researches until the budget runs out",
        "scrape a website and summarize it",
        "scrape a website and summarize",
        "a weekend summary of three URLs",
        "weekend summaries of three URLs",
        // The Session gallery's routing phrases (nika-session facts.rs).
        "show me a template for fetching a URL",
        "which example fetches a url and summarizes it?",
        "is there a template for invoices?",
        "what's a good template for a digest?",
        "do stuff with things",
        "fill the lines",
        "the workflow template",
        "make it good",
        "do the thing properly",
    ];
    for intent in cases {
        // The router's own path: its query expansion and its contract
        // gate, so a score printed here is a score `route` compares.
        let Some(query) = expand_query(intent) else {
            println!("CAL · {intent} → (no signal-bearing token)");
            continue;
        };
        let ranked = index.top_k(&query, names.len());
        let qualified = contract_qualified(&ranked, &names, &extract(intent));
        let row: Vec<String> = qualified
            .iter()
            .take(4)
            .map(|(n, s)| format!("{n}={s:.3}"))
            .collect();
        let said = |outcome: RoutingOutcome| match outcome {
            RoutingOutcome::Routed { template, .. } => format!("routes {template}"),
            RoutingOutcome::NeedsClarification { candidates, .. } => {
                format!("clarifies {candidates:?}")
            }
        };
        let verdict = said(route(intent));
        // The same corpus under the floor before 2026-11-25: a flip
        // between the two columns is a phrase the new floor moved.
        let before = said(route_impl(intent, &names, 5.5));
        // The skeleton door shares the query expansion: its verdict moves
        // with every alias and singular rule.
        let skeleton_ranked = skeleton_index.top_k(&query, skeleton_names.len());
        let skeleton_top: Vec<String> =
            contract_qualified(&skeleton_ranked, &skeleton_names, &extract(intent))
                .iter()
                .take(2)
                .map(|(n, s)| format!("{n}={s:.3}"))
                .collect();
        let skeleton = format!(
            "{} {}",
            said(route_skeletons(intent)),
            skeleton_top.join(" · ")
        );
        println!(
            "CAL · {intent} → {} · {verdict} · at 5.5: {before} · skeletons: {skeleton}",
            row.join(" · ")
        );
    }
}
