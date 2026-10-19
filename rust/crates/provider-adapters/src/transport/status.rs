//! Purpose: map a failed (>= 400) HTTP response to a typed `LlmError` (packages/llm/src/route/executor.ts
//! `statusError`, `statusReason`, `retryAfterMs`, `rateLimitDetails`, `requestId`, `providerMessage`).
//! Scope: pure mapping over already-read status, headers and body text. No I/O.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- provider-adapters/src/auth.rs
//! |-- provider-adapters/src/transport/overflow.rs
//! `-- provider-adapters/src/transport/redact.rs
//! External:
//! |-- f64::INFINITY
//! |-- f64::NAN
//! |-- f64::NEG_INFINITY
//! |-- indexmap::IndexMap
//! |-- regex::Regex
//! |-- serde_json::to_string
//! |-- time::OffsetDateTime
//! |-- time::OffsetDateTime::now_utc
//! |-- time::OffsetDateTime::parse
//! |-- time::format_description::well_known::Rfc2822
//! |-- time::format_description::well_known::Rfc3339
//! |-- time::macros::datetime
//! `-- u64::from_str_radix
//! Used by:
//! rust/crates/
//! |-- provider-adapters/src/transport/executor.rs
//! `-- provider-adapters/src/transport/mod.rs
//! Invariants: classification order is content-policy, 401, 403, 429 (quota vs rate), invalid
//! request (400/404/409/413/422, overflow classified), >= 500 or retryable status, unknown.
//! Classification reads the redacted, truncated body. Response headers are lowercased and
//! sorted by name (web `Headers` iteration order).
//! Contract tests: provider-adapters/tests/transport_executor.rs, provider-adapters/src/transport/status.rs (unit tests)

use std::sync::LazyLock;

use indexmap::IndexMap;
use opencode_llm_api::{
    AuthenticationKind, HttpContext, HttpRateLimitDetails, HttpRequestDetails, HttpResponseDetails,
    LlmError, LlmErrorReason, ProviderFailureClassification,
};
use regex::Regex;

use crate::auth::Headers;
use crate::transport::overflow::is_context_overflow;
use crate::transport::redact::{RedactedName, js_length, redact_headers, response_body};

static CONTENT_POLICY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)content[-_\s]?policy|content_filter|safety").expect("static regex")
});
static QUOTA: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)insufficient[-_\s]?quota|quota[-_\s]?exceeded").expect("static regex")
});
static OPENAI_LIMIT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^x-ratelimit-(limit|remaining|reset)-(.+)$").expect("regex"));
static ANTHROPIC_LIMIT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^anthropic-ratelimit-(.+)-(limit|remaining|reset)$").expect("static regex")
});

/// `retryableStatus`.
pub fn retryable_status(status: u16) -> bool {
    matches!(status, 429 | 503 | 504 | 529)
}

/// Inputs for one failed response. `request` is already redacted.
#[derive(Debug)]
pub struct FailedResponse<'a> {
    pub request: HttpRequestDetails,
    pub status: u16,
    /// Raw response headers (any case, any order, duplicates already joined).
    pub headers: Vec<(String, String)>,
    /// `None` when reading the body failed.
    pub body: Option<String>,
    pub secrets: &'a [String],
    pub redacted_names: &'a [RedactedName],
}

/// `statusError` for a response with status >= 400.
pub fn status_error(input: FailedResponse<'_>) -> LlmError {
    let mut headers: Vec<(String, String)> = input
        .headers
        .into_iter()
        .map(|(k, v)| (k.to_ascii_lowercase(), v))
        .collect();
    headers.sort_by(|a, b| a.0.cmp(&b.0));
    let normalized: IndexMap<String, String> = headers.iter().cloned().collect();
    let retry_after = retry_after_ms(&normalized, time::OffsetDateTime::now_utc());
    let rate_limit = rate_limit_details(&normalized, retry_after);
    let (body, body_truncated) = match input.body.as_deref() {
        Some(text) => {
            let (body, truncated) = response_body(text, input.secrets);
            (Some(body), truncated)
        }
        None => (None, None),
    };
    let message = provider_message(input.status, body.as_deref());
    let http = Box::new(HttpContext {
        request: input.request,
        response: Some(HttpResponseDetails {
            status: input.status,
            headers: redact_headers(
                headers.iter().map(|(k, v)| (k.as_str(), v.as_str())),
                input.redacted_names,
            ),
        }),
        body,
        body_truncated,
        request_id: request_id(&normalized),
        rate_limit: rate_limit.clone(),
    });
    let reason = status_reason(input.status, message, retry_after, rate_limit, http);
    LlmError::new("RequestExecutor", "execute", reason)
}

