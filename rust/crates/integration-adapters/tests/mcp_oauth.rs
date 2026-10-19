//! Purpose: fixture tests for MCP OAuth: needs_auth / needs_client_registration transitions,
//! full authenticate flow (RFC 9728 + 8414 discovery, dynamic registration, PKCE S256, state,
//! loopback callback server, code exchange, token persistence bound to the exact URL with mode
//! 0600), callback server validation/cancel/timeout, auth status and removal, store semantics.
//! Scope: integration tests through public boundaries. Label: fixture (local loopback OAuth AS,
//! simulated browser that follows the authorization redirect), not live.
//! Uses:
//! rust/crates/
//! |-- integration-adapters/src/mcp/auth_store.rs
//! |-- integration-adapters/src/mcp/mod.rs
//! |-- integration-adapters/tests/mcp_support/http.rs
//! |-- integration-adapters/tests/mcp_support/mod.rs
//! |-- integration-api/src/error.rs
//! `-- integration-api/src/mcp.rs
//! External:
//! |-- async_trait::async_trait
//! |-- reqwest::get
//! |-- serde_json::Map
//! |-- tempfile::tempdir
//! |-- tokio::spawn
//! `-- tokio::test
//! Used by:
//! `-- none, entrypoint
//! Invariants: the auth store lives in a temp dir, never the real data dir.
//! Contract tests: integration-adapters/tests/mcp_oauth.rs

mod mcp_support;

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use mcp_support::http::{free_port, spawn_peer};
use mcp_support::{Recorder, fixture};
use opencode_integration_adapters::mcp::auth_store::{Entry, McpAuthStore, Tokens};
use opencode_integration_adapters::mcp::{
    BrowserOpener, McpOptions, McpServiceImpl, OAuthCallback,
};
use opencode_integration_api::error::IntegrationError;
use opencode_integration_api::mcp::{
    AuthStatus, McpEvent, McpService, OAuthConfig, OAuthSetting, RemoteConfig, ServerConfig, Status,
};
use serde_json::Map;

/// Simulated user agent: follows the authorization URL, which the AS redirects to the callback.
struct FollowBrowser;
#[async_trait]
impl BrowserOpener for FollowBrowser {
    async fn open(&self, url: &str) -> Result<(), String> {
        let url = url.to_string();
        tokio::spawn(async move {
            let _ = reqwest::get(&url).await;
        });
        Ok(())
    }
}

struct BrokenBrowser;
#[async_trait]
impl BrowserOpener for BrokenBrowser {
    async fn open(&self, _url: &str) -> Result<(), String> {
        Err("no browser".into())
    }
}

fn secure(base: &str, port: u16) -> ServerConfig {
    ServerConfig::Remote(RemoteConfig {
        url: format!("{base}/secure"),
        enabled: None,
        headers: None,
        oauth: Some(OAuthSetting::Config(OAuthConfig {
            callback_port: Some(port),
            ..Default::default()
        })),
        timeout: Some(5000),
    })
}

fn svc_with(
    fx: &mcp_support::Fixture,
    browser: Arc<dyn BrowserOpener>,
    events: Arc<Recorder>,
) -> McpServiceImpl {
    let mut o = McpOptions::new(fx.workdir.clone(), fx.data.clone(), BTreeMap::new());
    o.browser = browser;
    o.events = events;
    o.callback = OAuthCallback::with_timeout(Duration::from_secs(5));
    McpServiceImpl::new(o)
}

