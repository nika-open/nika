// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! The received text a failure keeps in the trace (NIK-402): a final message the task `schema:`
//! rejected, or the last text of an exhausted budget, rides a terminal frame's `outcome` at
//! `payload.error.details.partial_output` (`payload.recovered_from.details.partial_output` once
//! `on_error:` recovered the task). The task record keeps it whole with the run's secrets
//! masked (the runtime masks every failed dispatch's record), for `on_error:` and
//! `${{ tasks.X.error }}`; the frame keeps its first [`RECEIVED_KEPT`] bytes. A seat turn's
//! whole text, which a success frame carries at `seat_transcript`, is cut the same way.
//!
//! The cut runs in [`RedactingSink`](super::RedactingSink) after the scrub, never before it: the
//! scrub replaces whole secret values, and a cut that split one first would leave a prefix the
//! scrub no longer matches (a password that holds a space or a comma included). A cut lands on
//! the last delimiter (a whitespace or a JSON punctuation mark) before the cap, never inside a
//! run of token characters, and is followed by `…[truncated · kept <kept> of <total> bytes]`.

use nika_event::settlement::OUTPUTS_KEPT;
use serde_json::Value;

/// The most of a received text a terminal frame keeps, in bytes: the size of the largest
/// outputs map a settlement keeps inline ([`OUTPUTS_KEPT`], 64 KiB), so a failure keeps as
/// much of the work as a success would.
pub(crate) const RECEIVED_KEPT: usize = OUTPUTS_KEPT;

/// Where an outcome holds a received text: the failure's error, or the error a recovery
/// repaired.
const RECEIVED_AT: [&str; 2] = [
    "/payload/error/details/partial_output",
    "/payload/recovered_from/details/partial_output",
];

/// `outcome` with each received text cut to [`RECEIVED_KEPT`] bytes, or `None` when there is
/// nothing to cut. The outcome is rewritten only when it reads back to its exact bytes (the
/// engine writes it compact, keys sorted), so every other value in it keeps its bytes; one that
/// does not is left whole, never reworded.
pub(crate) fn bounded_outcome(outcome: &str) -> Option<String> {
    // A received text is shorter than the outcome that quotes it.
    if outcome.len() <= RECEIVED_KEPT || !outcome.contains("\"partial_output\"") {
        return None;
    }
    let mut value: Value = serde_json::from_str(outcome).ok()?;
    if serde_json::to_string(&value).ok()? != outcome {
        return None;
    }
    let mut cut = false;
    for pointer in RECEIVED_AT {
        if let Some(Value::String(text)) = value.pointer_mut(pointer)
            && text.len() > RECEIVED_KEPT
        {
            *text = head(text, RECEIVED_KEPT);
            cut = true;
        }
    }
    if cut {
        serde_json::to_string(&value).ok()
    } else {
        None
    }
}

/// A received text a frame carries in a field of its own (a seat turn's whole text,
/// `seat_transcript`) cut to [`RECEIVED_KEPT`] bytes the same way, or `None` when it fits.
pub(crate) fn bounded_text(text: &str) -> Option<String> {
    (text.len() > RECEIVED_KEPT).then(|| head(text, RECEIVED_KEPT))
}

/// A character a cut may land on without splitting a token.
fn delimits(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '"' | '\'' | '`' | ',' | ':' | ';' | '{' | '}' | '[' | ']' | '(' | ')' | '<' | '>'
        )
}

/// `text` whole when it fits `cap` bytes; otherwise its head, cut back to the last delimiter
/// before `cap`, followed by the marker `…[truncated · kept <kept> of <total> bytes]`. A text
/// with no delimiter before the cap keeps only the marker.
fn head(text: &str, cap: usize) -> String {
    if text.len() <= cap {
        return text.to_owned();
    }
    let mut end = cap;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let end = text[..end].rfind(delimits).unwrap_or(0);
    format!(
        "{}…[truncated · kept {end} of {} bytes]",
        &text[..end],
        text.len()
    )
}

