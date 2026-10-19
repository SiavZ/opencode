//! Purpose: LSP JSON-RPC 2.0 over stdio with `Content-Length` framing: request/response
//! correlation, server-to-client requests and notifications. Mirrors vscode-jsonrpc as used by
//! packages/opencode/src/lsp/client.ts:132-207.
//! Scope: transport only. Knows nothing about LSP methods.
//! Uses:
//! rust/crates/
//! `-- integration-api/src/lib.rs
//! External:
//! |-- oneshot::Sender
//! |-- oneshot::channel
//! |-- serde_json::Value
//! |-- serde_json::from_slice
//! |-- serde_json::json
//! |-- serde_json::to_vec
//! |-- tokio::io::AsyncBufRead
//! |-- tokio::io::AsyncBufReadExt
//! |-- tokio::io::AsyncReadExt
//! |-- tokio::io::AsyncWrite
//! |-- tokio::io::AsyncWriteExt
//! |-- tokio::spawn
//! |-- tokio::sync::Mutex
//! |-- tokio::sync::oneshot
//! |-- tokio::task::JoinHandle
//! |-- tokio::time::timeout
//! `-- tracing::warn
//! Used by:
//! rust/crates/
//! |-- integration-adapters/src/lsp/client.rs
//! |-- integration-adapters/src/lsp/client/server_handler.rs
//! |-- integration-adapters/src/lsp/mod.rs
//! `-- integration-adapters/tests/lsp_jsonrpc.rs
//! Invariants: one frame per write under the writer lock, ids start at 0 and increase, unknown
//! server requests answer -32601, every pending request resolves (Closed) when the reader ends.
//! Contract tests: integration-adapters/tests/lsp_jsonrpc.rs

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use opencode_integration_api::IntegrationError;
use serde_json::{Value, json};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{Mutex, oneshot};
use tokio::task::JoinHandle;

type Writer = Pin<Box<dyn AsyncWrite + Send>>;
type Pending =
    Arc<std::sync::Mutex<Option<HashMap<i64, oneshot::Sender<Result<Value, RpcError>>>>>>;

/// JSON-RPC error object returned by the peer.
#[derive(Debug, Clone, PartialEq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

/// Inbound traffic from the server. Called inline on the reader task, so it must not block.
pub trait Handler: Send + Sync + 'static {
    /// Answer a server-to-client request. `None` means method not found (-32601).
    fn request(&self, method: &str, params: &Value) -> Option<Value>;
    fn notification(&self, method: &str, params: &Value);
}

/// Read one `Content-Length` framed message. `Ok(None)` on clean EOF.
pub async fn read_frame<R: AsyncBufRead + Unpin>(
    reader: &mut R,
) -> std::io::Result<Option<Vec<u8>>> {
    let mut length: Option<usize> = None;
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).await? == 0 {
            return Ok(None);
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            if length.is_some() {
                break;
            }
            continue;
        }
        let Some((name, value)) = trimmed.split_once(':') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case("content-length") {
            length = value.trim().parse().ok();
        }
    }
    let mut body = vec![0u8; length.unwrap_or(0)];
    reader.read_exact(&mut body).await?;
    Ok(Some(body))
}

pub fn encode_frame(body: &[u8]) -> Vec<u8> {
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend_from_slice(body);
    out
}

/// One JSON-RPC connection. The reader task is owned and joined by `close`.
pub struct Connection {
    writer: Arc<Mutex<Option<Writer>>>,
    pending: Pending,
    next_id: AtomicI64,
    reader: std::sync::Mutex<Option<JoinHandle<()>>>,
}

impl Connection {
    pub fn start<R, W>(reader: R, writer: W, handler: Arc<dyn Handler>) -> Arc<Self>
    where
        R: AsyncBufRead + Unpin + Send + 'static,
        W: AsyncWrite + Send + 'static,
    {
        let writer: Arc<Mutex<Option<Writer>>> = Arc::new(Mutex::new(Some(Box::pin(writer))));
        let pending: Pending = Arc::new(std::sync::Mutex::new(Some(HashMap::new())));
        let task = tokio::spawn(read_loop(reader, writer.clone(), pending.clone(), handler));
        Arc::new(Self {
            writer,
            pending,
            next_id: AtomicI64::new(0),
            reader: std::sync::Mutex::new(Some(task)),
        })
    }

