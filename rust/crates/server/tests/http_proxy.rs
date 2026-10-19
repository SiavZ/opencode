//! Purpose: legacy workspace routing and remote proxy over real sockets with a local upstream:
//! hop header stripping, query rewriting, streamed success, preserved 5xx, transport failure,
//! unknown workspace and broken sync answers.
//! Scope: live local listeners; a test-only legacy endpoint stands in for the legacy registry
//! (p06.legacy-registry) and a fixture `LegacyWorkspaces` for Phase 02.
//! Uses:
//! rust/crates/
//! |-- protocol/src/lib.rs
//! |-- protocol/src/operation.rs
//! |-- server/src/app.rs
//! |-- server/src/lifecycle.rs
//! |-- server/src/services/mod.rs
//! `-- server/tests/support/mod.rs
//! External:
//! |-- app::router_with
//! |-- async_trait::async_trait
//! |-- axum::Router
//! |-- axum::http::HeaderMap
//! |-- axum::http::StatusCode
//! |-- axum::http::Uri
//! |-- axum::routing::get
//! |-- axum::routing::post
//! |-- serde_json::Value
//! |-- tokio::net::TcpListener::bind
//! `-- tokio::test
//! Used by:
//! `-- none, entrypoint
//! Invariants: mirrors httpapi/middleware/workspace-routing.ts and proxy.ts.
//! Contract tests: this file.

mod support;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::Router;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use opencode_protocol::operation::Middleware;
use opencode_protocol::{Api, Endpoint, Group, Method};
use opencode_server::app::{self, ServerConfig};
use opencode_server::services::{LegacyWorkspaces, Services, WorkspaceInfo, WorkspaceTarget};

struct Workspaces {
    target: String,
    syncing: bool,
}

#[async_trait]
impl LegacyWorkspaces for Workspaces {
    fn configured_workspace_id(&self) -> Option<String> {
        None
    }
    async fn session(&self, _: &str) -> Option<(String, Option<String>)> {
        None
    }
    async fn workspace(&self, id: &str) -> Option<WorkspaceInfo> {
        (id == "wrk_remote").then(|| WorkspaceInfo { id: id.to_string() })
    }
    async fn target(&self, _: &WorkspaceInfo) -> WorkspaceTarget {
        WorkspaceTarget::Remote {
            url: self.target.clone(),
            headers: vec![("x-target-auth".into(), "t".into())],
        }
    }
    async fn is_syncing(&self, _: &str) -> bool {
        self.syncing
    }
}

fn legacy_api() -> &'static Api {
    let endpoint = |name: &str, method, path: &str| {
        Endpoint::new(name, name, method, path).middleware(Middleware::LegacyInstance)
    };
    Box::leak(Box::new(Api {
        identifier: "test".into(),
        title: "test".into(),
        version: "0".into(),
        description: None,
        groups: vec![
            Group::new("test.legacy", None, None)
                .endpoint(endpoint("config.get", Method::Get, "/config"))
                .endpoint(endpoint("config.update", Method::Post, "/config"))
                .endpoint(endpoint("boom", Method::Get, "/boom")),
        ],
        definitions: Vec::new(),
    }))
}

type Seen = Arc<Mutex<Vec<(String, HeaderMap)>>>;

async fn upstream(seen: Seen) -> opencode_server::lifecycle::Listener {
    let s1 = seen.clone();
    let s2 = seen.clone();
    let router = Router::new()
        .route(
            "/base/config",
            get(move |uri: axum::http::Uri, headers: HeaderMap| async move {
                s1.lock().unwrap().push((uri.to_string(), headers));
                (
                    [("content-encoding", "identity"), ("x-up", "1")],
                    "upstream-body",
                )
            }),
        )
        .route(
            "/base/config",
            post(move |headers: HeaderMap, body: String| async move {
                s2.lock().unwrap().push(("POST".into(), headers));
                body
            }),
        )
        .route(
            "/base/boom",
            get(|| async {
                (
                    StatusCode::BAD_GATEWAY,
                    [("content-type", "text/plain")],
                    "bad upstream",
                )
            }),
        );
    support::start(router).await
}

