//! Purpose: validate signed billing webhook lifecycle through Axum and local Stripe/MySQL fixtures.
//! Scope: customer updates, invoice credits and failures, refunds, subscriptions and retirement.
//! Uses:
//! rust/bins/
//! `-- hosted/src/console/billing.rs
//! rust/crates/
//! |-- hosted-adapters/src/console/db.rs
//! `-- hosted-adapters/src/console/stripe.rs
//! External:
//! |-- axum::{Router, body, extract, http}
//! |-- mysql_async::prelude::Queryable
//! |-- serde_json::{Value, json}
//! `-- tower::ServiceExt
//! Used by:
//! `-- none, entrypoint
//! Invariants: all provider calls are captured on loopback and all data uses an isolated migrated fixture database.
//! Contract tests: this file.

use super::super::state::ConsoleConfig;
use super::*;
use axum::body::{Body, to_bytes};
use axum::extract::{OriginalUri, State};
use axum::http::{Method, Request};
use mysql_async::prelude::Queryable;
use opencode_hosted_adapters::console::db::TestDatabase;
use opencode_hosted_adapters::console::stripe::sign_webhook_payload;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

const SECRET: &str = "whsec_lifecycle_fixture";
type Calls = Arc<Mutex<Vec<(String, String, String)>>>;

async fn provider(
    State(calls): State<Calls>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    body: String,
) -> Json<Value> {
    calls
        .lock()
        .unwrap()
        .push((method.to_string(), uri.path().into(), body));
    Json(match uri.path() {
        "/v1/payment_methods/pm_fixture" => {
            json!({"id":"pm_fixture","type":"card","card":{"last4":"1234"}})
        }
        "/v1/invoices/in_reload" => {
            json!({"payments":{"data":[{"payment":{"payment_intent":"pi_reload"}}]},"discounts":[]})
        }
        "/v1/invoices/in_subscription" => {
            json!({"payments":{"data":[]},"discounts":[{"coupon":{"id":"coupon_free"}}]})
        }
        "/v1/invoices/in_open" => json!({"id":"in_open","status":"open"}),
        "/v1/payment_intents/in_failed" => {
            json!({"last_payment_error":{"message":"Fixture declined"}})
        }
        "/v1/subscriptions" => json!({"id":"sub_black_enrolled"}),
        "/v1/checkout/sessions" => json!({"url":"https://checkout.example.test/fixture"}),
        "/v1/billing_portal/sessions" => json!({"url":"https://portal.example.test/fixture"}),
        "/v1/payment_intents/pi_reload" => json!({"latest_charge":"ch_fixture"}),
        "/v1/charges/ch_fixture" => json!({"receipt_url":"https://receipt.example.test/fixture"}),
        _ => json!({"id":"fixture"}),
    })
}

