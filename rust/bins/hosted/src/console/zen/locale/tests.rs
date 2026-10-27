//! Purpose: compare native gateway localization with actual TypeScript dictionary and selector fixtures.
//! Scope: all eighteen locales, twenty-one error keys, locale precedence and literal interpolation.
//! Uses:
//! rust/bins/
//! |-- hosted/src/console/state.rs
//! |-- hosted/src/console/zen.rs
//! `-- hosted/src/console/zen/locale.rs
//! rust/crates/
//! `-- hosted-adapters/src/console/db.rs
//! External:
//! |-- axum::{body, http}
//! |-- http::{HeaderMap, HeaderValue}
//! |-- serde_json::Value
//! |-- tokio::test
//! `-- tower::ServiceExt
//! Used by:
//! `-- none, entrypoint
//! Invariants: fixtures captured from baseline modules; no live provider or production data calls.
//! Contract tests: hosted/src/console/zen/locale/tests.rs

use super::{selection, translate};
use http::{HeaderMap, HeaderValue};
use serde_json::Value;

#[test]
fn actual_typescript_dictionary_and_selection_parity() {
    // 2026-10-27 capture: actual i18n/*.ts dictionaries plus language.ts localeFromRequest,
    // with handler.ts's exact resolve replacement semantics, executed under isolated HOME/XDG.
    let fixtures: Vec<Value> = serde_json::from_str(include_str!("fixtures.json")).unwrap();
    assert_eq!(fixtures.len(), 398);
    for (index, fixture) in fixtures.iter().enumerate() {
        let mut headers = HeaderMap::new();
        for (name, value) in fixture["headers"].as_object().unwrap() {
            headers.insert(
                http::header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_str(value.as_str().unwrap()).unwrap(),
            );
        }
        let path = fixture["path"].as_str().unwrap();
        assert_eq!(
            selection::select(&headers, path),
            fixture["locale"].as_str().unwrap(),
            "selector fixture {index}"
        );
        assert_eq!(
            translate(fixture["message"].as_str().unwrap(), &headers, path),
            fixture["expected"].as_str().unwrap(),
            "translation fixture {index}, key {}",
            fixture["key"]
        );
    }
}

#[test]
fn unknown_native_messages_and_placeholder_metacharacters_remain_literal() {
    let mut headers = HeaderMap::new();
    headers.insert("x-opencode-locale", HeaderValue::from_static("fr"));
    for message in [
        "Internal Server Error",
        "Error from provider (x): Missing API key.",
        "{\"error\":\"Unknown upstream body\"}",
        "Model x is not supported\ntrailing unrelated message",
    ] {
        assert_eq!(translate(message, &headers, "/zen/v1/messages"), message);
    }
    let output = translate(
        "Model $& ${name} \\backslash is not supported",
        &headers,
        "/zen/v1/messages",
    );
    assert!(output.contains("$& ${name} \\backslash"));
    let output = translate(
        "Your workspace has reached its monthly spending limit of $12.50. Manage your limits here: https://local.test/billing?x=$&",
        &headers,
        "/zen/v1/messages",
    );
    assert!(output.contains("12.50"));
    assert!(output.contains("https://local.test/billing?x=$&"));
}

#[tokio::test]
async fn native_http_error_is_localized_without_database_or_provider_call() {
    use tower::ServiceExt;
    let config = crate::console::state::ConsoleConfig {
        zen: crate::console::zen::Config::from_catalog(
            serde_json::json!({"zenModels":{},"liteModels":{},"providers":{}}),
        )
        .unwrap(),
        ..Default::default()
    };
    let app = crate::console::zen::routes().with_state(crate::console::state::ConsoleState {
        config: std::sync::Arc::new(config),
        db: opencode_hosted_adapters::console::db::ConsoleDb::connect(
            "mysql://unused:unused@127.0.0.1:1/unused",
        )
        .unwrap(),
    });
    let response = app
        .oneshot(
            axum::http::Request::post("/zen/v1/messages")
                .header("x-opencode-locale", "fr")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    r#"{"model":"not-configured","messages":[]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::UNAUTHORIZED);
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    let mut headers = HeaderMap::new();
    headers.insert("x-opencode-locale", HeaderValue::from_static("fr"));
    assert_eq!(
        body["error"]["message"],
        translate(
            "Model not-configured is not supported",
            &headers,
            "/zen/v1/messages"
        )
    );
    assert_ne!(
        body["error"]["message"],
        "Model not-configured is not supported"
    );
}

#[test]
fn malformed_cookie_is_safe_and_valid_cookie_ignores_later_duplicate() {
    let mut headers = HeaderMap::new();
    headers.insert("cookie", HeaderValue::from_static("oc_locale=%zz"));
    headers.insert("accept-language", HeaderValue::from_static("ja"));
    // TypeScript decodeURIComponent throws on malformed encodings. Native selection safely
    // falls through rather than allowing an invalid user cookie to panic the request.
    assert_eq!(selection::select(&headers, "/zen"), "ja");
    headers.insert(
        "cookie",
        HeaderValue::from_static("oc_locale=fr; oc_locale=de"),
    );
    assert_eq!(selection::select(&headers, "/zen"), "fr");
}
