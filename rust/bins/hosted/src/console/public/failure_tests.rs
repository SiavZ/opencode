//! Purpose: verify public upstream failure policy, cache expiry and GitHub projections.
//! Scope: loopback HTTP acceptance without baseline or production changes.
//! Uses:
//! rust/bins/
//! |-- hosted/src/console/public.rs
//! `-- hosted/src/console/public/tests.rs
//! External:
//! `-- axum::Router
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/public.rs
//! Invariants: stale cache is not silently returned in place of failed upstream.
//! Contract tests: this file

use super::tests::{call, state};
use super::*;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;

#[tokio::test]
async fn fixture_data_upstream_failure_and_expired_cache_are_not_success() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().fallback(|| async {
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    [("content-type", "text/html")],
                    "upstream unavailable",
                )
            }),
        )
        .await
        .unwrap();
    });
    let config = Config {
        data_origin: Some(base),
        ..Default::default()
    };
    config.cache.put(
        "stats:https://console.example/data?__opencode_stats_locale=en".into(),
        StatusCode::OK,
        HeaderMap::new(),
        b"cached fixture".to_vec(),
        1,
    );
    let app = routes().with_state(state(config));
    let (status, headers, body) = call(app.clone(), "GET", "/data", &[], "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["x-opencode-stats-cache"], "HIT");
    assert_eq!(body, b"cached fixture");
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let (status, headers, body) = call(app.clone(), "GET", "/data", &[], "").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(!headers.contains_key("x-opencode-stats-cache"));
    assert_eq!(body, b"upstream unavailable");
    task.abort();
    let _ = task.await;
    // Dropping a listening server does not guarantee existing pooled sockets close.
    // Reserve then close a fresh port and use a fresh client to prove connect failure.
    let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", closed.local_addr().unwrap());
    drop(closed);
    let app = routes().with_state(state(Config {
        data_origin: Some(base),
        ..Default::default()
    }));
    let (status, _, _) = call(app, "GET", "/data", &[], "").await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn fixture_compressed_data_html_decodes_rewrites_and_clears_encoding_length() {
    let bytes: &[u8] = &[
        31, 139, 8, 0, 0, 0, 0, 0, 0, 3, 179, 41, 78, 46, 202, 44, 40, 81, 40, 46, 74, 182, 85,
        210, 143, 79, 42, 205, 204, 73, 209, 79, 206, 207, 45, 40, 74, 45, 46, 78, 77, 209, 203,
        42, 86, 178, 179, 209, 135, 40, 178, 3, 0, 91, 253, 229, 219, 45, 0, 0, 0,
    ];
    let upstream = Router::new().fallback(move |headers: HeaderMap| async move {
        assert_eq!(headers["accept-encoding"], "gzip, identity;q=0.2");
        (
            [
                ("content-type", "text/html"),
                ("content-encoding", "gzip"),
                ("content-length", "61"),
                ("cache-control", "public, max-age=10"),
            ],
            bytes,
        )
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    let app = routes().with_state(state(Config {
        data_origin: Some(base),
        ..Default::default()
    }));
    let (status, headers, body) = call(
        app,
        "GET",
        "/data",
        &[
            ("accept-encoding", "gzip, identity;q=0.2"),
            ("host", "evil.example"),
        ],
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!headers.contains_key("content-encoding"));
    // Axum may synthesize the new decoded length; an upstream compressed length must not survive.
    assert_ne!(
        headers
            .get("content-length")
            .and_then(|value| value.to_str().ok()),
        Some("61")
    );
    assert_eq!(
        String::from_utf8(body).unwrap(),
        "<script src=\"/data/_build/compressed.js\"></script>"
    );
    task.abort();
    let _ = task.await;
}

#[tokio::test]
async fn fixture_github_metadata_first_release_contributor_link_and_null_failure() {
    let upstream = Router::new()
        .route("/repo", axum::routing::get(|headers: HeaderMap| async move { assert!(headers["user-agent"].to_str().unwrap().starts_with("Mozilla/5.0")); Json(json!({"stargazers_count":123})) }))
        .route("/repo/releases", axum::routing::get(|| async { Json(json!([{"name":"First","html_url":"https://example.test/v1","tag_name":"v1"},{"name":"Second"}])) }))
        .route("/repo/contributors", axum::routing::get(|request: axum::extract::Request| async move { assert_eq!(request.uri().query(), Some("per_page=1")); ([("link", "<https://api.github.com/contributors?per_page=1&page=42>; rel=\"last\"")], "[]").into_response() }))
        .route("/empty", axum::routing::get(|| async { Json(json!({})) }))
        .route("/empty/releases", axum::routing::get(|| async { Json(json!([])) }))
        .route("/empty/contributors", axum::routing::get(|| async { "[]" }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    let app = routes().with_state(state(Config {
        github_api_base: format!("{base}/repo"),
        ..Default::default()
    }));
    let (status, _, body) = call(app, "GET", "/github.json", &[("host", "evil.example")], "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap(),
        json!({"stars":123,"contributors":42,"release":{"name":"First","url":"https://example.test/v1","tag_name":"v1"}})
    );
    let app = routes().with_state(state(Config {
        github_api_base: format!("{base}/empty"),
        ..Default::default()
    }));
    let (status, _, body) = call(app, "GET", "/github.json", &[], "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(serde_json::from_slice::<Value>(&body).unwrap(), Value::Null);
    task.abort();
    let _ = task.await;
}