async fn send(app: &Router, event: Value) -> (StatusCode, Value) {
    let raw = serde_json::to_vec(&event).unwrap();
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let response = app
        .clone()
        .oneshot(
            Request::post("/stripe/webhook")
                .header("stripe-signature", sign_webhook_payload(&raw, now, SECRET))
                .body(Body::from(raw))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let value =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    (status, value)
}

fn event(kind: &str, object: Value) -> Value {
    json!({"id":format!("evt_{kind}"),"type":kind,"created":1_700_000_000,"data":{"object":object}})
}

#[tokio::test]
async fn fixture_signed_billing_lifecycle_and_action_operations() {
    let fixture = TestDatabase::create("billing_lifecycle_http")
        .await
        .unwrap();
    let mut conn = fixture.db.conn().await.unwrap();
    conn.exec_drop(
        "INSERT INTO workspace(id,name) VALUES ('wrk_billing_http','HTTP billing')",
        (),
    )
    .await
    .unwrap();
    conn.exec_drop("INSERT INTO billing(id,workspace_id,balance,customer_id,payment_method_id) VALUES ('bil_billing_http','wrk_billing_http',0,'cus_fixture','pm_fixture')",()).await.unwrap();
    conn.exec_drop("INSERT INTO account(id) VALUES ('acc_billing_ref')", ())
        .await
        .unwrap();
    conn.exec_drop(
        "INSERT INTO workspace(id,name) VALUES ('wrk_billing_ref_inviter','Referrer')",
        (),
    )
    .await
    .unwrap();
    conn.exec_drop("INSERT INTO user(id,workspace_id,account_id,name,role) VALUES ('usr_fixture','wrk_billing_http','acc_billing_ref','','admin')",()).await.unwrap();
    conn.exec_drop("INSERT INTO referral(id,workspace_id,invitee_account_id) VALUES ('ref_billing_http','wrk_billing_ref_inviter','acc_billing_ref')",()).await.unwrap();
    drop(conn);
    let calls: Calls = Arc::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let provider_app = Router::new().fallback(provider).with_state(calls.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, provider_app).await.unwrap();
    });
    let state = ConsoleState {
        db: fixture.db.clone(),
        config: Arc::new(ConsoleConfig {
            billing: Config {
                secret_key: Some("sk_fixture".into()),
                webhook_secret: Some(SECRET.into()),
                api_base: Some(base),
                resources: [
                    ("ZEN_LITE_PRICE.product".into(), "prod_lite".into()),
                    ("ZEN_BLACK_PRICE.product".into(), "prod_black".into()),
                    ("ZEN_BLACK_PRICE.plan20".into(), "price_black20".into()),
                ]
                .into_iter()
                .collect(),
            },
            ..Default::default()
        }),
    };
    let app = routes().with_state(state.clone());
    let mut changed = event(
        "customer.updated",
        json!({"id":"cus_fixture","invoice_settings":{"default_payment_method":"pm_fixture"}}),
    );
    assert_eq!(
        send(&app, changed.clone()).await.1,
        json!({"message":"ignored"})
    );
    changed["data"]["previous_attributes"] =
        json!({"invoice_settings":{"default_payment_method":null}});
    assert_eq!(send(&app, changed).await.0, StatusCode::OK);
    let store = BillingStore::new(fixture.db.clone());
    assert_eq!(
        store
            .summary("wrk_billing_http")
            .await
            .unwrap()
            .payment_method_last4
            .as_deref(),
        Some("1234")
    );
    let paid = event(
        "invoice.payment_succeeded",
        json!({"id":"in_reload","customer":"cus_fixture","billing_reason":"manual","metadata":{"workspaceID":"wrk_billing_http","amount":"2000"}}),
    );
    assert_eq!(send(&app, paid.clone()).await.0, StatusCode::OK);
    assert_eq!(send(&app, paid).await.0, StatusCode::OK);
    assert_eq!(
        store.summary("wrk_billing_http").await.unwrap().balance,
        2_000_000_000
    );
    let subscription_invoice = event(
        "invoice.payment_succeeded",
        json!({"id":"in_subscription","customer":"cus_fixture","billing_reason":"subscription_cycle","amount_paid":0,"currency":"inr","parent":{"subscription_details":{"subscription":"sub_lite"}},"lines":{"data":[{"pricing":{"price_details":{"product":"prod_lite"}}}]}}),
    );
    assert_eq!(
        send(&app, subscription_invoice.clone()).await.0,
        StatusCode::OK
    );
    assert_eq!(send(&app, subscription_invoice).await.0, StatusCode::OK);
    let payments = store.payments("wrk_billing_http").await.unwrap();
    assert_eq!(payments.len(), 2);
    assert!(payments.iter().any(|payment| payment.enrichment
        == Some(json!({"type":"lite","currency":"inr","couponID":"coupon_free"}))));
    let failed = event(
        "invoice.payment_failed",
        json!({"id":"in_failed","billing_reason":"manual","metadata":{"workspaceID":"wrk_billing_http"}}),
    );
    assert_eq!(send(&app, failed).await.0, StatusCode::OK);
    assert_eq!(send(&app,event("invoice.payment_action_required",json!({"id":"in_failed","billing_reason":"manual","metadata":{"workspaceID":"wrk_billing_http"}}))).await.0,StatusCode::OK);
    let summary = store.summary("wrk_billing_http").await.unwrap();
    assert_eq!(summary.reload, Some(false));
    assert_eq!(summary.reload_error.as_deref(), Some("Fixture declined"));
    let refund = event(
        "charge.refunded",
        json!({"customer":"cus_fixture","payment_intent":"pi_reload"}),
    );
    assert_eq!(send(&app, refund.clone()).await.0, StatusCode::OK);
    assert_eq!(send(&app, refund).await.0, StatusCode::OK);
    assert_eq!(store.summary("wrk_billing_http").await.unwrap().balance, 0);
    let created = event(
        "customer.subscription.created",
        json!({"id":"sub_lite","customer":"cus_fixture","latest_invoice":"in_subscription","default_payment_method":"pm_fixture","metadata":{"type":"lite","workspaceID":"wrk_billing_http","userID":"usr_fixture"}}),
    );
    assert_eq!(send(&app, created.clone()).await.0, StatusCode::OK);
    assert_eq!(send(&app, created).await.0, StatusCode::OK);
    let mut conn = fixture.db.conn().await.unwrap();
    let rewards:Option<i64> = conn.exec_first("SELECT COUNT(*) FROM referral_reward WHERE referral_id='ref_billing_http' AND amount=500000000",()).await.unwrap();
    assert_eq!(rewards, Some(2));
    drop(conn);
    assert_eq!(
        store
            .summary("wrk_billing_http")
            .await
            .unwrap()
            .lite_subscription_id
            .as_deref(),
        Some("sub_lite")
    );
    let retired = event(
        "customer.subscription.updated",
        json!({"id":"sub_black","status":"active","cancel_at_period_end":false,"cancel_at":null,"items":{"data":[{"price":{"product":"prod_black"}}]}}),
    );
    assert_eq!(send(&app, retired).await.0, StatusCode::OK);
    assert!(
        calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| call.1 == "/v1/subscriptions/sub_black"
                && call.2.contains("cancel_at_period_end=true"))
    );
    let deleted = event(
        "customer.subscription.deleted",
        json!({"id":"sub_lite","latest_invoice":{"id":"in_open"},"items":{"data":[{"price":{"product":"prod_lite"}}]}}),
    );
    assert_eq!(send(&app, deleted.clone()).await.0, StatusCode::OK);
    assert_eq!(send(&app, deleted).await.0, StatusCode::OK);
    assert!(
        store
            .summary("wrk_billing_http")
            .await
            .unwrap()
            .lite_subscription_id
            .is_none()
    );
    assert!(
        calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| call.1 == "/v1/invoices/in_open/void")
    );
    let query = std::collections::HashMap::new();
    let result = operations::execute(&state,"wrk_billing_http","usr_fixture","/checkout",&Method::POST,&query,&json!({"amount":20,"successUrl":"https://console.example.test/success","cancelUrl":"https://console.example.test/cancel"})).await.unwrap();
    assert_eq!(result, "https://checkout.example.test/fixture");
    let portal = operations::execute(
        &state,
        "wrk_billing_http",
        "usr_fixture",
        "/portal",
        &Method::POST,
        &query,
        &json!({"returnUrl":"https://console.example.test"}),
    )
    .await
    .unwrap();
    assert_eq!(portal, "https://portal.example.test/fixture");
    let receipt = operations::execute(
        &state,
        "wrk_billing_http",
        "usr_fixture",
        "/receipt",
        &Method::POST,
        &query,
        &json!({"paymentID":"pi_reload"}),
    )
    .await
    .unwrap();
    assert_eq!(receipt, "https://receipt.example.test/fixture");
    let mut conn = fixture.db.conn().await.unwrap();
    conn.exec_drop("UPDATE billing SET subscription_plan='20',time_subscription_selected=NOW(3) WHERE workspace_id='wrk_billing_http'",()).await.unwrap();
    drop(conn);
    assert_eq!(
        operations::execute(
            &state,
            "wrk_billing_http",
            "usr_fixture",
            "/subscription/enroll",
            &Method::POST,
            &query,
            &json!({})
        )
        .await
        .unwrap(),
        json!({})
    );
    assert_eq!(
        store
            .summary("wrk_billing_http")
            .await
            .unwrap()
            .subscription_id
            .as_deref(),
        Some("sub_black_enrolled")
    );
    store
        .set_use_balance("wrk_billing_http", false, true)
        .await
        .unwrap();
    assert_eq!(
        store
            .summary("wrk_billing_http")
            .await
            .unwrap()
            .subscription
            .unwrap()["seats"],
        1
    );
    let expired = event(
        "customer.subscription.updated",
        json!({"id":"sub_black_enrolled","status":"incomplete_expired","items":{"data":[{"price":{"product":"prod_black"}}]}}),
    );
    assert_eq!(send(&app, expired).await.0, StatusCode::OK);
    assert!(
        store
            .summary("wrk_billing_http")
            .await
            .unwrap()
            .subscription_id
            .is_none()
    );
    let summary = operations::execute(
        &state,
        "wrk_billing_http",
        "usr_fixture",
        "",
        &Method::GET,
        &query,
        &Value::Null,
    )
    .await
    .unwrap();
    assert_eq!(summary["customerID"], "cus_fixture");
    assert_eq!(summary["reloadAmountMin"], 10);
    assert!(summary.get("customerId").is_none());
    let retired_checkout = operations::execute(
        &state,
        "wrk_billing_http",
        "usr_fixture",
        "/lite/checkout",
        &Method::POST,
        &query,
        &Value::Null,
    )
    .await
    .unwrap_err();
    assert_eq!(retired_checkout.status(), StatusCode::BAD_REQUEST);
    let response = app
        .oneshot(
            Request::post("/workspaces/wrk_billing_http/billing/checkout")
                .header("origin", "https://evil.example.test")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(!response.status().is_success());
    server.abort();
    let _ = server.await;
    drop(state);
    drop(store);
    fixture.drop_database().await.unwrap();
}
