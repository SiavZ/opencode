//! Purpose: verify auxiliary console endpoints with local HTTP and isolated MySQL fixtures.
//! Scope: real router requests, no external network or user data.
//! Uses:
//! rust/bins/
//! `-- hosted/src/console/public.rs
//! rust/crates/
//! `-- hosted-adapters/src/console/db.rs
//! External:
//! |-- axum::Router
//! |-- mysql_async::prelude::Queryable
//! `-- tower::ServiceExt
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/public.rs
//! Invariants: fixture upstream listens on loopback only and is aborted after the test.
//! Contract tests: this file

use super::super::state::{ConsoleConfig, ConsoleState};
use super::*;
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use axum::response::IntoResponse;
use opencode_hosted_adapters::console::db::{ConsoleDb, TestDatabase};
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

pub(super) fn state(config: Config) -> ConsoleState {
    ConsoleState {
        config: Arc::new(ConsoleConfig {
            auth: super::super::auth::Config {
                public_origin: Some("https://console.example".into()),
                ..Default::default()
            },
            public: config,
            ..Default::default()
        }),
        db: ConsoleDb::connect("mysql://fixture:fixture@127.0.0.1:1/unused").unwrap(),
    }
}

pub(super) async fn call(
    app: Router,
    method: &str,
    uri: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> (StatusCode, HeaderMap, Vec<u8>) {
    let request = headers
        .iter()
        .fold(
            Request::builder().method(method).uri(uri),
            |request, (key, value)| request.header(*key, *value),
        )
        .body(Body::from(body.to_owned()))
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    (
        status,
        headers,
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
}

#[tokio::test]
async fn fixture_public_metadata_redirects_and_localized_validation() {
    let app = routes().with_state(state(Config::default()));
    let (status, headers, body) = call(
        app.clone(),
        "GET",
        "/oauth/opencode/client.json",
        &[("host", "evil.example")],
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["cache-control"], "public, max-age=300");
    assert_eq!(headers["access-control-allow-origin"], "*");
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        body["client_id"],
        "https://console.example/oauth/opencode/client.json"
    );
    assert_eq!(
        body["redirect_uris"],
        json!(["http://127.0.0.1/callback", "http://localhost/callback"])
    );
    for (uri, target) in [
        ("/discord", "https://discord.gg/opencode"),
        ("/desktop-feedback", "https://discord.gg/h5TNnkFVNy"),
    ] {
        let (status, headers, _) = call(app.clone(), "GET", uri, &[], "").await;
        assert_eq!(status, StatusCode::FOUND);
        assert_eq!(headers["location"], target);
    }
    let (status, headers, _) =
        call(app.clone(), "PATCH", "/stats/models?add=x%2Fy", &[], "body").await;
    assert_eq!(status, StatusCode::PERMANENT_REDIRECT);
    assert_eq!(
        headers["location"],
        "https://console.example/data/models?add=x%2Fy"
    );
    let (status, _, body) = call(
        app.clone(),
        "POST",
        "/bench/submission",
        &[
            ("content-type", "application/json"),
            ("accept-language", "fr"),
        ],
        "{}",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap()["error"],
        "Tous les champs sont requis."
    );
    assert_eq!(
        call(app.clone(), "GET", "/download/stable/unknown", &[], "")
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(app, "PUT", "/oauth/opencode/client.json", &[], "")
            .await
            .0,
        StatusCode::METHOD_NOT_ALLOWED
    );
}

#[tokio::test]
async fn fixture_public_proxies_preserve_method_query_headers_body_and_locale() {
    let upstream = Router::new().fallback(|request: axum::extract::Request| async move {
        let method = request.method().to_string();
        let path = request.uri().to_string();
        let language = request.headers().get("accept-language").and_then(|value| value.to_str().ok()).unwrap_or("").to_owned();
        let bytes = to_bytes(request.into_body(), usize::MAX).await.unwrap();
        let mut response = Json(json!({"method": method, "path": path, "language": language, "body": String::from_utf8_lossy(&bytes)})).into_response();
        response.headers_mut().append("set-cookie", "upstream=1; Domain=upstream.example; Path=/".parse().unwrap());
        response.headers_mut().insert("cache-control", "public, max-age=17".parse().unwrap());
        response
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    let app = routes().with_state(state(Config {
        docs_origin: Some(base.clone()),
        enterprise_origin: base.clone(),
        openapi_url: format!("{base}/openapi"),
        ..Default::default()
    }));
    let (status, headers, body) = call(
        app.clone(),
        "POST",
        "/docs/guide?q=a%2Fb",
        &[
            ("accept-language", "zh-TW"),
            ("content-type", "text/plain"),
            ("host", "evil.example"),
            ("x-forwarded-host", "evil.example"),
        ],
        "fixture body",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["cache-control"], "public, max-age=17");
    assert_eq!(headers.get_all("set-cookie").iter().count(), 2);
    assert!(
        headers
            .get_all("set-cookie")
            .iter()
            .any(|value| value == "upstream=1; Domain=upstream.example; Path=/")
    );
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["method"], "POST");
    assert_eq!(body["path"], "/docs/zh-tw/guide?q=a%2Fb");
    assert_eq!(body["language"], "zh-Hant");
    assert_eq!(body["body"], "fixture body");
    let (_, _, body) = call(
        app.clone(),
        "GET",
        "/s/share-id?x=1",
        &[("cookie", "oc_locale=br")],
        "",
    )
    .await;
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap()["path"],
        "/docs/pt-br/s/share-id?x=1"
    );
    let (_, headers, body) = call(
        app.clone(),
        "DELETE",
        "/t/item?q=2",
        &[("cookie", "oc_locale=ja")],
        "delete",
    )
    .await;
    assert_eq!(headers.get_all("set-cookie").iter().count(), 1);
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["path"], "//t/item?q=2");
    assert_eq!(body["language"], "ja");
    let (_, _, body) = call(app, "GET", "/openapi.json", &[], "").await;
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap()["path"],
        "/openapi"
    );
    task.abort();
    let _ = task.await;
}