#[cfg(test)]
#[allow(clippy::expect_used)] // REASON: test fixtures
mod tests {
    use super::*;

    #[test]
    fn a_text_that_fits_is_kept_whole() {
        assert_eq!(head("", 4), "");
        assert_eq!(head("abcd", 4), "abcd");
    }

    /// The cut never splits a token: `sk-12345678` is dropped whole, not kept as `sk-123`.
    #[test]
    fn a_cut_drops_the_token_it_would_split() {
        let text = r#"{"note": "done", "key": "sk-12345678"}"#;
        // Kept: `{"note": "done", "key": `, the 24 bytes before the token.
        assert_eq!(
            head(text, 31),
            format!("{}…[truncated · kept 24 of 38 bytes]", &text[..24])
        );
        assert_eq!(
            head("word another-long-token", 12),
            "word…[truncated · kept 4 of 23 bytes]"
        );
    }

    /// A seat turn's text in its own field is cut like a failure's received text, and kept
    /// whole when it fits.
    #[test]
    fn a_long_seat_transcript_is_cut_on_a_delimiter() {
        let fits = "word ".repeat(10);
        assert_eq!(bounded_text(&fits), None);
        let long = "word ".repeat(RECEIVED_KEPT / 5 + 10);
        let cut = bounded_text(&long).expect("past the cap");
        // 65 536 ≡ 1 (mod 5): the last space before the cap is two bytes before it.
        let kept = RECEIVED_KEPT - 2;
        assert_eq!(
            cut,
            format!(
                "{}…[truncated · kept {kept} of {} bytes]",
                &long[..kept],
                long.len()
            )
        );
    }

    /// A run with no delimiter keeps only the marker, and a cut inside a multi-byte character
    /// backs off to its boundary first.
    #[test]
    fn a_text_without_a_delimiter_keeps_only_the_marker() {
        assert_eq!(head("abcdefghij", 4), "…[truncated · kept 0 of 10 bytes]");
        assert_eq!(head("é é é", 4), "é…[truncated · kept 2 of 8 bytes]");
    }

    fn failure(received: &str) -> String {
        serde_json::json!({
            "class": "failure",
            "cause": "verb_error",
            "payload": {
                "attempts": 1,
                "error": {
                    "code": "NIKA-INFER-002",
                    "message": "agent final message failed schema validation",
                    "transient": false,
                    "details": {"partial_output": received, "violations": ["v"]},
                },
            },
        })
        .to_string()
    }

    /// Past the cap the failure's received text is cut on a delimiter, and only it: the
    /// violations, the message and the attempts keep their bytes.
    #[test]
    fn a_long_received_text_is_cut_and_nothing_else_moves() {
        let received = format!("{} tail", "word ".repeat(RECEIVED_KEPT / 5 + 1));
        let bounded = bounded_outcome(&failure(&received)).expect("cut");
        let value: Value = serde_json::from_str(&bounded).expect("JSON");
        // The last space before the 65 536-byte cap: every word is five bytes.
        let kept = 65_534;
        assert_eq!(
            value["payload"]["error"]["details"]["partial_output"],
            format!(
                "{}…[truncated · kept {kept} of {} bytes]",
                &received[..kept],
                received.len()
            )
        );
        let mut whole: Value = serde_json::from_str(&failure(&received)).expect("JSON");
        whole["payload"]["error"]["details"]["partial_output"] =
            value["payload"]["error"]["details"]["partial_output"].clone();
        assert_eq!(value, whole);
    }

    /// The error a recovery repaired is cut the same way; the recovered value is the task's
    /// output and keeps every byte.
    #[test]
    fn a_recovered_error_is_cut_and_the_value_kept() {
        let received = "word ".repeat(RECEIVED_KEPT / 5 + 1);
        let outcome = serde_json::json!({
            "class": "success",
            "cause": "recovered",
            "payload": {
                "attempts": 1,
                "recovered_from": {
                    "code": "NIKA-INFER-002",
                    "message": "m",
                    "transient": false,
                    "details": {"partial_output": received},
                },
                "value": {"partial_output": received},
            },
        })
        .to_string();
        let value: Value =
            serde_json::from_str(&bounded_outcome(&outcome).expect("cut")).expect("JSON");
        assert_eq!(
            value["payload"]["recovered_from"]["details"]["partial_output"],
            format!(
                "{}…[truncated · kept 65534 of {} bytes]",
                &received[..65_534],
                received.len()
            )
        );
        assert_eq!(value["payload"]["value"]["partial_output"], received);
    }

