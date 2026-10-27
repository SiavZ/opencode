//! Purpose: return provider JSON and SSE responses with native usage accounting.
//! Scope: response metadata, usage inspection, completion-only billing and cancellation ownership.
//! Uses:
//! rust/bins/
//! `-- hosted/src/console/zen/legacy.rs
//! rust/crates/
//! |-- hosted-adapters/src/console/formats.rs
//! |-- hosted-adapters/src/console/gateway/usage.rs
//! `-- hosted-adapters/src/console/limits.rs
//! External:
//! |-- axum::body::Body
//! |-- futures::StreamExt
//! `-- serde_json::Value
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/zen/legacy.rs
//! Invariants: dropping downstream drops upstream immediately; no detached task or partial completion charge.
//! Contract tests: hosted/src/console/zen/tests.rs

use super::{legacy::internal, proxy::usage_error};
use axum::{
    body::{Body, Bytes},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use futures::StreamExt;
use opencode_hosted_adapters::console::{
    formats::{Format, ProviderHelper, UsageInfo, UsageParser, build_cost_chunk},
    gateway::usage::{BillingContext, UsageService},
    limits::{RequestLimit, provider::ProviderBudget, store::ProviderCounters},
};
use serde_json::Value;

pub struct Completion {
    pub service: UsageService,
    pub trace: super::metrics::GatewayTrace,
    pub context: BillingContext,
    pub model: Value,
    pub model_id: String,
    pub provider: Value,
    pub provider_id: String,
    pub session: String,
    pub helper: ProviderHelper,
    pub format: Format,
    pub request_limit: Option<RequestLimit>,
    pub counters: ProviderCounters,
    pub trial: bool,
    pub ip: String,
    pub budget: Option<ProviderBudget>,
}

impl Completion {
    async fn track(
        &mut self,
        usage: &UsageInfo,
        first: i64,
        last: i64,
    ) -> Result<String, Box<Response>> {
        self.counters
            .track(&self.provider, &self.ip, self.trial, usage, first, last)
            .await
            .map_err(|_| internal())?;
        let result = self
            .service
            .record(
                &self.context,
                &self.session,
                &self.model_id,
                &self.model,
                &self.provider_id,
                usage,
            )
            .await
            .map_err(usage_error)?;
        if let Some(budget) = &self.budget {
            budget
                .track(&self.provider, result.raw_cost_in_cents)
                .await
                .map_err(|_| internal())?;
        }
        self.trace
            .usage(&self.model, usage, &result)
            .map_err(usage_error)?;
        Ok(result.occurred_cost)
    }
}

pub async fn complete(
    response: reqwest::Response,
    mut completion: Completion,
) -> Result<Response, Box<Response>> {
    let status = if response.status() == StatusCode::NOT_FOUND {
        StatusCode::BAD_REQUEST
    } else {
        response.status()
    };
    let mut headers = HeaderMap::new();
    for name in ["content-type", "cache-control"] {
        if let Some(value) = response.headers().get(name) {
            headers.insert(name, value.clone());
        }
    }
    if response.status() != StatusCode::OK {
        completion.trace.metric["llm.error.code"] = serde_json::json!(response.status().as_u16());
        completion.trace.metric["llm.error.message"] =
            serde_json::json!(response.status().canonical_reason().unwrap_or_default());
    }
    let streaming = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|value| value.to_lowercase().contains("text/event-stream"));
    if !streaming || [400, 404, 429, 529].contains(&response.status().as_u16()) {
        let mut json: Value = response.json().await.map_err(|_| internal())?;
        if let Some(limit) = &completion.request_limit {
            limit.track().await.map_err(|_| internal())?;
        }
        if let Some(usage) = completion.helper.extract_usage(&json) {
            let usage = completion.helper.normalize_usage(&usage);
            let cost = completion.track(&usage, 0, 0).await?;
            json["cost"] = Value::String(cost);
        }
        if let Some(message) = json.pointer("/error/message").and_then(Value::as_str) {
            let display = completion.provider["displayName"]
                .as_str()
                .map(|name| format!(" ({name})"))
                .unwrap_or_default();
            json["error"]["message"] =
                Value::String(format!("Error from provider{display}: {message}"));
        }
        let bytes = serde_json::to_vec(&json).map_err(|_| internal())?;
        completion
            .trace
            .finish(status.as_u16(), bytes.len(), 0, false)
            .await;
        let mut output = Response::new(Body::from(bytes));
        *output.status_mut() = status;
        *output.headers_mut() = headers;
        return Ok(output);
    }
    struct StreamState {
        upstream:
            std::pin::Pin<Box<dyn futures::Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
        completion: Completion,
        parser: UsageParser,
        buffer: Vec<u8>,
        first: i64,
        length: usize,
        status: u16,
        done: bool,
    }
    let format = completion.format;
    let stream = futures::stream::try_unfold(
        StreamState {
            upstream: Box::pin(response.bytes_stream()),
            completion,
            parser: UsageParser::new(format),
            buffer: Vec::new(),
            first: 0,
            length: 0,
            status: status.as_u16(),
            done: false,
        },
        |mut state| async move {
            if state.done {
                return Ok::<_, std::io::Error>(None);
            }
            if let Some(chunk) = state.upstream.next().await {
                let chunk = chunk.map_err(std::io::Error::other)?;
                if state.first == 0 {
                    state.first = timestamp();
                }
                state.length += chunk.len();
                state.buffer.extend_from_slice(&chunk);
                while let Some((end, separator)) = boundary(&state.buffer) {
                    let part = String::from_utf8_lossy(&state.buffer[..end]);
                    state.parser.parse(part.trim());
                    state.buffer.drain(..end + separator);
                }
                return Ok(Some((chunk, state)));
            }
            state.done = true;
            if let Some(limit) = &state.completion.request_limit {
                limit.track().await.map_err(std::io::Error::other)?;
            }
            if let Some(usage) = state.parser.retrieve() {
                let usage = state.completion.helper.normalize_usage(usage);
                let cost = state
                    .completion
                    .track(&usage, state.first, timestamp())
                    .await
                    .map_err(|_| std::io::Error::other("Usage accounting failed"))?;
                state
                    .completion
                    .trace
                    .clone()
                    .finish(state.status, state.length, state.first, true)
                    .await;
                return Ok(Some((
                    Bytes::from(build_cost_chunk(state.completion.format, &cost)),
                    state,
                )));
            }
            state
                .completion
                .trace
                .clone()
                .finish(state.status, state.length, state.first, true)
                .await;
            Ok(None)
        },
    );
    let mut output = Response::new(Body::from_stream(stream));
    *output.status_mut() = status;
    *output.headers_mut() = headers;
    Ok(output)
}

fn timestamp() -> i64 {
    (time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as i64
}
fn boundary(bytes: &[u8]) -> Option<(usize, usize)> {
    (0..bytes.len()).find_map(|i| {
        let rest = &bytes[i..];
        if rest.starts_with(b"\r\n\r\n") {
            return Some((i, 4));
        }
        if rest.starts_with(b"\n\n") || rest.starts_with(b"\r\r") {
            return Some((i, 2));
        }
        None
    })
}