#[test]
fn fixture_docs_locale_paths_match_baseline_asset_and_explicit_paths() {
    for (locale, path, expected) in [
        ("en", "/docs/en/guide", "/docs/guide"),
        ("zh", "/docs", "/docs/zh-cn"),
        ("br", "/docs/_astro/a.js", "/docs/_astro/a.js"),
        ("fr", "/docs/zh-tw/guide", "/docs/zh-tw/guide"),
        ("de", "/docs/feed.xml", "/docs/feed.xml"),
    ] {
        assert_eq!(locale::docs(locale, path), expected);
    }
    assert_eq!(
        locale::select(
            &HeaderMap::from_iter([(
                "accept-language".parse().unwrap(),
                "en;q=0.2,zh-Hant;q=1".parse().unwrap()
            )]),
            "/"
        ),
        "zht"
    );
}

#[tokio::test]
async fn fixture_public_benchmark_submission_persists_baseline_fields() {
    use mysql_async::prelude::Queryable;
    let fixture = TestDatabase::create("public_benchmark").await.unwrap();
    let mut state = state(Config::default());
    state.db = fixture.db.clone();
    let app = routes().with_state(state);
    let result = json!({"averageScore": 1, "tasks": [{"averageScore": 0.5, "task": {"id": "fixture-task"}, "runs": []}]});
    let input =
        json!({"model":"fixture-model","agent":"fixture-agent","result":result.to_string()});
    let (status, _, body) = call(
        app.clone(),
        "POST",
        "/bench/submission",
        &[("content-type", "application/json")],
        &input.to_string(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap(),
        json!({"success":true})
    );
    let rows: Vec<(String, String, String, String)> = fixture
        .db
        .conn()
        .await
        .unwrap()
        .query("SELECT `id`, `model`, `agent`, `result` FROM `benchmark`")
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].0.starts_with("ben_"));
    assert_eq!(rows[0].1, "fixture-model");
    assert_eq!(rows[0].2, "fixture-agent");
    assert_eq!(rows[0].3, result.to_string());
    let (status, _, body) = call(app.clone(), "GET", "/benchmarks", &[], "").await;
    assert_eq!(status, StatusCode::OK);
    let listed: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        listed,
        json!([{"id":rows[0].0,"model":"fixture-model","agent":"fixture-agent","averageScore":1,"taskScores":{"fixture-task":0.5}}])
    );
    let (status, _, body) = call(
        app.clone(),
        "GET",
        &format!("/benchmarks/{}/tasks/fixture-task", rows[0].0),
        &[],
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap(),
        result["tasks"][0]
    );
    let (_, _, body) = call(app, "GET", "/benchmarks/ben_missing/tasks/unknown", &[], "").await;
    assert_eq!(serde_json::from_slice::<Value>(&body).unwrap(), Value::Null);
    fixture.drop_database().await.unwrap();
}
