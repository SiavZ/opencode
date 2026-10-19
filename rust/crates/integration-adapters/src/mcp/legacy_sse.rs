//! Purpose: legacy HTTP+SSE MCP client transport (spec 2024-11-05), Rust-owned because rmcp
//! dropped it: GET event stream, wait for the `endpoint` event, POST messages to it, OAuth retry.
//! Scope: one remote server URL. The event-stream reader task is owned and joined on close.
//! Uses:
//! rust/crates/
//! |-- integration-adapters/src/mcp/remote.rs
//! |-- integration-adapters/src/mcp/sse.rs
//! |-- integration-adapters/src/mcp/streamable.rs
//! `-- integration-adapters/src/mcp/transport.rs
//! External:
//! |-- async_trait::async_trait
//! |-- mpsc::Receiver
//! |-- mpsc::Sender
//! |-- mpsc::channel
//! |-- oneshot::channel
//! |-- reqwest::RequestBuilder
//! |-- serde_json::Value
//! |-- serde_json::from_str
//! |-- str::to_string
//! |-- tokio::spawn
//! |-- tokio::sync::mpsc
//! |-- tokio::sync::oneshot
//! |-- tracing::warn
//! `-- url::Url
//! Used by:
//! rust/crates/
//! |-- integration-adapters/src/mcp/connect.rs
//! `-- integration-adapters/src/mcp/mod.rs
//! Invariants: the endpoint must share the connection origin; start fails with SDK-shaped
//! messages ("SSE error: Non-200 status code (N)"); POST failures read
//! "Error POSTing to endpoint (HTTP N): body"; the stream ending closes the transport.
//! Contract tests: integration-adapters/tests/mcp_http.rs

use std::sync::Mutex;

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};

use super::remote::{RemoteOptions, Tasks, apply_headers, authorize, lock, record_hints};
use super::sse::pump;
use super::streamable::fetch_error;
use super::transport::{AuthHints, INBOUND_CAPACITY, Inbound, Transport, TransportError};

pub struct LegacySseTransport {
    opts: RemoteOptions,
    tx: mpsc::Sender<Inbound>,
    endpoint: Mutex<Option<url::Url>>,
    protocol_version: Mutex<Option<String>>,
    hints: Mutex<AuthHints>,
    tasks: Tasks,
}

impl LegacySseTransport {
    pub fn new(opts: RemoteOptions) -> (Self, mpsc::Receiver<Inbound>) {
        let (tx, rx) = mpsc::channel(INBOUND_CAPACITY);
        let t = Self {
            opts,
            tx,
            endpoint: Mutex::new(None),
            protocol_version: Mutex::new(None),
            hints: Mutex::default(),
            tasks: Tasks::default(),
        };
        (t, rx)
    }

    pub async fn finish_auth(&self, code: &str) -> Result<(), TransportError> {
        authorize(&self.opts, &self.hints, Some(code)).await
    }

    async fn headers(&self, b: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let version = lock(&self.protocol_version).clone();
        apply_headers(b, &self.opts, &[("mcp-protocol-version", version)]).await
    }

    /// Open the event stream and wait for the endpoint event (SDK `_startOrAuth`).
    pub async fn start(&self) -> Result<(), TransportError> {
        self.start_inner(false).await
    }

    async fn start_inner(&self, after_auth: bool) -> Result<(), TransportError> {
        let b = self.opts.http.get(self.opts.url.clone());
        let b = self.headers(b).await.header("accept", "text/event-stream");
        let r = b
            .send()
            .await
            .map_err(|e| TransportError::Failed(format!("SSE error: {}", fetch_error(&e))))?;
        let status = r.status();
        if status == 401 && self.opts.provider.is_some() && !after_auth {
            let www = r
                .headers()
                .get("www-authenticate")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string);
            if www.is_some() {
                record_hints(&self.hints, www.as_deref());
            }
            authorize(&self.opts, &self.hints, None).await?;
            return Box::pin(self.start_inner(true)).await;
        }
        if !status.is_success() {
            return Err(TransportError::Failed(format!(
                "SSE error: Non-200 status code ({})",
                status.as_u16()
            )));
        }
        let (ready_tx, ready_rx) = oneshot::channel::<Result<url::Url, String>>();
        let tx = self.tx.clone();
        let base = self.opts.url.clone();
        self.tasks.push(tokio::spawn(async move {
            let mut ready = Some(ready_tx);
            let result = pump(r.bytes_stream(), |ev| {
                let tx = tx.clone();
                let ready_now = if ev.event == "endpoint" {
                    ready.take()
                } else {
                    None
                };
                let base = base.clone();
                async move {
                    if ev.event == "endpoint" {
                        let outcome = match base.join(&ev.data) {
                            Ok(u) if u.origin() == base.origin() => Ok(u),
                            Ok(u) => Err(format!(
                                "Endpoint origin does not match connection origin: {}",
                                u.origin().ascii_serialization()
                            )),
                            Err(e) => Err(e.to_string()),
                        };
                        let ok = outcome.is_ok();
                        if let Some(r) = ready_now {
                            let _ = r.send(outcome);
                        }
                        return ok;
                    }
                    if ev.event != "message" {
                        return true;
                    }
                    match serde_json::from_str::<Value>(&ev.data) {
                        Ok(v) => tx.send(Inbound::Message(v)).await.is_ok(),
                        Err(e) => {
                            tracing::warn!(error = %e, "MCP SSE: invalid message");
                            true
                        }
                    }
                }
            })
            .await;
            if let Some(r) = ready.take() {
                let _ = r.send(Err("SSE error: stream ended before endpoint event".into()));
            }
            let reason = result.err().unwrap_or_else(|| "Connection closed".into());
            let _ = tx.send(Inbound::Closed(reason)).await;
        }));
        match ready_rx.await {
            Ok(Ok(url)) => {
                *lock(&self.endpoint) = Some(url);
                Ok(())
            }
            Ok(Err(e)) => Err(TransportError::Failed(e)),
            Err(_) => Err(TransportError::Failed("SSE error: stream closed".into())),
        }
    }

    async fn post(&self, message: &Value, retried: bool) -> Result<(), TransportError> {
        let Some(endpoint) = lock(&self.endpoint).clone() else {
            return Err(TransportError::Failed("Not connected".into()));
        };
        let b = self.opts.http.post(endpoint);
        let b = self
            .headers(b)
            .await
            .header("content-type", "application/json")
            .body(message.to_string());
        let r = b
            .send()
            .await
            .map_err(|e| TransportError::Failed(fetch_error(&e)))?;
        let status = r.status();
        if status.is_success() {
            return Ok(());
        }
        let www = r
            .headers()
            .get("www-authenticate")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let text = r.text().await.unwrap_or_default();
        if status == 401 && self.opts.provider.is_some() && !retried {
            record_hints(&self.hints, www.as_deref());
            authorize(&self.opts, &self.hints, None).await?;
            return Box::pin(self.post(message, true)).await;
        }
        Err(TransportError::Failed(format!(
            "Error POSTing to endpoint (HTTP {}): {text}",
            status.as_u16()
        )))
    }
}

#[async_trait]
impl Transport for LegacySseTransport {
    async fn send(&self, message: Value) -> Result<(), TransportError> {
        self.post(&message, false).await
    }

    async fn close(&self) {
        self.tasks.join_all().await;
        let _ = self.tx.try_send(Inbound::Closed("closed".into()));
    }

    fn set_protocol_version(&self, version: &str) {
        *lock(&self.protocol_version) = Some(version.to_string());
    }

    fn auth_hints(&self) -> AuthHints {
        lock(&self.hints).clone()
    }
}
