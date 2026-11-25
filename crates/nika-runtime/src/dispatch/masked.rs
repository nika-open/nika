// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! A failed dispatch's texts with every resolved secret value masked.
//!
//! The run renders a task's fields with secrets resolved: an `exec:`'s
//! `cwd:` and argv, an `infer:`/`agent:`'s `model:`. When the dispatch fails
//! (the route refused before any request by the launch gates' own laws, a lane
//! that cannot serve the rendered model, a spawn that fails) its note and its
//! error record can name what was rendered. The event lane masks them through
//! the run's `RedactingSink`, but the error record is also data the sink never
//! sees: a later task reads it as `tasks.<id>.error`, and the embedder receives
//! it in `RunOutcome.records`. So the record is masked here, at the one seam
//! every verb's failure crosses, with the scrub the run applies to its
//! `outputs:` map (`secret::scrub_outputs`). What the run sends is
//! unchanged: only the texts of a failure are rewritten.

use std::collections::BTreeMap;

use serde_json::Value;

use super::Dispatched;
use crate::secret::{REDACTED, scrub_outputs};

impl Dispatched {
    /// This dispatch with every resolved secret value in its note, error
    /// message and error details replaced by the marker, when it failed. A
    /// success is returned as it is.
    pub(super) fn masked(mut self, secrets: &BTreeMap<String, Value>) -> Self {
        if secrets.is_empty() || self.result.is_ok() {
            return self;
        }
        if let Err(failed) = &mut self.result {
            let record = &mut failed.record;
            record.message = masked_text(std::mem::take(&mut record.message), secrets);
            if let Some(details) = record.details.take() {
                // The scrub rewrites strings only, so the map stays a map.
                record.details = match masked_value(Value::Object(details), secrets) {
                    Value::Object(map) => Some(map),
                    _ => None,
                };
            }
        }
        self.note = masked_text(std::mem::take(&mut self.note), secrets);
        self
    }
}

/// `value` with every resolved secret value inside it masked, at any depth.
fn masked_value(value: Value, secrets: &BTreeMap<String, Value>) -> Value {
    let mut one = BTreeMap::from([(String::new(), value)]);
    scrub_outputs(&mut one, secrets);
    one.into_values().next().unwrap_or(Value::Null)
}

/// `text` with every resolved secret value inside it masked. A value the
/// scrub could not return as text (it always can) is masked whole.
fn masked_text(text: String, secrets: &BTreeMap<String, Value>) -> String {
    match masked_value(Value::String(text), secrets) {
        Value::String(text) => text,
        _ => REDACTED.to_owned(),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::{Value, json};

    use super::super::Dispatched;

    const SECRET: &str = "sk-live-9f2c7e4a1b6d";

    fn secrets() -> BTreeMap<String, Value> {
        BTreeMap::from([("token".to_owned(), Value::from(SECRET))])
    }

    fn failed(note: &str, message: &str, details: Option<Value>) -> Dispatched {
        let mut dispatched = Dispatched::comp_refusal(note, "NIKA-EXEC-001", message.to_owned());
        if let Err(failed) = &mut dispatched.result {
            failed.record.details = details.and_then(|d| d.as_object().cloned());
        }
        dispatched
    }

    #[test]
    fn a_failure_names_no_resolved_secret_value() {
        let dispatched = failed(
            &format!("infer · openai/{SECRET}"),
            &format!(
                "refused before the provider request: cloud model `openai/{SECRET}` is unpriced"
            ),
            Some(json!({ "seat": format!("openai/{SECRET}"), "calls": 1 })),
        )
        .masked(&secrets());
        assert_eq!(dispatched.note, "infer · openai/***");
        let record = dispatched.result.err().expect("still a failure").record;
        assert_eq!(record.code, "NIKA-EXEC-001");
        assert_eq!(
            record.message,
            "refused before the provider request: cloud model `openai/***` is unpriced"
        );
        assert_eq!(
            Value::Object(record.details.expect("details kept")),
            json!({ "seat": "openai/***", "calls": 1 })
        );
    }

    #[test]
    fn no_secret_and_no_failure_leave_the_dispatch_as_it_was() {
        let text = format!("cwd `{SECRET}` was not found");
        let unscrubbed = failed("exec · true", &text, None).masked(&BTreeMap::new());
        assert_eq!(
            unscrubbed.result.err().expect("failure").record.message,
            text
        );
        let other = BTreeMap::from([("token".to_owned(), Value::from("another-value"))]);
        let untouched = failed("exec · true", &text, None).masked(&other);
        assert_eq!(
            untouched.result.err().expect("failure").record.message,
            text
        );
    }
}
