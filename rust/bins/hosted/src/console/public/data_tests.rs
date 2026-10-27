//! Purpose: accept stats proxy, changelog and downloads against controlled local upstreams.
//! Scope: auxiliary public router fixture tests.
//! Uses:
//! rust/bins/
//! |-- hosted/src/console/public.rs
//! `-- hosted/src/console/public/tests.rs
//! External:
//! |-- axum::Router
//! `-- tokio::test
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/public.rs
//! Invariants: all upstreams bind loopback and no production requests are sent.
//! Contract tests: this file

use super::tests::{call, state};
use super::*;
use axum::Router;
use axum::body::to_bytes;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[tokio::test]
async fn fixture_stats_cache_locale_markdown_assets_redirects_and_html() {
    let count = Arc::new(AtomicUsize::new(0));
    let upstream = Router::new().fallback({
        let count = count.clone();
        move |request: axum::extract::Request| {
            let count = count.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                if request.uri().path() == "/data/redirect" {
                    let location = format!("http://{}/data/canonical?ok=1#part", request.headers()["host"].to_str().unwrap());
                    return (StatusCode::FOUND, [("location", location)], "redirect").into_response();
                }
                let path = request.uri().to_string();
                let language = request.headers()["accept-language"].to_str().unwrap().to_owned();
                let explicit = request.headers()["x-opencode-locale"].to_str().unwrap().to_owned();
                let method = request.method().to_string();
                let markdown = request.headers().get("accept").and_then(|value| value.to_str().ok()).is_some_and(|value| value.contains("markdown"));
                let bytes = to_bytes(request.into_body(), usize::MAX).await.unwrap();
                if markdown || path.starts_with("/_build/") || path.starts_with("/data/api/") {
                    return Json(json!({"path":path,"language":language,"locale":explicit,"method":method,"body": String::from_utf8_lossy(&bytes)})).into_response();
                }
                ([("content-type", "text/html"), ("cache-control", "public, max-age=60"), ("etag", "upstream-tag"), ("set-cookie", "secret=upstream"), ("vary", "Accept")], format!("<script src=\"/_build/app.js\"></script><a href='/_build/a'>{language}:{path}</a>")).into_response()
            }
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    let app = routes().with_state(state(Config {
        data_origin: Some(base),
        ..Default::default()
    }));
    let (status, headers, _) = call(
        app.clone(),
        "GET",
        "/data?add=a%2Fb",
        &[("accept-language", "fr")],
        "",
    )
    .await;
    assert_eq!(status, StatusCode::PERMANENT_REDIRECT);
    assert_eq!(
        headers["location"],
        "https://console.example/fr/data?add=a%2Fb"
    );
    assert!(
        headers["set-cookie"]
            .to_str()
            .unwrap()
            .starts_with("oc_locale=fr;")
    );
    assert_eq!(count.load(Ordering::SeqCst), 0);
    let (status, headers, body) = call(app.clone(), "GET", "/fr/data?add=localized", &[], "").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers["set-cookie"]
            .to_str()
            .unwrap()
            .starts_with("oc_locale=fr;")
    );
    assert!(
        String::from_utf8(body)
            .unwrap()
            .contains("fr:/data?add=localized")
    );
    count.store(0, Ordering::SeqCst);
    let (status, headers, body) = call(
        app.clone(),
        "GET",
        "/data?add=a&ignored=1",
        &[("x-opencode-locale", "fr")],
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["x-opencode-stats-cache"], "MISS");
    assert!(!headers.contains_key("etag"));
    assert!(!headers.contains_key("set-cookie"));
    assert_eq!(
        headers["vary"],
        "Accept, Accept-Language, Cookie, x-opencode-locale"
    );
    assert!(
        String::from_utf8(body)
            .unwrap()
            .contains("\"/data/_build/app.js\"")
    );
    let (_, headers, _) = call(
        app.clone(),
        "GET",
        "/data?add=a&ignored=2",
        &[("x-opencode-locale", "fr")],
        "",
    )
    .await;
    assert_eq!(headers["x-opencode-stats-cache"], "HIT");
    assert_eq!(count.load(Ordering::SeqCst), 1);
    let (_, headers, _) = call(
        app.clone(),
        "GET",
        "/data?add=b",
        &[("x-opencode-locale", "fr")],
        "",
    )
    .await;
    assert_eq!(headers["x-opencode-stats-cache"], "MISS");
    let (_, headers, _) = call(
        app.clone(),
        "GET",
        "/data?add=a",
        &[("x-opencode-locale", "ja")],
        "",
    )
    .await;
    assert_eq!(headers["x-opencode-stats-cache"], "MISS");
    let (_, headers, body) = call(
        app.clone(),
        "GET",
        "/data",
        &[
            ("accept", "text/markdown, text/html;q=0.5"),
            ("accept-language", "fr"),
        ],
        "",
    )
    .await;
    assert!(!headers.contains_key("x-opencode-stats-cache"));
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap()["language"],
        "fr"
    );
    let (_, _, body) = call(
        app.clone(),
        "GET",
        "/data/_build/a.js?x=1",
        &[("accept-language", "de")],
        "",
    )
    .await;
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap()["path"],
        "/_build/a.js?x=1"
    );
    let (_, _, body) = call(
        app.clone(),
        "PATCH",
        "/data/api/write",
        &[("accept-language", "es")],
        "patch-body",
    )
    .await;
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["method"], "PATCH");
    assert_eq!(body["body"], "patch-body");
    let (status, headers, _) = call(
        app,
        "GET",
        "/data/redirect",
        &[("x-opencode-locale", "en")],
        "",
    )
    .await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(headers["location"], "/data/canonical?ok=1#part");
    task.abort();
    let _ = task.await;
}

