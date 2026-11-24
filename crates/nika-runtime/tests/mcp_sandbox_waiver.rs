// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! The MCP half of the witnessed waiver (#889): an MCP server always runs
//! inside a boundary, so on a host with no OS sandbox backend it starts
//! only under `NIKA_SANDBOX=off` (the spawn site refuses `auto` and
//! `require` with NIKA-1710). A run that can start one — an installed MCP
//! plane and a workflow naming an `mcp:` tool — attests that waiver on its
//! opening frame, where an `exec:` child under `permits:` attests its own.

use std::sync::Arc;

use nika_clock::SystemClock;
use nika_event::EventKind;
use nika_kernel::tool_executor::{ToolCall, ToolResult};
use nika_kernel_mock::{MockProvider, MockShell, MockToolDefinitionProvider, MockToolExecutor};
use nika_providers::{ProviderRegistry, ProvidersConfig};
use nika_runtime::{DeterministicStamper, Runtime, RuntimeConfig, VecSink};
use nika_verb_agent::AgentVerb;
use nika_verb_exec::ExecVerb;
use nika_verb_infer::InferVerb;
use nika_verb_invoke::{InvokeVerb, McpPlane};

/// One `mcp:` task, granted.
const MCP_TASK: &str = "nika: w\npermits:\n  tools: [\"mcp:s/ping\"]\ntasks:\n  a:\n    invoke: { tool: \"mcp:s/ping\" }\n";

/// A plane whose server answers every call.
fn answering_plane() -> McpPlane {
    McpPlane::new(
        Arc::new(|call: ToolCall| Ok(ToolResult::success(call.id.to_string(), "pong"))),
        Arc::new(Vec::new),
    )
}

/// Run `yaml` under the recorded posture (`backend` · `policy`), with or
/// without the MCP plane, and return the opening frame's three sandbox
/// fields: `sandbox` · `sandbox_policy` · `sandbox_waived`.
async fn opening_frame(
    yaml: &str,
    backend: &str,
    policy: &str,
    plane: bool,
) -> (Option<String>, Option<String>, Option<String>) {
    let wf = nika_schema::parse(
        yaml,
        nika_schema::FileId::new(0),
        nika_schema::ParseMode::Strict,
    )
    .expect("fixture parses");
    let report = nika_check::check(&wf);
    assert!(report.is_clean(), "{:?}", report.findings);
    let registry = Arc::new(ProviderRegistry::without_http(ProvidersConfig::default()));
    let invoke = Arc::new(InvokeVerb::new(Arc::new(MockToolExecutor::new())));
    let config = RuntimeConfig::default()
        .with_sandbox_backend(backend)
        .with_sandbox_policy(policy);
    let runtime = Runtime::new(
        ExecVerb::new(Arc::new(MockShell::new())),
        Arc::clone(&invoke),
        InferVerb::new(registry, "mock/echo"),
        AgentVerb::new(
            Arc::new(MockProvider::new("mock")),
            invoke,
            Arc::new(MockToolDefinitionProvider::new()),
            "mock/echo",
        ),
        SystemClock,
        config,
    );
    let runtime = if plane {
        runtime.with_mcp_plane(answering_plane())
    } else {
        runtime
    };
    let mut stamper = DeterministicStamper::new();
    let mut sink = VecSink::new();
    runtime
        .run(&wf, &report, &mut stamper, &mut sink)
        .await
        .expect("the run settles");
    let started = sink
        .events()
        .iter()
        .find(|e| e.kind == EventKind::WorkflowStarted)
        .expect("the opening frame");
    let field = |key: &str| started.str_field(key).map(str::to_owned);
    (
        field("sandbox"),
        field("sandbox_policy"),
        field("sandbox_waived"),
    )
}

#[tokio::test]
async fn an_mcp_server_started_under_the_waiver_is_attested_on_the_opening_frame() {
    let (sandbox, policy, waived) = opening_frame(MCP_TASK, "noop", "off", true).await;
    assert_eq!(sandbox.as_deref(), Some("noop"));
    assert_eq!(policy.as_deref(), Some("off"));
    assert_eq!(
        waived.as_deref(),
        Some("true"),
        "the run can start its MCP server unconfined: the waiver is witnessed"
    );
}

#[tokio::test]
async fn nothing_is_waived_where_no_mcp_server_starts_unconfined() {
    // No plane: no `mcp:` call can reach a server.
    let (_, _, waived) = opening_frame(MCP_TASK, "noop", "off", false).await;
    assert_eq!(waived, None, "no plane, no server");
    // `auto` and `require`: the spawn site refuses (NIKA-1710).
    for refused in ["auto", "require"] {
        let (_, policy, waived) = opening_frame(MCP_TASK, "noop", refused, true).await;
        assert_eq!(policy.as_deref(), Some(refused));
        assert_eq!(waived, None, "{refused} refuses the spawn: nothing waived");
    }
    // A backend confines the server whatever the policy.
    let (_, _, waived) = opening_frame(MCP_TASK, "seatbelt", "off", true).await;
    assert_eq!(waived, None, "a confined server waives nothing");
}
