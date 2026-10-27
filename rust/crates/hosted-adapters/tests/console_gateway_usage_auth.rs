//! Purpose: check legacy inference authentication and automatic reload through public APIs.
//! Scope: local migrated MySQL and localhost Stripe fake only.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/db.rs
//! |-- hosted-adapters/src/console/formats.rs
//! |-- hosted-adapters/src/console/gateway/usage.rs
//! `-- hosted-adapters/src/console/stripe.rs
//! External:
//! |-- axum::Router
//! |-- mysql_async::prelude::Queryable
//! `-- serde_json::json
//! Used by:
//! `-- none, entrypoint
//! Invariants: no live provider or payment requests; accepted auth and durable effects use real implementation.
//! Contract tests: this file.

use mysql_async::prelude::Queryable;
use opencode_hosted_adapters::console::db::TestDatabase;
use opencode_hosted_adapters::console::formats::UsageInfo;
use opencode_hosted_adapters::console::gateway::usage::{BillingSource, UsageService};
use opencode_hosted_adapters::console::stripe::StripeClient;
use serde_json::json;

async fn fixture(label: &str) -> TestDatabase {
    let fixture = TestDatabase::create(label).await.unwrap();
    let mut conn = fixture.db.conn().await.unwrap();
    conn.query_drop("INSERT INTO workspace (id,name) VALUES ('wrk_auth_usage','Auth usage')")
        .await
        .unwrap();
    conn.query_drop("INSERT INTO billing (id,workspace_id,balance,payment_method_id,customer_id,reload,reload_trigger,reload_amount) VALUES ('bil_auth_usage','wrk_auth_usage',100000000,'pm_fake','cus_fake',TRUE,5,20)").await.unwrap();
    conn.query_drop("INSERT INTO user (id,workspace_id,name,role) VALUES ('usr_auth_usage','wrk_auth_usage','Test','admin')").await.unwrap();
    conn.query_drop("INSERT INTO `key` (id,workspace_id,user_id,name,`key`) VALUES ('key_auth_usage','wrk_auth_usage','usr_auth_usage','Key','auth_usage')").await.unwrap();
    drop(conn);
    fixture
}

