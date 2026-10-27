//! Purpose: exercise console auth and workspace routes through Axum with isolated MySQL fixtures.
//! Scope: fixture HTTP acceptance tests; OAuth issuer is a local Axum server only.
//! Uses:
//! rust/bins/
//! |-- hosted/src/console/auth.rs
//! |-- hosted/src/console/mod.rs
//! |-- hosted/src/console/state.rs
//! `-- hosted/src/console/workspace.rs
//! rust/crates/
//! |-- hosted-adapters/src/console/auth.rs
//! |-- hosted-adapters/src/console/db.rs
//! `-- hosted-adapters/src/console/tenant.rs
//! External:
//! |-- axum::body::{Body, to_bytes}
//! |-- axum::http::{Request, StatusCode}
//! |-- opencode_hosted_adapters::console::db::TestDatabase
//! |-- serde_json::{Value, json}
//! |-- tokio::test
//! `-- tower::ServiceExt
//! Used by:
//! `-- hosted/src/console/mod.rs
//! Invariants: each test uses a unique database; all external auth traffic targets a local fixture.
//! Contract tests: this file.

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use opencode_hosted_adapters::console::auth::SessionCodec;
use opencode_hosted_adapters::console::db::TestDatabase;
use opencode_hosted_adapters::console::tenant::TenantStore;
use opencode_hosted_domain::console::common::Actor;
use serde_json::{Value, json};
use tower::ServiceExt;

use super::auth::Config;
use super::state::{ConsoleConfig, ConsoleState};

const SECRET: &str = "hosted-console-test-secret-with-at-least-32-bytes";

async fn app(db: opencode_hosted_adapters::console::db::ConsoleDb) -> axum::Router {
    let config = ConsoleConfig {
        auth: Config {
            issuer_url: Some("https://auth.example".into()),
            session_secret: Some(SECRET.into()),
            state_redis_url: None,
            console_url: Some("https://console.opencode.ai".into()),
            public_origin: Some("https://console.opencode.ai".into()),
        },
        ..Default::default()
    };
    super::router(ConsoleState {
        config: std::sync::Arc::new(config),
        db,
    })
}

fn account_cookie(account_id: &str, email: &str) -> String {
    let codec = SessionCodec::new(SECRET);
    let now = (time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as i64;
    let mut session = codec.load(None, now);
    session.data.login(account_id, email);
    codec.set_cookie(&session, now).unwrap()
}

async fn request(
    app: axum::Router,
    method: &str,
    uri: &str,
    cookie: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, axum::http::HeaderMap, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "console.opencode.ai");
    if let Some(cookie) = cookie {
        builder = builder.header("cookie", cookie.split(';').next().unwrap());
    }
    let body = match body {
        Some(body) => {
            builder = builder.header("content-type", "application/json");
            Body::from(body.to_string())
        }
        None => Body::empty(),
    };
    let response = app.oneshot(builder.body(body).unwrap()).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, headers, body)
}