#[tokio::test(flavor = "multi_thread")]
async fn fixture_oauth_full_flow() {
    let fx = fixture();
    let (peer, task) = spawn_peer(true).await;
    let events = Arc::new(Recorder::default());
    let svc = svc_with(&fx, Arc::new(FollowBrowser), events.clone());
    let port = free_port();
    let st = svc.add("sec", secure(&peer.base, port)).await.unwrap();
    assert_eq!(st["sec"], Status::NeedsAuth);
    assert_eq!(
        events.1.lock().unwrap().as_slice(),
        ["Server \"sec\" requires authentication. Run: opencode mcp auth sec"]
    );
    assert!(svc.supports_oauth("sec").await.unwrap());
    assert_eq!(svc.auth_status("sec").await, AuthStatus::NotAuthenticated);

    let status = svc.authenticate("sec").await.unwrap();
    assert_eq!(status, Status::Connected);
    let r = svc
        .call_tool("sec", "ping", Map::new(), None)
        .await
        .unwrap();
    assert_eq!(r["content"][0]["text"], "pong");

    let reg = peer.registered.lock().unwrap()[0].clone();
    assert_eq!(reg["client_name"], "OpenCode");
    assert_eq!(
        reg["redirect_uris"][0],
        format!("http://127.0.0.1:{port}/mcp/oauth/callback")
    );
    assert_eq!(reg["token_endpoint_auth_method"], "none");

    let path = fx.data.join("mcp-auth.json");
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let entry = svc.auth_store().get("sec").await.unwrap();
    assert_eq!(
        entry.server_url.as_deref(),
        Some(format!("{}/secure", peer.base).as_str())
    );
    assert_eq!(entry.client_info.unwrap().client_id, "dyn-client");
    assert!(entry.oauth_state.is_none() && entry.code_verifier.is_none());
    assert_eq!(entry.tokens.unwrap().refresh_token.as_deref(), Some("r1"));
    assert!(svc.has_stored_tokens("sec").await);
    assert_eq!(svc.auth_status("sec").await, AuthStatus::Authenticated);

    svc.remove_auth("sec").await.unwrap();
    assert!(!svc.has_stored_tokens("sec").await);
    assert_eq!(svc.auth_status("sec").await, AuthStatus::NotAuthenticated);
    svc.shutdown().await;
    task.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn fixture_oauth_registration_required_and_browser_failure() {
    let fx = fixture();
    let (peer, task) = spawn_peer(false).await;
    let events = Arc::new(Recorder::default());
    let svc = svc_with(&fx, Arc::new(BrokenBrowser), events.clone());
    // No registration endpoint: the SDK message lacks "OAuth", so the baseline classifies it as
    // a plain failure and falls through to the SSE transport (whose error wins).
    let st = svc
        .add("plain", secure(&peer.base, free_port()))
        .await
        .unwrap();
    assert_eq!(
        st["plain"],
        Status::Failed {
            error: "SSE error: Non-200 status code (405)".into()
        }
    );

    // Registration answered with a non-OAuth error body -> needs_client_registration + toast.
    *peer.registration.lock().unwrap() = true;
    *peer.registration_broken.lock().unwrap() = true;
    let st = svc
        .add("sec", secure(&peer.base, free_port()))
        .await
        .unwrap();
    assert_eq!(
        st["sec"],
        Status::NeedsClientRegistration {
            error: "Server does not support dynamic client registration. Please provide clientId in config.".into()
        }
    );
    assert!(
        events
            .1
            .lock()
            .unwrap()
            .iter()
            .any(|t| t.contains("requires a pre-registered client ID"))
    );

    // With a configured clientId the flow reaches the browser; a failing launcher publishes
    // mcp.browser.open.failed and the wait is then cancelled.
    let mut cfg = secure(&peer.base, free_port());
    if let ServerConfig::Remote(r) = &mut cfg
        && let Some(OAuthSetting::Config(c)) = &mut r.oauth
    {
        c.client_id = Some("static".into());
    }
    svc.add("pre", cfg).await.unwrap();
    let s2 = svc.clone();
    let auth = tokio::spawn(async move { s2.authenticate("pre").await });
    let ev = events.clone();
    let opened = mcp_support::eventually(|| {
        ev.0.lock().unwrap().iter().any(|e| matches!(e, McpEvent::BrowserOpenFailed { properties } if properties.mcp_name == "pre"))
    })
    .await;
    assert!(opened);
    svc.remove_auth("pre").await.unwrap();
    let err = auth.await.unwrap().unwrap_err();
    assert_eq!(
        err,
        IntegrationError::Failed {
            name: "pre".into(),
            message: "Authorization cancelled".into()
        }
    );

    let local = svc.start_auth("missing").await.unwrap_err();
    assert!(matches!(local, IntegrationError::NotFound { .. }));
    let nf = svc.finish_auth("sec", "code").await.unwrap_err();
    assert_eq!(
        nf,
        IntegrationError::Failed {
            name: "sec".into(),
            message: "No pending OAuth flow for MCP server: sec".into()
        }
    );
    svc.shutdown().await;
    task.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn fixture_callback_server_validation() {
    let cb = OAuthCallback::with_timeout(Duration::from_millis(400));
    let port = free_port();
    let redirect = format!("http://127.0.0.1:{port}/mcp/oauth/callback");
    cb.ensure_running(Some(&redirect)).await.unwrap();
    assert!(cb.is_running());
    let base = format!("http://127.0.0.1:{port}");
    let get = |q: &str| {
        let url = format!("{base}{q}");
        async move { reqwest::get(url).await.unwrap() }
    };
    assert_eq!(get("/other").await.status(), 404);
    let r = get("/mcp/oauth/callback?code=x").await;
    assert_eq!(r.status(), 400);
    assert!(
        r.text()
            .await
            .unwrap()
            .contains("Missing required state parameter")
    );
    assert_eq!(
        get("/mcp/oauth/callback?code=x&state=unknown")
            .await
            .status(),
        400
    );
    assert_eq!(get("/mcp/oauth/callback?state=s").await.status(), 400);

    let w = cb.register("good", Some("n"));
    let r = get("/mcp/oauth/callback?code=abc&state=good").await;
    assert_eq!(r.status(), 200);
    assert_eq!(w.wait().await.unwrap(), "abc");

    let w = cb.register("e", None);
    cb.ensure_running(Some(&redirect)).await.unwrap();
    let r = get("/mcp/oauth/callback?state=e&error=access_denied&error_description=nope").await;
    assert_eq!(r.status(), 200);
    assert_eq!(w.wait().await.unwrap_err(), "nope");

    let w = cb.register("t", None);
    assert_eq!(
        w.wait().await.unwrap_err(),
        "OAuth callback timeout - authorization took too long"
    );
    cb.stop().await;
    assert!(!cb.is_running());
}

#[tokio::test]
async fn fixture_auth_store_semantics() {
    let dir = tempfile::tempdir().unwrap();
    let store = McpAuthStore::new(dir.path());
    assert!(store.all().await.is_empty());
    store
        .update_tokens(
            "a",
            Tokens {
                access_token: "t".into(),
                expires_at: Some(1_700_000_000.0),
                ..Default::default()
            },
            Some("https://x/mcp"),
        )
        .await
        .unwrap();
    assert!(store.get_for_url("a", "https://x/mcp").await.is_some());
    assert!(
        store.get_for_url("a", "https://x/mcp/").await.is_none(),
        "exact URL binding"
    );
    store.update_oauth_state("a", "st").await.unwrap();
    assert_eq!(store.get_oauth_state("a").await.as_deref(), Some("st"));
    let text = std::fs::read_to_string(store.path()).unwrap();
    assert!(
        text.contains("\"expiresAt\": 1700000000"),
        "JS number formatting: {text}"
    );
    assert!(text.contains("\"serverUrl\": \"https://x/mcp\""));
    store.set("b", Entry::default(), None).await.unwrap();
    store.remove("a").await.unwrap();
    assert_eq!(
        store.all().await.keys().cloned().collect::<Vec<_>>(),
        vec!["b"]
    );
    std::fs::write(store.path(), "not json").unwrap();
    assert!(store.all().await.is_empty(), "invalid files read as empty");
}
