// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! `GET /v1/jobs`: the job list (ADR-155).
//!
//! The list is how an application finds its jobs again: by the owner and
//! labels it stated, by status, workflow, origin and admission date. It is
//! a view inside one authority domain. Filtering by owner is never an
//! authorization boundary: whoever holds the token reads every job.

use std::collections::BTreeMap;

use hyper::{Response, StatusCode};
use serde::Serialize;
use serde_json::Value;

use super::AppState;
use super::error::{ApiError, ResponseBody, ServerError};
use super::route::json_response;
use nika_cadence::attribution::{valid_label_key, valid_label_value, valid_owner};

use crate::job::{DEFAULT_PAGE_LEN, JobListFilter, JobPage, JobSummary, MAX_PAGE_LEN, OriginKind};
use crate::{JobOrigin, JobStatus, JobStoreError};

/// Longest query string the list reads, in bytes.
const MAX_QUERY_BYTES: usize = 8 * 1024;
/// Longest workflow name a filter states, in bytes (the by-name job form's).
const MAX_WORKFLOW_BYTES: usize = 4096;
/// Most `label` members one query states (a job carries at most as many).
const MAX_LABEL_FILTERS: usize = 16;
/// What every cursor of this list starts with: its format generation.
const CURSOR_PREFIX: &str = "c1.";

/// What one request asks of the list.
#[derive(Debug, PartialEq, Eq)]
struct ListQuery {
    filter: JobListFilter,
    limit: usize,
    /// The ordinal the cursor names: the page holds strictly older jobs.
    before: Option<u64>,
}

/// Which member of the query is outside its contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refused {
    Shape,
    Owner,
    Status,
    Workflow,
    Origin,
    Label,
    Instant,
    Limit,
    Cursor,
}

impl Refused {
    const fn into_error(self) -> ApiError {
        let message = match self {
            Self::Shape => {
                "the job list takes owner, status, workflow, origin, label, created_after, created_before, limit and cursor, percent-encoded in a query of at most 8192 bytes; an unknown member is refused"
            }
            Self::Owner => "owner must be 1 to 128 bytes of letters, digits and ._:@-, stated once",
            Self::Status => {
                "status must be queued, running, interrupted, paused, succeeded, failed or cancelled; repeat the member or separate values with commas"
            }
            Self::Workflow => "workflow must be one served name of 1 to 4096 bytes, stated once",
            Self::Origin => "origin must be manual or schedule, stated once",
            Self::Label => {
                "label must be key:value, a key of 1 to 64 bytes of letters, digits and ._/- and a value of at most 256 bytes; at most 16 label members"
            }
            Self::Instant => {
                "created_after and created_before must be RFC 3339 instants such as 2026-11-23T10:00:00Z, each stated once; write a + offset as %2B"
            }
            Self::Limit => "limit must be an integer from 1 to 200, stated once",
            Self::Cursor => {
                return ApiError::new(
                    StatusCode::BAD_REQUEST,
                    "invalid_job_cursor",
                    "cursor must be the next_cursor of a previous page of this list, stated once",
                );
            }
        };
        ApiError::new(StatusCode::BAD_REQUEST, "invalid_job_query", message)
    }
}

/// Answer one page of the list.
pub(super) async fn list(query: Option<&str>, state: &AppState) -> Response<ResponseBody> {
    let query = match parse(query.unwrap_or_default()) {
        Ok(query) => query,
        Err(refused) => return refused.into_error().into_response(),
    };
    match state
        .store
        .list(query.filter, query.limit, query.before)
        .await
    {
        Ok(page) => json_response(StatusCode::OK, &JobListBody::of(&page)),
        Err(ServerError::JobStore(JobStoreError::Busy) | ServerError::StoreQueueFull) => {
            ApiError::store_busy().into_response()
        }
        Err(_) => ApiError::internal().into_response(),
    }
}

