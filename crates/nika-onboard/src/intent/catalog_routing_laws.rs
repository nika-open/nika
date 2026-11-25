//! The routing laws of the whole-catalog corpus (jobs, lessons and
//! skeletons): the phrases each door must route or ask, and the rules
//! under them.

use super::*;

/// The singular rule drops `-es` only where English adds it, so a verb
/// such as « summarizes » keeps the `e` of its stem (2026-11-25: it
/// read `summariz`, and the gallery question lost 05-fetch-chain).
#[test]
fn the_singular_drops_es_only_where_english_adds_it() {
    for (plural, singular) in [
        ("processes", "process"),
        ("boxes", "box"),
        ("buzzes", "buzz"),
        ("fetches", "fetch"),
        ("wishes", "wish"),
        ("summarizes", "summarize"),
        ("releases", "release"),
        ("cases", "case"),
        ("invoices", "invoice"),
        ("urls", "url"),
    ] {
        assert_eq!(singular_of(plural), Some(singular), "{plural}");
    }
    for word in ["class", "its", "bus"] {
        assert_eq!(singular_of(word), None, "{word}");
    }
}

/// The margin clause on synthetic scores: a rival of the SAME facet
/// inside 1.3× makes a coin-flip, one of another facet never does,
/// and a same-facet rival hiding behind a cross-facet runner-up still
/// counts. Without the clause the first and fourth cases would route.
#[test]
fn the_margin_judges_only_a_same_facet_rival() {
    let q = |rows: &[(&str, f64)]| -> Vec<(String, f64)> {
        rows.iter().map(|(n, s)| ((*n).to_owned(), *s)).collect()
    };
    assert_eq!(facet_of("invoice-chaser"), facet_of("refund-chaser"));
    assert_ne!(facet_of("05-fetch-chain"), facet_of("website-brief"));
    assert!(!clears(
        &q(&[("invoice-chaser", 20.0), ("refund-chaser", 16.0)]),
        12.5
    ));
    assert!(clears(
        &q(&[("invoice-chaser", 20.0), ("refund-chaser", 15.0)]),
        12.5
    ));
    assert!(clears(
        &q(&[("05-fetch-chain", 14.0), ("website-brief", 13.0)]),
        12.5
    ));
    assert!(!clears(
        &q(&[
            ("invoice-chaser", 20.0),
            ("website-brief", 19.0),
            ("refund-chaser", 16.0),
        ]),
        12.5
    ));
    assert!(!clears(&q(&[("invoice-chaser", 12.0)]), 12.5));
    assert!(!clears(&[], 12.5));
}

#[test]
fn release_notes_intent_leads_the_named_job() {
    match route("write release notes from git commits") {
        RoutingOutcome::Routed { template, .. } => assert_eq!(template, "release-notes"),
        RoutingOutcome::NeedsClarification { candidates, .. } => assert_eq!(
            candidates.first().map(String::as_str),
            Some("release-notes"),
            "the named job must lead the clarify list: {candidates:?}"
        ),
    }
}

/// THE LAW (RAMS-1 · G-16 · entry-doors probes 2026-07-31): the
/// three probes that each failed on the skeleton-only corpus (« 3
/// sondes, 3 échecs ») now find the job-shaped matter. The first
/// two route outright; the invoice probe may honestly clarify (its
/// runner-up is a same-facet coin-flip) but the job MUST lead the
/// candidates — visible either way, never the 10-skeleton dump.
#[test]
fn the_three_evidence_probes_see_the_jobs() {
    let p1 = route("extract action items from meeting notes with owners and deadlines");
    assert!(
        matches!(p1, RoutingOutcome::Routed { ref template, .. } if template == "meeting-actions"),
        "probe 1 must route to the job: {p1:?}"
    );
    // Probe 2 routes outright: its runner-up `07-for-each-locales` is
    // a LESSON, another facet, so the margin does not judge the pair,
    // and the `translate` alias carries the job's own word
    // « localization » (14.950 on 2026-11-25).
    let p2 = route("translate my docs folder to spanish");
    assert!(
        matches!(p2, RoutingOutcome::Routed { ref template, .. } if template == "localization-factory"),
        "probe 2 must route to the job: {p2:?}"
    );
    match route("chase unpaid invoices every friday") {
        RoutingOutcome::Routed { template, .. } => assert_eq!(template, "invoice-chaser"),
        RoutingOutcome::NeedsClarification { candidates, .. } => assert_eq!(
            candidates.first().map(String::as_str),
            Some("invoice-chaser"),
            "the job leads the clarify list: {candidates:?}"
        ),
    }
}

/// The one flat namespace the catalog rests on: no example slug may
/// collide with a template name (facet derivation + body lookup
/// both assume the sets are disjoint — a collision would shadow).
#[test]
fn catalog_names_never_collide() {
    let templates = nika_pack::template_names();
    for slug in nika_pack::example_slugs() {
        assert!(
            !templates.contains(&slug),
            "example `{slug}` shadows a template name"
        );
    }
}

/// The facet derivation reads the catalog itself: every template is
/// a Skeleton, every numbered example a Lesson, everything else a
/// Job — and the counts match the corpus (a moved file shows up
/// here before it surprises the router).
#[test]
fn facets_partition_the_whole_catalog() {
    let names = catalog_names();
    let (mut jobs, mut lessons, mut skeletons) = (0, 0, 0);
    for n in &names {
        match facet_of(n) {
            Facet::Job => jobs += 1,
            Facet::Lesson => lessons += 1,
            Facet::Skeleton => skeletons += 1,
        }
    }
    assert_eq!(skeletons, nika_pack::template_names().len());
    assert!(lessons >= 12, "the numbered curriculum: {lessons}");
    assert!(jobs >= 20, "the job corpus: {jobs}");
    assert_eq!(jobs + lessons + skeletons, names.len());
}

/// The wizard's world is UNTOUCHED: its route sees the skeleton set
/// only, so a job-shaped utterance still lands the generic skeleton
/// there (the SLOT gesture) while read-only discovery sees the catalog.
#[test]
fn the_wizard_route_stays_skeleton_only() {
    match route_skeletons("translate my docs folder to spanish") {
        RoutingOutcome::Routed { template, .. } => {
            assert!(
                nika_pack::template(&template).is_some(),
                "the wizard corpus is the template set: {template}"
            );
        }
        RoutingOutcome::NeedsClarification { candidates, .. } => {
            for c in &candidates {
                assert!(
                    nika_pack::template(c).is_some(),
                    "a wizard candidate is always a skeleton: {c}"
                );
            }
        }
    }
}
