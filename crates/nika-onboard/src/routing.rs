// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! Read-only gallery discovery shared by MCP consumers. Compile does not use
//! this probabilistic router: authoring resolves exact skeletons only.

/// Closed conversational acts; these never classify an open workflow request.
pub mod conversation;
/// Explicit Run flags, read without choosing an execution path.
pub use nika_compile::run_words as run_options;

use crate::intent::RoutingOutcome;

/// Where a plain-words query landed.
#[derive(Debug, PartialEq, Eq)]
pub enum RoutedEntry {
    /// A complete example available for read-only discovery.
    Example(String),
    /// A skeleton template (SLOTs to fill).
    Skeleton(String),
    /// Below the confidence bar — the closest names, for recognition.
    Clarify(Vec<String>),
}

/// Route plain words through the WHOLE catalog (examples + skeletons) —
/// the read-only gallery (G-16).
#[must_use]
pub fn route_query(query: &str) -> RoutedEntry {
    classify(crate::intent::route(query))
}

/// Route plain words within the read-only skeleton gallery.
#[must_use]
pub fn route_skeleton_query(query: &str) -> RoutedEntry {
    classify(crate::intent::route_skeletons(query))
}

fn classify(outcome: RoutingOutcome) -> RoutedEntry {
    match outcome {
        RoutingOutcome::Routed { template: name, .. } => {
            if nika_pack::example(&name).is_some() {
                RoutedEntry::Example(name)
            } else {
                RoutedEntry::Skeleton(name)
            }
        }
        RoutingOutcome::NeedsClarification { candidates, .. } => RoutedEntry::Clarify(candidates),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two read-only discovery doors agree: a job phrase routes to the
    /// example (whole catalog) · the same phrase in the skeleton door
    /// stays within skeletons · nonsense clarifies, never guesses.
    #[test]
    fn the_gallery_doors_share_one_router() {
        assert_eq!(
            route_query("chase unpaid invoices"),
            RoutedEntry::Example("invoice-chaser".to_owned())
        );
        assert!(
            !matches!(
                route_skeleton_query("chase unpaid invoices"),
                RoutedEntry::Example(_)
            ),
            "the skeleton door must never leak an example"
        );
        assert!(
            matches!(route_query("zzz qqq xxx"), RoutedEntry::Clarify(_)),
            "nonsense must clarify, never guess"
        );
    }

    /// B01 · `hello` is the 01-hello lesson, never a second file.
    #[test]
    fn hello_is_the_numbered_lesson() {
        assert_eq!(
            route_query("hello"),
            RoutedEntry::Example("01-hello".to_owned())
        );
        assert_eq!(
            route_query("01-hello"),
            RoutedEntry::Example("01-hello".to_owned())
        );
    }

    /// A request to read one site and sum it up, and the gallery's
    /// question for the same shape, route to the lesson that does exactly
    /// that: « summarize » carries the « summary » its body says, and
    /// « summarizes » reads as « summarize » and takes its alias
    /// (2026-11-25, 14.415 and 18.650 over the 12.5 floor).
    #[test]
    fn a_website_summary_routes_to_the_fetch_chain() {
        for words in [
            "scrape a website and summarize it",
            "which example fetches a url and summarizes it?",
        ] {
            assert_eq!(
                route_query(words),
                RoutedEntry::Example("05-fetch-chain".to_owned()),
                "{words}"
            );
        }
    }

    /// The Session answers its gallery questions from this door, so each
    /// one gets an exact outcome here, never « route or ask »: the shape
    /// question routes, and the three short ones ask with the job first,
    /// because they name a word two jobs share (invoices, digest) or too
    /// few of the job's own words (fetching a URL). Their Session reply
    /// is then « closest shapes: … », which still answers the question.
    #[test]
    fn the_session_gallery_questions_get_their_exact_outcome() {
        assert_eq!(
            route_query("which example fetches a url and summarizes it?"),
            RoutedEntry::Example("05-fetch-chain".to_owned())
        );
        for (words, first, beside) in [
            (
                "show me a template for fetching a URL",
                "05-fetch-chain",
                None,
            ),
            (
                "is there a template for invoices?",
                "invoice-chaser",
                Some("subscription-renewal-radar"),
            ),
            (
                "what's a good template for a digest?",
                "05-fetch-chain",
                Some("feed-digest"),
            ),
        ] {
            let RoutedEntry::Clarify(names) = route_query(words) else {
                panic!("`{words}` asks which job is meant");
            };
            assert_eq!(
                names.first().map(String::as_str),
                Some(first),
                "{words}: {names:?}"
            );
            if let Some(other) = beside {
                assert!(
                    names.iter().any(|n| n == other),
                    "{words} names {other}: {names:?}"
                );
            }
        }
    }

    /// C09 · a weekend URL summary writes 05-fetch-chain or names it.
    #[test]
    fn a_weekend_summary_of_three_urls_names_fetch_chain() {
        let outcome = route_query("a weekend summary of three URLs");
        assert!(
            !matches!(outcome, RoutedEntry::Skeleton(_)),
            "must not land a skeleton: {outcome:?}"
        );
        match outcome {
            RoutedEntry::Example(name) => assert_eq!(
                name, "05-fetch-chain",
                "the weekend URL summary is the fetch-chain lesson"
            ),
            RoutedEntry::Clarify(candidates) => assert!(
                candidates.iter().any(|c| c == "05-fetch-chain"),
                "must name 05-fetch-chain, got {candidates:?}"
            ),
            RoutedEntry::Skeleton(_) => {}
        }
    }
}
