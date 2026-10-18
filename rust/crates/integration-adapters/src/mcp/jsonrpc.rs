//! Purpose: JSON-RPC 2.0 peer for MCP: request/response correlation, timeouts with progress
//! reset, cancellation notifications, server-to-client requests (roots/list, ping) and
//! notification dispatch. Mirrors the SDK `Protocol` class semantics used by the baseline.
//! Scope: one connection. Owns the reader task, joined on close.
//! Uses:
//! rust/crates/
//! `-- integration-adapters/src/mcp/transport.rs
//! External:
//! |-- serde_json::{Value, json}
//! |-- tokio::sync::{Mutex, mpsc, oneshot}
//! `-- tokio_util::sync::CancellationToken
//! Used by:
//! rust/crates/
//! |-- integration-adapters/src/mcp/client.rs
//! `-- integration-adapters/src/mcp/mod.rs
//! Invariants: every pending request resolves exactly once (result, error, timeout, cancel or
//! "Connection closed"); timeouts send `notifications/cancelled`; unknown server requests get
//! -32601; the close callback runs once.
//! Contract tests: integration-adapters/tests/mcp_stdio.rs

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use super::transport::{Inbound, Transport, TransportError};

pub const REQUEST_TIMEOUT: i64 = -32001;
pub const CONNECTION_CLOSED: i64 = -32000;
pub const METHOD_NOT_FOUND: i64 = -32601;

