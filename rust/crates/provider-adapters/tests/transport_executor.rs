//! Purpose: wiremock contract tests for the HTTP request executor: status mapping table,
//! retry counts and delays, redaction of request/response diagnostics, transport errors.
//! Scope: fixture-based (local wiremock server), no live provider calls.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- provider-adapters/src/route.rs
//! |-- provider-adapters/src/transport/mod.rs
//! `-- provider-adapters/src/transport/redact.rs
//! External:
//! |-- serde_json::json
//! |-- serde_json::to_value
//! |-- tokio::test
//! |-- wiremock::Mock
//! |-- wiremock::MockServer
//! |-- wiremock::ResponseTemplate
//! |-- wiremock::matchers::method
//! `-- wiremock::matchers::path
//! Used by:
//! `-- none, entrypoint
//! Invariants: mirrors packages/llm/test/executor.test.ts expectations.
//! Contract tests: provider-adapters/tests/transport_executor.rs

use std::time::{Duration, Instant};

use opencode_llm_api::{
    AuthenticationKind, LlmError, LlmErrorReason, ProviderFailureClassification,
};
use opencode_provider_adapters::route::HttpRequestParts;
use opencode_provider_adapters::transport::HttpExecutor;
use opencode_provider_adapters::transport::redact::RedactedName;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn request(server: &MockServer, query: &str, auth: &str) -> HttpRequestParts {
    HttpRequestParts {
        method: "POST".into(),
        url: format!("{}/v1/chat?{query}", server.uri()),
        headers: [
            ("authorization".to_string(), auth.to_string()),
            ("x-safe".to_string(), "visible".to_string()),
            ("content-type".to_string(), "application/json".to_string()),
        ]
        .into_iter()
        .collect(),
        body: r#"{"model":"m"}"#.into(),
        json_body: json!({"model": "m"}),
    }
}

fn http(error: &LlmError) -> &opencode_llm_api::HttpContext {
    let http = match error.reason.as_ref() {
        LlmErrorReason::InvalidRequest { http, .. }
        | LlmErrorReason::Authentication { http, .. }
        | LlmErrorReason::RateLimit { http, .. }
        | LlmErrorReason::QuotaExceeded { http, .. }
        | LlmErrorReason::ContentPolicy { http, .. }
        | LlmErrorReason::ProviderInternal { http, .. }
        | LlmErrorReason::Transport { http, .. }
        | LlmErrorReason::UnknownProvider { http, .. } => http,
        other => panic!("no http context on {other:?}"),
    };
    http.as_deref().expect("http context")
}

async fn fail_with(template: ResponseTemplate, retries: u32) -> (LlmError, usize) {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat"))
        .respond_with(template)
        .mount(&server)
        .await;
    let executor = HttpExecutor::new().unwrap().max_retries(retries);
    let error = executor
        .execute(&request(&server, "debug=1", "Bearer secret"))
        .await
        .expect_err("expected failure");
    let count = server.received_requests().await.unwrap().len();
    (error, count)
}

#[tokio::test]
async fn status_mapping_table() {
    type Check = fn(&LlmErrorReason) -> bool;
    let cases: Vec<(u16, &str, &str, bool, Check)> = vec![
        (401, "nope", "Authentication", false, |r| {
            matches!(
                r,
                LlmErrorReason::Authentication {
                    kind: AuthenticationKind::Invalid,
                    ..
                }
            )
        }),
        (403, "nope", "Authentication", false, |r| {
            matches!(
                r,
                LlmErrorReason::Authentication {
                    kind: AuthenticationKind::InsufficientPermissions,
                    ..
                }
            )
        }),
        (
            429,
            r#"{"error":{"code":"insufficient_quota"}}"#,
            "QuotaExceeded",
            false,
            |_| true,
        ),
        (429, "slow down", "RateLimit", true, |_| true),
        (
            400,
            r#"{"error":{"code":"context_length_exceeded","message":"prompt too long"}}"#,
            "InvalidRequest",
            false,
            |r| r.is_context_overflow(),
        ),
        (413, "request too large", "InvalidRequest", false, |r| {
            !r.is_context_overflow()
        }),
        (400, "invalid parameter", "InvalidRequest", false, |r| {
            !r.is_context_overflow()
        }),
        (422, "", "InvalidRequest", false, |_| true),
        (
            400,
            "flagged by content_filter",
            "ContentPolicy",
            false,
            |_| true,
        ),
        (500, "boom", "ProviderInternal", true, |r| {
            matches!(r, LlmErrorReason::ProviderInternal { status: 500, .. })
        }),
        (503, "busy", "ProviderInternal", true, |_| true),
        (504, "retry", "ProviderInternal", true, |r| {
            matches!(r, LlmErrorReason::ProviderInternal { status: 504, .. })
        }),
        (529, "overloaded", "ProviderInternal", true, |r| {
            matches!(r, LlmErrorReason::ProviderInternal { status: 529, .. })
        }),
        (418, "teapot", "UnknownProvider", false, |r| {
            matches!(
                r,
                LlmErrorReason::UnknownProvider {
                    status: Some(418),
                    ..
                }
            )
        }),
    ];
    for (status, body, tag, retryable, check) in cases {
        let template = ResponseTemplate::new(status).set_body_string(body);
        let (error, count) = fail_with(template, 0).await;
        assert_eq!(error.reason.tag(), tag, "status {status} body {body}");
        assert_eq!(error.retryable(), retryable, "status {status}");
        assert!(check(&error.reason), "status {status}: {:?}", error.reason);
        assert_eq!(count, 1);
        assert_eq!(
            (error.module.as_str(), error.method.as_str()),
            ("RequestExecutor", "execute")
        );
        let expected = if body.is_empty() {
            format!("Provider request failed with HTTP {status}")
        } else {
            format!("Provider request failed with HTTP {status}: {body}")
        };
        assert_eq!(error.reason.message(), expected);
    }
}

