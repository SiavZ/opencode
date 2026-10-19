//! Purpose: the HTTP request executor (packages/llm/src/route/executor.ts `RequestExecutor`): send
//! one prepared request with reqwest, map >= 400 statuses to typed errors with redacted
//! diagnostics, map transport failures, and retry retryable statuses with jittered backoff.
//! Scope: one request lifecycle up to response headers. Body streaming belongs to the caller.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- provider-adapters/src/route.rs
//! |-- provider-adapters/src/transport/redact.rs
//! `-- provider-adapters/src/transport/status.rs
//! External:
//! |-- rand::random_range
//! |-- reqwest::Client
//! |-- reqwest::Client::builder
//! |-- reqwest::Error
//! |-- reqwest::Method::from_bytes
//! |-- reqwest::RequestBuilder
//! |-- reqwest::Response
//! |-- reqwest::Url::parse
//! |-- reqwest::header::HeaderMap
//! |-- reqwest::header::HeaderMap::new
//! |-- reqwest::header::HeaderName::from_bytes
//! |-- reqwest::header::HeaderValue::from_str
//! |-- str::to_owned
//! |-- tokio::time::sleep
//! `-- tokio::time::timeout
//! Used by:
//! rust/crates/
//! |-- provider-adapters/src/transport/http.rs
//! |-- provider-adapters/src/transport/mod.rs
//! `-- runtime/src/execution_support.rs
//! Invariants: at most `max_retries + 1` requests, and retries happen only for retryable
//! (`RateLimit`/`ProviderInternal`) status errors, before any body byte reaches the caller.
//! Transport errors are never retried. Delay is `min(retryAfterMs, 10s)` or a uniform jitter in
//! `[0.8, 1.2] * 500ms * 2^attempt` capped at 10s, rounded. Dropping the future cancels the
//! in-flight request and any pending backoff (no spawned tasks).
//! Contract tests: provider-adapters/tests/transport_executor.rs

use std::time::Duration;

use opencode_llm_api::{HttpContext, LlmError, LlmErrorReason};

use crate::route::HttpRequestParts;
use crate::transport::redact::{RedactedName, default_redacted_names, secret_values};
use crate::transport::status::{FailedResponse, request_details, status_error};

pub const MAX_RETRIES: u32 = 2;
pub const BASE_DELAY_MS: f64 = 500.0;
pub const MAX_DELAY_MS: f64 = 10_000.0;
/// Upper bound on bytes read from a failed response body for diagnostics.
pub const ERROR_BODY_READ_LIMIT: usize = 1024 * 1024;

#[derive(Clone, Debug)]
pub struct HttpExecutor {
    client: reqwest::Client,
    redacted_names: Vec<RedactedName>,
    max_retries: u32,
    header_timeout: Option<Duration>,
}

impl HttpExecutor {
    /// Executor over a fresh rustls reqwest client.
    pub fn new() -> Result<Self, LlmError> {
        let client = reqwest::Client::builder().build().map_err(|e| {
            transport_error(
                format!("Failed to build HTTP client: {}", error_text(&e)),
                None,
                None,
            )
        })?;
        Ok(Self::with_client(client))
    }

    pub fn with_client(client: reqwest::Client) -> Self {
        Self {
            client,
            redacted_names: default_redacted_names(),
            max_retries: MAX_RETRIES,
            header_timeout: None,
        }
    }

    /// Extra header names to redact (`Headers.CurrentRedactedNames`), appended to the defaults.
    pub fn redacted_names(mut self, names: impl IntoIterator<Item = RedactedName>) -> Self {
        self.redacted_names.extend(names);
        self
    }

    pub fn max_retries(mut self, retries: u32) -> Self {
        self.max_retries = retries;
        self
    }

    /// Fail with a `Transport { kind: "Timeout" }` error when response headers take longer.
    pub fn header_timeout(mut self, timeout: Duration) -> Self {
        self.header_timeout = Some(timeout);
        self
    }

    /// `execute`: send with retries; returns the first response with status < 400.
    pub async fn execute(&self, request: &HttpRequestParts) -> Result<reqwest::Response, LlmError> {
        let mut attempt = 0;
        loop {
            let error = match self.execute_once(request).await {
                Ok(response) => return Ok(response),
                Err(error) => error,
            };
            if !error.retryable() || attempt >= self.max_retries {
                return Err(error);
            }
            let delay = retry_delay(&error, attempt, rand::random_range(0.0..1.0));
            tokio::time::sleep(Duration::from_millis(delay)).await;
            attempt += 1;
        }
    }

    async fn execute_once(
        &self,
        request: &HttpRequestParts,
    ) -> Result<reqwest::Response, LlmError> {
        let send = self.build(request)?.send();
        let sent = match self.header_timeout {
            Some(limit) => tokio::time::timeout(limit, send).await.map_err(|_| {
                transport_error(
                    format!("Request timed out after {}ms", limit.as_millis()),
                    Some("Timeout"),
                    None,
                )
            })?,
            None => send.await,
        };
        let response = sent.map_err(|e| self.map_reqwest_error(&e, request))?;
        if response.status().as_u16() < 400 {
            return Ok(response);
        }
        Err(self.status_failure(request, response).await)
    }