    pub async fn notify(&self, method: &str, params: Value) -> Result<(), IntegrationError> {
        let msg = json!({"jsonrpc": "2.0", "method": method, "params": params});
        write_message(&self.writer, &msg).await
    }

    /// Send a request and wait for its response. `timeout` None waits until the peer answers or
    /// the connection closes (baseline requests other than initialize/diagnostics have no timeout).
    pub async fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Option<Duration>,
    ) -> Result<Value, IntegrationError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        {
            let mut guard = self.pending.lock().unwrap_or_else(|e| e.into_inner());
            let Some(map) = guard.as_mut() else {
                return Err(IntegrationError::Closed("lsp connection closed".into()));
            };
            map.insert(id, tx);
        }
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        if let Err(err) = write_message(&self.writer, &msg).await {
            self.forget(id);
            return Err(err);
        }
        let outcome = match timeout {
            Some(limit) => match tokio::time::timeout(limit, rx).await {
                Ok(v) => v,
                Err(_) => {
                    self.forget(id);
                    return Err(IntegrationError::Timeout {
                        ms: limit.as_millis() as u64,
                    });
                }
            },
            None => rx.await,
        };
        match outcome {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(e)) => Err(IntegrationError::Failed {
                name: method.to_string(),
                message: format!("{} ({})", e.message, e.code),
            }),
            Err(_) => Err(IntegrationError::Closed("lsp connection closed".into())),
        }
    }

    fn forget(&self, id: i64) {
        let mut guard = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(map) = guard.as_mut() {
            map.remove(&id);
        }
    }

    /// End the connection: drop the writer (closes the server's stdin) and join the reader once
    /// the stream ends. Callers kill the process first so the reader observes EOF.
    pub async fn close(&self) {
        let writer = self.writer.lock().await.take();
        if let Some(mut w) = writer {
            let _ = w.shutdown().await;
        }
        let task = self.reader.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(task) = task {
            let _ = task.await;
        }
    }
}

async fn write_message(
    writer: &Mutex<Option<Writer>>,
    msg: &Value,
) -> Result<(), IntegrationError> {
    let body = serde_json::to_vec(msg).map_err(|e| IntegrationError::Validation(e.to_string()))?;
    let mut guard = writer.lock().await;
    let Some(w) = guard.as_mut() else {
        return Err(IntegrationError::Closed("lsp connection closed".into()));
    };
    let frame = encode_frame(&body);
    let result = async {
        w.write_all(&frame).await?;
        w.flush().await
    }
    .await;
    result.map_err(|e| IntegrationError::Closed(e.to_string()))
}

async fn read_loop<R: AsyncBufRead + Unpin>(
    mut reader: R,
    writer: Arc<Mutex<Option<Writer>>>,
    pending: Pending,
    handler: Arc<dyn Handler>,
) {
    while let Ok(Some(body)) = read_frame(&mut reader).await {
        let Ok(msg) = serde_json::from_slice::<Value>(&body) else {
            tracing::warn!("lsp: dropping malformed frame");
            continue;
        };
        dispatch(msg, &writer, &pending, handler.as_ref()).await;
    }
    let drained = pending.lock().unwrap_or_else(|e| e.into_inner()).take();
    drop(drained);
}

async fn dispatch(
    msg: Value,
    writer: &Mutex<Option<Writer>>,
    pending: &Pending,
    handler: &dyn Handler,
) {
    let method = msg.get("method").and_then(Value::as_str);
    let id = msg.get("id").cloned().filter(|v| !v.is_null());
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    match (method, id) {
        (Some(method), Some(id)) => {
            let reply = match handler.request(method, &params) {
                Some(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
                None => json!({"jsonrpc": "2.0", "id": id, "error": {
                    "code": -32601, "message": format!("Unhandled method {method}")
                }}),
            };
            let _ = write_message(writer, &reply).await;
        }
        (Some(method), None) => handler.notification(method, &params),
        (None, Some(id)) => {
            let Some(id) = id.as_i64() else { return };
            let tx = {
                let mut guard = pending.lock().unwrap_or_else(|e| e.into_inner());
                guard.as_mut().and_then(|m| m.remove(&id))
            };
            let Some(tx) = tx else { return };
            let outcome = match msg.get("error") {
                Some(err) => Err(RpcError {
                    code: err.get("code").and_then(Value::as_i64).unwrap_or(0),
                    message: err
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                }),
                None => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
            };
            let _ = tx.send(outcome);
        }
        (None, None) => {}
    }
}