#[tokio::test]
async fn retries_exactly_max_retries_plus_one_then_fails() {
    let template = ResponseTemplate::new(503)
        .insert_header("retry-after-ms", "0")
        .set_body_string("busy");
    let (error, count) =
        fail_with(template, opencode_provider_adapters::transport::MAX_RETRIES).await;
    assert_eq!(count, 3);
    assert_eq!(error.reason.tag(), "ProviderInternal");
    assert_eq!(error.retry_after_ms(), Some(0.0));
}

#[tokio::test]
async fn non_retryable_status_is_sent_once_and_body_truncated() {
    let (error, count) = fail_with(
        ResponseTemplate::new(401).set_body_string("x".repeat(20_000)),
        2,
    )
    .await;
    assert_eq!(count, 1);
    assert_eq!(error.reason.tag(), "Authentication");
    let http = http(&error);
    assert_eq!(http.body_truncated, Some(true));
    assert_eq!(http.body.as_ref().unwrap().len(), 16_384);
    assert_eq!(
        error.reason.message(),
        "Provider request failed with HTTP 401"
    );
}

#[tokio::test]
async fn retry_then_success_returns_response() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(503).insert_header("retry-after-ms", "0"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .mount(&server)
        .await;
    let response = HttpExecutor::new()
        .unwrap()
        .execute(&request(&server, "debug=1", "Bearer s"))
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(response.text().await.unwrap(), "ok");
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn honors_retry_after_seconds() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(503).insert_header("retry-after", "1"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let started = Instant::now();
    let response = HttpExecutor::new()
        .unwrap()
        .execute(&request(&server, "", "Bearer s"))
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    assert!(started.elapsed() >= Duration::from_millis(1000));
}

#[tokio::test]
async fn rate_limit_diagnostics_are_redacted() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after-ms", "0")
                .insert_header("x-request-id", "req_123")
                .insert_header("x-api-key", "secret")
                .insert_header("x-ratelimit-limit-requests", "500")
                .insert_header("x-ratelimit-limit-tokens", "30000")
                .insert_header("x-ratelimit-remaining-requests", "499")
                .insert_header("x-ratelimit-remaining-tokens", "29900")
                .insert_header("x-ratelimit-reset-requests", "1s")
                .insert_header("x-ratelimit-reset-tokens", "10s")
                .set_body_string("rate limited"),
        )
        .mount(&server)
        .await;
    let error = HttpExecutor::new()
        .unwrap()
        .execute(&request(
            &server,
            "api_key=secret&key=secret&debug=1",
            "Bearer secret",
        ))
        .await
        .unwrap_err();
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
    assert!(error.retryable());
    assert_eq!(error.retry_after_ms(), Some(0.0));
    let LlmErrorReason::RateLimit { rate_limit, .. } = error.reason.as_ref() else {
        panic!("expected RateLimit: {:?}", error.reason);
    };
    assert_eq!(
        serde_json::to_value(rate_limit).unwrap(),
        json!({
            "retryAfterMs": 0,
            "limit": {"requests": "500", "tokens": "30000"},
            "remaining": {"requests": "499", "tokens": "29900"},
            "reset": {"requests": "1s", "tokens": "10s"}
        })
    );
    let http = http(&error);
    assert_eq!(http.request_id.as_deref(), Some("req_123"));
    assert_eq!(http.body.as_deref(), Some("rate limited"));
    assert_eq!(
        http.request.url,
        format!(
            "{}/v1/chat?api_key=%3Credacted%3E&key=%3Credacted%3E&debug=1",
            server.uri()
        )
    );
    assert_eq!(http.request.headers["authorization"], "<redacted>");
    assert_eq!(http.request.headers["x-safe"], "visible");
    let response = http.response.as_ref().unwrap();
    assert_eq!(response.status, 429);
    assert_eq!(response.headers["x-api-key"], "<redacted>");
    assert_eq!(response.headers["retry-after-ms"], "0");
    assert_eq!(response.headers["x-request-id"], "req_123");
}

