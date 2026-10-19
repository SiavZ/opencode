//! Purpose: fixture tests for the Action-side token lifecycle: use_github_token parsing,
//! OIDC ID token fetch (audience query, bearer, `.value`, 5xx retry), PAT vs OIDC exchange
//! routing, exchange error message and revoke behaviour.
//! Scope: test (fixture-only, local axum servers).
//! Uses:
//! rust/crates/
//! |-- ecosystem/src/github/auth/client.rs
//! `-- ecosystem/tests/github_auth_support/mod.rs
//! External:
//! |-- axum::Json
//! |-- axum::Router
//! |-- axum::extract::Query
//! |-- axum::extract::RawQuery
//! |-- axum::http::HeaderMap
//! |-- axum::http::StatusCode
//! |-- axum::response::IntoResponse
//! |-- axum::routing::delete
//! |-- axum::routing::get
//! |-- axum::routing::post
//! |-- serde_json::Value
//! |-- serde_json::json
//! `-- tokio::test
//! Used by:
//! `-- none, entrypoint
//! Invariants: no network beyond 127.0.0.1, tokens are fixture strings.
//! Contract tests: this file.

mod github_auth_support;

use std::sync::{Arc, Mutex};

use axum::extract::{Query, RawQuery};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::{Json, Router, routing::delete, routing::get, routing::post};
use github_auth_support::serve;
use opencode_ecosystem::github::auth::client::{AcquiredToken, TokenClient, TokenEnv, TokenSource};
use serde_json::{Value, json};

type Log = Arc<Mutex<Vec<String>>>;

async fn fake(log: Log) -> String {
    let (a, b, c, d) = (log.clone(), log.clone(), log.clone(), log.clone());
    let attempts = Arc::new(Mutex::new(0));
    let router = Router::new()
        .route(
            "/idtoken",
            get(move |RawQuery(q): RawQuery, headers: HeaderMap| {
                let (log, attempts) = (a.clone(), attempts.clone());
                async move {
                    let mut n = attempts.lock().unwrap();
                    *n += 1;
                    if *n == 1 {
                        return StatusCode::SERVICE_UNAVAILABLE.into_response();
                    }
                    log.lock().unwrap().push(format!(
                        "idtoken {} {}",
                        q.unwrap_or_default(),
                        headers["authorization"].to_str().unwrap()
                    ));
                    Json(json!({ "value": "oidc-jwt" })).into_response()
                }
            }),
        )
        .route(
            "/exchange_github_app_token",
            post(move |headers: HeaderMap| {
                let log = b.clone();
                async move {
                    let auth = headers["authorization"].to_str().unwrap().to_string();
                    log.lock().unwrap().push(format!("oidc-exchange {auth}"));
                    if auth == "Bearer reject" {
                        return (
                            StatusCode::FORBIDDEN,
                            Json(json!({ "error": "Invalid or expired token" })),
                        )
                            .into_response();
                    }
                    Json(json!({ "token": "ghs_app" })).into_response()
                }
            }),
        )
        .route(
            "/exchange_github_app_token_with_pat",
            post(move |headers: HeaderMap, body: String| {
                let log = c.clone();
                async move {
                    log.lock().unwrap().push(format!(
                        "pat-exchange {} {body}",
                        headers["authorization"].to_str().unwrap()
                    ));
                    Json(json!({ "token": "ghs_from_pat" }))
                }
            }),
        )
        .route(
            "/installation/token",
            delete(move |headers: HeaderMap, Query(_q): Query<Value>| {
                let log = d.clone();
                async move {
                    log.lock().unwrap().push(format!(
                        "revoke {} {}",
                        headers["authorization"].to_str().unwrap(),
                        headers["x-github-api-version"].to_str().unwrap()
                    ));
                    StatusCode::NO_CONTENT
                }
            }),
        );
    serve(router).await
}

fn env(base: &str) -> TokenEnv {
    TokenEnv {
        oidc_base_url: Some(format!("{base}///")),
        id_token_request_url: Some(format!("{base}/idtoken?api-version=2.0")),
        id_token_request_token: Some("req-token".into()),
        ..Default::default()
    }
}

#[test]
fn fixture_use_github_token_and_base_url_normalization() {
    let mut e = TokenEnv::default();
    assert!(!e.use_github_token().unwrap());
    assert_eq!(e.oidc_base_url(), "https://api.opencode.ai");
    e.use_github_token = Some("true".into());
    assert!(e.use_github_token().unwrap());
    e.use_github_token = Some("yes".into());
    assert_eq!(
        e.use_github_token().unwrap_err().to_string(),
        "Invalid use_github_token value: yes. Must be a boolean."
    );
    e.oidc_base_url = Some("https://x.example//".into());
    assert_eq!(e.oidc_base_url(), "https://x.example");
}

