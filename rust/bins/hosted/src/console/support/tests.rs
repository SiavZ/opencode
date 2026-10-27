//! Purpose: exercise support mutation authorization, enterprise validation and Honeycomb forwarding.
//! Scope: Axum HTTP acceptance with isolated MySQL and loopback Discord capture.
//! Uses:
//! rust/bins/
//! |-- hosted/src/console/state.rs
//! `-- hosted/src/console/support.rs
//! rust/crates/
//! |-- hosted-adapters/src/console/db.rs
//! `-- hosted-adapters/src/console/tenant.rs
//! External:
//! |-- axum::{Router, body::{Body, to_bytes}, http::{Request, StatusCode}}
//! |-- mysql_async::prelude::Queryable
//! |-- serde_json::{Value, json}
//! `-- tower::ServiceExt
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/support.rs
//! Invariants: no live integration calls, fixture database only; every spawned server is joined.
//! Contract tests: this file.

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use mysql_async::prelude::Queryable;
use opencode_hosted_adapters::console::db::{ConsoleDb, TestDatabase};
use serde_json::{Value, json};
use tower::ServiceExt;

use super::super::state::{ConsoleConfig, ConsoleState};
use super::Config;

pub(super) fn app(db: ConsoleDb, support: Config, stage: &str) -> Router {
    super::routes().with_state(ConsoleState {
        db,
        config: std::sync::Arc::new(ConsoleConfig {
            support,
            stage: stage.into(),
            ..Default::default()
        }),
    })
}

