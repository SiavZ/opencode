//! Purpose: fixture tests for the hosted exchange routes (OIDC, PAT, installation lookup)
//! against a local fake GitHub API and JWKS server, asserting baseline statuses and bodies.
//! Scope: test (fixture-only, local axum servers, test RSA keys).
//! Uses:
//! rust/crates/
//! |-- ecosystem/src/github/auth/app.rs
//! |-- ecosystem/src/github/auth/handlers.rs
//! |-- ecosystem/src/github/auth/oidc.rs
//! `-- ecosystem/tests/github_auth_support/mod.rs
//! External:
//! `-- axum::{Router, routing}
//! Used by:
//! `-- none, entrypoint
//! Invariants: no network beyond 127.0.0.1.
//! Contract tests: this file.

mod github_auth_support;

use std::sync::{Arc, Mutex};

use axum::extract::Path;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::{Json, Router, routing::get, routing::post};
use github_auth_support::{
    APP_KEY_PEM, AUDIENCE, ISSUER, good_claims, jwk, jwks, mint, now, serve,
};
use opencode_ecosystem::github::auth::app::{AppCredentials, GitHubApi, octokit_message};
use opencode_ecosystem::github::auth::handlers::{ExchangeService, HttpRequest};
use opencode_ecosystem::github::auth::oidc::OidcVerifier;
use serde_json::{Value, json};

type Seen = Arc<Mutex<Vec<String>>>;

/// Fake GitHub: `acme/widgets` is installed (id 42), `acme/none` is not, `acme/boom` 500s.
/// `/repos/{o}/{r}` answers permissions by token: `tok-write` push, `tok-read` none.
async fn fake_github(seen: Seen) -> String {
    let key = jsonwebtoken::DecodingKey::from_jwk(&jwk(APP_KEY_PEM, "app")).unwrap();
    let s1 = seen.clone();
    let s2 = seen.clone();
    let router = Router::new()
        .route(
            "/repos/{owner}/{repo}/installation",
            get(move |Path((owner, repo)): Path<(String, String)>, headers: HeaderMap| {
                let key = key.clone();
                let seen = s1.clone();
                async move {
                    let jwt = headers["authorization"].to_str().unwrap().trim_start_matches("Bearer ").to_string();
                    let mut v = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::RS256);
                    v.required_spec_claims = ["exp", "iat", "iss"].iter().map(|s| s.to_string()).collect();
                    v.validate_aud = false;
                    let claims = jsonwebtoken::decode::<Value>(&jwt, &key, &v).expect("valid app jwt").claims;
                    assert_eq!(claims["iss"], "12345");
                    assert_eq!(claims["exp"].as_u64().unwrap() - claims["iat"].as_u64().unwrap(), 600);
                    seen.lock().unwrap().push(format!("installation {owner}/{repo}"));
                    match repo.as_str() {
                        "widgets" => Json(json!({ "id": 42, "account": { "login": owner } })).into_response(),
                        "boom" => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "message": "Server Error" }))).into_response(),
                        _ => (StatusCode::NOT_FOUND, Json(json!({ "message": "Not Found", "documentation_url": "https://docs.github.com/rest" }))).into_response(),
                    }
                }
            }),
        )
        .route(
            "/app/installations/{id}/access_tokens",
            post(move |Path(id): Path<u64>| {
                let seen = s2.clone();
                async move {
                    seen.lock().unwrap().push(format!("access_tokens {id}"));
                    Json(json!({ "token": "ghs_fixture_installation", "expires_at": "2099-01-01T00:00:00Z" }))
                }
            }),
        )
        .route(
            "/repos/{owner}/{repo}",
            get(|headers: HeaderMap| async move {
                match headers["authorization"].to_str().unwrap() {
                    "token tok-write" => Json(json!({ "permissions": { "admin": false, "push": true, "maintain": false } })).into_response(),
                    "token tok-read" => Json(json!({ "permissions": { "admin": false, "push": false, "maintain": false } })).into_response(),
                    "token tok-anon" => Json(json!({ "name": "widgets" })).into_response(),
                    _ => (StatusCode::UNAUTHORIZED, Json(json!({ "message": "Bad credentials", "documentation_url": "https://docs.github.com/rest" }))).into_response(),
                }
            }),
        )
        .route("/jwks", get(|| async { Json(jwks(vec![jwk(APP_KEY_PEM, "k1")])) }));
    serve(router).await
}

async fn service() -> (ExchangeService, Seen) {
    let seen = Seen::default();
    let base = fake_github(seen.clone()).await;
    let svc = ExchangeService::new(
        OidcVerifier::new(format!("{base}/jwks"), ISSUER, AUDIENCE),
        GitHubApi::new(&base),
        AppCredentials::from_pem("12345", &APP_KEY_PEM.replace('\n', "\\n")).unwrap(),
    );
    (svc, seen)
}

fn req(method: &str, path: &str, auth: Option<&str>, body: &str) -> HttpRequest {
    HttpRequest {
        method: method.into(),
        path: path.into(),
        headers: auth
            .map(|a| vec![("Authorization".into(), a.into())])
            .unwrap_or_default(),
        body: body.as_bytes().to_vec(),
        ..Default::default()
    }
}

