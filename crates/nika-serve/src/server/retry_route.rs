// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! `POST /v1/jobs/{id}/retry` (ADR-156 section 9): a request starts the
//! latest attempt of a settled job again, as a new attempt of the same job.
//!
//! The request is the caller's consent to run the workflow's effects
//! again. The bound of attempts, the one ceiling over every attempt and,
//! for `resume`, the judgment of the journal hold as for a run that lost
//! its owner. The place in the owner's lane is taken before anything is
//! written, so an owner at its bound is refused with nothing changed. The
//! same request sent again answers `200` with the job as it stands, and
//! writes nothing.

use std::sync::Arc;

use hyper::body::Incoming;
use hyper::{Request, Response, StatusCode};
use serde_json::Value;

use super::error::{ApiError, ResponseBody};
use super::route::{admission_error, collect_body, json_response};
use super::{AppState, ServerError};
use crate::job::{Retried, RetryAsk, RetryRefusal, is_replay, retryable};
use crate::{JobId, JobRecord, JobStoreError};

/// Answer one retry request.
pub(super) async fn retry_job(
    request: Request<Incoming>,
    id: &str,
    state: Arc<AppState>,
) -> Response<ResponseBody> {
    match retry(request, id, &state).await {
        Ok(response) | Err(response) => response,
    }
}

async fn retry(
    request: Request<Incoming>,
    id: &str,
    state: &AppState,
) -> Result<Response<ResponseBody>, Response<ResponseBody>> {
    let id = JobId::parse(id).map_err(|_| ApiError::job_not_found().into_response())?;
    let body = collect_body(request, state.limits.max_body_bytes())
        .await
        .map_err(ApiError::into_response)?;
    let ask = serde_json::from_slice::<Value>(&body)
        .ok()
        .as_ref()
        .and_then(RetryAsk::read)
        .ok_or_else(|| invalid_request().into_response())?;
    let record = state
        .store
        .get(id.clone())
        .await
        .map_err(|error| store_error(&error))?
        .ok_or_else(|| ApiError::job_not_found().into_response())?;
    if is_replay(&record, &ask) {
        return Ok(replayed(&record));
    }
    if !retryable(&record) || record.attempt() != ask.attempt {
        return Err(not_retryable());
    }
    let owner = record.owner().map(str::to_owned);
    let world = state.store.load_world(id.clone()).await.ok();
    let judge = Arc::clone(&state.retry_judge);
    let decided =
        tokio::task::spawn_blocking(move || judge.retried(&record, world.as_deref(), &ask))
            .await
            .map_err(|_| ApiError::internal().into_response())?;
    let retried = decided.map_err(refused)?;
    let place = state
        .coordinator
        .place(owner.as_deref())
        .map_err(|error| admission_error(&error))?;
    match state
        .store
        .retry(id, retried)
        .await
        .map_err(|error| store_error(&error))?
    {
        Retried::Queued(mutation) => {
            state.coordinator.send_queued(place, mutation.record());
            Ok(accepted(mutation.record()))
        }
        Retried::Replayed(record) => Ok(replayed(&record)),
        Retried::NotRetryable => Err(not_retryable()),
    }
}

/// The job as the request left it: queued for its new attempt.
fn accepted(record: &JobRecord) -> Response<ResponseBody> {
    json_response(
        StatusCode::ACCEPTED,
        &super::model::JobResponse::from(record),
    )
}

/// The job the same request already retried, as it stands.
fn replayed(record: &JobRecord) -> Response<ResponseBody> {
    json_response(StatusCode::OK, &super::model::JobResponse::from(record))
}

fn invalid_request() -> ApiError {
    ApiError::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid_job_retry",
        "a retry request is an object of attempt (the number of the attempt it retries, 1 or more), mode (restart or resume) and an optional max_cost_usd (a non-negative amount)",
    )
}

fn not_retryable() -> Response<ResponseBody> {
    ApiError::new(
        StatusCode::CONFLICT,
        "job_not_retryable",
        "only the latest attempt of a job that ended interrupted or failed, and that its caller did not cancel, can be retried; attempt must name it",
    )
    .into_response()
}

/// A decision that refuses the request, by its own word.
fn refused(refusal: RetryRefusal) -> Response<ResponseBody> {
    ApiError::new(StatusCode::CONFLICT, refusal.code(), refusal.message()).into_response()
}

fn store_error(error: &ServerError) -> Response<ResponseBody> {
    match error {
        ServerError::JobStore(JobStoreError::JobNotFound(_)) => {
            ApiError::job_not_found().into_response()
        }
        other => admission_error(other),
    }
}