pub(super) async fn request(
    app: Router,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    for (key, value) in headers {
        request = request.header(*key, *value);
    }
    let response = app
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

fn offline_db() -> ConsoleDb {
    ConsoleDb::connect("mysql://fixture:fixture@127.0.0.1:1/not_connected").unwrap()
}
pub(super) fn config() -> Config {
    Config {
        api_key: Some("fixture-support-key".into()),
        honeycomb_secret: Some("fixture-honey-secret".into()),
        ..Default::default()
    }
}
pub(super) const AUTH: &[(&str, &str)] = &[("authorization", "Bearer fixture-support-key")];

#[tokio::test]
async fn support_authentication_precedes_validation_and_sql() {
    for (method, path) in [
        ("POST", "block-workspaces"),
        ("POST", "unblock-workspaces"),
        ("POST", "unblock-workspace"),
        ("POST", "reset-quota"),
        ("POST", "create-referral"),
        ("DELETE", "delete-account"),
    ] {
        let path = format!("/api/support/actions/{path}");
        for authorization in [
            None,
            Some("Bearer wrong"),
            Some("bearer fixture-support-key"),
        ] {
            let headers = authorization
                .map(|value| vec![("authorization", value)])
                .unwrap_or_default();
            assert_eq!(
                request(
                    app(offline_db(), config(), "production"),
                    method,
                    &path,
                    &headers,
                    "not-json"
                )
                .await,
                (StatusCode::UNAUTHORIZED, json!({"error":"Unauthorized"}))
            );
        }
        let (status, body) = request(
            app(offline_db(), config(), "production"),
            method,
            &path,
            AUTH,
            "not-json",
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "Invalid request");
        assert!(body["issues"].is_array());
    }
    let debug = format!("{:?}", config());
    assert!(!debug.contains("fixture-support-key") && !debug.contains("fixture-honey-secret"));
    let (status, body) = request(
        app(offline_db(), config(), "production"),
        "POST",
        "/api/support/actions/reset-quota",
        AUTH,
        r#"{"workspaceID":"bad","plan":"other"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["issues"].as_array().unwrap().len(), 2);
    assert_eq!(
        request(
            app(offline_db(), Config::default(), "production"),
            "POST",
            "/api/support/actions/unblock-workspace",
            AUTH,
            "{}"
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn fixture_support_block_batches_and_account_erasure_persist() {
    let fixture = TestDatabase::create("support_http").await.unwrap();
    let mut conn = fixture.db.conn().await.unwrap();
    conn.query_drop(
        "INSERT INTO workspace (id,name) VALUES ('wrk_support','Support'),('wrk_other','Other')",
    )
    .await
    .unwrap();
    let body = r#"{"workspaceIDs":["wrk_support","wrk_missing","wrk_support","wrk_other"]}"#;
    let (status, reply) = request(
        app(fixture.db.clone(), config(), "production"),
        "POST",
        "/api/support/actions/block-workspaces",
        AUTH,
        body,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        reply,
        json!({"success":true,"message":"Workspaces blocked","result":{"blocked":["wrk_support","wrk_other"],"notFound":["wrk_missing"]}})
    );
    assert_eq!(
        conn.query_first::<u64, _>("SELECT COUNT(*) FROM workspace WHERE is_blocked = true")
            .await
            .unwrap(),
        Some(2)
    );
    assert_eq!(
        request(
            app(fixture.db.clone(), config(), "production"),
            "POST",
            "/api/support/actions/block-workspaces",
            AUTH,
            body
        )
        .await
        .1,
        reply
    );
    let reply = request(
        app(fixture.db.clone(), config(), "production"),
        "POST",
        "/api/support/actions/unblock-workspaces",
        AUTH,
        body,
    )
    .await;
    assert_eq!(
        reply.1["result"]["unblocked"],
        json!(["wrk_support", "wrk_other"])
    );
    assert_eq!(
        request(
            app(fixture.db.clone(), config(), "production"),
            "POST",
            "/api/support/actions/unblock-workspace",
            AUTH,
            r#"{"workspaceID":"wrk_missing"}"#
        )
        .await,
        (
            StatusCode::BAD_REQUEST,
            json!({"error":"Workspace not found"})
        )
    );
    conn.query_drop("INSERT INTO account (id) VALUES ('acc_support')")
        .await
        .unwrap();
    conn.query_drop("INSERT INTO auth (id,provider,subject,account_id) VALUES ('aut_support','email','delete@example.test','acc_support')").await.unwrap();
    conn.query_drop("INSERT INTO `user` (id,workspace_id,account_id,name,role,email) VALUES ('usr_support','wrk_support','acc_support','Private','admin','delete@example.test')").await.unwrap();
    conn.query_drop("INSERT INTO `key` (id,workspace_id,user_id,name,`key`) VALUES ('key_support','wrk_support','usr_support','Secret','sk-fixture-secret')").await.unwrap();
    conn.query_drop("INSERT INTO coupon (email,type) VALUES ('delete@example.test','BUILDATHON')")
        .await
        .unwrap();
    let (status, reply) = request(
        app(fixture.db.clone(), config(), "production"),
        "DELETE",
        "/api/support/actions/delete-account",
        AUTH,
        r#"{"email":"delete@example.test"}"#,
    )
    .await;
    assert_eq!(
        (status, reply),
        (
            StatusCode::OK,
            json!({"success":true,"message":"Account deleted"})
        )
    );
    assert_eq!(
        conn.query_first::<u64, _>("SELECT COUNT(*) FROM auth")
            .await
            .unwrap(),
        Some(0)
    );
    assert_eq!(
        conn.query_first::<u64, _>("SELECT COUNT(*) FROM coupon")
            .await
            .unwrap(),
        Some(0)
    );
    assert_eq!(
        conn.query_first::<u64, _>("SELECT COUNT(*) FROM `key` WHERE time_deleted IS NOT NULL")
            .await
            .unwrap(),
        Some(1)
    );
    assert_eq!(conn.query_first::<u64,_>("SELECT COUNT(*) FROM `user` WHERE account_id IS NULL AND email IS NULL AND name = '' AND time_deleted IS NOT NULL").await.unwrap(),Some(1));
    drop(conn);
    fixture.drop_database().await.unwrap();
}

#[tokio::test]
async fn support_enterprise_honeypot_validation_locales_and_delivery_failure() {
    let endpoint = "/api/enterprise";
    assert_eq!(
        request(
            app(offline_db(), config(), "production"),
            "POST",
            endpoint,
            &[],
            r#"{"alias":"bot"}"#
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        request(
            app(offline_db(), config(), "production"),
            "POST",
            endpoint,
            &[("x-opencode-locale", "fr")],
            "{}"
        )
        .await,
        (
            StatusCode::BAD_REQUEST,
            json!({"error":"Tous les champs sont requis."})
        )
    );
    let inquiry =
        r#"{"name":"Ada","role":"Engineer","email":"ada@example.test","message":"Interested"}"#;
    assert_eq!(
        request(
            app(offline_db(), config(), "production"),
            "POST",
            endpoint,
            &[],
            inquiry
        )
        .await
        .0,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(
        request(
            app(offline_db(), config(), "dev"),
            "POST",
            endpoint,
            &[],
            inquiry
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        request(
            app(offline_db(), config(), "production"),
            "POST",
            endpoint,
            &[],
            "{"
        )
        .await
        .0,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(
        request(
            app(offline_db(), config(), "production"),
            "POST",
            endpoint,
            &[],
            r#"{"name":"Ada","role":"Engineer","email":"bad","message":"Interested"}"#
        )
        .await,
        (
            StatusCode::BAD_REQUEST,
            json!({"error":"Invalid email format."})
        )
    );
}

#[tokio::test]
async fn support_honeycomb_capture_formats_incidents_and_ignores_resolved() {
    use std::sync::{Arc, Mutex};
    let capture = Arc::new(Mutex::new(Vec::<Value>::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/discord", listener.local_addr().unwrap());
    let sink = capture.clone();
    let stop = tokio_util::sync::CancellationToken::new();
    let shutdown = stop.clone();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route(
                "/discord",
                axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
                    let sink = sink.clone();
                    async move {
                        sink.lock().unwrap().push(body);
                        StatusCode::NO_CONTENT
                    }
                }),
            ),
        )
        .with_graceful_shutdown(shutdown.cancelled_owned())
        .await
        .unwrap();
    });
    let support = Config {
        discord_webhook_url: Some(url),
        ..config()
    };
    let headers = &[("x-honeycomb-webhook-token", "fixture-honey-secret")];
    let mut payload = json!({"type":"model_low_tps","name":"Low TPS","status":"TRIGGERED","isTest":true,"url":"https://example.test/alert","groups":[{"result":"12.6","group":[{"key":"model","value":"fixture-model"}]}]});
    assert_eq!(
        request(
            app(offline_db(), support.clone(), "production"),
            "POST",
            "/honeycomb/webhook",
            &[],
            "broken"
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(
            app(offline_db(), support.clone(), "production"),
            "POST",
            "/honeycomb/webhook",
            headers,
            &payload.to_string()
        )
        .await,
        (StatusCode::OK, json!({"message":"sent"}))
    );
    assert_eq!(
        capture.lock().unwrap()[0],
        json!({"content":"[**[TEST] Low TPS**](https://example.test/alert)\n- fixture-model (13 TPS)\n\n<@&1520924666359713863>","allowed_mentions":{"roles":["1520924666359713863"]},"flags":4})
    );
    payload["status"] = json!("RESOLVED");
    assert_eq!(
        request(
            app(offline_db(), support.clone(), "production"),
            "POST",
            "/honeycomb/webhook",
            headers,
            &payload.to_string()
        )
        .await
        .1,
        json!({"message":"ignored"})
    );
    assert_eq!(capture.lock().unwrap().len(), 1);
    payload["type"] = json!("unknown");
    assert_eq!(
        request(
            app(offline_db(), support, "production"),
            "POST",
            "/honeycomb/webhook",
            headers,
            &payload.to_string()
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    stop.cancel();
    server.await.unwrap();
}