    /// Nothing to cut: a short outcome, a long one without a received text, and one the engine
    /// did not write (not compact) are all left as they are.
    #[test]
    fn an_outcome_with_nothing_to_cut_is_left_whole() {
        assert_eq!(bounded_outcome(&failure("short")), None);
        let value = "word ".repeat(RECEIVED_KEPT / 5 + 1);
        let success = serde_json::json!({"class": "success", "cause": "normal",
            "payload": {"attempts": 1, "value": value}})
        .to_string();
        assert_eq!(bounded_outcome(&success), None);
        let spaced = failure(&value).replacen("{\"cause\":", "{ \"cause\":", 1);
        assert_eq!(bounded_outcome(&spaced), None);
    }

    /// `event` through the run's sink, with `resolved` as the run's secrets: the frame a lane
    /// receives, as its journal line.
    fn through_the_sink(resolved: &[(&str, &str)], outcome: String) -> (String, Value) {
        use crate::stamp::EventSink as _;
        let resolved = (resolved.iter())
            .map(|(name, value)| ((*name).to_owned(), Value::String((*value).to_owned())))
            .collect();
        let event = nika_event::Event::new(
            nika_types::id::EventId::new(uuid::Uuid::nil()),
            nika_types::timestamp::Timestamp::from_unix_ms(0),
            nika_event::EventKind::TaskFailed,
        )
        .with_field(nika_types::resource::KeyValue::new(
            "outcome",
            nika_types::resource::Value::String(outcome),
        ));
        let mut inner = crate::VecSink::new();
        super::super::RedactingSink::new(&mut inner, &resolved).emit(event);
        let event = inner.into_events().pop().expect("the frame");
        let outcome = event.str_field("outcome").expect("outcome").to_owned();
        let line = serde_json::to_string(&event).expect("line");
        (line, serde_json::from_str(&outcome).expect("JSON"))
    }

    /// A password that holds a delimiter, across the cap of a rejected message: masked whole
    /// first, then the cut. A cut first would land on its space or its comma and keep `ab:cd`
    /// in clear, a prefix the scrub no longer matches.
    #[test]
    fn the_sink_masks_a_secret_before_it_cuts() {
        let secret = "ab:cd,ef gh";
        let lead = format!("{{\"note\": \"{}", "word ".repeat(13_104));
        assert!(lead.len() < RECEIVED_KEPT && RECEIVED_KEPT < lead.len() + secret.len());
        let received = format!("{lead}{secret}{}\"}}", " word".repeat(200));
        let (line, outcome) = through_the_sink(&[("pass", secret)], failure(&received));
        assert!(
            !line.contains("ab:cd"),
            "no prefix of the secret rides the line"
        );
        let masked = received.replace(secret, super::super::REDACTED);
        assert_eq!(
            outcome["payload"]["error"]["details"]["partial_output"],
            format!(
                "{lead}{}…[truncated · kept {} of {} bytes]",
                super::super::REDACTED,
                lead.len() + 3,
                masked.len()
            )
        );
    }

    /// A run without secrets cuts a received text all the same.
    #[test]
    fn the_sink_cuts_on_a_run_without_secrets() {
        let received = "word ".repeat(RECEIVED_KEPT / 5 + 1);
        let (_, outcome) = through_the_sink(&[], failure(&received));
        assert_eq!(
            outcome["payload"]["error"]["details"]["partial_output"],
            format!(
                "{}…[truncated · kept 65534 of {} bytes]",
                &received[..65_534],
                received.len()
            )
        );
    }
}
