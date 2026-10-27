//! Purpose: fixture tests for Stripe raw-body signatures, controlled HTTP lookups, and MySQL credit idempotency.
//! Scope: adapter public APIs; local Docker MySQL and a local HTTP server only.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/billing_store.rs
//! |-- hosted-adapters/src/console/db.rs
//! `-- hosted-adapters/src/console/stripe.rs
//! External:
//! |-- axum::{Json, Router, routing::get}
//! |-- mysql_async::prelude::Queryable
//! |-- serde_json::json
//! |-- tokio::test
//! `-- tokio::net::TcpListener
//! Used by:
//! `-- none, entrypoint
//! Invariants: never calls Stripe's production API or creates a payment; database is a unique local migrated fixture.
//! Contract tests: this file.

use axum::routing::get;
use axum::{Json, Router};
use mysql_async::prelude::Queryable;
use opencode_hosted_adapters::console::billing_store::{
    BillingStore, CheckoutCredit, CreditOutcome,
};
use opencode_hosted_adapters::console::db::TestDatabase;
use opencode_hosted_adapters::console::stripe::{
    StripeClient, StripeError, sign_webhook_payload, verify_webhook_signature,
};
use serde_json::{Value, json};
use tokio::net::TcpListener;

#[tokio::test]
async fn fixture_stripe_lifecycle_encodes_forms_and_orders_reload() {
    use axum::extract::{OriginalUri, State};
    use axum::http::{HeaderMap, Method};
    use std::sync::{Arc, Mutex};
    type Calls = Arc<Mutex<Vec<(String, String, String)>>>;
    async fn capture(
        State(calls): State<Calls>,
        method: Method,
        OriginalUri(uri): OriginalUri,
        headers: HeaderMap,
        body: String,
    ) -> Json<Value> {
        assert_eq!(headers["authorization"], "Bearer sk_fixture_only");
        assert_eq!(headers["stripe-version"], "2025-03-31.basil");
        if method != Method::GET {
            assert_eq!(headers["content-type"], "application/x-www-form-urlencoded");
        }
        calls
            .lock()
            .unwrap()
            .push((method.to_string(), uri.to_string(), body));
        Json(match uri.path() {
            "/v1/checkout/sessions" => {
                json!({"id":"cs_fixture","url":"https://checkout.example.test/session"})
            }
            "/v1/billing_portal/sessions" => json!({"url":"https://portal.example.test/session"}),
            "/v1/invoices" => json!({"id":"in_fixture"}),
            _ => json!({"id":"fixture"}),
        })
    }
    let calls: Calls = Arc::default();
    let router = Router::new().fallback(capture).with_state(calls.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let stripe = StripeClient::new(
        "sk_fixture_only",
        &format!("http://{}", listener.local_addr().unwrap()),
    )
    .unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let session = stripe
        .create_object(
            "checkout/sessions",
            &[
                ("metadata[workspaceID]".into(), "wrk fixture & +".into()),
                (
                    "success_url".into(),
                    "https://console.example.test/success?x=1&y=2".into(),
                ),
            ],
        )
        .await
        .unwrap();
    assert_eq!(session["url"], "https://checkout.example.test/session");
    stripe
        .create_object(
            "billing_portal/sessions",
            &[("customer".into(), "cus_fixture".into())],
        )
        .await
        .unwrap();
    stripe
        .reload("cus_fixture", "pm_fixture", "wrk_fixture", 20)
        .await
        .unwrap();
    stripe
        .update_subscription(
            "sub_fixture",
            &[("cancel_at_period_end".into(), "true".into())],
        )
        .await
        .unwrap();
    stripe.cancel_subscription("sub_fixture").await.unwrap();
    stripe
        .get_object("invoices", "in_fixture", &["payments", "discounts"])
        .await
        .unwrap();
    let captured = calls.lock().unwrap();
    assert_eq!(captured.len(), 10);
    let fields = url::form_urlencoded::parse(captured[0].2.as_bytes())
        .into_owned()
        .collect::<std::collections::HashMap<_, _>>();
    assert_eq!(fields["metadata[workspaceID]"], "wrk fixture & +");
    assert_eq!(
        fields["success_url"],
        "https://console.example.test/success?x=1&y=2"
    );
    assert_eq!(captured[2].1, "/v1/invoices");
    let draft = url::form_urlencoded::parse(captured[2].2.as_bytes())
        .into_owned()
        .collect::<std::collections::HashMap<_, _>>();
    assert_eq!(draft["metadata[amount]"], "2000");
    assert_eq!(captured[3].1, "/v1/invoiceitems");
    assert!(captured[4].2.contains("amount=123"));
    assert_eq!(captured[5].1, "/v1/invoices/in_fixture/finalize");
    assert_eq!(captured[6].1, "/v1/invoices/in_fixture/pay");
    assert_eq!(captured[8].0, "DELETE");
    assert!(captured[9].1.contains("expand%5B%5D=payments"));
    drop(captured);
    server.abort();
}

#[tokio::test]
async fn fixture_stripe_preserves_provider_decline_message_without_redirects() {
    let router = Router::new().fallback(|| async {
        (
            axum::http::StatusCode::PAYMENT_REQUIRED,
            Json(json!({"error":{"message":"Your card was declined."}})),
        )
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let stripe = StripeClient::new(
        "sk_fixture_only",
        &format!("http://{}", listener.local_addr().unwrap()),
    )
    .unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let failure = stripe
        .reload("cus_fixture", "pm_fixture", "wrk_fixture", 20)
        .await
        .unwrap_err();
    assert_eq!(failure.to_string(), "Your card was declined.");
    assert!(matches!(
        failure,
        StripeError::Api {
            status: axum::http::StatusCode::PAYMENT_REQUIRED,
            ..
        }
    ));
    server.abort();
    let _ = server.await;
}

async fn stripe_fixture() -> (String, tokio::task::JoinHandle<()>) {
    async fn intent() -> Json<Value> {
        Json(json!({
            "id":"pi_local_1",
            "payment_method":{"id":"pm_local_1","type":"card","card":{"last4":"4242"}}
        }))
    }

    let router = Router::new().route("/v1/payment_intents/pi_local_1", get(intent));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (format!("http://{address}/"), server)
}

#[tokio::test]
async fn fixture_stripe_signature_and_payment_method_lookup() {
    let raw = br#"{ "id":"evt_local" }"#;
    let signature = sign_webhook_payload(raw, 1_700_000_000, "whsec_local");
    assert!(verify_webhook_signature(raw, &signature, "whsec_local", 1_700_000_300).is_ok());
    assert!(matches!(
        verify_webhook_signature(raw, &signature, "whsec_wrong", 1_700_000_000),
        Err(StripeError::InvalidSignature)
    ));
    assert!(matches!(
        verify_webhook_signature(raw, &signature, "whsec_local", 1_700_000_301),
        Err(StripeError::StaleSignature)
    ));

    let (base, server) = stripe_fixture().await;
    let stripe = StripeClient::new("sk_fixture_only", &base).unwrap();
    let intent = stripe.payment_intent("pi_local_1").await.unwrap();
    assert_eq!(intent.id, "pi_local_1");
    assert_eq!(intent.payment_method.id, "pm_local_1");
    assert_eq!(
        intent.payment_method.card.unwrap().last4.as_deref(),
        Some("4242")
    );
    server.abort();
}

#[tokio::test]
async fn fixture_credit_transaction_uses_persisted_payment_ids_for_idempotency() {
    let fixture = TestDatabase::create("console_commerce_credit")
        .await
        .unwrap();
    let mut conn = fixture.db.conn().await.unwrap();
    conn.exec_drop(
        "INSERT INTO `workspace` (`id`, `name`) VALUES ('wrk_commerce_1', 'Commerce fixture')",
        (),
    )
    .await
    .unwrap();
    conn.exec_drop(
        "INSERT INTO `billing` (`id`, `workspace_id`, `balance`, `reload_error`, `time_reload_error`) VALUES ('bil_commerce_1', 'wrk_commerce_1', 0, 'first association clears this', NOW(3))",
        (),
    )
    .await
    .unwrap();
    drop(conn);

    let store = BillingStore::new(fixture.db.clone());
    let credit = CheckoutCredit {
        event_id: "evt_local_1".into(),
        workspace_id: "wrk_commerce_1".into(),
        customer_id: "cus_local_1".into(),
        payment_id: "pi_local_1".into(),
        invoice_id: "in_local_1".into(),
        amount_cents: 2_000,
        payment_method_id: "pm_local_1".into(),
        payment_method_type: "card".into(),
        payment_method_last4: Some("4242".into()),
    };
    assert_eq!(
        store.complete_checkout(&credit).await.unwrap(),
        CreditOutcome::Applied
    );
    assert_eq!(
        store.complete_checkout(&credit).await.unwrap(),
        CreditOutcome::Duplicate
    );
    let replay = CheckoutCredit {
        payment_id: "pi_replayed_with_new_id".into(),
        invoice_id: "in_replayed_with_new_id".into(),
        ..credit.clone()
    };
    assert_eq!(
        store.complete_checkout(&replay).await.unwrap(),
        CreditOutcome::Duplicate
    );

    conn = fixture.db.conn().await.unwrap();
    let balance: i64 = conn
        .exec_first(
            "SELECT `balance` FROM `billing` WHERE `workspace_id` = 'wrk_commerce_1'",
            (),
        )
        .await
        .unwrap()
        .expect("billing row exists");
    let payments: i64 = conn
        .exec_first(
            "SELECT COUNT(*) FROM `payment` WHERE `workspace_id` = 'wrk_commerce_1' AND `payment_id` = 'pi_local_1'",
            (),
        )
        .await
        .unwrap()
        .expect("payment count query returns a row");
    assert_eq!(balance, 2_000_000_000);
    assert_eq!(payments, 1);
    let summary = store.summary("wrk_commerce_1").await.unwrap();
    assert!(summary.reload_error.is_none());
    assert!(summary.time_reload_error.is_none());
    drop(conn);
    fixture.drop_database().await.unwrap();
}