async fn proxy_server(target: String, syncing: bool) -> opencode_server::lifecycle::Listener {
    let services = Services {
        workspaces: Some(Arc::new(Workspaces { target, syncing })),
        ..Services::default()
    };
    let router = app::router_with(&[legacy_api()], services, ServerConfig::default());
    support::start(router).await
}

#[tokio::test]
async fn forwards_with_sanitized_headers_and_query() {
    let seen: Seen = Arc::default();
    let up = upstream(seen.clone()).await;
    let server = proxy_server(format!("{}/base", up.url()), true).await;
    let response = support::client()
        .get(format!(
            "{}/config?workspace=wrk_remote&directory=/x&keep=1",
            server.url()
        ))
        .header("x-opencode-directory", "/secret")
        .header("connection", "keep-alive")
        .header("x-custom", "c")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert!(response.headers().get("content-encoding").is_none());
    assert_eq!(response.headers()["x-up"], "1");
    assert_eq!(response.text().await.unwrap(), "upstream-body");
    let (uri, headers) = seen.lock().unwrap()[0].clone();
    assert_eq!(uri, "/base/config?keep=1");
    assert!(headers.get("x-opencode-directory").is_none());
    assert_eq!(headers["x-custom"], "c");
    assert_eq!(headers["x-target-auth"], "t");
    server.stop(false).await;
    up.stop(false).await;
}

#[tokio::test]
async fn forwards_request_bodies() {
    let seen: Seen = Arc::default();
    let up = upstream(seen.clone()).await;
    let server = proxy_server(format!("{}/base", up.url()), true).await;
    let response = support::client()
        .post(format!("{}/config?workspace=wrk_remote", server.url()))
        .body("payload")
        .send()
        .await
        .unwrap();
    assert_eq!(response.text().await.unwrap(), "payload");
    server.stop(false).await;
    up.stop(false).await;
}

#[tokio::test]
async fn preserves_upstream_5xx_body_and_type() {
    let up = upstream(Arc::default()).await;
    let server = proxy_server(format!("{}/base", up.url()), true).await;
    let response = support::client()
        .get(format!("{}/boom?workspace=wrk_remote", server.url()))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 502);
    assert_eq!(response.headers()["content-type"], "text/plain");
    assert_eq!(response.text().await.unwrap(), "bad upstream");
    server.stop(false).await;
    up.stop(false).await;
}

#[tokio::test]
async fn transport_failure_is_empty_500() {
    let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = closed.local_addr().unwrap();
    drop(closed);
    let server = proxy_server(format!("http://{address}/base"), true).await;
    let response = support::client()
        .get(format!("{}/config?workspace=wrk_remote", server.url()))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 500);
    assert_eq!(response.text().await.unwrap(), "");
    server.stop(false).await;
}

#[tokio::test]
async fn unknown_workspace_and_broken_sync() {
    let up = upstream(Arc::default()).await;
    let server = proxy_server(format!("{}/base", up.url()), false).await;
    let response = support::client()
        .get(format!("{}/config?workspace=wrk_nope", server.url()))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 500);
    assert_eq!(
        response.text().await.unwrap(),
        "Workspace not found: wrk_nope"
    );
    let response = support::client()
        .get(format!("{}/config?workspace=wrk_remote", server.url()))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 503);
    assert_eq!(
        response.text().await.unwrap(),
        "broken sync connection for workspace: wrk_remote"
    );
    server.stop(false).await;
    up.stop(false).await;
}

#[tokio::test]
async fn local_route_without_workspace_reaches_typed_unported_handler() {
    let server = proxy_server("http://127.0.0.1:1".into(), true).await;
    let response = support::client()
        .get(format!("{}/config", server.url()))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 503);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["service"], "config.get");
    server.stop(false).await;
}