#[tokio::test]
async fn fixture_oidc_exchange_success_and_failures() {
    let (svc, seen) = service().await;
    let ok = svc
        .handle(&req(
            "POST",
            "/exchange_github_app_token",
            Some(&format!(
                "Bearer {}",
                mint(APP_KEY_PEM, Some("k1"), good_claims())
            )),
            "",
        ))
        .await
        .unwrap();
    assert_eq!(
        (ok.status, ok.body.as_str()),
        (200, r#"{"token":"ghs_fixture_installation"}"#)
    );
    assert_eq!(
        *seen.lock().unwrap(),
        vec!["installation acme/widgets", "access_tokens 42"]
    );

    let missing = svc
        .handle(&req("POST", "/exchange_github_app_token", None, ""))
        .await
        .unwrap();
    assert_eq!(
        (missing.status, missing.body.as_str()),
        (401, r#"{"error":"Authorization header is required"}"#)
    );
    assert_eq!(missing.headers(), [("content-type", "application/json")]);

    let mut expired = good_claims();
    expired["exp"] = json!(now() - 10);
    let bad = svc
        .handle(&req(
            "POST",
            "/exchange_github_app_token",
            Some(&format!(
                "Bearer {}",
                mint(APP_KEY_PEM, Some("k1"), expired)
            )),
            "",
        ))
        .await
        .unwrap();
    assert_eq!(
        (bad.status, bad.body.as_str()),
        (403, r#"{"error":"Invalid or expired token"}"#)
    );

    let mut not_installed = good_claims();
    not_installed["repository"] = json!("acme/none");
    let upstream = svc
        .handle(&req(
            "POST",
            "/exchange_github_app_token",
            Some(&format!(
                "Bearer {}",
                mint(APP_KEY_PEM, Some("k1"), not_installed)
            )),
            "",
        ))
        .await
        .unwrap();
    assert_eq!(
        (upstream.status, upstream.body.as_str()),
        (
            502,
            r#"{"error":"Failed to exchange GitHub App token for acme/none"}"#
        )
    );
}

#[tokio::test]
async fn fixture_pat_exchange_checks_write_permission() {
    let (svc, _) = service().await;
    let body = r#"{"owner":"acme","repo":"widgets"}"#;
    let path = "/exchange_github_app_token_with_pat";
    let ok = svc
        .handle(&req("POST", path, Some("Bearer tok-write"), body))
        .await
        .unwrap();
    assert_eq!(
        (ok.status, ok.body.as_str()),
        (200, r#"{"token":"ghs_fixture_installation"}"#)
    );

    let cases = [
        (None, r#"{"error":"Authorization header is required"}"#),
        (
            Some("Bearer tok-read"),
            r#"{"error":"User does not have write permissions"}"#,
        ),
        (
            Some("Bearer tok-anon"),
            r#"{"error":"Cannot read properties of undefined (reading 'admin')"}"#,
        ),
        (
            Some("Bearer bogus"),
            r#"{"error":"Bad credentials - https://docs.github.com/rest"}"#,
        ),
    ];
    for (auth, expected) in cases {
        let r = svc.handle(&req("POST", path, auth, body)).await.unwrap();
        assert_eq!((r.status, r.body.as_str()), (401, expected), "{auth:?}");
    }
    let not_installed = svc
        .handle(&req(
            "POST",
            path,
            Some("Bearer tok-write"),
            r#"{"owner":"acme","repo":"none"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(not_installed.status, 401);
    assert_eq!(
        not_installed.json_body().unwrap()["error"],
        "Not Found - https://docs.github.com/rest"
    );

    let malformed = svc
        .handle(&req("POST", path, Some("Bearer tok-write"), "{"))
        .await
        .unwrap();
    assert_eq!(
        (malformed.status, malformed.body.as_str()),
        (500, "Internal Server Error")
    );
    let null = svc
        .handle(&req("POST", path, Some("Bearer tok-write"), "null"))
        .await
        .unwrap();
    assert_eq!(
        (null.status, null.body.as_str()),
        (500, "Internal Server Error")
    );
}

#[tokio::test]
async fn fixture_installation_lookup() {
    let (svc, _) = service().await;
    let lookup = |repo: &str| HttpRequest {
        method: "GET".into(),
        path: "/get_github_app_installation".into(),
        query: vec![
            ("owner".into(), "acme".into()),
            ("repo".into(), repo.into()),
        ],
        ..Default::default()
    };
    let found = svc.handle(&lookup("widgets")).await.unwrap();
    assert_eq!(
        found.body,
        r#"{"installation":{"id":42,"account":{"login":"acme"}}}"#
    );
    let missing = svc.handle(&lookup("none")).await.unwrap();
    assert_eq!((missing.status, missing.body.as_str()), (200, "{}"));
    let boom = svc.handle(&lookup("boom")).await.unwrap();
    assert_eq!(boom.status, 500);
    assert!(svc.handle(&req("GET", "/other", None, "")).await.is_none());
}

#[test]
fn fixture_octokit_error_message_format() {
    assert_eq!(octokit_message(&json!("plain")), "plain");
    assert_eq!(octokit_message(&json!({"message": "Nope"})), "Nope");
    assert_eq!(
        octokit_message(
            &json!({"message": "Validation Failed", "errors": [{"code": "x"}], "documentation_url": "u"})
        ),
        r#"Validation Failed: {"code":"x"} - u"#
    );
    assert_eq!(
        octokit_message(&json!({"a": 1})),
        r#"Unknown error: {"a":1}"#
    );
}

#[test]
fn fixture_app_credentials_debug_redacts_key() {
    let creds = AppCredentials::from_pem("1", APP_KEY_PEM).unwrap();
    assert!(!format!("{creds:?}").contains("PRIVATE"));
    assert!(AppCredentials::from_pem("1", "not a key").is_err());
}