fn parse(raw: &str) -> Result<ListQuery, Refused> {
    if raw.len() > MAX_QUERY_BYTES {
        return Err(Refused::Shape);
    }
    let mut filter = JobListFilter::default();
    let mut limit = None;
    let mut before = None;
    for pair in raw.split('&').filter(|pair| !pair.is_empty()) {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        let name = decode(name).ok_or(Refused::Shape)?;
        match name.as_str() {
            "owner" => {
                let owner = decode(value).filter(|owner| valid_owner(owner));
                once(&mut filter.owner, owner, Refused::Owner)?;
            }
            "status" => {
                let stated = decode(value).ok_or(Refused::Status)?;
                for word in stated.split(',') {
                    let status = status(word).ok_or(Refused::Status)?;
                    if !filter.statuses.contains(&status) {
                        filter.statuses.push(status);
                    }
                }
            }
            "workflow" => {
                let workflow = decode(value)
                    .filter(|name| !name.is_empty() && name.len() <= MAX_WORKFLOW_BYTES);
                once(&mut filter.workflow, workflow, Refused::Workflow)?;
            }
            "origin" => {
                let origin = match value {
                    "manual" => Some(OriginKind::Manual),
                    "schedule" => Some(OriginKind::Schedule),
                    _ => None,
                };
                once(&mut filter.origin, origin, Refused::Origin)?;
            }
            "label" => {
                let stated = decode(value).ok_or(Refused::Label)?;
                let (key, value) = stated.split_once(':').ok_or(Refused::Label)?;
                if filter.labels.len() >= MAX_LABEL_FILTERS
                    || !valid_label_key(key)
                    || !valid_label_value(value)
                {
                    return Err(Refused::Label);
                }
                filter.labels.push((key.to_owned(), value.to_owned()));
            }
            "created_after" => once(&mut filter.created_after, instant(value), Refused::Instant)?,
            "created_before" => {
                once(&mut filter.created_before, instant(value), Refused::Instant)?;
            }
            "limit" => {
                let stated = value
                    .parse::<usize>()
                    .ok()
                    .filter(|limit| (1..=MAX_PAGE_LEN).contains(limit))
                    .filter(|_| value.bytes().all(|byte| byte.is_ascii_digit()));
                once(&mut limit, stated, Refused::Limit)?;
            }
            "cursor" => once(&mut before, ordinal(value), Refused::Cursor)?,
            _ => return Err(Refused::Shape),
        }
    }
    Ok(ListQuery {
        filter,
        limit: limit.unwrap_or(DEFAULT_PAGE_LEN),
        before,
    })
}

/// Set a member that a query states at most once, with a legal value.
fn once<T>(slot: &mut Option<T>, value: Option<T>, refused: Refused) -> Result<(), Refused> {
    match (slot.is_some(), value) {
        (false, Some(value)) => {
            *slot = Some(value);
            Ok(())
        }
        _ => Err(refused),
    }
}

/// One status by the word the wire uses for it.
fn status(word: &str) -> Option<JobStatus> {
    serde_json::from_value(Value::String(word.to_owned())).ok()
}

fn instant(value: &str) -> Option<jiff::Timestamp> {
    decode(value)?.parse().ok()
}

/// The ordinal a cursor names. A cursor is opaque to its caller: only this
/// list writes one, and only this function reads it.
fn ordinal(cursor: &str) -> Option<u64> {
    let digits = cursor.strip_prefix(CURSOR_PREFIX)?;
    let canonical = !digits.is_empty()
        && digits.len() <= 16
        && !(digits.len() > 1 && digits.starts_with('0'))
        && digits
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    u64::from_str_radix(digits, 16)
        .ok()
        .filter(|ordinal| canonical && *ordinal > 0)
}

fn cursor(ordinal: u64) -> String {
    format!("{CURSOR_PREFIX}{ordinal:x}")
}

/// One query component, decoded once as a form field: `+` is a space and
/// `%XX` is a byte. `None` for an escape that is not two hexadecimal
/// digits, bytes that are not UTF-8, or a NUL.
fn decode(raw: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(raw.len());
    let mut rest = raw.as_bytes();
    while let Some((&byte, tail)) = rest.split_first() {
        match byte {
            b'%' => {
                let (hex, after) = tail.split_at_checked(2)?;
                bytes.push(super::artifacts::hex_pair(hex)?);
                rest = after;
            }
            b'+' => {
                bytes.push(b' ');
                rest = tail;
            }
            _ => {
                bytes.push(byte);
                rest = tail;
            }
        }
    }
    String::from_utf8(bytes)
        .ok()
        .filter(|text| !text.contains('\0'))
}

/// One page as the wire carries it. Both objects are open by contract: a
/// later engine may add a member, and a client ignores what it does not know.
#[derive(Debug, Serialize)]
struct JobListBody<'a> {
    jobs: Vec<JobSummaryBody<'a>>,
    /// Present while an older match exists; absent on the last page.
    #[serde(skip_serializing_if = "Option::is_none")]
    next_cursor: Option<String>,
}

/// What the list shows of one job: never its inputs, its outputs, its
/// receipt or its settlement.
#[derive(Debug, Serialize)]
struct JobSummaryBody<'a> {
    id: &'a str,
    status: JobStatus,
    workflow: &'a str,
    origin: &'a JobOrigin,
    #[serde(skip_serializing_if = "Option::is_none")]
    owner: Option<&'a str>,
    labels: &'a BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    created_at: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    execution_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    trace_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<ErrorBody<'a>>,
}

#[derive(Debug, Serialize)]
struct ErrorBody<'a> {
    code: &'a str,
    message: &'a str,
}

impl<'a> JobListBody<'a> {
    fn of(page: &'a JobPage) -> Self {
        Self {
            jobs: page.jobs.iter().map(JobSummaryBody::of).collect(),
            next_cursor: page.next.map(cursor),
        }
    }
}