#[tokio::test]
async fn anthropic_rate_limit_headers_on_529() {
    let template = ResponseTemplate::new(529)
        .insert_header("retry-after-ms", "0")
        .insert_header("anthropic-ratelimit-requests-limit", "100")
        .insert_header("anthropic-ratelimit-requests-remaining", "12")
        .insert_header("anthropic-ratelimit-requests-reset", "2026-05-06T12:00:00Z")
        .insert_header("anthropic-ratelimit-input-tokens-limit", "10000")
        .insert_header("anthropic-ratelimit-input-tokens-remaining", "9000")
        .insert_header(
            "anthropic-ratelimit-input-tokens-reset",
            "2026-05-06T12:00:10Z",
        )
        .set_body_string("overloaded");
    let (error, count) = fail_with(template, 2).await;
    assert_eq!(count, 3);
    assert_eq!(error.reason.tag(), "ProviderInternal");
    assert_eq!(
        serde_json::to_value(&http(&error).rate_limit).unwrap(),
        json!({
            "retryAfterMs": 0,
            "limit": {"input-tokens": "10000", "requests": "100"},
            "remaining": {"input-tokens": "9000", "requests": "12"},
            "reset": {"input-tokens": "2026-05-06T12:00:10Z", "requests": "2026-05-06T12:00:00Z"}
        })
    );
}

#[tokio::test]
async fn response_body_secret_fields_and_echoes_are_redacted() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(400).set_body_string(
            r#"{"error":{"message":"bad","key":"body-secret","detail":"api_key=query-secret"}} provider echoed query-secret-123 and authorization header-secret-456"#,
        ))
        .mount(&server)
        .await;
    let error = HttpExecutor::new()
        .unwrap()
        .execute(&request(
            &server,
            "api_key=query-secret-123&debug=1",
            "Bearer header-secret-456",
        ))
        .await
        .unwrap_err();
    let body = http(&error).body.clone().unwrap();
    assert!(body.contains(r#""key":"<redacted>""#), "{body}");
    assert!(body.contains("api_key=<redacted>"), "{body}");
    assert!(body.contains("provider echoed <redacted>"), "{body}");
    assert!(body.contains("authorization <redacted>"), "{body}");
    for secret in ["body-secret", "query-secret", "header-secret-456"] {
        assert!(!body.contains(secret), "{body}");
        assert!(!error.reason.message().contains(secret));
    }
}

#[tokio::test]
async fn configured_redacted_names_apply_to_request_and_response() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(400).insert_header("x-safe", "response-secret"))
        .mount(&server)
        .await;
    let error = HttpExecutor::new()
        .unwrap()
        .redacted_names([RedactedName::Exact("x-safe".into())])
        .execute(&request(&server, "", "Bearer s"))
        .await
        .unwrap_err();
    let http = http(&error);
    assert_eq!(http.request.headers["x-safe"], "<redacted>");
    assert_eq!(
        http.response.as_ref().unwrap().headers["x-safe"],
        "<redacted>"
    );
}

#[tokio::test]
async fn connection_failure_is_transport_error_without_retry() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let parts = HttpRequestParts {
        method: "POST".into(),
        url: format!("http://127.0.0.1:{port}/v1?key=abcdef"),
        headers: Default::default(),
        body: "{}".into(),
        json_body: json!({}),
    };
    let error = HttpExecutor::new()
        .unwrap()
        .execute(&parts)
        .await
        .unwrap_err();
    let LlmErrorReason::Transport {
        kind, url, http, ..
    } = error.reason.as_ref()
    else {
        panic!("expected Transport: {:?}", error.reason);
    };
    assert_eq!(kind.as_deref(), Some("TransportError"));
    let expected = format!("http://127.0.0.1:{port}/v1?key=%3Credacted%3E");
    assert_eq!(url.as_deref(), Some(expected.as_str()));
    assert_eq!(http.as_ref().unwrap().request.url, expected);
    assert!(!error.reason.message().contains("abcdef"));
    assert!(!error.retryable());
}

#[tokio::test]
async fn header_timeout_maps_to_timeout_kind() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(5)))
        .mount(&server)
        .await;
    let error = HttpExecutor::new()
        .unwrap()
        .header_timeout(Duration::from_millis(50))
        .execute(&request(&server, "", "Bearer s"))
        .await
        .unwrap_err();
    let LlmErrorReason::Transport { kind, .. } = error.reason.as_ref() else {
        panic!("expected Transport: {:?}", error.reason);
    };
    assert_eq!(kind.as_deref(), Some("Timeout"));
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn overflow_classification_serializes_kebab_case() {
    let (error, _) = fail_with(
        ResponseTemplate::new(400).set_body_string("prompt is too long: 1 tokens"),
        0,
    )
    .await;
    let LlmErrorReason::InvalidRequest { classification, .. } = error.reason.as_ref() else {
        panic!();
    };
    assert_eq!(
        *classification,
        Some(ProviderFailureClassification::ContextOverflow)
    );
    let value = serde_json::to_value(error.reason.as_ref()).unwrap();
    assert_eq!(value["classification"], "context-overflow");
    assert_eq!(value["_tag"], "InvalidRequest");
}
