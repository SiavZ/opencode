//! Purpose: focused auth route helpers and configuration contract tests.
//! Scope: pure route behavior that does not require a live issuer or database.
//! Uses:
//! rust/bins/
//! `-- hosted/src/console/auth.rs
//! External:
//! `-- none
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/auth.rs
//! Invariants: tests never send mail, contact providers or charge accounts.
//! Contract tests: this module.

use super::{Config, redirect, request};
use axum::http::{HeaderMap, HeaderValue, Uri};
use opencode_hosted_domain::console::identity::callback_next_path;

fn test_config() -> Config {
    Config {
        issuer_url: Some("https://auth.example".into()),
        session_secret: Some("secret".into()),
        state_redis_url: Some("redis://user:password@localhost/0".into()),
        console_url: None,
        public_origin: Some("https://console.example".into()),
    }
}

#[test]
fn config_debug_redacts_secret_values() {
    let mut config = test_config();
    config.session_secret = Some("super-secret".into());
    let debug = format!("{config:?}");
    assert!(debug.contains("issuer_configured: true"));
    assert!(debug.contains("session_configured: true"));
    assert!(debug.contains("state_store_configured: true"));
    assert!(!debug.contains("super-secret"));
    assert!(!debug.contains("password"));
}

#[test]
fn config_rejects_missing_oauth_state_store() {
    let mut config = test_config();
    config.state_redis_url = None;
    assert!(config.validate().is_err());
}

#[test]
fn callback_paths_match_solid_route() {
    assert_eq!(callback_next_path("/auth/callback"), "/auth");
    assert_eq!(
        callback_next_path("/auth/callback/workspace/wrk_1"),
        "/workspace/wrk_1"
    );
}

#[test]
fn redirects_set_location_and_optional_cache_control() {
    let response = redirect("/auth", 302, true).unwrap();
    assert_eq!(response.status(), 302);
    assert_eq!(response.headers()["location"], "/auth");
    assert_eq!(response.headers()["cache-control"], "no-store");
}

#[test]
fn continuation_rejects_external_and_backslash_paths() {
    assert!(request::require_local_path("//evil.example/path").is_err());
    assert!(request::require_local_path("/\\evil.example").is_err());
    assert!(request::require_local_path("/workspace/wrk_1").is_ok());
}

#[test]
fn configured_origin_ignores_untrusted_host_headers() {
    let config = test_config();
    let mut headers = HeaderMap::new();
    headers.insert("host", HeaderValue::from_static("attacker.example"));
    let actual = request::url(
        &config,
        &Uri::from_static("/auth/callback?code=x"),
        &headers,
    )
    .unwrap();
    assert_eq!(
        actual.as_str(),
        "https://console.example/auth/callback?code=x"
    );
}

#[test]
fn actor_action_origin_guard_rejects_cross_site_and_allows_same_site() {
    let config = test_config();
    let state = crate::console::state::ConsoleConfig {
        auth: config,
        ..Default::default()
    };
    let state = crate::console::ConsoleState {
        config: std::sync::Arc::new(state),
        db: opencode_hosted_adapters::console::db::ConsoleDb::connect("mysql://u:p@127.0.0.1/test")
            .unwrap(),
    };
    let mut headers = HeaderMap::new();
    headers.insert(
        "origin",
        HeaderValue::from_static("https://attacker.example"),
    );
    assert!(super::require_same_origin(&state, &headers).is_err());
    headers.insert(
        "origin",
        HeaderValue::from_static("https://console.example"),
    );
    assert!(super::require_same_origin(&state, &headers).is_ok());
}
