//! Purpose: fixture tests for remote MCP transports against in-process axum peers: Streamable
//! HTTP (JSON and SSE responses, session id, protocol version, custom headers), fallback to the
//! Rust-owned legacy HTTP+SSE transport, invalid/unreachable URLs, and connection close.
//! Scope: integration tests through `McpService`. Label: fixture (local loopback peers), not live.
//! Uses:
//! rust/crates/
//! |-- integration-adapters/tests/mcp_support/http.rs
//! |-- integration-adapters/tests/mcp_support/mod.rs
//! `-- integration-api/src/mcp.rs
//! External:
//! |-- serde_json::Map
//! |-- tokio::test
//! `-- tokio::time::sleep
//! Used by:
//! `-- none, entrypoint
//! Invariants: peers bind 127.0.0.1:0 and are aborted at test end.
//! Contract tests: integration-adapters/tests/mcp_http.rs

mod mcp_support;

use std::collections::BTreeMap;
use std::sync::Arc;

use mcp_support::http::spawn_peer;
use mcp_support::{Recorder, fixture, service};
use opencode_integration_api::mcp::{McpService, OAuthSetting, RemoteConfig, ServerConfig, Status};
use serde_json::Map;

fn remote(url: &str, headers: &[(&str, &str)]) -> ServerConfig {
    ServerConfig::Remote(RemoteConfig {
        url: url.into(),
        enabled: None,
        headers: (!headers.is_empty()).then(|| {
            headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        }),
        oauth: None,
        timeout: Some(5000),
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn fixture_streamable_http_json_and_sse() {
    let fx = fixture();
    let (peer, task) = spawn_peer(true).await;
    let svc = service(&fx, BTreeMap::new(), Arc::new(Recorder::default()));
    let url = format!("{}/mcp", peer.base);
    let st = svc
        .add("h", remote(&url, &[("X-Api-Key", "k1")]))
        .await
        .unwrap();
    assert_eq!(st["h"], Status::Connected);
    assert!(svc.tools().await.contains_key("h_ping"));
    let r = svc.call_tool("h", "ping", Map::new(), None).await.unwrap();
    assert_eq!(r["content"][0]["text"], "pong");

    *peer.sse_responses.lock().unwrap() = true;
    let r = svc.call_tool("h", "ping", Map::new(), None).await.unwrap();
    assert_eq!(
        r["content"][0]["text"], "pong",
        "SSE-framed POST responses are parsed"
    );

    let headers = peer.headers.lock().unwrap().clone();
    assert!(
        headers
            .iter()
            .all(|h| h.get("x-api-key").is_some_and(|v| v == "k1"))
    );
    assert!(headers[0].get("mcp-session-id").is_none());
    let last = headers.last().unwrap();
    assert_eq!(last.get("mcp-session-id").unwrap(), "sess-1");
    assert_eq!(last.get("mcp-protocol-version").unwrap(), "2025-11-25");
    assert!(headers.iter().all(|h| h.get("authorization").is_none()));
    svc.shutdown().await;
    task.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn fixture_legacy_sse_fallback() {
    let fx = fixture();
    let (peer, task) = spawn_peer(true).await;
    let svc = service(&fx, BTreeMap::new(), Arc::new(Recorder::default()));
    let mut cfg = remote(&format!("{}/sse", peer.base), &[("X-Api-Key", "k2")]);
    if let ServerConfig::Remote(r) = &mut cfg {
        r.oauth = Some(OAuthSetting::Disabled(
            opencode_integration_api::mcp::FalseLiteral,
        ));
    }
    let st = svc.add("legacy", cfg).await.unwrap();
    assert_eq!(
        st["legacy"],
        Status::Connected,
        "streamable POST 404 falls back to SSE"
    );
    let r = svc
        .call_tool("legacy", "ping", Map::new(), None)
        .await
        .unwrap();
    assert_eq!(r["content"][0]["text"], "pong");
    let headers = peer.headers.lock().unwrap().clone();
    assert!(
        headers
            .iter()
            .all(|h| h.get("x-api-key").is_some_and(|v| v == "k2"))
    );
    svc.shutdown().await;
    task.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn fixture_remote_failures() {
    let fx = fixture();
    let svc = service(&fx, BTreeMap::new(), Arc::new(Recorder::default()));
    let st = svc.add("bad", remote("not a url", &[])).await.unwrap();
    assert_eq!(
        st["bad"],
        Status::Failed {
            error: "Invalid MCP URL for \"bad\"".into()
        }
    );

    let port = mcp_support::http::free_port();
    let st = svc
        .add("down", remote(&format!("http://127.0.0.1:{port}/mcp"), &[]))
        .await
        .unwrap();
    let Status::Failed { error } = &st["down"] else {
        panic!("expected failed, got {:?}", st["down"])
    };
    assert!(
        error.starts_with("SSE error:"),
        "last transport (SSE) error wins: {error}"
    );

    // A peer that drops: closing the connection marks the server failed.
    let (peer, task) = spawn_peer(true).await;
    let st = svc
        .add("gone", remote(&format!("{}/sse", peer.base), &[]))
        .await
        .unwrap();
    assert_eq!(st["gone"], Status::Connected);
    peer.drop_sse();
    let mut closed = false;
    for _ in 0..100 {
        if svc.status().await["gone"]
            == (Status::Failed {
                error: "Connection closed".into(),
            })
        {
            closed = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(closed, "SSE stream end marks the server failed");
    svc.shutdown().await;
    task.abort();
}
