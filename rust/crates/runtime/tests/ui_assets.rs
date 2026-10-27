//! Purpose: exercise embedded assets and the real UI proxy through the production HTTP listener.
//! Scope: path fallback, MIME/CSP, method/body forwarding and genuine upstream failures.
//! Uses:
//! rust/crates/
//! |-- runtime/src/process.rs
//! `-- server/src/lifecycle.rs
//! External:
//! |-- axum::Router
//! |-- reqwest::Client
//! `-- tempfile::tempdir
//! Used by:
//! `-- none, entrypoint
//! Invariants: all listeners and files are isolated; no public upstream is contacted.
//! Contract tests: this file

use std::collections::BTreeMap;

use axum::body::Body;
use axum::http::{Request, Response, StatusCode};
use axum::routing::any;
use opencode_runtime::{Process, ProcessOptions};
use opencode_server::lifecycle::ListenOptions;
use serde_json::{Value, json};

async fn serve(process: &Process) -> opencode_server::lifecycle::Listener {
    process
        .serve(
            ListenOptions {
                hostname: "127.0.0.1".into(),
                port: 0,
            },
            Vec::new(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn embedded_assets_use_manifest_fallback_and_real_html_csp() {
    let root = tempfile::tempdir().unwrap();
    let index = root.path().join("index.html");
    let css = root.path().join("app.css");
    std::fs::write(&index, "<html><script src='external.js' id='oc-theme-preload-script'>ignored</script><script id=\"oc-theme-preload-script\">abc</script></html>").unwrap();
    std::fs::write(&css, "body{color:red}").unwrap();
    let mut options = ProcessOptions::isolated(&root.path().join("state"));
    options.ui_assets = Some(BTreeMap::from([
        ("index.html".into(), index),
        ("app.css".into(), css),
        ("missing.js".into(), root.path().join("absent.js")),
    ]));
    let process = Process::start(options).await.unwrap();
    let listener = serve(&process).await;
    let client = reqwest::Client::new();
    let response = client
        .get(format!(
            "{}/ui-assets-test/deep?ignored=true",
            listener.url()
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "text/html");
    assert!(
        response.headers()["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("'sha256-ungWv48Bz+pBQUDeXa4iI7ADYaOWF3qctBD/YfIAFa0='")
    );
    assert!(response.text().await.unwrap().contains("external.js"));
    let response = client
        .get(format!("{}/app.css", listener.url()))
        .send()
        .await
        .unwrap();
    assert_eq!(response.headers()["content-type"], "text/css");
    assert!(response.headers().get("content-security-policy").is_none());
    assert_eq!(response.text().await.unwrap(), "body{color:red}");
    let response = client
        .get(format!("{}/missing.js", listener.url()))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        response.json::<Value>().await.unwrap(),
        json!({"error":"Not Found"})
    );
    let response = client
        .head(format!("{}/ui-assets-test/deep", listener.url()))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.bytes().await.unwrap().is_empty());
    listener.stop(true).await;
    process.shutdown().await;
}

#[tokio::test]
async fn proxy_forwards_real_method_body_and_sanitized_headers_without_query() {
    let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = socket.local_addr().unwrap();
    let (sender, mut receiver) = tokio::sync::mpsc::channel(4);
    let app = axum::Router::new().fallback(any(move |request: Request<Body>| {
        let sender = sender.clone();
        async move {
            let (parts, body) = request.into_parts();
            let bytes = axum::body::to_bytes(body, 1024).await.unwrap();
            sender.send((parts, bytes)).await.unwrap();
            Response::builder()
                .status(201)
                .header("content-type", "text/plain")
                .header("x-upstream", "yes")
                .body(Body::from("actual upstream body"))
                .unwrap()
        }
    }));
    let stop = tokio_util::sync::CancellationToken::new();
    let shutdown = stop.clone();
    let upstream = tokio::spawn(async move {
        axum::serve(socket, app)
            .with_graceful_shutdown(shutdown.cancelled_owned())
            .await
            .unwrap();
    });
    let root = tempfile::tempdir().unwrap();
    let mut options = ProcessOptions::isolated(&root.path().join("state"));
    options.ui_upstream = format!("http://{address}/base?also=discarded");
    let process = Process::start(options).await.unwrap();
    let listener = serve(&process).await;
    let response = reqwest::Client::new()
        .post(format!("{}/asset-route?discarded=yes", listener.url()))
        .header("x-opencode-directory", root.path().to_str().unwrap())
        .header("x-opencode-workspace", "not-forwarded")
        .header("x-custom", "preserved")
        .header("authorization", "Bearer local-test")
        .body("real request body")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(response.headers()["x-upstream"], "yes");
    assert!(response.headers().contains_key("content-security-policy"));
    assert_eq!(response.text().await.unwrap(), "actual upstream body");
    let (parts, body) = receiver.recv().await.unwrap();
    assert_eq!(parts.method, "POST");
    assert_eq!(parts.uri.to_string(), "/asset-route");
    assert_eq!(parts.headers["host"], address.to_string());
    assert_eq!(parts.headers["x-custom"], "preserved");
    assert_eq!(parts.headers["authorization"], "Bearer local-test");
    assert!(parts.headers.get("x-opencode-directory").is_none());
    assert!(parts.headers.get("x-opencode-workspace").is_none());
    assert_eq!(body.as_ref(), b"real request body");
    listener.stop(true).await;
    process.shutdown().await;
    stop.cancel();
    upstream.await.unwrap();
}

#[tokio::test]
async fn missing_upstream_is_an_actual_transport_defect_not_an_absent_ui_port() {
    let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = socket.local_addr().unwrap();
    drop(socket);
    let root = tempfile::tempdir().unwrap();
    let mut options = ProcessOptions::isolated(&root.path().join("state"));
    options.ui_upstream = format!("http://{address}");
    let process = Process::start(options).await.unwrap();
    let listener = serve(&process).await;
    let response = reqwest::Client::new()
        .get(format!("{}/does-not-exist-route", listener.url()))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = response.text().await.unwrap();
    assert!(!body.contains("UiAssets unavailable"), "{body}");
    listener.stop(true).await;
    process.shutdown().await;
}