/// Error from a request. Display matches the SDK `McpError` (`MCP error <code>: <message>`).
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum RpcError {
    #[error("MCP error {code}: {message}")]
    Mcp {
        code: i64,
        message: String,
        data: Option<Value>,
    },
    #[error(transparent)]
    Transport(#[from] TransportError),
    #[error("This operation was aborted")]
    Cancelled,
    /// Plain SDK `Error` (no MCP code), e.g. protocol version or output-schema contract failures.
    #[error("{0}")]
    Other(String),
}

impl RpcError {
    pub fn timeout() -> Self {
        Self::Mcp {
            code: REQUEST_TIMEOUT,
            message: "Request timed out".into(),
            data: None,
        }
    }
    pub fn closed() -> Self {
        Self::Mcp {
            code: CONNECTION_CLOSED,
            message: "Connection closed".into(),
            data: None,
        }
    }
    pub fn is_timeout(&self) -> bool {
        matches!(self, Self::Mcp { code, .. } if *code == REQUEST_TIMEOUT)
    }
}

pub type NotificationHandler = Arc<dyn Fn(String, Value) + Send + Sync>;
pub type RequestHandler = Arc<dyn Fn(&str, &Value) -> Option<Value> + Send + Sync>;
pub type CloseHandler = Box<dyn FnOnce(String) + Send>;

type Pending = HashMap<
    u64,
    (
        oneshot::Sender<Result<Value, RpcError>>,
        Option<mpsc::Sender<()>>,
    ),
>;

pub struct RequestOptions {
    pub timeout: Duration,
    pub reset_on_progress: bool,
    pub cancel: Option<CancellationToken>,
}

pub struct RpcPeer {
    transport: Arc<dyn Transport>,
    next_id: AtomicU64,
    pending: Arc<Mutex<Pending>>,
    closed: Arc<AtomicBool>,
    stop: CancellationToken,
    reader: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl RpcPeer {
    /// Start the reader task. `on_notification` receives every server notification except
    /// progress; `on_request` answers server requests (`None` -> method not found).
    pub fn start(
        transport: Arc<dyn Transport>,
        mut inbound: mpsc::Receiver<Inbound>,
        on_request: RequestHandler,
        on_notification: NotificationHandler,
        on_close: CloseHandler,
    ) -> Arc<Self> {
        let pending: Arc<Mutex<Pending>> = Arc::default();
        let closed = Arc::new(AtomicBool::new(false));
        let stop = CancellationToken::new();
        let peer = Arc::new(Self {
            transport: transport.clone(),
            next_id: AtomicU64::new(0),
            pending: pending.clone(),
            closed: closed.clone(),
            stop: stop.clone(),
            reader: Mutex::new(None),
        });
        let handle = tokio::spawn(async move {
            let reason = loop {
                let msg = tokio::select! {
                    _ = stop.cancelled() => break "closed".to_string(),
                    msg = inbound.recv() => msg,
                };
                match msg {
                    None => break "Connection closed".to_string(),
                    Some(Inbound::Closed(reason)) => break reason,
                    Some(Inbound::Message(value)) => {
                        dispatch(&transport, &pending, &on_request, &on_notification, value).await
                    }
                }
            };
            closed.store(true, Ordering::SeqCst);
            let drained: Vec<_> = pending.lock().await.drain().collect();
            for (_, (tx, _)) in drained {
                let _ = tx.send(Err(RpcError::closed()));
            }
            on_close(reason);
        });
        if let Ok(mut slot) = peer.reader.try_lock() {
            *slot = Some(handle);
        }
        peer
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    pub fn transport(&self) -> &Arc<dyn Transport> {
        &self.transport
    }

    pub async fn notify(&self, method: &str, params: Option<Value>) -> Result<(), RpcError> {
        let mut msg = json!({"jsonrpc": "2.0", "method": method});
        if let Some(p) = params {
            msg["params"] = p;
        }
        Ok(self.transport.send(msg).await?)
    }

    pub async fn request(
        &self,
        method: &str,
        params: Option<Value>,
        opts: RequestOptions,
    ) -> Result<Value, RpcError> {
        if self.is_closed() {
            return Err(TransportError::Closed.into());
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        let (progress_tx, mut progress_rx) = mpsc::channel(16);
        let mut params = params.unwrap_or(Value::Null);
        if opts.reset_on_progress {
            // The SDK only sends a progress token when an onprogress hook is present.
            if !params.is_object() {
                params = json!({});
            }
            params["_meta"] = json!({"progressToken": id});
        }
        self.pending
            .lock()
            .await
            .insert(id, (tx, opts.reset_on_progress.then_some(progress_tx)));
        let mut msg = json!({"jsonrpc": "2.0", "id": id, "method": method});
        if !params.is_null() {
            msg["params"] = params;
        }
        if let Err(e) = self.transport.send(msg).await {
            self.pending.lock().await.remove(&id);
            return Err(e.into());
        }
        let cancel = opts.cancel.unwrap_or_default();
        let mut rx = rx;
        loop {
            let deadline = tokio::time::sleep(opts.timeout);
            tokio::select! {
                res = &mut rx => return res.unwrap_or_else(|_| Err(RpcError::closed())),
                Some(()) = progress_rx.recv() => continue,
                _ = deadline => {
                    self.cancel_request(id, "McpError: MCP error -32001: Request timed out").await;
                    return Err(RpcError::timeout());
                }
                _ = cancel.cancelled() => {
                    self.cancel_request(id, "AbortError: This operation was aborted").await;
                    return Err(RpcError::Cancelled);
                }
            }
        }
    }

    async fn cancel_request(&self, id: u64, reason: &str) {
        self.pending.lock().await.remove(&id);
        let _ = self
            .notify(
                "notifications/cancelled",
                Some(json!({"requestId": id, "reason": reason})),
            )
            .await;
    }

    /// Close the transport and join the reader task.
    pub async fn close(&self) {
        self.transport.close().await;
        self.stop.cancel();
        let handle = self.reader.lock().await.take();
        if let Some(h) = handle {
            let _ = h.await;
        }
    }
}

async fn dispatch(
    transport: &Arc<dyn Transport>,
    pending: &Mutex<Pending>,
    on_request: &RequestHandler,
    on_notification: &NotificationHandler,
    value: Value,
) {
    let method = value
        .get("method")
        .and_then(Value::as_str)
        .map(str::to_string);
    let id = value.get("id").cloned().filter(|v| !v.is_null());
    match (method, id) {
        (Some(method), Some(id)) => {
            let params = value.get("params").cloned().unwrap_or(Value::Null);
            let reply = match on_request(&method, &params) {
                Some(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
                None => json!({"jsonrpc": "2.0", "id": id,
                    "error": {"code": METHOD_NOT_FOUND, "message": "Method not found"}}),
            };
            let _ = transport.send(reply).await;
        }
        (Some(method), None) => {
            let params = value.get("params").cloned().unwrap_or(Value::Null);
            if method == "notifications/progress" {
                let token = params.get("progressToken").and_then(Value::as_u64);
                if let Some(token) = token
                    && let Some((_, Some(p))) = pending.lock().await.get(&token)
                {
                    let _ = p.try_send(());
                }
                return;
            }
            on_notification(method, params);
        }
        (None, Some(id)) => {
            let Some(id) = id
                .as_u64()
                .or_else(|| id.as_str().and_then(|s| s.parse().ok()))
            else {
                return;
            };
            let Some((tx, _)) = pending.lock().await.remove(&id) else {
                return;
            };
            let result = match value.get("error") {
                Some(err) => Err(RpcError::Mcp {
                    code: err.get("code").and_then(Value::as_i64).unwrap_or(-32603),
                    message: err
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    data: err.get("data").cloned(),
                }),
                None => Ok(value.get("result").cloned().unwrap_or(Value::Null)),
            };
            let _ = tx.send(result);
        }
        (None, None) => tracing::warn!(message = %value, "unknown MCP message type"),
    }
}
