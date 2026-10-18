//! Purpose: the narrow opencode session port consumed by GitHub run orchestration and the
//! Slack bot: create, share, completed prompt, and part-update event stream. Provides an
//! HTTP adapter over the legacy server routes used by the baseline SDK
//! (`POST /session`, `POST /session/{id}/share`, `POST /session/{id}/message`, `GET /event`).
//! Does not execute sessions, and does not own share URLs (phase 13) or the public client
//! (phase 08). The in-process composition root (phase 09/91) may implement the trait directly.
//! Scope: per-run client handle, cheap to clone.
//! Uses:
//! `-- none
//! External:
//! |-- async_trait::async_trait
//! |-- futures_util::StreamExt
//! |-- reqwest::{Client, StatusCode}
//! |-- serde::{Deserialize, Serialize}
//! |-- serde_json::Value
//! |-- tokio::sync::mpsc
//! `-- tokio_util::sync::CancellationToken
//! Used by:
//! rust/crates/
//! |-- ecosystem/src/lib.rs
//! `-- ecosystem/tests/session_http.rs
//! Invariants: prompt returns only after the producer completed the turn (legacy
//! `/session/{id}/message` semantics), never on V2 admission. Event stream is bounded and
//! stops on cancellation. Wire field names match the baseline SDK JSON exactly.
//! Contract tests: ecosystem/tests/session_http.rs

use async_trait::async_trait;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Errors from the session port. Variants keep the distinctions the callers branch on.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("session request failed with status {status}: {body}")]
    Status { status: u16, body: String },
    #[error("session transport error: {0}")]
    Transport(String),
    #[error("session response decode error: {0}")]
    Decode(String),
    #[error("session operation cancelled")]
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share: Option<ShareInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShareInfo {
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelRef {
    #[serde(rename = "providerID")]
    pub provider_id: String,
    #[serde(rename = "modelID")]
    pub model_id: String,
}

/// Body of `POST /session/{id}/message`. Parts stay raw JSON so text/file/agent parts keep
/// the baseline shape byte for byte.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct PromptRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    pub parts: Vec<Value>,
}

impl PromptRequest {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            parts: vec![serde_json::json!({ "type": "text", "text": text.into() })],
            ..Self::default()
        }
    }
}

/// Completed assistant turn: `{ info, parts }`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PromptResponse {
    #[serde(default)]
    pub info: Value,
    #[serde(default)]
    pub parts: Vec<Value>,
}

impl PromptResponse {
    /// Last text part, the baseline's `parts.findLast((p) => p.type === "text")`.
    pub fn last_text(&self) -> Option<&str> {
        self.parts
            .iter()
            .rev()
            .find(|p| p.get("type").and_then(Value::as_str) == Some("text"))
            .and_then(|p| p.get("text").and_then(Value::as_str))
    }

    /// All text parts joined by newlines, the Slack bot's projection.
    pub fn joined_text(&self) -> String {
        self.parts
            .iter()
            .filter(|p| p.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// One bus event `{ type, properties }` from `GET /event`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SessionEvent {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub properties: Value,
}

#[async_trait]
pub trait SessionPort: Send + Sync {
    async fn create(&self, title: Option<&str>) -> Result<SessionInfo, SessionError>;
    async fn share(&self, session_id: &str) -> Result<SessionInfo, SessionError>;
    async fn prompt(
        &self,
        session_id: &str,
        request: &PromptRequest,
    ) -> Result<PromptResponse, SessionError>;
    /// Subscribe to the event stream. Events are delivered into a bounded channel. When the
    /// receiver lags the producer awaits (backpressure). Cancelling `cancel` ends the stream.
    async fn subscribe(
        &self,
        cancel: CancellationToken,
    ) -> Result<mpsc::Receiver<SessionEvent>, SessionError>;
}

/// HTTP adapter for the legacy opencode server routes.
#[derive(Debug, Clone)]
pub struct HttpSessionPort {
    base: String,
    http: reqwest::Client,
    directory: Option<String>,
}

impl HttpSessionPort {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base: base_url.into().trim_end_matches('/').to_string(),
            http: reqwest::Client::new(),
            directory: None,
        }
    }

    pub fn with_directory(mut self, directory: impl Into<String>) -> Self {
        self.directory = Some(directory.into());
        self
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let builder = self.http.request(method, format!("{}{}", self.base, path));
        match &self.directory {
            Some(dir) => builder.query(&[("directory", dir)]),
            None => builder,
        }
    }
}

async fn decode<T: serde::de::DeserializeOwned>(
    response: Result<reqwest::Response, reqwest::Error>,
) -> Result<T, SessionError> {
    let response = response.map_err(|e| SessionError::Transport(e.to_string()))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| SessionError::Transport(e.to_string()))?;
    if !status.is_success() {
        return Err(SessionError::Status {
            status: status.as_u16(),
            body,
        });
    }
    serde_json::from_str(&body).map_err(|e| SessionError::Decode(e.to_string()))
}

#[async_trait]
impl SessionPort for HttpSessionPort {
    async fn create(&self, title: Option<&str>) -> Result<SessionInfo, SessionError> {
        let body = match title {
            Some(title) => serde_json::json!({ "title": title }),
            None => serde_json::json!({}),
        };
        decode(
            self.request(reqwest::Method::POST, "/session")
                .json(&body)
                .send()
                .await,
        )
        .await
    }

    async fn share(&self, session_id: &str) -> Result<SessionInfo, SessionError> {
        decode(
            self.request(
                reqwest::Method::POST,
                &format!("/session/{session_id}/share"),
            )
            .send()
            .await,
        )
        .await
    }

    async fn prompt(
        &self,
        session_id: &str,
        request: &PromptRequest,
    ) -> Result<PromptResponse, SessionError> {
        decode(
            self.request(
                reqwest::Method::POST,
                &format!("/session/{session_id}/message"),
            )
            .json(request)
            .send()
            .await,
        )
        .await
    }

    async fn subscribe(
        &self,
        cancel: CancellationToken,
    ) -> Result<mpsc::Receiver<SessionEvent>, SessionError> {
        let response = self
            .request(reqwest::Method::GET, "/event")
            .header("accept", "text/event-stream")
            .send()
            .await
            .map_err(|e| SessionError::Transport(e.to_string()))?;
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            return Err(SessionError::Status { status, body });
        }
        let (tx, rx) = mpsc::channel(256);
        let mut stream = response.bytes_stream();
        // The reader task is owned by the channel: it exits when the receiver drops, the
        // stream ends, or `cancel` fires.
        tokio::spawn(async move {
            let mut buffer = String::new();
            loop {
                let chunk = tokio::select! {
                    _ = cancel.cancelled() => return,
                    chunk = stream.next() => chunk,
                };
                let Some(Ok(chunk)) = chunk else { return };
                buffer.push_str(&String::from_utf8_lossy(&chunk));
                while let Some(end) = buffer.find("\n\n") {
                    let frame: String = buffer.drain(..end + 2).collect();
                    let data: String = frame
                        .lines()
                        .filter_map(|line| line.strip_prefix("data:"))
                        .map(str::trim_start)
                        .collect::<Vec<_>>()
                        .join("\n");
                    if data.is_empty() {
                        continue;
                    }
                    let Ok(event) = serde_json::from_str::<SessionEvent>(&data) else {
                        continue;
                    };
                    if tx.send(event).await.is_err() {
                        return;
                    }
                }
            }
        });
        Ok(rx)
    }
}
