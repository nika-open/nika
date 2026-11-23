// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! The attribution members of a request (ADR-155): `owner` and `labels`.
//!
//! Both are read as raw JSON so a wrong type, a null or a value outside its
//! bounds is refused with the code that names the member, before a job
//! exists. Nothing is truncated or repaired.

use std::collections::BTreeMap;

use hyper::StatusCode;
use serde_json::Value;

use super::error::ApiError;
use crate::job::{AttributionError, JobAttribution};

/// What a request stated, validated.
///
/// # Errors
/// `422 invalid_job_owner` or `422 invalid_job_labels`.
pub(super) fn stated(
    owner: Option<&Value>,
    labels: Option<&Value>,
) -> Result<JobAttribution, ApiError> {
    let owner = match owner {
        None => None,
        Some(Value::String(owner)) => Some(owner.clone()),
        Some(_) => return Err(invalid_owner()),
    };
    let labels = match labels {
        None => BTreeMap::new(),
        Some(Value::Object(labels)) => labels
            .iter()
            .map(|(key, value)| Some((key.clone(), value.as_str()?.to_owned())))
            .collect::<Option<BTreeMap<_, _>>>()
            .ok_or_else(invalid_labels)?,
        Some(_) => return Err(invalid_labels()),
    };
    JobAttribution::new(owner, labels).map_err(|error| match error {
        AttributionError::Owner => invalid_owner(),
        AttributionError::Labels => invalid_labels(),
    })
}

fn invalid_owner() -> ApiError {
    ApiError::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid_job_owner",
        "owner must be a string of 1 to 128 bytes of letters, digits and ._:@-",
    )
}

fn invalid_labels() -> ApiError {
    ApiError::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid_job_labels",
        "labels must be at most 16 string pairs: a key of 1 to 64 bytes of letters, digits and ._/-, a value of at most 256 bytes without control characters",
    )
}
