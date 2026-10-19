//! Purpose: replay one baseline fixture case against an in-process Rust router and classify the
//! result (pass, or a mismatch with category and first difference).
//! Scope: test-only.
//! Uses:
//! rust/crates/
//! |-- server/tests/support/replay/matcher.rs
//! `-- server/tests/support/replay/mod.rs
//! External:
//! |-- axum::Router
//! |-- axum::body::Body
//! |-- axum::http::Request
//! |-- axum::http::Request::builder
//! |-- base64::Engine
//! |-- base64::engine::general_purpose::STANDARD
//! |-- form_urlencoded::Serializer::new
//! |-- futures_util::StreamExt
//! |-- http_body_util::BodyExt
//! |-- serde_json::Value
//! |-- serde_json::from_slice
//! |-- serde_json::from_str
//! |-- serde_json::to_vec
//! |-- sha2::Digest
//! |-- sha2::Sha256
//! |-- tokio::time::Instant::now
//! |-- tokio::time::timeout
//! |-- tokio::time::timeout_at
//! `-- tower::ServiceExt
//! Used by:
//! rust/crates/
//! |-- server/tests/replay_baseline.rs
//! `-- server/tests/support/replay/mod.rs
//! Invariants: requests are rebuilt from the fixture with placeholders filled from the current
//! bindings; credentials come from the `auth` marker, never from the fixture. Status is compared
//! first, then the recorded headers, then the body by kind. Categories: `not-mounted` (no Rust
//! route, the UI fallback answered 503), `not-ported` (route mounted, typed 503 because the
//! handler or service is missing), `status`, `headers`, `body`, `stream`, `timeout`.
//! Contract tests: server/tests/replay_baseline.rs

use std::time::Duration;

use axum::Router;
use axum::body::Body;
use base64::Engine;
use futures_util::StreamExt;
use http_body_util::BodyExt;
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use tower::ServiceExt;

use super::Case;
use super::matcher::Bindings;

pub const PASSWORD: &str = "fixture-password";

/// Headers the capture kept; every one present on either side is compared.
const COMPARED_HEADERS: &[&str] = &[
    "content-type",
    "www-authenticate",
    "access-control-allow-origin",
    "access-control-allow-methods",
    "access-control-allow-headers",
    "access-control-allow-credentials",
    "access-control-max-age",
    "access-control-expose-headers",
    "vary",
    "content-encoding",
    "cache-control",
    "location",
    "content-security-policy",
];

#[derive(Debug, Clone)]
pub struct Outcome {
    pub category: Option<String>,
    pub detail: String,
    pub status: Option<u16>,
}

impl Outcome {
    fn pass(status: u16) -> Self {
        Outcome {
            category: None,
            detail: String::new(),
            status: Some(status),
        }
    }
    fn fail(category: &str, detail: String, status: Option<u16>) -> Self {
        Outcome {
            category: Some(category.to_string()),
            detail,
            status,
        }
    }
}

