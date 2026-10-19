//! Purpose: fixture tests for the console issuer success flow and token exchange against local
//! fake GitHub/OpenAuth servers (axum on 127.0.0.1) and the local Docker MySQL.
//! Scope: integration test (fixture only, no real OAuth providers).
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/auth.rs
//! |-- hosted-adapters/src/console/auth/issuer.rs
//! |-- hosted-adapters/src/console/db.rs
//! |-- hosted-adapters/src/console/tenant.rs
//! |-- hosted-domain/src/console/common.rs
//! `-- hosted-domain/src/console/identity.rs
//! External:
//! |-- axum::Json
//! |-- axum::Router
//! |-- axum::http::StatusCode::BAD_REQUEST
//! |-- axum::http::StatusCode::OK
//! |-- axum::routing::get
//! |-- axum::routing::post
//! |-- axum::serve
//! |-- mysql_async::prelude::Queryable
//! |-- reqwest::Client::new
//! |-- serde_json::json
//! |-- tokio::net::TcpListener::bind
//! |-- tokio::spawn
//! `-- tokio::test
//! Used by:
//! `-- none, entrypoint
//! Invariants: servers bind 127.0.0.1:0 and are dropped with the test.
//! Contract tests: this file.

use axum::Json;
use axum::Router;
use axum::routing::{get, post};
use mysql_async::prelude::Queryable;
use opencode_hosted_adapters::console::auth::issuer::{
    authorize_precheck, github_identity, issuer_success,
};
use opencode_hosted_adapters::console::auth::{AuthClient, AuthClientError};
use opencode_hosted_adapters::console::db::TestDatabase;
use opencode_hosted_adapters::console::tenant::TenantStore;
use opencode_hosted_domain::console::common::Actor;
use opencode_hosted_domain::console::identity::{AuthProvider, IssuerOutcome};

async fn serve(router: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    format!("http://{addr}")
}

#[tokio::test]
async fn fixture_issuer_success_paths() {
    let github = serve(
        Router::new()
            .route(
                "/user/emails",
                get(|| async {
                    Json(serde_json::json!([
                        {"email": "other@anoma.ly", "primary": false, "verified": true},
                        {"email": "erin@anoma.ly", "primary": true, "verified": true}
                    ]))
                }),
            )
            .route(
                "/user",
                get(|| async { Json(serde_json::json!({"id": 4242})) }),
            ),
    )
    .await;
    let http = reqwest::Client::new();
    let identity = github_identity(&http, &github, "gho_fixture")
        .await
        .unwrap();

    let t = TestDatabase::create("issuer").await.unwrap();
    let store = TenantStore::new(t.db.clone());

    // Unknown identity -> new console redirect, no rows created.
    assert_eq!(
        issuer_success(&store, &identity, "production")
            .await
            .unwrap(),
        IssuerOutcome::RedirectToNewConsole
    );
    // Non-production stage rejects non-anoma.ly emails before any lookup.
    let google = opencode_hosted_domain::console::identity::ProviderIdentity::Google {
        sub: Some("g1".into()),
        email: Some("x@example.com".into()),
        email_verified: true,
    };
    assert_eq!(
        issuer_success(&store, &google, "dev")
            .await
            .unwrap_err()
            .to_string(),
        "Invalid email"
    );

    // Existing email link, but no Black workspace -> redirect.
    let acc = store.create_account(None).await.unwrap();
    let mut conn = t.db.conn().await.unwrap();
    conn.exec_drop(
        "INSERT INTO `auth` (`id`, `account_id`, `provider`, `subject`) VALUES ('aut_seed', ?, 'email', 'erin@anoma.ly')",
        (&acc,),
    )
    .await
    .unwrap();
    assert_eq!(
        issuer_success(&store, &identity, "production")
            .await
            .unwrap(),
        IssuerOutcome::RedirectToNewConsole
    );

    // Black workspace -> account subject issued and GitHub link upserted.
    let ws = store
        .create_workspace(
            &Actor::Account {
                account_id: acc.clone(),
                email: "erin@anoma.ly".into(),
            },
            "Black",
        )
        .await
        .unwrap();
    conn.exec_drop(
        "UPDATE `billing` SET `subscription_id` = 'sub_black' WHERE `workspace_id` = ?",
        (&ws,),
    )
    .await
    .unwrap();
    assert_eq!(
        issuer_success(&store, &identity, "production")
            .await
            .unwrap(),
        IssuerOutcome::IssueAccount {
            account_id: acc.clone(),
            email: "erin@anoma.ly".into()
        }
    );
    let links = store
        .auth_matches(AuthProvider::Github, "4242", "erin@anoma.ly")
        .await
        .unwrap();
    assert_eq!(links.len(), 2);
    drop(conn);
    t.drop_database().await.unwrap();
}

#[tokio::test]
async fn fixture_token_exchange_against_fake_issuer() {
    let issuer = serve(Router::new().route(
        "/token",
        post(|body: String| async move {
            if body.contains("code=good") && body.ends_with("&client_id=app&code_verifier=") {
                return (axum::http::StatusCode::OK, Json(serde_json::json!({"access_token": "tok", "refresh_token": "r", "expires_in": 60})));
            }
            (axum::http::StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "invalid_grant"})))
        }),
    ))
    .await;
    let client = AuthClient::new(issuer, reqwest::Client::new());
    assert_eq!(
        client
            .exchange("good", "http://localhost/auth/callback")
            .await
            .unwrap(),
        "tok"
    );
    assert!(matches!(
        client
            .exchange("bad", "http://localhost/auth/callback")
            .await,
        Err(AuthClientError::InvalidAuthorizationCode)
    ));
    assert_eq!(
        client.exchange("bad", "x").await.unwrap_err().to_string(),
        "Invalid authorization code"
    );
}

#[test]
fn fixture_issuer_authorize_precheck() {
    assert_eq!(
        authorize_precheck(Some("app"), Some("https://opencode.ai/cb")),
        None
    );
    assert_eq!(authorize_precheck(Some("app"), None), None);
    assert_eq!(
        authorize_precheck(Some("app"), Some("https://evil.example/cb")),
        Some("Unauthorized client")
    );
    assert_eq!(
        authorize_precheck(None, Some("https://opencode.ai/cb")),
        Some("Unauthorized client")
    );
}
