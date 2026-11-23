// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! Who a job is attributed to (ADR-155).
//!
//! An owner and labels are what the caller STATES about a job so its
//! application can find the job again. They are opaque to the engine and
//! never an authorization boundary: one bearer credential is one authority
//! domain (ADR-117), and whoever holds it reads every job.
//!
//! Their shape is the one law of `nika_cadence::attribution`, which a
//! schedule declaration is judged by too: a job a schedule fires carries
//! exactly what its schedule states.

use std::collections::BTreeMap;

use nika_cadence::attribution::{valid_labels, valid_owner};
use serde_json::Value;

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
        if !valid_labels(&labels) {
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