pub async fn replay(router: &Router, case: &Case, bindings: &mut Bindings) -> Outcome {
    let request = build(case, bindings);
    let wait = if case.response.kind == "timeout" {
        3
    } else {
        10
    };
    let response = match tokio::time::timeout(
        Duration::from_secs(wait),
        router.clone().oneshot(request),
    )
    .await
    {
        Err(_) if case.response.kind == "timeout" => return Outcome::pass(0),
        Err(_) => return Outcome::fail("timeout", format!("no response within {wait}s"), None),
        Ok(Err(never)) => match never {},
        Ok(Ok(response)) => response,
    };
    let status = response.status().as_u16();
    if case.response.kind == "timeout" {
        return Outcome::fail(
            "status",
            format!("baseline long-polls (no response in 3s), Rust answered {status}"),
            Some(status),
        );
    }
    let headers = response.headers().clone();
    let is_sse = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/event-stream"));
    let body = if is_sse {
        read_stream(response.into_body()).await
    } else {
        match tokio::time::timeout(Duration::from_secs(10), response.into_body().collect()).await {
            Ok(Ok(collected)) => collected.to_bytes().to_vec(),
            Ok(Err(e)) => return Outcome::fail("body", format!("body error: {e}"), Some(status)),
            Err(_) => {
                return Outcome::fail("timeout", "body did not finish in 10s".into(), Some(status));
            }
        }
    };
    let expected_status = case.response.status.unwrap_or(0);
    if status != expected_status {
        let text = String::from_utf8_lossy(&body);
        let unavailable =
            status == 503 && text.contains("ServiceUnavailableError") && expected_status != 503;
        // The full host's UI fallback answers 503 with `service: "ui"` when no route matched.
        let category = if unavailable && text.contains("\"service\":\"ui\"") {
            "not-mounted"
        } else if unavailable {
            "not-ported"
        } else {
            "status"
        };
        return Outcome::fail(
            category,
            format!(
                "status {status} != baseline {expected_status}; body {}",
                clip(&text)
            ),
            Some(status),
        );
    }
    for name in COMPARED_HEADERS {
        let expected = case.response.headers.get(*name);
        let actual = headers.get(*name).and_then(|v| v.to_str().ok());
        match (expected, actual) {
            (None, None) => {}
            (Some(e), Some(a)) => {
                if let Err(m) = bindings.text(e, a) {
                    return Outcome::fail("headers", format!("header {name}: {m}"), Some(status));
                }
            }
            (e, a) => {
                return Outcome::fail(
                    "headers",
                    format!("header {name}: baseline {e:?}, Rust {a:?}"),
                    Some(status),
                );
            }
        }
    }
    if let Err(m) = compare_body(case, &body, bindings) {
        let category = if case.response.kind == "sse" {
            "stream"
        } else {
            "body"
        };
        return Outcome::fail(category, m, Some(status));
    }
    Outcome::pass(status)
}

