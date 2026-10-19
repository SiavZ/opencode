//! Purpose: MCP client session over any transport: initialize handshake (protocol negotiation,
//! roots capability), server capabilities and instructions, typed request helpers, and change
//! signals for `notifications/tools/list_changed` and connection close.
//! Scope: one connected server. Mirrors `createClient` (baseline index.ts:75-81) plus the SDK
//! `Client.connect/_initialize` behavior used by opencode.
//! Uses:
//! rust/crates/
//! |-- integration-adapters/src/mcp/jsonrpc.rs
//! `-- integration-adapters/src/mcp/transport.rs
//! External:
//! |-- mpsc::Receiver
//! |-- serde_json::Value
//! |-- serde_json::json
//! |-- str::to_string
//! |-- tokio::sync::mpsc
//! |-- tokio::sync::watch
//! |-- tokio_util::sync::CancellationToken
//! |-- tracing::debug
//! |-- tracing::error
//! |-- tracing::info
//! |-- tracing::warn
//! |-- url::Url::from_file_path
//! |-- watch::Receiver
//! `-- watch::channel
//! Used by:
//! rust/crates/
//! |-- integration-adapters/src/mcp/catalog.rs
//! |-- integration-adapters/src/mcp/connect.rs
//! |-- integration-adapters/src/mcp/mod.rs
//! |-- integration-adapters/src/mcp/service.rs
//! |-- integration-adapters/src/mcp/service_api.rs
//! `-- integration-adapters/src/mcp/service_auth.rs
//! Invariants: roots/list answers exactly one root (file URL of the Location directory); ping
//! answers `{}`; a failed handshake closes the transport; list_changed signals coalesce.
//! Contract tests: integration-adapters/tests/mcp_stdio.rs

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

use super::jsonrpc::{RequestOptions, RpcError, RpcPeer};
use super::transport::{Inbound, Transport};

pub const LATEST_PROTOCOL_VERSION: &str = "2025-11-25";
pub const SUPPORTED_PROTOCOL_VERSIONS: [&str; 5] = [
    "2025-11-25",
    "2025-06-18",
    "2025-03-26",
    "2024-11-05",
    "2024-10-07",
];
/// SDK `DEFAULT_REQUEST_TIMEOUT_MSEC`, used when no per-call timeout is configured.
pub const SDK_DEFAULT_REQUEST_TIMEOUT_MS: u64 = 60_000;

pub struct McpClient {
    peer: Arc<RpcPeer>,
    capabilities: Value,
    instructions: Option<String>,
    tools_changed: watch::Receiver<u64>,
    closed: watch::Receiver<Option<String>>,
}

impl McpClient {
    /// Run the handshake over an already started transport. On failure the transport is closed.
    pub async fn connect(
        transport: Arc<dyn Transport>,
        inbound: mpsc::Receiver<Inbound>,
        directory: &Path,
    ) -> Result<Self, RpcError> {
        let root = url::Url::from_file_path(directory)
            .map(|u| u.to_string())
            .unwrap_or_else(|_| format!("file://{}", directory.display()));
        let (changed_tx, changed_rx) = watch::channel(0u64);
        let (closed_tx, closed_rx) = watch::channel(None);
        let on_request = Arc::new(move |method: &str, _params: &Value| match method {
            "roots/list" => Some(json!({"roots": [{"uri": root}]})),
            "ping" => Some(json!({})),
            _ => None,
        });
        let on_notification =
            Arc::new(move |method: String, params: Value| match method.as_str() {
                "notifications/tools/list_changed" => changed_tx.send_modify(|n| *n += 1),
                "notifications/message" => server_log(&params),
                _ => {}
            });
        let on_close = Box::new(move |reason: String| {
            let _ = closed_tx.send(Some(reason));
        });
        let peer = RpcPeer::start(transport, inbound, on_request, on_notification, on_close);
        match initialize(&peer).await {
            Ok((capabilities, instructions)) => Ok(Self {
                peer,
                capabilities,
                instructions,
                tools_changed: changed_rx,
                closed: closed_rx,
            }),
            Err(e) => {
                peer.close().await;
                Err(e)
            }
        }
    }

    pub fn capabilities(&self) -> &Value {
        &self.capabilities
    }

    pub fn has_capability(&self, name: &str) -> bool {
        self.capabilities.get(name).is_some_and(|v| !v.is_null())
    }

    /// Trimmed server instructions (baseline `getInstructions()?.trim()`).
    pub fn instructions(&self) -> Option<String> {
        self.instructions.as_deref().map(|s| s.trim().to_string())
    }

    pub fn pid(&self) -> Option<u32> {
        self.peer.transport().pid()
    }

    pub fn tools_changed(&self) -> watch::Receiver<u64> {
        self.tools_changed.clone()
    }

    pub fn closed(&self) -> watch::Receiver<Option<String>> {
        self.closed.clone()
    }

    pub async fn request(
        &self,
        method: &str,
        params: Option<Value>,
        timeout_ms: Option<u64>,
    ) -> Result<Value, RpcError> {
        self.peer
            .request(method, params, options(timeout_ms, false, None))
            .await
    }

    /// `tools/call` with progress-reset timeout (baseline catalog.ts:54-67).
    pub async fn call_tool(
        &self,
        name: &str,
        arguments: Value,
        timeout_ms: Option<u64>,
        cancel: Option<CancellationToken>,
    ) -> Result<Value, RpcError> {
        self.peer
            .request(
                "tools/call",
                Some(json!({"name": name, "arguments": arguments})),
                options(timeout_ms, true, cancel),
            )
            .await
    }

    pub async fn close(&self) {
        self.peer.close().await;
    }
}

fn options(
    timeout_ms: Option<u64>,
    reset: bool,
    cancel: Option<CancellationToken>,
) -> RequestOptions {
    RequestOptions {
        timeout: Duration::from_millis(timeout_ms.unwrap_or(SDK_DEFAULT_REQUEST_TIMEOUT_MS)),
        reset_on_progress: reset,
        cancel,
    }
}

async fn initialize(peer: &RpcPeer) -> Result<(Value, Option<String>), RpcError> {
    let result = peer
        .request(
            "initialize",
            Some(json!({
                "protocolVersion": LATEST_PROTOCOL_VERSION,
                "capabilities": {"roots": {}},
                "clientInfo": {"name": "opencode", "version": env!("CARGO_PKG_VERSION")},
            })),
            options(None, false, None),
        )
        .await?;
    let version = result
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if !SUPPORTED_PROTOCOL_VERSIONS.contains(&version.as_str()) {
        return Err(RpcError::Other(format!(
            "Server's protocol version is not supported: {version}"
        )));
    }
    peer.transport().set_protocol_version(&version);
    let capabilities = result.get("capabilities").cloned().unwrap_or(json!({}));
    let instructions = result
        .get("instructions")
        .and_then(Value::as_str)
        .map(str::to_string);
    peer.notify("notifications/initialized", None).await?;
    Ok((capabilities, instructions))
}

/// Baseline `serverLog` (index.ts:474-490): route MCP logging notifications by level.
fn server_log(params: &Value) {
    let level = params
        .get("level")
        .and_then(Value::as_str)
        .unwrap_or("info");
    let logger = params.get("logger").and_then(Value::as_str).unwrap_or("");
    let data = params.get("data").cloned().unwrap_or(Value::Null);
    match level {
        "debug" => tracing::debug!(logger, %data, "MCP server log"),
        "info" | "notice" => tracing::info!(logger, %data, "MCP server log"),
        "warning" => tracing::warn!(logger, %data, "MCP server log"),
        _ => tracing::error!(logger, %data, "MCP server log"),
    }
}
