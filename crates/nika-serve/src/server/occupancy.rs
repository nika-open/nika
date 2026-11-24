// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! What the resident holds against its limits (ADR-155): `GET /v1/occupancy`
//! and the refusal an owner meets at its own bound.
//!
//! Occupancy is authenticated, never on the public `/health`: it names owners.

use hyper::header::{HeaderValue, RETRY_AFTER};
use hyper::{Response, StatusCode};
use serde_json::{Value, json};

use super::AppState;
use super::error::{ApiError, ResponseBody};
use super::fair_queue::QueueLoad;
use super::provider_gate::ProviderLoad;
use super::route::json_response;

/// Owners one answer names, busiest first. The rest are counted.
const MAX_LISTED_OWNERS: usize = 100;
/// Seconds an owner at its queue bound is told to wait. A hint: the resident
/// cannot know when one of that owner's jobs will start.
const OWNER_RETRY_AFTER_SECONDS: &str = "5";

/// `GET /v1/occupancy`.
pub(super) fn read(state: &AppState) -> Response<ResponseBody> {
    let (limits, load) = state.coordinator.occupancy(MAX_LISTED_OWNERS);
    json_response(
        StatusCode::OK,
        &json!({
            "jobs": {
                "running": load.running,
                "queued": load.queued,
                "max_concurrent": limits.max_concurrent_jobs(),
                "queue_capacity": limits.queue_capacity(),
                "max_concurrent_per_owner": limits.max_concurrent_per_owner(),
                "max_queued_per_owner": limits.max_queued_per_owner(),
            },
            "owners": owners(&load),
            "owners_omitted": load.owners_omitted,
            "providers": providers(&state.coordinator.provider_load()),
        }),
    )
}

/// Every provider the operator capped, by id. A provider with no cap is not
/// listed: the resident does not count its calls.
fn providers(load: &[ProviderLoad]) -> Vec<Value> {
    load.iter()
        .map(|provider| {
            json!({
                "provider": provider.provider,
                "max_concurrent": provider.limit,
                "in_flight": provider.in_flight,
                "waiting": provider.waiting,
            })
        })
        .collect()
}

/// The busiest owners. `owner` is null for the jobs that state none.
fn owners(load: &QueueLoad) -> Vec<Value> {
    load.owners
        .iter()
        .map(|owner| {
            json!({
                "owner": owner.owner,
                "running": owner.running,
                "queued": owner.queued,
            })
        })
        .collect()
}

/// `429 owner_queue_full`: this owner holds every waiting place one owner
/// may hold. Other owners are still admitted, so this is the caller's to
/// wait out, never the server's unavailability.
pub(super) fn owner_queue_full() -> Response<ResponseBody> {
    let mut response = ApiError::new(
        StatusCode::TOO_MANY_REQUESTS,
        "owner_queue_full",
        "this owner already has as many jobs waiting as one owner may have; retry when one starts",
    )
    .into_response();
    response.headers_mut().insert(
        RETRY_AFTER,
        HeaderValue::from_static(OWNER_RETRY_AFTER_SECONDS),
    );
    response
}