#[tokio::test]
async fn fixture_workspace_routes_enforce_tenant_boundaries_and_persist_updates() {
    let fixture = TestDatabase::create("console_http_workspace")
        .await
        .unwrap();
    let store = TenantStore::new(fixture.db.clone());
    let account_a = store.create_account(None).await.unwrap();
    let account_b = store.create_account(None).await.unwrap();
    let actor_a = Actor::Account {
        account_id: account_a.clone(),
        email: "a@example.test".into(),
    };
    let actor_b = Actor::Account {
        account_id: account_b.clone(),
        email: "b@example.test".into(),
    };
    let workspace_a = store.create_workspace(&actor_a, "Tenant A").await.unwrap();
    let workspace_b = store.create_workspace(&actor_b, "Tenant B").await.unwrap();
    let mut conn = fixture.db.conn().await.unwrap();
    mysql_async::prelude::Queryable::exec_drop(
        &mut conn,
        "UPDATE `billing` SET `subscription_id` = 'sub_fx_a' WHERE `workspace_id` = ?",
        (&workspace_a,),
    )
    .await
    .unwrap();
    mysql_async::prelude::Queryable::exec_drop(
        &mut conn,
        "UPDATE `billing` SET `subscription_id` = 'sub_fx_b' WHERE `workspace_id` = ?",
        (&workspace_b,),
    )
    .await
    .unwrap();
    drop(conn);
    let router = app(fixture.db.clone()).await;
    let cookie_a = account_cookie(&account_a, "a@example.test");

    let (status, _, _) = request(
        router.clone(),
        "GET",
        &format!("/workspaces/{workspace_b}"),
        Some(&cookie_a),
        None,
    )
    .await;
    assert_ne!(
        status,
        StatusCode::OK,
        "another account must not read this workspace"
    );
    let (status, _, _) = request(
        router.clone(),
        "PATCH",
        &format!("/workspaces/{workspace_b}"),
        Some(&cookie_a),
        Some(json!({"name":"stolen"})),
    )
    .await;
    assert_ne!(
        status,
        StatusCode::OK,
        "another account must not mutate this workspace"
    );

    let (status, _, info) = request(
        router.clone(),
        "GET",
        &format!("/workspaces/{workspace_a}"),
        Some(&cookie_a),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(info["name"], "Tenant A");
    let (status, _, _) = request(
        router.clone(),
        "PATCH",
        &format!("/workspaces/{workspace_a}"),
        Some(&cookie_a),
        Some(json!({"name":"Updated A","region":["us"],"allow_training":false})),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _, info) = request(
        router.clone(),
        "GET",
        &format!("/workspaces/{workspace_a}"),
        Some(&cookie_a),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(info["name"], "Updated A");

    let mut conn = fixture.db.conn().await.unwrap();
    let row: Option<(String, bool)> = mysql_async::prelude::Queryable::exec_first(
        &mut conn,
        "SELECT `region`, `allow_training` FROM `workspace` WHERE `id` = ?",
        (&workspace_a,),
    )
    .await
    .unwrap();
    let (region, allow_training) = row.expect("workspace update persisted");
    assert_eq!(
        serde_json::from_str::<Value>(&region).unwrap(),
        json!(["us"])
    );
    assert!(!allow_training);
    drop(conn);
    fixture.drop_database().await.unwrap();
}

#[tokio::test]
async fn fixture_workspace_member_cannot_patch_but_can_read_and_region_only_fills_once() {
    let fixture = TestDatabase::create("console_http_member").await.unwrap();
    let store = TenantStore::new(fixture.db.clone());
    let account = store.create_account(None).await.unwrap();
    let owner = Actor::Account {
        account_id: account.clone(),
        email: "owner@example.test".into(),
    };
    let workspace = store.create_workspace(&owner, "Members").await.unwrap();
    let mut conn = fixture.db.conn().await.unwrap();
    mysql_async::prelude::Queryable::exec_drop(
        &mut conn,
        "UPDATE `billing` SET `subscription_id` = 'sub_fx_f' WHERE `workspace_id` = ?",
        (&workspace,),
    )
    .await
    .unwrap();
    mysql_async::prelude::Queryable::exec_drop(
        &mut conn,
        "INSERT INTO `account` (`id`) VALUES ('acc_member_fixture')",
        (),
    )
    .await
    .unwrap();
    mysql_async::prelude::Queryable::exec_drop(
        &mut conn,
        "INSERT INTO `user` (`id`, `name`, `account_id`, `workspace_id`, `role`) VALUES ('usr_member_fixture', '', 'acc_member_fixture', ?, 'member')",
        (&workspace,),
    )
    .await
    .unwrap();
    drop(conn);
    let router = app(fixture.db.clone()).await;
    let member_cookie = account_cookie("acc_member_fixture", "member@example.test");
    let (status, _, info) = request(
        router.clone(),
        "GET",
        &format!("/workspaces/{workspace}"),
        Some(&member_cookie),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(info["name"], "Members");
    let (status, _, _) = request(
        router.clone(),
        "PATCH",
        &format!("/workspaces/{workspace}"),
        Some(&member_cookie),
        Some(json!({"name":"forbidden"})),
    )
    .await;
    assert_ne!(
        status,
        StatusCode::OK,
        "member must not change workspace settings"
    );

    let (status, _, first) = request(
        router.clone(),
        "POST",
        &format!("/workspaces/{workspace}/default-region"),
        Some(&member_cookie),
        Some(json!({"country":"US"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!first.as_array().unwrap().is_empty());
    let (status, _, second) = request(
        router.clone(),
        "POST",
        &format!("/workspaces/{workspace}/default-region"),
        Some(&member_cookie),
        Some(json!({"country":"CN"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let mut conn = fixture.db.conn().await.unwrap();
    let region: Option<String> = mysql_async::prelude::Queryable::exec_first(
        &mut conn,
        "SELECT `region` FROM `workspace` WHERE `id` = ?",
        (&workspace,),
    )
    .await
    .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&region.unwrap()).unwrap(),
        first
    );
    assert_ne!(
        first, second,
        "request returns computed candidate although stored region stays unchanged"
    );
    drop(conn);
    fixture.drop_database().await.unwrap();
}

#[tokio::test]
async fn auth_authorize_fails_closed_without_redis_state_store() {
    let router = app(opencode_hosted_adapters::console::db::ConsoleDb::connect(
        "mysql://u:p@127.0.0.1/test",
    )
    .unwrap())
    .await;
    let (status, _, body) = request(router, "GET", "/auth/authorize", None, None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"], "Console auth state store is not configured");
}

#[tokio::test]
async fn fixture_auth_status_rejects_missing_and_tampered_session_cookies() {
    let fixture = TestDatabase::create("console_http_auth").await.unwrap();
    let router = app(fixture.db.clone()).await;
    for cookie in [None, Some("auth=not-a-valid-seal")] {
        let (status, _, body) = request(router.clone(), "GET", "/auth/status", cookie, None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.get("current").is_none() || body["current"].is_null());
        assert!(
            body.get("account").is_none()
                || body["account"]
                    .as_object()
                    .is_some_and(|accounts| accounts.is_empty())
        );
    }
    fixture.drop_database().await.unwrap();
}

#[tokio::test]
async fn fixture_workspace_mutation_rejects_cross_site_origin() {
    let fixture = TestDatabase::create("console_http_origin").await.unwrap();
    let store = TenantStore::new(fixture.db.clone());
    let account = store.create_account(None).await.unwrap();
    let actor = Actor::Account {
        account_id: account.clone(),
        email: "origin@example.test".into(),
    };
    let workspace = store.create_workspace(&actor, "Origin").await.unwrap();
    let mut conn = fixture.db.conn().await.unwrap();
    mysql_async::prelude::Queryable::exec_drop(
        &mut conn,
        "UPDATE `billing` SET `subscription_id` = 'sub_fx_o' WHERE `workspace_id` = ?",
        (&workspace,),
    )
    .await
    .unwrap();
    drop(conn);
    let router = app(fixture.db.clone()).await;
    let cookie = account_cookie(&account, "origin@example.test");
    let mut request = Request::builder()
        .method("PATCH")
        .uri(format!("/workspaces/{workspace}"))
        .header("host", "console.opencode.ai")
        .header("origin", "https://evil.example")
        .header("cookie", cookie.split(';').next().unwrap())
        .header("content-type", "application/json")
        .body(Body::from(json!({"name":"cross-site"}).to_string()))
        .unwrap();
    request
        .headers_mut()
        .insert("sec-fetch-site", "cross-site".parse().unwrap());
    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    fixture.drop_database().await.unwrap();
}

#[tokio::test]
async fn fixture_actor_action_resolves_account_and_rejects_cross_site() {
    let fixture = TestDatabase::create("console_http_actor").await.unwrap();
    let store = TenantStore::new(fixture.db.clone());
    let account = store.create_account(None).await.unwrap();
    let workspace = store
        .create_workspace(
            &Actor::Account {
                account_id: account.clone(),
                email: "actor@example.test".into(),
            },
            "Actor",
        )
        .await
        .unwrap();
    let mut conn = fixture.db.conn().await.unwrap();
    mysql_async::prelude::Queryable::exec_drop(
        &mut conn,
        "UPDATE `billing` SET `subscription_id` = 'sub_fx_r' WHERE `workspace_id` = ?",
        (&workspace,),
    )
    .await
    .unwrap();
    drop(conn);
    let router = app(fixture.db.clone()).await;
    let cookie = account_cookie(&account, "actor@example.test");
    let (status, _, body) = request(
        router.clone(),
        "POST",
        "/auth/action/actor",
        Some(&cookie),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["type"], "account");
    assert_eq!(body["properties"]["accountID"], account);

    let mut request = Request::builder()
        .method("POST")
        .uri("/auth/action/actor")
        .header("host", "console.opencode.ai")
        .header("origin", "https://evil.example")
        .header("cookie", cookie.split(';').next().unwrap())
        .header("content-type", "application/json")
        .body(Body::from(json!({"workspaceID":workspace}).to_string()))
        .unwrap();
    request
        .headers_mut()
        .insert("sec-fetch-site", "cross-site".parse().unwrap());
    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    fixture.drop_database().await.unwrap();
}

#[tokio::test]
async fn fixture_workspace_delete_soft_deletes_only_admitted_workspace() {
    let fixture = TestDatabase::create("console_http_delete").await.unwrap();
    let store = TenantStore::new(fixture.db.clone());
    let account = store.create_account(None).await.unwrap();
    let other = store.create_account(None).await.unwrap();
    let workspace = store
        .create_workspace(
            &Actor::Account {
                account_id: account.clone(),
                email: "delete@example.test".into(),
            },
            "Delete me",
        )
        .await
        .unwrap();
    let other_workspace = store
        .create_workspace(
            &Actor::Account {
                account_id: other.clone(),
                email: "other-delete@example.test".into(),
            },
            "Keep me",
        )
        .await
        .unwrap();
    let mut conn = fixture.db.conn().await.unwrap();
    mysql_async::prelude::Queryable::exec_drop(
        &mut conn,
        "UPDATE `billing` SET `subscription_id` = 'sub_fx_d' WHERE `workspace_id` = ?",
        (&workspace,),
    )
    .await
    .unwrap();
    mysql_async::prelude::Queryable::exec_drop(
        &mut conn,
        "UPDATE `billing` SET `subscription_id` = 'sub_fx_e' WHERE `workspace_id` = ?",
        (&other_workspace,),
    )
    .await
    .unwrap();
    drop(conn);
    let router = app(fixture.db.clone()).await;
    let cookie = account_cookie(&account, "delete@example.test");
    let (status, _, _) = request(
        router.clone(),
        "DELETE",
        &format!("/workspaces/{workspace}"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let mut conn = fixture.db.conn().await.unwrap();
    let deleted: Option<u64> = mysql_async::prelude::Queryable::exec_first(
        &mut conn,
        "SELECT COUNT(*) FROM `workspace` WHERE `id` = ? AND `time_deleted` IS NOT NULL",
        (&workspace,),
    )
    .await
    .unwrap();
    assert_eq!(deleted, Some(1));
    drop(conn);
    let (status, _, rows) =
        request(router.clone(), "GET", "/workspaces", Some(&cookie), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        rows.as_array()
            .unwrap()
            .iter()
            .all(|row| row["id"] != workspace)
    );
    let (status, _, _) = request(
        router,
        "GET",
        &format!("/workspaces/{other_workspace}"),
        Some(&cookie),
        None,
    )
    .await;
    assert_ne!(status, StatusCode::OK);
    assert!(status.is_redirection() || status == StatusCode::FORBIDDEN);
    fixture.drop_database().await.unwrap();
}