#[tokio::test]
async fn fixture_auth_matches_legacy_live_filters_and_disabled_model_exemptions() {
    let fixture = fixture("usage_auth").await;
    let service = UsageService::new(
        fixture.db.clone(),
        json!({"lite":{"rollingLimit":1,"rollingWindow":5,"weeklyLimit":2,"monthlyLimit":3}}),
    );
    let model = json!({"cost":{"input":0.000001,"output":0}});
    let mut context = service
        .authenticate(Some("auth_usage"), "model", &model, false)
        .await
        .unwrap();
    assert!(context.region.is_null());
    service
        .default_region(&mut context, Some("cn"))
        .await
        .unwrap();
    assert_eq!(context.region, json!(["us", "eu", "sg", "cn"]));
    let mut reloaded = service
        .authenticate(Some("auth_usage"), "model", &model, false)
        .await
        .unwrap();
    service
        .default_region(&mut reloaded, Some("US"))
        .await
        .unwrap();
    assert_eq!(reloaded.region, context.region);
    assert_eq!(
        service
            .authenticate(Some("wrong"), "model", &model, false)
            .await
            .unwrap_err()
            .message,
        "Invalid API key."
    );
    let mut conn = fixture.db.conn().await.unwrap();
    conn.query_drop("INSERT INTO model (id,workspace_id,model) VALUES ('mod_auth_usage','wrk_auth_usage','model')").await.unwrap();
    conn.query_drop("UPDATE model SET time_deleted=NOW(3) WHERE workspace_id='wrk_auth_usage'")
        .await
        .unwrap();
    drop(conn);
    // The handler joins model settings without a deleted predicate, unlike the model list.
    assert_eq!(
        service
            .authenticate(Some("auth_usage"), "model", &model, false)
            .await
            .unwrap_err()
            .kind,
        "ModelError"
    );
    let mut conn = fixture.db.conn().await.unwrap();
    conn.query_drop("UPDATE billing SET lite=JSON_OBJECT('useBalance',false) WHERE workspace_id='wrk_auth_usage'").await.unwrap();
    conn.query_drop("INSERT INTO lite (id,workspace_id,user_id) VALUES ('lit_auth_usage','wrk_auth_usage','usr_auth_usage')").await.unwrap();
    drop(conn);
    assert_eq!(
        service
            .authenticate(Some("auth_usage"), "model", &model, true)
            .await
            .unwrap()
            .source,
        BillingSource::Lite
    );
    assert!(service.go_usage(Some("auth_usage")).await.is_ok());
    let mut conn = fixture.db.conn().await.unwrap();
    conn.query_drop("UPDATE workspace SET time_deleted=NOW(3) WHERE id='wrk_auth_usage'")
        .await
        .unwrap();
    drop(conn);
    assert!(
        service
            .authenticate(Some("auth_usage"), "other-model", &model, false)
            .await
            .is_ok()
    );
    assert_eq!(
        service
            .go_usage(Some("auth_usage"))
            .await
            .unwrap_err()
            .message,
        "Unauthorized"
    );
    let mut conn = fixture.db.conn().await.unwrap();
    conn.query_drop("UPDATE workspace SET time_deleted=NULL,is_flagged_by_anthropic=TRUE WHERE id='wrk_auth_usage'").await.unwrap();
    drop(conn);
    assert_eq!(
        service
            .authenticate(Some("auth_usage"), "claude-test", &model, false)
            .await
            .unwrap_err()
            .kind,
        "AuthError"
    );
    assert!(
        service
            .authenticate(Some("auth_usage"), "gpt-test", &model, false)
            .await
            .is_ok()
    );
    let production = service.clone().with_production(true);
    assert_eq!(
        production
            .authenticate(Some("auth_usage"), "alpha-test", &model, false)
            .await
            .unwrap_err()
            .kind,
        "AuthError"
    );
    let mut conn = fixture.db.conn().await.unwrap();
    conn.query_drop("UPDATE `key` SET time_deleted=NOW(3) WHERE workspace_id='wrk_auth_usage'")
        .await
        .unwrap();
    drop(conn);
    assert_eq!(
        service
            .authenticate(Some("auth_usage"), "other", &model, false)
            .await
            .unwrap_err()
            .kind,
        "AuthError"
    );
    fixture.drop_database().await.unwrap();
}

#[tokio::test]
async fn fixture_identity_allows_region_producer_before_credit_rejection() {
    let fixture = fixture("usage_identity_order").await;
    let mut conn = fixture.db.conn().await.unwrap();
    conn.query_drop(
        "UPDATE billing SET balance=0,payment_method_id=NULL WHERE workspace_id='wrk_auth_usage'",
    )
    .await
    .unwrap();
    drop(conn);
    let service = UsageService::new(fixture.db.clone(), json!({}));
    let model = json!({"cost":{"input":0.000001,"output":0}});
    let mut context = service
        .authenticate_identity(Some("auth_usage"), "model", &model, false)
        .await
        .unwrap();
    service.default_region(&mut context, None).await.unwrap();
    assert_eq!(context.region, json!(["us", "eu", "sg"]));
    assert_eq!(
        service
            .validate_context(&mut context, &model, false)
            .unwrap_err()
            .kind,
        "CreditsError"
    );
    let reloaded = service
        .authenticate_identity(Some("auth_usage"), "model", &model, false)
        .await
        .unwrap();
    assert_eq!(context.region, reloaded.region);
    fixture.drop_database().await.unwrap();
}

#[tokio::test]
async fn fixture_fractional_cost_response_preserves_signed_to_fixed_rounding() {
    let fixture = fixture("usage_fractional").await;
    let service = UsageService::new(fixture.db.clone(), json!({}));
    let model = json!({"cost":{"input":0.000000005,"output":0}});
    let context = service
        .authenticate(Some("auth_usage"), "model", &model, false)
        .await
        .unwrap();
    let record = service
        .record_with_id(
            &context,
            "s",
            "model",
            &model,
            "p",
            &UsageInfo {
                input_tokens: -1,
                ..Default::default()
            },
            "usg_negative_half",
        )
        .await
        .unwrap();
    assert_eq!(record.cost_in_microcents, 0);
    assert_eq!(record.occurred_cost, "-0.00000001");
    let retry = service
        .record_with_id(
            &context,
            "s",
            "model",
            &model,
            "p",
            &UsageInfo {
                input_tokens: -1,
                ..Default::default()
            },
            "usg_negative_half",
        )
        .await
        .unwrap();
    assert_eq!(record, retry);
    fixture.drop_database().await.unwrap();
}

