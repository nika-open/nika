// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! Registry probes — small, focused async functions that check whether a
//! catalog entry is resolvable upstream.
//!
//! Every probe returns `Ok(())` on success and a [`ProbeError`] on
//! failure. The error variant captures **why** the probe failed so the
//! report can distinguish "network down" from "package deleted" from
//! "registry returned a 5xx".

use std::time::Duration;

use reqwest::{Client, Method, StatusCode};
use thiserror::Error;

/// Every probe timeout. 10s is plenty for an HTTP HEAD/GET against a
/// public registry; anything slower = network problem, not data drift.
pub(crate) const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Reason a probe failed. `#[non_exhaustive]` so future registry types
/// (crates.io, GitHub Packages) can add variants without breaking match.
#[derive(Debug, Error)]
#[non_exhaustive]
pub(crate) enum ProbeError {
    /// Package / endpoint confirmed absent (404 from authoritative source).
    #[error("not found upstream: {0}")]
    NotFound(String),

    /// Registry replied with a non-OK status that is not 404.
    #[error("registry returned {status}: {url}")]
    BadStatus {
        /// The URL probed.
        url: String,
        /// The observed HTTP status.
        status: StatusCode,
    },

    /// Network failure (DNS, TCP, TLS, timeout, unreachable, etc).
    #[error("network error probing {url}: {source}")]
    Network {
        /// The URL probed.
        url: String,
        /// Underlying reqwest error.
        #[source]
        source: reqwest::Error,
    },
}

/// Build a reqwest client with sane defaults for catalog verification.
pub(crate) fn build_client() -> Result<Client, reqwest::Error> {
    Client::builder()
        .timeout(PROBE_TIMEOUT)
        .user_agent(concat!(
            "nika-catalog-verify/",
            env!("CARGO_PKG_VERSION"),
            " (+https://github.com/nika-open/nika)",
        ))
        .build()
}

/// Probe an npm package. Hits `registry.npmjs.org/<pkg>/latest` — fast,
/// authoritative, returns 404 for deleted packages.
pub(crate) async fn probe_npm(client: &Client, package: &str) -> Result<(), ProbeError> {
    let url = format!(
        "https://registry.npmjs.org/{}/latest",
        urlencoded_package(package)
    );
    http_probe(client, Method::GET, &url).await
}

/// Probe a `PyPI` package. Hits `pypi.org/pypi/<pkg>/json`.
pub(crate) async fn probe_pypi(client: &Client, package: &str) -> Result<(), ProbeError> {
    let url = format!("https://pypi.org/pypi/{}/json", package.replace('/', "%2F"));
    http_probe(client, Method::GET, &url).await
}

/// Probe a crates.io package. Hits `crates.io/api/v1/crates/<pkg>`.
pub(crate) async fn probe_cargo(client: &Client, package: &str) -> Result<(), ProbeError> {
    let url = format!("https://crates.io/api/v1/crates/{package}");
    http_probe(client, Method::GET, &url).await
}

/// Probe an OCI image manifest. `identifier` is `<registry>/<image>` —
/// e.g. `docker.io/grafana/mcp-grafana`. Tag defaults to `latest`. Docker
/// Hub's registry API lives on `registry-1.docker.io`: `docker.io` itself
/// redirects to the website, where every manifest path is a 404.
///
/// Note: many OCI registries require authentication for manifest reads.
/// A 401/403 here generally means "registry exists" and is not treated
/// as drift — only 404 and network errors are.
pub(crate) async fn probe_oci(client: &Client, identifier: &str) -> Result<(), ProbeError> {
    let (registry, image) = identifier
        .split_once('/')
        .ok_or_else(|| ProbeError::NotFound(format!("malformed OCI identifier {identifier:?}")))?;
    let registry = if registry == "docker.io" {
        "registry-1.docker.io"
    } else {
        registry
    };
    let url = format!("https://{registry}/v2/{image}/manifests/latest");
    match http_probe(client, Method::HEAD, &url).await {
        Ok(()) => Ok(()),
        // 401/403 = registry answered, likely auth gate — treat as present.
        Err(ProbeError::BadStatus { status, .. })
            if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN =>
        {
            Ok(())
        }
        other => other,
    }
}

