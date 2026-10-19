//! Purpose: Streamable HTTP MCP client transport (spec 2025-03-26+), port of the SDK
//! `StreamableHTTPClientTransport` paths the baseline uses: POST with JSON or SSE responses,
//! `mcp-session-id`, `mcp-protocol-version`, optional GET listen stream, OAuth 401/403 retry.
//! Scope: one remote server URL. Response/listen stream tasks are owned and joined on close.
//! Uses:
//! rust/crates/
//! |-- integration-adapters/src/mcp/oauth.rs
//! |-- integration-adapters/src/mcp/remote.rs
//! |-- integration-adapters/src/mcp/sse.rs
//! `-- integration-adapters/src/mcp/transport.rs
//! External:
//! |-- async_trait::async_trait
//! |-- i64::from
//! |-- mpsc::Receiver
//! |-- mpsc::Sender
//! |-- mpsc::channel
//! |-- reqwest::Error
//! |-- reqwest::RequestBuilder
//! |-- reqwest::Response
//! |-- serde_json::Value
//! |-- serde_json::from_str
//! |-- str::to_string
//! |-- tokio::spawn
//! |-- tokio::sync::mpsc
//! |-- tracing::debug
//! `-- tracing::warn
//! Used by:
//! rust/crates/
//! |-- integration-adapters/src/mcp/connect.rs
//! |-- integration-adapters/src/mcp/legacy_sse.rs
//! |-- integration-adapters/src/mcp/mod.rs
//! `-- integration-adapters/src/mcp/service_auth.rs
//! Invariants: error text matches SDK `StreamableHTTPError` ("Streamable HTTP error: ...");
//! one auth attempt per send (circuit breaker on 401 after auth, 403 upscoping once per header);
//! a 405 on the GET listen stream is not an error.
//! Contract tests: integration-adapters/tests/mcp_http.rs

use std::sync::Mutex;

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::mpsc;

use super::remote::{RemoteOptions, Tasks, apply_headers, authorize, lock, record_hints};
use super::sse::pump;
use super::transport::{AuthHints, INBOUND_CAPACITY, Inbound, Transport, TransportError};

pub struct StreamableTransport {
    opts: RemoteOptions,
    tx: mpsc::Sender<Inbound>,
    session_id: Mutex<Option<String>>,
    protocol_version: Mutex<Option<String>>,
    hints: Mutex<AuthHints>,
    tasks: Tasks,
}

/// SDK `StreamableHTTPError(code, message)`; the code is not part of the message text.
fn http_error(_status: i64, message: &str) -> TransportError {
    TransportError::Failed(format!("Streamable HTTP error: {message}"))
}

impl StreamableTransport {
    pub fn new(opts: RemoteOptions) -> (Self, mpsc::Receiver<Inbound>) {
        let (tx, rx) = mpsc::channel(INBOUND_CAPACITY);
        let t = Self {
            opts,
            tx,
            session_id: Mutex::new(None),
            protocol_version: Mutex::new(None),
            hints: Mutex::default(),
            tasks: Tasks::default(),
        };
        (t, rx)
    }

    async fn headers(&self, b: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let session = lock(&self.session_id).clone();
        let version = lock(&self.protocol_version).clone();
        apply_headers(
            b,
            &self.opts,
            &[
                ("mcp-session-id", session),
                ("mcp-protocol-version", version),
            ],
        )
        .await
    }

    /// `finishAuth(code)`: exchange the authorization code using the hints learned earlier.
    pub async fn finish_auth(&self, code: &str) -> Result<(), TransportError> {
        authorize(&self.opts, &self.hints, Some(code)).await
    }

    fn spawn_stream(&self, response: reqwest::Response) {
        let tx = self.tx.clone();
        self.tasks.push(tokio::spawn(async move {
            let _ = pump(response.bytes_stream(), |ev| {
                let tx = tx.clone();
                async move {
                    if ev.event != "message" {
                        return true;
                    }
                    match serde_json::from_str::<Value>(&ev.data) {
                        Ok(v) => tx.send(Inbound::Message(v)).await.is_ok(),
                        Err(e) => {
                            tracing::warn!(error = %e, "MCP streamable: invalid SSE message");
                            true
                        }
                    }
                }
            })
            .await;
        }));
    }

