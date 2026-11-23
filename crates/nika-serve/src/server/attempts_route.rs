// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! `GET /v1/jobs/{id}/attempts` (ADR-156 section 10): every attempt of a
//! job, the policy in force for it and where it stands about being started
//! again; and the attempt `GET /v1/jobs/{id}/trace/verify?attempt=N` names.

use hyper::{Response, StatusCode};
use serde::Serialize;

use super::AppState;
use super::error::{ApiError, ResponseBody};
use super::route::{admission_error, json_response};
use crate::job::{AttemptView, ClosedAttempt, JobAttempts, RetryRefusal, Spend, retryable};
use crate::{JobId, JobReceipt, JobRecord, JobStatus};

/// Answer one attempts read.
pub(super) async fn read(id: &str, state: &AppState) -> Response<ResponseBody> {
    let Ok(id) = JobId::parse(id) else {
        return ApiError::job_not_found().into_response();
    };
    match state.store.attempts(id).await {
        Ok(Some(read)) => json_response(StatusCode::OK, &AttemptsBody::of(&read, state)),
        Ok(None) => ApiError::job_not_found().into_response(),
        Err(error) => admission_error(&error),
    }
}

/// Why a trace query names no attempt of the job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TraceQueryRefusal {
    /// Anything but `attempt=N` with `N` 1 or more.
    Malformed,
    /// A number the job has not reached.
    Beyond,
}

impl TraceQueryRefusal {
    pub(super) fn into_response(self) -> Response<ResponseBody> {
        match self {
            Self::Malformed => ApiError::new(
                StatusCode::BAD_REQUEST,
                "invalid_trace_query",
                "the trace verification takes one optional query member, attempt=N, with N the number of one of the job's attempts (the first is 1)",
            ),
            Self::Beyond => ApiError::new(
                StatusCode::NOT_FOUND,
                "attempt_not_found",
                "the job has not taken this attempt",
            ),
        }
        .into_response()
    }
}

/// The attempt a trace query names: `None` for the latest (no query, or
/// its own number), the closed attempt otherwise.
///
/// # Errors
/// Why the query names no attempt of the job.
pub(super) fn named_attempt<'a>(
    query: Option<&str>,
    record: &'a JobRecord,
) -> Result<Option<&'a ClosedAttempt>, TraceQueryRefusal> {
    let Some(query) = query.filter(|query| !query.is_empty()) else {
        return Ok(None);
    };
    let number = query
        .strip_prefix("attempt=")
        .filter(|digits| !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|digits| digits.parse::<u32>().ok())
        .filter(|number| *number >= 1)
        .ok_or(TraceQueryRefusal::Malformed)?;
    if number == record.attempt() {
        return Ok(None);
    }
    usize::try_from(number - 1)
        .ok()
        .and_then(|index| record.attempts.get(index))
        .map(Some)
        .ok_or(TraceQueryRefusal::Beyond)
}

/// What the attempts route answers. Open: a later engine may add a member.
#[derive(Debug, Serialize)]
struct AttemptsBody<'a> {
    job_id: &'a str,
    policy: PolicyBody,
    retry_state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    retry_reason: Option<&'static str>,
    attempts: Vec<AttemptBody<'a>>,
}

/// The policy in force for the job: the resident's, as the job's own
/// `retry` statement narrows it.
#[derive(Debug, Serialize)]
struct PolicyBody {
    on_interrupted: &'static str,
    max_attempts: u32,
}

#[derive(Debug, Serialize)]
struct AttemptBody<'a> {
    attempt: u32,
    status: JobStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    execution_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    trace_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mode: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    resumed_from: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    receipt: Option<&'a JobReceipt>,
    #[serde(skip_serializing_if = "Option::is_none")]
    spend: Option<&'a Spend>,
    #[serde(skip_serializing_if = "Option::is_none")]
    started_at: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ended_at: Option<&'a str>,
}

impl<'a> AttemptsBody<'a> {
    fn of(read: &'a JobAttempts, state: &AppState) -> Self {
        let policy = state
            .limits
            .retry_policy()
            .for_job(read.record.retry.as_ref());
        let retry_state = read.retry_state.at_bound(
            retryable(&read.record),
            read.record.attempt(),
            policy.max_attempts(),
        );
        Self {
            job_id: read.record.id().as_str(),
            policy: PolicyBody {
                on_interrupted: policy.on_interrupted().as_str(),
                max_attempts: policy.max_attempts(),
            },
            retry_state: retry_state.as_str(),
            retry_reason: retry_state.reason().map(RetryRefusal::code),
            attempts: read.attempts.iter().map(AttemptBody::of).collect(),
        }
    }
}

impl<'a> AttemptBody<'a> {
    fn of(view: &'a AttemptView) -> Self {
        Self {
            attempt: view.attempt,
            status: view.status,
            execution_id: view.execution_id.as_deref(),
            trace_id: view.trace_id.as_deref(),
            mode: view.mode.map(crate::job::AttemptMode::as_str),
            resumed_from: view.resumed_from.as_deref(),
            receipt: view.receipt.as_ref(),
            spend: view.spend.as_ref(),
            started_at: view.started_at.as_deref(),
            ended_at: view.ended_at.as_deref(),
        }
    }
}