#[tokio::test]
async fn fixture_changelog_parser_cache_cors_and_upstream_failure() {
    let count = Arc::new(AtomicUsize::new(0));
    let upstream = Router::new().route("/releases", axum::routing::get({
        let count = count.clone(); move |headers: axum::http::HeaderMap| { let count = count.clone(); async move {
            count.fetch_add(1, Ordering::SeqCst);
            assert_eq!(headers["user-agent"], "OpenCode-Console");
            Json(json!([{"tag_name":"v1","name":"Release","published_at":"2026-10-27","html_url":"https://example.test/release","body":"## Fixed\n- Keep query\n**Thank you contributors**\n- skip me\n## Added\n- Feature\n<highlight source=\"desktop\"><h2>Title</h2><p short=\"Short\">Description</p><img width=\"10\" height=\"20\" alt=\"x\" src=\"https://example.test/a.png\" /></highlight>\n<highlight source=\"desktop\"><h2>Video</h2>\nhttps://github.com/user-attachments/assets/abc-def\n</highlight>"}]))
        } }
    })).route("/failure", axum::routing::get(|| async { StatusCode::SERVICE_UNAVAILABLE }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    let app = routes().with_state(state(Config {
        releases_url: format!("{base}/releases"),
        ..Default::default()
    }));
    let (status, headers, body) = call(app.clone(), "GET", "/changelog.json", &[], "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["access-control-allow-origin"], "*");
    assert!(
        headers["cache-control"]
            .to_str()
            .unwrap()
            .contains("s-maxage=300")
    );
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        body["releases"][0]["sections"],
        json!([{"title":"Fixed","items":["Keep query"]},{"title":"Added","items":["Feature"]}])
    );
    assert_eq!(
        body["releases"][0]["highlights"][0]["items"][0]["shortDescription"],
        "Short"
    );
    assert_eq!(
        body["releases"][0]["highlights"][0]["items"][1]["media"]["type"],
        "video"
    );
    call(app.clone(), "GET", "/changelog.json", &[], "").await;
    assert_eq!(count.load(Ordering::SeqCst), 1);
    let (status, headers, body) = call(app, "OPTIONS", "/changelog.json", &[], "").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.is_empty());
    assert_eq!(headers["access-control-allow-methods"], "GET, OPTIONS");
    let app = routes().with_state(state(Config {
        releases_url: format!("{base}/failure"),
        ..Default::default()
    }));
    let (status, headers, body) = call(app, "GET", "/changelog.json", &[], "").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        headers["cache-control"]
            .to_str()
            .unwrap()
            .contains("s-maxage=60")
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap(),
        json!({"releases":[]})
    );
    task.abort();
    let _ = task.await;
}

#[tokio::test]
async fn fixture_download_channel_assets_missing_and_upstream_status() {
    let upstream = Router::new().route("/update/api/latest/desktop/opencode", axum::routing::get(|| async { Json(json!({"metadata":{"files":{"opencode-desktop-mac-arm64.dmg":{"url":"https://downloads.example/stable.dmg"}}}})) }))
        .route("/update/api/beta/desktop/opencode", axum::routing::get(|| async { StatusCode::SERVICE_UNAVAILABLE }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    let app = routes().with_state(state(Config {
        update_origin: base,
        ..Default::default()
    }));
    let (status, headers, _) = call(
        app.clone(),
        "GET",
        "/download/stable/darwin-aarch64-dmg",
        &[],
        "",
    )
    .await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(headers["location"], "https://downloads.example/stable.dmg");
    assert_eq!(
        call(
            app.clone(),
            "GET",
            "/download/stable/linux-x64-deb",
            &[],
            ""
        )
        .await
        .0,
        StatusCode::BAD_GATEWAY
    );
    assert_eq!(
        call(app, "GET", "/download/anything/linux-x64-deb", &[], "")
            .await
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    task.abort();
    let _ = task.await;
}