    fn build(&self, request: &HttpRequestParts) -> Result<reqwest::RequestBuilder, LlmError> {
        let method = reqwest::Method::from_bytes(request.method.as_bytes()).map_err(|_| {
            self.request_error(request, "HTTP transport failed: EncodeError", "EncodeError")
        })?;
        let url = reqwest::Url::parse(&request.url).map_err(|_| {
            self.request_error(
                request,
                "HTTP transport failed: InvalidUrlError",
                "InvalidUrlError",
            )
        })?;
        let mut headers = reqwest::header::HeaderMap::new();
        for (name, value) in &request.headers {
            let name = reqwest::header::HeaderName::from_bytes(name.as_bytes());
            let value = reqwest::header::HeaderValue::from_str(value);
            let (Ok(name), Ok(value)) = (name, value) else {
                return Err(self.request_error(
                    request,
                    "HTTP transport failed: EncodeError",
                    "EncodeError",
                ));
            };
            headers.append(name, value);
        }
        Ok(self
            .client
            .request(method, url)
            .headers(headers)
            .body(request.body.clone()))
    }

    fn request_error(&self, request: &HttpRequestParts, message: &str, kind: &str) -> LlmError {
        let details = request_details(
            &request.method,
            &request.url,
            &request.headers,
            &self.redacted_names,
        );
        transport_error(
            message.to_owned(),
            Some(kind),
            Some(HttpContext {
                request: details,
                response: None,
                body: None,
                body_truncated: None,
                request_id: None,
                rate_limit: None,
            }),
        )
    }

    /// `toHttpError`: timeouts carry no request context, everything else is a `TransportError`.
    fn map_reqwest_error(&self, error: &reqwest::Error, request: &HttpRequestParts) -> LlmError {
        if error.is_timeout() {
            return transport_error(error_text(error), Some("Timeout"), None);
        }
        if error.is_builder() {
            return self.request_error(
                request,
                "HTTP transport failed: InvalidUrlError",
                "InvalidUrlError",
            );
        }
        self.request_error(request, &error_text(error), "TransportError")
    }

    async fn status_failure(
        &self,
        request: &HttpRequestParts,
        response: reqwest::Response,
    ) -> LlmError {
        let status = response.status().as_u16();
        let headers = joined_headers(response.headers());
        let body = read_limited(response, ERROR_BODY_READ_LIMIT).await;
        let secrets = secret_values(&request.url, &request.headers);
        status_error(FailedResponse {
            request: request_details(
                &request.method,
                &request.url,
                &request.headers,
                &self.redacted_names,
            ),
            status,
            headers,
            body,
            secrets: &secrets,
            redacted_names: &self.redacted_names,
        })
    }
}

/// `retryDelay` in whole milliseconds. `unit` is a uniform sample in `[0, 1)`.
pub fn retry_delay(error: &LlmError, attempt: u32, unit: f64) -> u64 {
    if let Some(after) = error.retry_after_ms() {
        return after.clamp(0.0, MAX_DELAY_MS) as u64;
    }
    let base = BASE_DELAY_MS * 2f64.powi(attempt as i32);
    let low = (base * 0.8).min(MAX_DELAY_MS);
    let high = (base * 1.2).min(MAX_DELAY_MS);
    (low + (high - low) * unit).round() as u64
}

/// Web `Headers` semantics: duplicate names joined with `", "` in arrival order.
fn joined_headers(map: &reqwest::header::HeaderMap) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for (name, value) in map {
        let value = String::from_utf8_lossy(value.as_bytes()).into_owned();
        match out.iter_mut().find(|(n, _)| n == name.as_str()) {
            Some((_, existing)) => {
                existing.push_str(", ");
                existing.push_str(&value);
            }
            None => out.push((name.as_str().to_owned(), value)),
        }
    }
    out
}

/// Read at most `limit` bytes of the body as lossy UTF-8; `None` if the first read fails.
async fn read_limited(mut response: reqwest::Response, limit: usize) -> Option<String> {
    let mut bytes = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                let take = chunk.len().min(limit - bytes.len());
                bytes.extend_from_slice(&chunk[..take]);
                if bytes.len() >= limit {
                    break;
                }
            }
            Ok(None) => break,
            Err(_) if bytes.is_empty() => return None,
            Err(_) => break,
        }
    }
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// Error text without the request URL (which may carry secrets), with its source chain.
pub fn error_text(error: &reqwest::Error) -> String {
    let mut text = error.to_string();
    if let Some(url) = error.url() {
        text = text.replace(&format!(" for url ({url})"), "");
    }
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        let cause_text = cause.to_string();
        if !text.contains(&cause_text) {
            text.push_str(": ");
            text.push_str(&cause_text);
        }
        source = cause.source();
    }
    text
}

fn transport_error(message: String, kind: Option<&str>, http: Option<HttpContext>) -> LlmError {
    let url = http.as_ref().map(|h| h.request.url.clone());
    LlmError::new(
        "RequestExecutor",
        "execute",
        LlmErrorReason::Transport {
            message,
            kind: kind.map(str::to_owned),
            url,
            http: http.map(Box::new),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn internal(retry_after_ms: Option<f64>) -> LlmError {
        LlmError::new(
            "RequestExecutor",
            "execute",
            LlmErrorReason::ProviderInternal {
                message: "busy".into(),
                status: 503,
                retry_after_ms,
                provider_metadata: None,
                http: None,
            },
        )
    }

    #[test]
    fn delay_schedule() {
        assert_eq!(retry_delay(&internal(None), 0, 0.5), 500);
        assert_eq!(retry_delay(&internal(None), 1, 0.5), 1000);
        assert_eq!(retry_delay(&internal(None), 0, 0.0), 400);
        assert_eq!(retry_delay(&internal(None), 0, 0.999_999), 600);
        assert_eq!(retry_delay(&internal(None), 10, 0.5), 10_000);
        assert_eq!(retry_delay(&internal(Some(2000.0)), 0, 0.5), 2000);
        assert_eq!(retry_delay(&internal(Some(60_000.0)), 0, 0.5), 10_000);
    }
}