#[tokio::test]
async fn fixture_successful_reload_is_locked_and_does_not_credit_without_webhook() {
    use axum::{Json, Router, extract::Request, routing::post};
    use std::sync::{Arc, Mutex};
    let fixture = fixture("usage_reload_success").await;
    let requests = Arc::new(Mutex::new(Vec::<String>::new()));
    let observed = requests.clone();
    let app = Router::new().route(
        "/v1/{*path}",
        post(move |request: Request| {
            let observed = observed.clone();
            async move {
                observed
                    .lock()
                    .unwrap()
                    .push(request.uri().path().to_string());
                Json(json!({"id":"in_usage_local"}))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let service = UsageService::new(fixture.db.clone(), json!({}))
        .with_stripe(StripeClient::new("fake", &format!("http://{address}")).unwrap());
    let model = json!({"cost":{"input":0.000001,"output":0}});
    let context = service
        .authenticate(Some("auth_usage"), "model", &model, false)
        .await
        .unwrap();
    let usage = UsageInfo {
        input_tokens: 1,
        ..Default::default()
    };
    assert!(
        service
            .record_with_id(&context, "s", "model", &model, "p", &usage, "usg_success_1")
            .await
            .unwrap()
            .reload_claimed
    );
    assert!(
        !service
            .record_with_id(&context, "s", "model", &model, "p", &usage, "usg_success_2")
            .await
            .unwrap()
            .reload_claimed
    );
    assert_eq!(
        *requests.lock().unwrap(),
        vec![
            "/v1/invoices",
            "/v1/invoiceitems",
            "/v1/invoiceitems",
            "/v1/invoices/in_usage_local/finalize",
            "/v1/invoices/in_usage_local/pay"
        ]
    );
    let mut conn = fixture.db.conn().await.unwrap();
    let row: Option<(i64, bool, Option<String>)> = conn
        .query_first(
            "SELECT balance,reload,reload_error FROM billing WHERE workspace_id='wrk_auth_usage'",
        )
        .await
        .unwrap();
    assert_eq!(row, Some((99999800, true, None)));
    drop(conn);
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
    fixture.drop_database().await.unwrap();
}

#[tokio::test]
async fn fixture_reload_lock_serializes_effects_and_failure_disables_reload() {
    use axum::{Router, routing::post};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let fixture = fixture("usage_reload").await;
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let app = Router::new().route(
        "/v1/invoices",
        post(move || {
            let observed = observed.clone();
            async move {
                observed.fetch_add(1, Ordering::SeqCst);
                http::StatusCode::PAYMENT_REQUIRED
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let service = UsageService::new(fixture.db.clone(), json!({}))
        .with_stripe(StripeClient::new("fake", &format!("http://{address}")).unwrap());
    let model = json!({"cost":{"input":0.000001,"output":0}});
    let auth = service
        .authenticate(Some("auth_usage"), "model", &model, false)
        .await
        .unwrap();
    let usage = UsageInfo {
        input_tokens: 1,
        ..Default::default()
    };
    let (first, second) = tokio::join!(
        service.record_with_id(&auth, "s", "model", &model, "p", &usage, "usg_reload_one"),
        service.record_with_id(&auth, "s", "model", &model, "p", &usage, "usg_reload_two")
    );
    let first = first.unwrap();
    let second = second.unwrap();
    assert_ne!(first.reload_claimed, second.reload_claimed);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let replay = service
        .record_with_id(&auth, "s", "model", &model, "p", &usage, "usg_reload_one")
        .await
        .unwrap();
    assert!(!replay.reload_claimed);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let mut conn = fixture.db.conn().await.unwrap();
    let row: Option<(bool, String, i64)> = conn
        .query_first(
            "SELECT reload,reload_error,balance FROM billing WHERE workspace_id='wrk_auth_usage'",
        )
        .await
        .unwrap();
    let row = row.unwrap();
    assert!(!row.0);
    assert!(row.1.contains("402"));
    assert_eq!(row.2, 99999800);
    drop(conn);
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
    fixture.drop_database().await.unwrap();
}