/// The request an MCP client opens a session with.
const INITIALIZE: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"nika-catalog-verify","version":"1"}}}"#;

/// Probe a remote MCP endpoint with the `initialize` POST an MCP client
/// opens with. An MCP endpoint answers POST, so a HEAD says nothing about
/// it (measured 2026-11-24: `https://mcp.stripe.com` answers HEAD 404 and
/// `initialize` 401). The body is never read.
pub(crate) async fn probe_remote(client: &Client, url: &str) -> Result<(), ProbeError> {
    let resp = client
        .post(url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(
            reqwest::header::ACCEPT,
            "application/json, text/event-stream",
        )
        .body(INITIALIZE)
        .send()
        .await
        .map_err(|source| ProbeError::Network {
            url: url.to_string(),
            source,
        })?;
    remote_verdict(url, resp.status())
}

/// What an `initialize` answer says about a remote: a success, or 401/403
/// (the server answered and asks for its auth), is present; 404 and 410
/// are drift; anything else is reported without a verdict.
fn remote_verdict(url: &str, status: StatusCode) -> Result<(), ProbeError> {
    if status.is_success() || status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN
    {
        return Ok(());
    }
    if status == StatusCode::NOT_FOUND || status == StatusCode::GONE {
        return Err(ProbeError::NotFound(url.to_string()));
    }
    Err(ProbeError::BadStatus {
        url: url.to_string(),
        status,
    })
}

// ─── internals ──────────────────────────────────────────────────────────

/// Minimal URL-escape for npm package names. npm accepts `@scope/name`
/// unescaped in its JSON API, but percent-encoding the slash is also
/// valid and avoids quirks with scoped packages.
fn urlencoded_package(pkg: &str) -> String {
    if pkg.starts_with('@') {
        // @scope/name → @scope%2Fname
        pkg.replacen('/', "%2F", 1)
    } else {
        pkg.to_string()
    }
}

async fn http_probe(client: &Client, method: Method, url: &str) -> Result<(), ProbeError> {
    let resp = client
        .request(method, url)
        .send()
        .await
        .map_err(|source| ProbeError::Network {
            url: url.to_string(),
            source,
        })?;
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    if status == StatusCode::NOT_FOUND {
        return Err(ProbeError::NotFound(url.to_string()));
    }
    Err(ProbeError::BadStatus {
        url: url.to_string(),
        status,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoped_npm_package_url_encodes_slash() {
        assert_eq!(
            urlencoded_package("@modelcontextprotocol/server-filesystem"),
            "@modelcontextprotocol%2Fserver-filesystem"
        );
    }

    #[test]
    fn plain_npm_package_passes_through() {
        assert_eq!(urlencoded_package("redis-mcp"), "redis-mcp");
    }

    #[test]
    fn a_remote_that_asks_for_its_auth_is_present_and_only_a_404_or_410_is_drift() {
        let url = "https://mcp.example.com/mcp";
        for ok in [
            StatusCode::OK,
            StatusCode::ACCEPTED,
            StatusCode::UNAUTHORIZED,
            StatusCode::FORBIDDEN,
        ] {
            assert!(remote_verdict(url, ok).is_ok(), "{ok}");
        }
        for gone in [StatusCode::NOT_FOUND, StatusCode::GONE] {
            assert!(
                matches!(remote_verdict(url, gone), Err(ProbeError::NotFound(u)) if u == url),
                "{gone}"
            );
        }
        assert!(matches!(
            remote_verdict(url, StatusCode::METHOD_NOT_ALLOWED),
            Err(ProbeError::BadStatus { status, .. }) if status == StatusCode::METHOD_NOT_ALLOWED
        ));
    }

    #[test]
    fn the_initialize_body_is_a_json_rpc_request() {
        let v: serde_json::Value = serde_json::from_str(INITIALIZE).expect("valid JSON");
        assert_eq!(v["jsonrpc"], "2.0");
        assert_eq!(v["method"], "initialize");
    }

    #[test]
    fn probe_timeout_is_ten_seconds() {
        assert_eq!(PROBE_TIMEOUT, Duration::from_secs(10));
    }
}
