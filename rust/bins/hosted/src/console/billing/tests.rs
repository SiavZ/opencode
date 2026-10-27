//! Purpose: acceptance fixture for Stripe webhook verification, provider lookup, and durable idempotent credits.
//! Scope: Axum request boundary, local Stripe HTTP fixture, and unique migrated local MySQL database.
//! Uses:
//! rust/bins/
//! |-- hosted/src/console/billing.rs
//! `-- hosted/src/console/state.rs
//! rust/crates/
//! |-- hosted-adapters/src/console/db.rs
//! |-- hosted-adapters/src/console/stripe.rs
//! `-- hosted-adapters/src/console/tenant.rs
//! External:
//! |-- axum::{Router, body::Body, body::to_bytes, http::Request, routing::get}
//! |-- mysql_async::prelude::Queryable
//! |-- serde_json::json
//! |-- tokio::test
//! `-- tower::ServiceExt
//! Used by:
//! `-- none, entrypoint
//! Invariants: no real Stripe requests or payments; DB is unique and fixture-only.
//! Contract tests: this file.

use std::sync::Arc;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use axum::routing::get;
use mysql_async::prelude::Queryable;
use opencode_hosted_adapters::console::db::TestDatabase;
use opencode_hosted_adapters::console::stripe::sign_webhook_payload;
use opencode_hosted_adapters::console::tenant::TenantStore;
use opencode_hosted_domain::console::common::Actor;
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tower::ServiceExt;

use super::super::billing::Config;
use super::super::state::{ConsoleConfig, ConsoleState};

const WEBHOOK_SECRET: &str = "whsec_console_fixture";
const STRIPE_SECRET: &str = "sk_test_console_fixture";

async fn controlled_stripe() -> (String, tokio::task::JoinHandle<()>) {
    async fn payment_intent() -> axum::Json<Value> {
        axum::Json(json!({
            "id":"pi_fixture_1",
            "payment_method":{"id":"pm_fixture_1","type":"card","card":{"last4":"4242"}}
        }))
    }
    let app = Router::new()
        .route("/v1/payment_intents/pi_fixture_1", get(payment_intent))
        .route(
            "/v1/customers/cus_fixture_1",
            axum::routing::post(|| async { axum::Json(json!({"id":"cus_fixture_1"})) }),
        );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (base, server)
}

fn webhook_event(workspace_id: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "id":"evt_fixture_checkout_1",
        "type":"checkout.session.completed",
        "data":{"object":{
            "mode":"payment",
            "metadata":{"workspaceID":workspace_id,"amount":"2000"},
            "customer":"cus_fixture_1",
            "payment_intent":"pi_fixture_1",
            "invoice":"in_fixture_1"
        }}
    }))
    .unwrap()
}

async fn send(app: Router, raw: &[u8], signature: &str) -> (StatusCode, Value) {
    let response = app
        .oneshot(
            Request::post("/stripe/webhook")
                .header("content-type", "application/json")
                .header("stripe-signature", signature)
                .body(Body::from(raw.to_vec()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

#[tokio::test]
async fn fixture_checkout_webhook_is_signed_and_durably_idempotent() {
    let fixture = TestDatabase::create("console_billing_webhook")
        .await
        .unwrap();
    let tenant = TenantStore::new(fixture.db.clone());
    let account_id = tenant.create_account(None).await.unwrap();
    let workspace_id = tenant
        .create_workspace(
            &Actor::Account {
                account_id,
                email: "billing-fixture@example.test".into(),
            },
            "billing-fixture",
        )
        .await
        .unwrap();
    let (stripe_base, server) = controlled_stripe().await;
    let state = ConsoleState {
        config: Arc::new(ConsoleConfig {
            billing: Config {
                secret_key: Some(STRIPE_SECRET.into()),
                webhook_secret: Some(WEBHOOK_SECRET.into()),
                api_base: Some(stripe_base),
                ..Default::default()
            },
            ..Default::default()
        }),
        db: fixture.db.clone(),
    };
    let app = super::super::router(state);
    let raw = webhook_event(&workspace_id);
    let timestamp = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()) as i64;
    let signature = sign_webhook_payload(&raw, timestamp, WEBHOOK_SECRET);

    let (status, _) = send(app.clone(), &raw, &format!("t={timestamp},v1=bad")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let mut conn = fixture.db.conn().await.unwrap();
    let before: Option<i64> = conn
        .exec_first(
            "SELECT `balance` FROM `billing` WHERE `workspace_id` = ?",
            (&workspace_id,),
        )
        .await
        .unwrap();
    assert_eq!(before, Some(0));
    drop(conn);

    assert_eq!(send(app.clone(), &raw, &signature).await.0, StatusCode::OK);
    assert_eq!(send(app.clone(), &raw, &signature).await.0, StatusCode::OK);

    conn = fixture.db.conn().await.unwrap();
    let balance: Option<i64> = conn
        .exec_first(
            "SELECT `balance` FROM `billing` WHERE `workspace_id` = ?",
            (&workspace_id,),
        )
        .await
        .unwrap();
    let payments: Option<i64> = conn
        .exec_first(
            "SELECT COUNT(*) FROM `payment` WHERE `workspace_id` = ? AND `payment_id` = 'pi_fixture_1'",
            (&workspace_id,),
        )
        .await
        .unwrap();
    let payment_method: Option<Option<String>> = conn
        .exec_first(
            "SELECT `payment_method_last4` FROM `billing` WHERE `workspace_id` = ?",
            (&workspace_id,),
        )
        .await
        .unwrap();
    assert_eq!(balance, Some(2_000_000_000));
    assert_eq!(payments, Some(1));
    assert_eq!(payment_method.flatten().as_deref(), Some("4242"));
    drop(conn);

    server.abort();
    fixture.drop_database().await.unwrap();
}