fn provider_message(status: u16, body: Option<&str>) -> String {
    match body {
        Some(body) if !body.is_empty() && js_length(body) <= 500 => {
            format!("Provider request failed with HTTP {status}: {body}")
        }
        _ => format!("Provider request failed with HTTP {status}"),
    }
}

fn status_reason(
    status: u16,
    message: String,
    retry_after_ms: Option<f64>,
    rate_limit: Option<HttpRateLimitDetails>,
    http: Box<HttpContext>,
) -> LlmErrorReason {
    let body = http.body.clone().unwrap_or_default();
    let http = Some(http);
    if CONTENT_POLICY.is_match(&body) {
        return LlmErrorReason::ContentPolicy {
            message,
            provider_metadata: None,
            http,
        };
    }
    let auth = |kind| LlmErrorReason::Authentication {
        message: message.clone(),
        kind,
        provider_metadata: None,
        http: http.clone(),
    };
    match status {
        401 => auth(AuthenticationKind::Invalid),
        403 => auth(AuthenticationKind::InsufficientPermissions),
        429 if QUOTA.is_match(&body) => LlmErrorReason::QuotaExceeded {
            message,
            provider_metadata: None,
            http,
        },
        429 => LlmErrorReason::RateLimit {
            message,
            retry_after_ms,
            rate_limit,
            provider_metadata: None,
            http,
        },
        400 | 404 | 409 | 413 | 422 => LlmErrorReason::InvalidRequest {
            message,
            parameter: None,
            classification: is_context_overflow(&body)
                .then_some(ProviderFailureClassification::ContextOverflow),
            provider_metadata: None,
            http,
        },
        s if s >= 500 || retryable_status(s) => LlmErrorReason::ProviderInternal {
            message,
            status,
            retry_after_ms,
            provider_metadata: None,
            http,
        },
        _ => LlmErrorReason::UnknownProvider {
            message,
            status: Some(status),
            provider_metadata: None,
            http,
        },
    }
}

/// `requestId`: first present of the known request-id headers.
pub fn request_id(headers: &IndexMap<String, String>) -> Option<String> {
    [
        "x-request-id",
        "request-id",
        "x-amzn-requestid",
        "x-amz-request-id",
        "x-goog-request-id",
        "cf-ray",
    ]
    .iter()
    .find_map(|name| headers.get(*name).cloned())
}

/// JS `Number(value)`: trims whitespace, empty is 0, hex/octal/binary prefixes, else decimal.
pub fn js_number(value: &str) -> f64 {
    let text = value.trim();
    if text.is_empty() {
        return 0.0;
    }
    for (prefix, radix) in [
        ("0x", 16),
        ("0X", 16),
        ("0o", 8),
        ("0O", 8),
        ("0b", 2),
        ("0B", 2),
    ] {
        if let Some(digits) = text.strip_prefix(prefix) {
            return u64::from_str_radix(digits, radix).map_or(f64::NAN, |v| v as f64);
        }
    }
    let lowered = text.to_ascii_lowercase();
    if lowered.contains("inf") || lowered.contains("nan") {
        return match text.trim_start_matches(['+', '-']) {
            "Infinity" if text.starts_with('-') => f64::NEG_INFINITY,
            "Infinity" => f64::INFINITY,
            _ => f64::NAN,
        };
    }
    text.parse::<f64>().unwrap_or(f64::NAN)
}

/// `retryAfterMs`: `retry-after-ms`, then `retry-after` seconds, then an HTTP date.
pub fn retry_after_ms(
    headers: &IndexMap<String, String>,
    now: time::OffsetDateTime,
) -> Option<f64> {
    if let Some(millis) = headers.get("retry-after-ms").map(|v| js_number(v))
        && millis.is_finite()
    {
        return Some(millis.max(0.0));
    }
    let value = headers.get("retry-after").filter(|v| !v.is_empty())?;
    let seconds = js_number(value);
    if seconds.is_finite() {
        return Some((seconds * 1000.0).max(0.0));
    }
    let date = parse_http_date(value)?;
    let delta = (date - now).whole_milliseconds() as f64;
    Some(delta.max(0.0))
}

