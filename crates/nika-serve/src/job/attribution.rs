// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! Who a job is attributed to (ADR-155).
//!
//! An owner and labels are what the caller STATES about a job so its
//! application can find the job again. They are opaque to the engine and
//! never an authorization boundary: one bearer credential is one authority
//! domain (ADR-117), and whoever holds it reads every job.

use std::collections::BTreeMap;

use serde_json::Value;

/// Longest stated owner, in bytes.
pub(crate) const MAX_OWNER_BYTES: usize = 128;
/// Most labels one job carries.
pub(crate) const MAX_LABELS: usize = 16;
/// Longest label key, in bytes.
pub(crate) const MAX_LABEL_KEY_BYTES: usize = 64;
/// Longest label value, in bytes.
pub(crate) const MAX_LABEL_VALUE_BYTES: usize = 256;

/// Which stated member a request got wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AttributionError {
    /// The owner is empty, too long, or outside `[A-Za-z0-9._:@-]`.
    Owner,
    /// Too many labels, or a key or a value outside its bounds.
    Labels,
}

/// The validated owner and labels of one job. The default is unattributed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct JobAttribution {
    owner: Option<String>,
    labels: BTreeMap<String, String>,
}

impl JobAttribution {
    /// Validate what a caller stated.
    ///
    /// # Errors
    /// Names the member outside its bounds; nothing is truncated or repaired.
    pub(crate) fn new(
        owner: Option<String>,
        labels: BTreeMap<String, String>,
    ) -> Result<Self, AttributionError> {
        if owner.as_deref().is_some_and(|owner| !valid_owner(owner)) {
            return Err(AttributionError::Owner);
        }
        if labels.len() > MAX_LABELS
            || !labels
                .iter()
                .all(|(key, value)| valid_label_key(key) && valid_label_value(value))
        {
            return Err(AttributionError::Labels);
        }
        Ok(Self { owner, labels })
    }

    pub(crate) fn is_unattributed(&self) -> bool {
        self.owner.is_none() && self.labels.is_empty()
    }

    pub(crate) fn into_parts(self) -> (Option<String>, BTreeMap<String, String>) {
        (self.owner, self.labels)
    }

    /// Add what is stated to a hash preimage. An unattributed job adds
    /// nothing, so its hashes are what they were before attribution existed.
    pub(crate) fn bind(
        owner: Option<&str>,
        labels: &BTreeMap<String, String>,
        preimage: &mut Value,
    ) {
        if let Some(owner) = owner {
            preimage["owner"] = Value::String(owner.to_owned());
        }
        if !labels.is_empty() {
            preimage["labels"] = Value::Object(
                labels
                    .iter()
                    .map(|(key, value)| (key.clone(), Value::String(value.clone())))
                    .collect(),
            );
        }
    }
}

/// What one admission binds onto the workflow it runs: literal inputs, the
/// ceiling its requester asked for, and the attribution it stated.
#[derive(Debug, Clone, Default)]
pub(crate) struct JobBindings {
    /// Literal API input overrides.
    pub(crate) inputs: BTreeMap<String, Value>,
    /// The per-job ceiling its requester bound (USD).
    pub(crate) max_cost_usd: Option<f64>,
    /// The stated owner and labels.
    pub(crate) attribution: JobAttribution,
}

impl JobBindings {
    /// Literal inputs and a requested ceiling, unattributed.
    pub(crate) fn of(inputs: BTreeMap<String, Value>, max_cost_usd: Option<f64>) -> Self {
        Self {
            inputs,
            max_cost_usd,
            attribution: JobAttribution::default(),
        }
    }

    /// The same bindings under a stated attribution.
    pub(crate) fn attributed(mut self, attribution: JobAttribution) -> Self {
        self.attribution = attribution;
        self
    }
}

/// 1 to 128 bytes of `[A-Za-z0-9._:@-]`.
pub(crate) fn valid_owner(owner: &str) -> bool {
    !owner.is_empty()
        && owner.len() <= MAX_OWNER_BYTES
        && owner.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'@' | b'-')
        })
}

/// 1 to 64 bytes of `[A-Za-z0-9._/-]`: no colon, so a `key:value` filter
/// splits on its first colon.
pub(crate) fn valid_label_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= MAX_LABEL_KEY_BYTES
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'/' | b'-'))
}

/// At most 256 bytes without a control character.
pub(crate) fn valid_label_value(value: &str) -> bool {
    value.len() <= MAX_LABEL_VALUE_BYTES && !value.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
        entries
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn an_owner_is_one_to_128_bytes_of_its_alphabet() {
        for owner in ["u", "user_123", "org:acme@eu-west.1", &"a".repeat(128)] {
            assert!(valid_owner(owner), "{owner} is a legal owner");
        }
        for owner in ["", "user 123", "user/123", "üser", "a\n", &"a".repeat(129)] {
            assert!(!valid_owner(owner), "{owner:?} is refused");
        }
    }

    #[test]
    fn a_label_key_never_carries_the_filter_separator() {
        for key in ["app", "billing/plan", "tier.v2", "a-b_c", &"k".repeat(64)] {
            assert!(valid_label_key(key), "{key} is a legal key");
        }
        for key in ["", "app:eu", "a b", "clé", &"k".repeat(65)] {
            assert!(!valid_label_key(key), "{key:?} is refused");
        }
    }

    #[test]
    fn a_label_value_is_bounded_text_without_control_characters() {
        for value in ["", "pro", "été 2026: bêta", &"v".repeat(256)] {
            assert!(valid_label_value(value), "{value} is a legal value");
        }
        for value in ["line\nbreak", "tab\there", "nul\0", &"v".repeat(257)] {
            assert!(!valid_label_value(value), "{value:?} is refused");
        }
    }

    #[test]
    fn attribution_names_the_member_outside_its_bounds() {
        let stated = JobAttribution::new(
            Some("user_123".to_owned()),
            labels(&[("app", "translation"), ("plan", "pro")]),
        )
        .expect("legal attribution");
        let (owner, stated_labels) = stated.into_parts();
        assert_eq!(owner.as_deref(), Some("user_123"));
        assert_eq!(stated_labels.len(), 2);
        assert_eq!(
            JobAttribution::default().into_parts(),
            (None, BTreeMap::new())
        );

        assert_eq!(
            JobAttribution::new(Some("user 123".to_owned()), BTreeMap::new()),
            Err(AttributionError::Owner)
        );
        assert_eq!(
            JobAttribution::new(None, labels(&[("app:eu", "x")])),
            Err(AttributionError::Labels)
        );
        let seventeen = (0..17)
            .map(|index| (format!("k{index}"), String::new()))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(
            JobAttribution::new(None, seventeen),
            Err(AttributionError::Labels)
        );
        let sixteen = (0..16)
            .map(|index| (format!("k{index}"), String::new()))
            .collect::<BTreeMap<_, _>>();
        assert!(JobAttribution::new(None, sixteen).is_ok());
    }

    #[test]
    fn an_unattributed_job_adds_nothing_to_a_preimage() {
        let mut preimage = serde_json::json!({"job_id": "j"});
        JobAttribution::bind(None, &BTreeMap::new(), &mut preimage);
        assert_eq!(preimage, serde_json::json!({"job_id": "j"}));

        JobAttribution::bind(Some("user_123"), &labels(&[("app", "x")]), &mut preimage);
        assert_eq!(
            preimage,
            serde_json::json!({"job_id": "j", "labels": {"app": "x"}, "owner": "user_123"})
        );
    }
}