fn build(case: &Case, bindings: &Bindings) -> axum::http::Request<Body> {
    let req = &case.request;
    let mut uri = bindings.fill(&req.path);
    let mut pairs: Vec<(String, String)> = req
        .query
        .iter()
        .map(|(k, v)| (k.clone(), bindings.fill(v)))
        .collect();
    let engine = base64::engine::general_purpose::STANDARD;
    if req.auth == "query-valid" {
        pairs.push((
            "auth_token".into(),
            engine.encode(format!("opencode:{PASSWORD}")),
        ));
    }
    if !pairs.is_empty() {
        let query: String = form_urlencoded::Serializer::new(String::new())
            .extend_pairs(pairs.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .finish();
        uri = format!("{uri}?{query}");
    }
    let mut builder = axum::http::Request::builder()
        .method(req.method.as_str())
        .uri(uri)
        .header("host", "127.0.0.1:4096");
    if let Some(proj) = bindings.literals.iter().find(|(k, _)| k == "{PROJ}") {
        builder = builder.header("x-opencode-directory", proj.1.clone());
    }
    match (req.auth.as_str(), case.server.as_str()) {
        ("valid", "pass") => {
            builder = builder.header(
                "authorization",
                format!("Basic {}", engine.encode(format!("opencode:{PASSWORD}"))),
            )
        }
        ("invalid", _) => {
            builder = builder.header(
                "authorization",
                format!("Basic {}", engine.encode("opencode:wrong")),
            )
        }
        _ => {}
    }
    let mut has_type = false;
    for (k, v) in &req.headers {
        has_type |= k == "content-type";
        builder = builder.header(k.as_str(), bindings.fill(v));
    }
    let body = if let Some(raw) = &req.raw_body {
        Body::from(bindings.fill(raw))
    } else if let Some(json) = &req.body {
        if !has_type {
            builder = builder.header("content-type", "application/json");
        }
        Body::from(serde_json::to_vec(&bindings.fill_value(json)).unwrap())
    } else {
        Body::empty()
    };
    builder.body(body).unwrap()
}

async fn read_stream(body: Body) -> Vec<u8> {
    let mut stream = body.into_data_stream();
    let mut out = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_millis(2500);
    while out.len() < 16384 {
        match tokio::time::timeout_at(deadline, stream.next()).await {
            Ok(Some(Ok(chunk))) => out.extend_from_slice(&chunk),
            _ => break,
        }
    }
    out
}

fn compare_body(case: &Case, body: &[u8], bindings: &mut Bindings) -> Result<(), String> {
    let resp = &case.response;
    match resp.kind.as_str() {
        "empty" => {
            if body.is_empty() {
                return Ok(());
            }
            Err(format!(
                "baseline empty body, Rust {}",
                clip(&String::from_utf8_lossy(body))
            ))
        }
        "json" => {
            let actual: Value = serde_json::from_slice(body).map_err(|e| {
                format!(
                    "Rust body is not JSON ({e}): {}",
                    clip(&String::from_utf8_lossy(body))
                )
            })?;
            bindings.matches(resp.body.as_ref().unwrap_or(&Value::Null), &actual, "$")
        }
        "text" => {
            let expected = resp
                .body
                .as_ref()
                .and_then(Value::as_str)
                .unwrap_or_default();
            bindings.text(expected, &String::from_utf8_lossy(body))
        }
        "json-digest" | "text-digest" => {
            let digest = resp.digest.as_ref().ok_or("digest missing")?;
            let sha = hex(&Sha256::digest(body));
            if sha == digest.sha256 {
                return Ok(());
            }
            if !digest.paths.is_empty() {
                let actual: Value = serde_json::from_slice(body).map_err(|e| e.to_string())?;
                let mut paths: Vec<String> = actual["paths"]
                    .as_object()
                    .map(|m| m.keys().cloned().collect())
                    .unwrap_or_default();
                paths.sort();
                let missing: Vec<&String> =
                    digest.paths.iter().filter(|p| !paths.contains(p)).collect();
                return Err(format!(
                    "digest differs ({} bytes vs baseline {}); {} of {} baseline paths missing, first {:?}",
                    body.len(),
                    digest.bytes,
                    missing.len(),
                    digest.paths.len(),
                    missing.first()
                ));
            }
            Err(format!(
                "digest differs ({} bytes vs baseline {})",
                body.len(),
                digest.bytes
            ))
        }
        "sse" => {
            let text = String::from_utf8_lossy(body);
            let frames: Vec<&str> = text
                .split("\n\n")
                .filter(|f| !f.trim().is_empty())
                .collect();
            if frames.len() < resp.frames.len() {
                return Err(format!(
                    "{} frames, baseline {}: {}",
                    frames.len(),
                    resp.frames.len(),
                    clip(&text)
                ));
            }
            for (i, (expected, actual)) in resp.frames.iter().zip(&frames).enumerate() {
                let shape: Vec<String> = actual
                    .split('\n')
                    .map(|l| {
                        if l.starts_with("data:") {
                            "data: {DATA}".to_string()
                        } else {
                            l.to_string()
                        }
                    })
                    .collect();
                let shape = shape.join("\n");
                bindings
                    .text(&expected.shape, &shape)
                    .map_err(|m| format!("frame {i} shape: {m}"))?;
                let data: String = actual
                    .split('\n')
                    .filter_map(|l| l.strip_prefix("data:"))
                    .map(|l| l.strip_prefix(' ').unwrap_or(l))
                    .collect::<Vec<_>>()
                    .join("\n");
                match serde_json::from_str::<Value>(&data) {
                    Ok(value) if !expected.data.is_string() => {
                        bindings.matches(&expected.data, &value, &format!("frame{i}"))?
                    }
                    _ => bindings
                        .text(expected.data.as_str().unwrap_or_default(), &data)
                        .map_err(|m| format!("frame {i} data: {m}"))?,
                }
            }
            Ok(())
        }
        other => Err(format!("unknown fixture kind {other}")),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn clip(s: &str) -> String {
    let s = s.replace('\n', "\\n");
    if s.chars().count() <= 200 {
        return s;
    }
    format!("{}...", s.chars().take(200).collect::<String>())
}