#[tokio::test]
async fn fixture_github_token_path_never_exchanges_or_revokes() {
    let log = Log::default();
    let base = fake(log.clone()).await;
    let client = TokenClient::new(&base);
    let mut e = env(&base);
    e.use_github_token = Some("true".into());
    let err = client
        .acquire(&e, "acme", "widgets", None)
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "GITHUB_TOKEN environment variable is not set. When using use_github_token, you must provide GITHUB_TOKEN."
    );
    e.github_token = Some("ghs_workflow".into());
    let token = client.acquire(&e, "acme", "widgets", None).await.unwrap();
    assert_eq!(token.source, TokenSource::GithubToken);
    assert_eq!(client.revoke(&token).await, None);
    assert!(log.lock().unwrap().is_empty());
    assert!(!format!("{token:?}").contains("ghs_workflow"));
}

#[tokio::test]
async fn fixture_oidc_fetch_retry_exchange_and_revoke() {
    let log = Log::default();
    let base = fake(log.clone()).await;
    let client = TokenClient::new(&base);
    let token = client
        .acquire(&env(&base), "acme", "widgets", None)
        .await
        .unwrap();
    assert_eq!(token.token, "ghs_app");
    assert!(token.must_revoke());
    assert_eq!(client.revoke(&token).await, Some(204));
    assert_eq!(
        *log.lock().unwrap(),
        vec![
            "idtoken api-version=2.0&audience=opencode-github-action Bearer req-token",
            "oidc-exchange Bearer oidc-jwt",
            "revoke Bearer ghs_app 2022-11-28",
        ]
    );
}

#[tokio::test]
async fn fixture_pat_token_uses_pat_route_with_owner_repo_body() {
    let log = Log::default();
    let base = fake(log.clone()).await;
    let client = TokenClient::new(&base);
    let token = client
        .acquire(&env(&base), "acme", "widgets", Some("github_pat_fixture"))
        .await
        .unwrap();
    assert_eq!(token.token, "ghs_from_pat");
    assert_eq!(
        *log.lock().unwrap(),
        vec![r#"pat-exchange Bearer github_pat_fixture {"owner":"acme","repo":"widgets"}"#]
    );
}

#[tokio::test]
async fn fixture_exchange_failure_message_and_missing_oidc_env() {
    let log = Log::default();
    let base = fake(log.clone()).await;
    let client = TokenClient::new(&base);
    let err = client
        .exchange(&base, "reject", "a", "b")
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        r#"App token exchange failed: 403 Forbidden - {"error":"Invalid or expired token"}"#
    );
    let err = client
        .acquire(&TokenEnv::default(), "a", "b", None)
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "Could not fetch an OIDC token. Make sure to add `id-token: write` to your workflow permissions."
    );
    let err = client.oidc_token(&TokenEnv::default()).await.unwrap_err();
    assert_eq!(
        err.to_string(),
        "Error message: Unable to get ACTIONS_ID_TOKEN_REQUEST_TOKEN env variable"
    );
}

#[tokio::test]
async fn fixture_revoke_skips_empty_and_tolerates_unreachable() {
    let client = TokenClient::new("http://127.0.0.1:9");
    let empty = AcquiredToken {
        token: String::new(),
        source: TokenSource::AppInstallation,
    };
    assert_eq!(client.revoke(&empty).await, None);
    let real = AcquiredToken {
        token: "ghs_x".into(),
        source: TokenSource::AppInstallation,
    };
    assert_eq!(client.revoke(&real).await, None);
}

#[tokio::test]
async fn fixture_root_exchange_uses_mock_mode_and_json_error_field() {
    let log = Log::default();
    let base = fake(log.clone()).await;
    let client = TokenClient::new(&base);
    assert_eq!(
        client
            .exchange_root(&base, "classic-pat", "a", "b", true)
            .await
            .unwrap(),
        "ghs_from_pat"
    );
    assert_eq!(
        *log.lock().unwrap(),
        vec![r#"pat-exchange Bearer classic-pat {"owner":"a","repo":"b"}"#]
    );
    let err = client
        .exchange_root(&base, "reject", "a", "b", false)
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "App token exchange failed: 403 Forbidden - Invalid or expired token"
    );
}

#[tokio::test]
async fn fixture_transport_errors_do_not_disclose_oidc_url_secrets() {
    let env = TokenEnv {
        id_token_request_url: Some("http://127.0.0.1:9/?secret=fixture-secret".into()),
        id_token_request_token: Some("fixture-request-token".into()),
        ..Default::default()
    };
    let client = TokenClient::new("http://127.0.0.1:9");
    let error = client.oidc_token(&env).await.unwrap_err().to_string();
    assert!(!error.contains("fixture-secret"));
    assert!(!format!("{env:?}").contains("fixture-secret"));
    assert!(!format!("{env:?}").contains("fixture-request-token"));
}