fn parse_http_date(value: &str) -> Option<time::OffsetDateTime> {
    use time::format_description::well_known::{Rfc2822, Rfc3339};
    let text = value.trim();
    time::OffsetDateTime::parse(text, &Rfc2822)
        .or_else(|_| time::OffsetDateTime::parse(text, &Rfc3339))
        .ok()
}

/// `rateLimitDetails`: OpenAI `x-ratelimit-*-<k>` and Anthropic `anthropic-ratelimit-<k>-*`.
pub fn rate_limit_details(
    headers: &IndexMap<String, String>,
    retry_after_ms: Option<f64>,
) -> Option<HttpRateLimitDetails> {
    let mut groups: [IndexMap<String, String>; 3] = Default::default();
    let slot = |kind: &str| match kind {
        "limit" => 0,
        "remaining" => 1,
        _ => 2,
    };
    for (name, value) in headers {
        if let Some(c) = OPENAI_LIMIT.captures(name) {
            groups[slot(&c[1])].insert(c[2].to_owned(), value.clone());
            continue;
        }
        if let Some(c) = ANTHROPIC_LIMIT.captures(name) {
            groups[slot(&c[2])].insert(c[1].to_owned(), value.clone());
        }
    }
    if retry_after_ms.is_none() && groups.iter().all(IndexMap::is_empty) {
        return None;
    }
    let [limit, remaining, reset] = groups.map(|g| (!g.is_empty()).then_some(g));
    Some(HttpRateLimitDetails {
        retry_after_ms,
        limit,
        remaining,
        reset,
    })
}

/// Redacted request details for diagnostics.
pub fn request_details(
    method: &str,
    url: &str,
    headers: &Headers,
    redacted_names: &[RedactedName],
) -> HttpRequestDetails {
    HttpRequestDetails {
        method: method.to_owned(),
        url: crate::transport::redact::redact_url(url),
        headers: redact_headers(
            headers.iter().map(|(k, v)| (k.as_str(), v.as_str())),
            redacted_names,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> IndexMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn retry_after_sources() {
        let now = time::macros::datetime!(2026-05-06 12:00:00 UTC);
        assert_eq!(
            retry_after_ms(&map(&[("retry-after-ms", "250")]), now),
            Some(250.0)
        );
        assert_eq!(
            retry_after_ms(&map(&[("retry-after-ms", "-5")]), now),
            Some(0.0)
        );
        assert_eq!(
            retry_after_ms(&map(&[("retry-after-ms", "")]), now),
            Some(0.0)
        );
        assert_eq!(
            retry_after_ms(&map(&[("retry-after-ms", "x"), ("retry-after", "2")]), now),
            Some(2000.0)
        );
        assert_eq!(
            retry_after_ms(
                &map(&[("retry-after", "Wed, 06 May 2026 12:00:03 GMT")]),
                now
            ),
            Some(3000.0)
        );
        assert_eq!(
            retry_after_ms(
                &map(&[("retry-after", "Wed, 06 May 2026 11:00:00 GMT")]),
                now
            ),
            Some(0.0)
        );
        assert_eq!(retry_after_ms(&map(&[("retry-after", "")]), now), None);
        assert_eq!(retry_after_ms(&map(&[("retry-after", "soon")]), now), None);
        assert_eq!(js_number("0x10"), 16.0);
        assert!(js_number("inf").is_nan());
    }

    #[test]
    fn rate_limit_groups() {
        let details = rate_limit_details(
            &map(&[
                ("anthropic-ratelimit-input-tokens-limit", "10000"),
                ("x-ratelimit-limit-requests", "500"),
                ("x-ratelimit-reset-tokens", "10s"),
            ]),
            None,
        )
        .unwrap();
        let json = serde_json::to_string(&details).unwrap();
        assert_eq!(
            json,
            r#"{"limit":{"input-tokens":"10000","requests":"500"},"reset":{"tokens":"10s"}}"#
        );
        assert_eq!(rate_limit_details(&map(&[("x-other", "1")]), None), None);
    }
}