    /// Optional GET listen stream started after `notifications/initialized` was accepted.
    async fn start_listen(&self) -> Result<(), TransportError> {
        let b = self.opts.http.get(self.opts.url.clone());
        let b = self.headers(b).await.header("accept", "text/event-stream");
        let r = b
            .send()
            .await
            .map_err(|e| TransportError::Failed(e.to_string()))?;
        if r.status().is_success() {
            self.spawn_stream(r);
            return Ok(());
        }
        if r.status() == 401 && self.opts.provider.is_some() {
            return authorize(&self.opts, &self.hints, None).await;
        }
        if r.status() == 405 {
            return Ok(());
        }
        let status = r.status();
        Err(http_error(
            i64::from(status.as_u16()),
            &format!(
                "Failed to open SSE stream: {}",
                status.canonical_reason().unwrap_or("")
            ),
        ))
    }

    async fn post(
        &self,
        message: &Value,
        auth_done: bool,
        upscoped: Option<String>,
    ) -> Result<(), TransportError> {
        let b = self.opts.http.post(self.opts.url.clone());
        let b = self
            .headers(b)
            .await
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .body(message.to_string());
        let r = b
            .send()
            .await
            .map_err(|e| TransportError::Failed(fetch_error(&e)))?;
        if let Some(sid) = r
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
        {
            *lock(&self.session_id) = Some(sid.to_string());
        }
        let status = r.status();
        if !status.is_success() {
            let www = header(&r, "www-authenticate");
            let text = r.text().await.unwrap_or_default();
            if status == 401 && self.opts.provider.is_some() {
                if auth_done {
                    return Err(http_error(
                        401,
                        "Server returned 401 after successful authentication",
                    ));
                }
                record_hints(&self.hints, www.as_deref());
                authorize(&self.opts, &self.hints, None).await?;
                return Box::pin(self.post(message, true, upscoped)).await;
            }
            if status == 403 && self.opts.provider.is_some() {
                let error = {
                    let (rm, scope, error) = super::oauth::www_authenticate_params(www.as_deref());
                    let mut h = lock(&self.hints);
                    if let Some(s) = scope {
                        h.scope = Some(s);
                    }
                    if let Some(u) = rm {
                        h.resource_metadata_url = Some(u);
                    }
                    error
                };
                if error.as_deref() == Some("insufficient_scope") {
                    if upscoped.is_some() && upscoped == www {
                        return Err(http_error(
                            403,
                            "Server returned 403 after trying upscoping",
                        ));
                    }
                    authorize(&self.opts, &self.hints, None).await?;
                    return Box::pin(self.post(message, auth_done, www)).await;
                }
            }
            return Err(http_error(
                i64::from(status.as_u16()),
                &format!("Error POSTing to endpoint: {text}"),
            ));
        }
        if status == 202 {
            if message.get("method").and_then(Value::as_str) == Some("notifications/initialized")
                && let Err(e) = self.start_listen().await
            {
                tracing::debug!(error = %e, "MCP streamable: listen stream unavailable");
            }
            return Ok(());
        }
        let has_request =
            message.get("method").is_some() && message.get("id").is_some_and(|i| !i.is_null());
        if !has_request {
            return Ok(());
        }
        let ctype = header(&r, "content-type").unwrap_or_default();
        if ctype.contains("text/event-stream") {
            self.spawn_stream(r);
            return Ok(());
        }
        if ctype.contains("application/json") {
            let data: Value = r
                .json()
                .await
                .map_err(|e| TransportError::Failed(e.to_string()))?;
            let msgs = match data {
                Value::Array(a) => a,
                v => vec![v],
            };
            for m in msgs {
                let _ = self.tx.send(Inbound::Message(m)).await;
            }
            return Ok(());
        }
        Err(http_error(-1, &format!("Unexpected content type: {ctype}")))
    }
}

fn header(r: &reqwest::Response, name: &str) -> Option<String> {
    r.headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

/// Node `fetch` failures surface as `fetch failed`; keep the cause for diagnostics.
pub fn fetch_error(e: &reqwest::Error) -> String {
    if e.is_connect() {
        return format!("fetch failed: {e}");
    }
    e.to_string()
}

#[async_trait]
impl Transport for StreamableTransport {
    async fn send(&self, message: Value) -> Result<(), TransportError> {
        self.post(&message, false, None).await
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
