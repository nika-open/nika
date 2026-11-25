// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! A failed dispatch's texts with every resolved secret value masked.
//!
//! The run renders with secrets resolved the fields an `egress:` rule lets a
//! secret reach: an `exec:`'s argv and env, a prompt, a tool's arguments. (A
//! `model:` and a `cwd:` never carry one: the run refuses such a task before it
//! starts, `task::secret_route`.) When the dispatch fails (a spawn that fails,
//! a provider or a tool that echoes what it received) its note and its error
//! record can name what was rendered. The event lane masks them through
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
        let mut dispatched = Dispatched::comp_refusal(note, "NIKA-EXEC-002", message.to_owned());
        if let Err(failed) = &mut dispatched.result {
            failed.record.details = details.and_then(|d| d.as_object().cloned());
        }
        dispatched
    }

    /// The failure of an `exec:` whose argv an `egress:` rule lets carry the
    /// token, when the program refuses it and the shell error echoes it.
    fn refused_spawn(secret: &str) -> Dispatched {
        failed(
            &format!("exec · deploy {secret}"),
            &format!("shell execution failed: shell error: deploy refused the key `{secret}`"),
            Some(json!({ "argv": ["deploy", secret], "exit_code": 1 })),
        )
    }

    #[test]
    fn a_failure_names_no_resolved_secret_value() {
        let dispatched = refused_spawn(SECRET).masked(&secrets());
        assert_eq!(dispatched.note, "exec · deploy ***");
        let record = dispatched.result.err().expect("still a failure").record;
        assert_eq!(record.code, "NIKA-EXEC-002");
        assert_eq!(
            record.message,
            "shell execution failed: shell error: deploy refused the key `***`"
        );
        assert_eq!(
            Value::Object(record.details.expect("details kept")),
            json!({ "argv": ["deploy", "***"], "exit_code": 1 })
        );
    }

    /// One secret inside another (`abc` in `abcdef`): the longer is masked
    /// whole before the shorter can cut it, so no fragment of either remains
    /// (never `***def`), in the note, the message or the details.
    #[test]
    fn a_secret_inside_another_leaves_no_fragment_of_either() {
        let secrets = BTreeMap::from([
            ("short".to_owned(), Value::from("abc")),
            ("long".to_owned(), Value::from("abcdef")),
        ]);
        let dispatched = failed(
            "exec · deploy abcdef abc",
            "deploy refused the keys `abcdef` and `abc`",
            Some(json!({ "argv": ["deploy", "abcdef", "abc", "xabcdefx"] })),
        )
        .masked(&secrets);
        assert_eq!(dispatched.note, "exec · deploy *** ***");
        let record = dispatched.result.err().expect("still a failure").record;
        assert_eq!(record.message, "deploy refused the keys `***` and `***`");
        assert_eq!(
            Value::Object(record.details.expect("details kept")),
            json!({ "argv": ["deploy", "***", "***", "x***x"] })
        );
    }

    #[test]
    fn no_secret_and_no_failure_leave_the_dispatch_as_it_was() {
        let text = format!("deploy refused the key `{SECRET}`");
        let unscrubbed = failed("exec · deploy", &text, None).masked(&BTreeMap::new());
        assert_eq!(
            unscrubbed.result.err().expect("failure").record.message,
            text
        );
        let other = BTreeMap::from([("token".to_owned(), Value::from("another-value"))]);
        let untouched = failed("exec · deploy", &text, None).masked(&other);
        assert_eq!(
            untouched.result.err().expect("failure").record.message,
            text
        );
    }
}