impl<'a> JobSummaryBody<'a> {
    fn of(job: &'a JobSummary) -> Self {
        Self {
            id: job.id.as_str(),
            status: job.status,
            workflow: &job.workflow,
            origin: &job.origin,
            owner: job.owner.as_deref(),
            labels: &job.labels,
            created_at: job.created_at.as_deref(),
            execution_id: job.execution_id.as_deref(),
            trace_id: job.trace_id.as_deref(),
            error: job
                .error
                .as_ref()
                .map(|(code, message)| ErrorBody { code, message }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refused(query: &str) -> Refused {
        parse(query).expect_err("refused")
    }

    #[test]
    fn an_empty_query_is_the_newest_fifty_of_everything() {
        let query = parse("").expect("empty query");
        assert_eq!(query.filter, JobListFilter::default());
        assert_eq!(query.limit, 50);
        assert_eq!(query.before, None);
        assert_eq!(parse("&&").expect("only separators"), query);
    }

    #[test]
    fn every_member_is_read_decoded_and_bounded() {
        let query = parse(
            "owner=user_123&status=failed,interrupted&status=failed&workflow=flows%2Fdaily.nika\
             &origin=schedule&label=app:translation&label=note%3A%C3%A9t%C3%A9+2026%3A+b\
             &created_after=2026-11-16T00:00:00Z&created_before=2026-11-23T10%3A00%3A00%2B02:00\
             &limit=200&cursor=c1.2a",
        )
        .expect("legal query");
        assert_eq!(query.filter.owner.as_deref(), Some("user_123"));
        assert_eq!(
            query.filter.statuses,
            [JobStatus::Failed, JobStatus::Interrupted]
        );
        assert_eq!(query.filter.workflow.as_deref(), Some("flows/daily.nika"));
        assert_eq!(query.filter.origin, Some(OriginKind::Schedule));
        assert_eq!(
            query.filter.labels,
            [
                ("app".to_owned(), "translation".to_owned()),
                ("note".to_owned(), "été 2026: b".to_owned()),
            ]
        );
        assert_eq!(
            query.filter.created_after,
            "2026-11-16T00:00:00Z".parse().ok()
        );
        assert_eq!(
            query.filter.created_before,
            "2026-11-23T08:00:00Z".parse().ok()
        );
        assert_eq!(query.limit, 200);
        assert_eq!(query.before, Some(42));
    }

    #[test]
    fn a_member_outside_its_contract_is_named() {
        let long = format!("owner={}", "a".repeat(MAX_QUERY_BYTES));
        for (query, expected) in [
            ("page=2", Refused::Shape),
            ("inputs=1", Refused::Shape),
            ("%zz=1", Refused::Shape),
            (long.as_str(), Refused::Shape),
            ("owner=", Refused::Owner),
            ("owner=user+123", Refused::Owner),
            ("owner=a&owner=a", Refused::Owner),
            ("owner=%ff", Refused::Owner),
            ("status=done", Refused::Status),
            ("status=", Refused::Status),
            ("status=failed,", Refused::Status),
            ("status=Failed", Refused::Status),
            ("workflow=", Refused::Workflow),
            ("workflow=a.nika&workflow=b.nika", Refused::Workflow),
            ("origin=api", Refused::Origin),
            ("origin=manual&origin=manual", Refused::Origin),
            ("label=app", Refused::Label),
            ("label=:x", Refused::Label),
            ("label=a%20b:x", Refused::Label),
            ("label=app:line%0Abreak", Refused::Label),
            ("created_after=yesterday", Refused::Instant),
            ("created_after=2026-11-23", Refused::Instant),
            ("created_before=2026-11-23T10:00:00+02:00", Refused::Instant),
            (
                "created_after=2026-11-23T10:00:00Z&created_after=2026-11-23T10:00:00Z",
                Refused::Instant,
            ),
            ("limit=0", Refused::Limit),
            ("limit=201", Refused::Limit),
            ("limit=+5", Refused::Limit),
            ("limit=ten", Refused::Limit),
            ("limit=5&limit=5", Refused::Limit),
            ("cursor=42", Refused::Cursor),
            ("cursor=c1.", Refused::Cursor),
            ("cursor=c1.0", Refused::Cursor),
            ("cursor=c1.02a", Refused::Cursor),
            ("cursor=c1.2A", Refused::Cursor),
            ("cursor=c2.2a", Refused::Cursor),
            ("cursor=c1.10000000000000000", Refused::Cursor),
            ("cursor=c1.2a&cursor=c1.2a", Refused::Cursor),
        ] {
            assert_eq!(refused(query), expected, "{query}");
        }
        let seventeen = (0..17)
            .map(|index| format!("label=k{index}:v"))
            .collect::<Vec<_>>()
            .join("&");
        assert_eq!(refused(&seventeen), Refused::Label);
        assert_eq!(Refused::Cursor.into_error().code(), "invalid_job_cursor");
        assert_eq!(Refused::Label.into_error().code(), "invalid_job_query");
    }

    #[test]
    fn a_cursor_is_what_this_list_wrote() {
        for ordinal_value in [1, 42, u64::MAX] {
            assert_eq!(ordinal(&cursor(ordinal_value)), Some(ordinal_value));
        }
    }
}
