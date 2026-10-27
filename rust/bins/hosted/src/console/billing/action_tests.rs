//! Purpose: validate authenticated billing actions and subscription queries against local MySQL.
//! Scope: sealed session admission, settings, redemption, Lite quota and model breakdown projections.
//! Uses:
//! rust/bins/
//! |-- hosted/src/console/billing.rs
//! `-- hosted/src/console/zen.rs
//! rust/crates/
//! |-- hosted-adapters/src/console/auth.rs
//! |-- hosted-adapters/src/console/db.rs
//! `-- hosted-adapters/src/console/tenant.rs
//! External:
//! |-- axum::{body, http}
//! |-- mysql_async::prelude::Queryable
//! `-- tower::ServiceExt
//! Used by:
//! `-- none, entrypoint
//! Invariants: fixture cookies only; no external provider calls; tenant mismatch and cross-origin requests fail.
//! Contract tests: this file.

use super::super::state::ConsoleConfig;
use super::*;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request};
use mysql_async::prelude::Queryable;
use opencode_hosted_adapters::console::auth::SessionCodec;
use opencode_hosted_adapters::console::db::TestDatabase;
use opencode_hosted_adapters::console::tenant::TenantStore;
use opencode_hosted_domain::console::common::Actor;
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

const SECRET: &str = "billing-actions-fixture-secret-at-least-32-bytes";

async fn request(
    app: &Router,
    cookie: Option<&str>,
    method: Method,
    path: &str,
    input: Value,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if let Some(cookie) = cookie {
        request = request.header("cookie", cookie);
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::from(input.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let value = serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
        .unwrap_or(Value::Null);
    (status, value)
}

#[tokio::test]
async fn fixture_authenticated_billing_settings_and_subscription_queries() {
    let fixture = TestDatabase::create("billing_actions").await.unwrap();
    let tenant = TenantStore::new(fixture.db.clone());
    let account = tenant.create_account(None).await.unwrap();
    let workspace = tenant
        .create_workspace(
            &Actor::Account {
                account_id: account.clone(),
                email: "fixture@example.test".into(),
            },
            "Billing actions",
        )
        .await
        .unwrap();
    let mut conn = fixture.db.conn().await.unwrap();
    let user: String = conn
        .exec_first(
            "SELECT id FROM user WHERE workspace_id=? AND account_id=?",
            (&workspace, &account),
        )
        .await
        .unwrap()
        .unwrap();
    conn.exec_drop("INSERT INTO auth (id,account_id,provider,subject) VALUES ('aut_billing_fixture',?,'email','fixture@example.test')",(&account,)).await.unwrap();
    conn.exec_drop(
        "INSERT INTO coupon(email,type) VALUES ('fixture@example.test','BUILDATHON')",
        (),
    )
    .await
    .unwrap();
    conn.exec_drop("UPDATE billing SET subscription_id='sub_legacy_access',lite_subscription_id='sub_actions',lite=JSON_OBJECT() WHERE workspace_id=?",(&workspace,)).await.unwrap();
    conn.exec_drop("INSERT INTO lite(id,workspace_id,user_id,rolling_usage,weekly_usage,monthly_usage,time_rolling_updated,time_weekly_updated,time_monthly_updated) VALUES ('lit_actions',?,?,250000000,250000000,250000000,NOW(3),NOW(3),NOW(3))",(&workspace,&user)).await.unwrap();
    let key: String = conn
        .exec_first(
            "SELECT id FROM `key` WHERE workspace_id=? AND user_id=? LIMIT 1",
            (&workspace, &user),
        )
        .await
        .unwrap()
        .unwrap();
    conn.exec_drop("INSERT INTO `usage` (id,workspace_id,model,provider,input_tokens,output_tokens,cost,key_id,enrichment) VALUES ('usg_actions',?,'fixture-model','fixture',10,5,125000000,?,JSON_OBJECT('plan','lite','costMultiplier',2))",(&workspace,&key)).await.unwrap();
    drop(conn);
    let codec = SessionCodec::new(SECRET);
    let now = (time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as i64;
    let mut session = codec.load(None, now);
    session.data.login(&account, "fixture@example.test");
    let cookie = codec.set_cookie(&session, now).unwrap();
    let cookie = cookie.split(';').next().unwrap();
    let config=ConsoleConfig {auth:super::super::auth::Config {session_secret:Some(SECRET.into()),console_url:Some("https://console.example.test".into()),public_origin:Some("https://console.example.test".into()),..Default::default()},zen:super::super::zen::Config::from_catalog(json!({"providers":{},"zenModels":{},"liteModels":{"fixture-model":{"name":"Fixture Model","costMultiplier":2}}})).unwrap().with_billing(json!({"lite":{"rollingLimit":5,"rollingWindow":5,"weeklyLimit":10,"monthlyLimit":20},"black":{"20":{"rollingLimit":5,"rollingWindow":5,"fixedLimit":20}}}),None),..Default::default()};
    let state = ConsoleState {
        config: Arc::new(config),
        db: fixture.db.clone(),
    };
    let app = routes().with_state(state);
    let root = format!("/workspaces/{workspace}/billing");
    assert!(
        !request(&app, None, Method::GET, &root, Value::Null)
            .await
            .0
            .is_success()
    );
    assert_eq!(
        request(&app, Some(cookie), Method::GET, &root, Value::Null)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        request(
            &app,
            Some(cookie),
            Method::POST,
            &format!("{root}/monthly-limit"),
            json!({"limit":0})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        request(
            &app,
            Some(cookie),
            Method::POST,
            &format!("{root}/monthly-limit"),
            json!({"limit":-1})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(
            &app,
            Some(cookie),
            Method::PATCH,
            &format!("{root}/reload"),
            json!({"reload":true,"reloadAmount":10,"reloadTrigger":5})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        request(
            &app,
            Some(cookie),
            Method::PATCH,
            &format!("{root}/reload"),
            json!({"reload":true,"reloadAmount":9,"reloadTrigger":5})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let redeemed = request(
        &app,
        Some(cookie),
        Method::POST,
        &format!("{root}/redeem"),
        json!({"code":" buildathon "}),
    )
    .await;
    assert_eq!(redeemed.1, json!({"data":true}));
    let again = request(
        &app,
        Some(cookie),
        Method::POST,
        &format!("{root}/redeem"),
        json!({"code":"BUILDATHON"}),
    )
    .await;
    assert_eq!(again.1, json!({"error":"Coupon already redeemed"}));
    assert_eq!(
        request(
            &app,
            Some(cookie),
            Method::PATCH,
            &format!("{root}/lite/use-balance"),
            json!({"useBalance":true})
        )
        .await
        .0,
        StatusCode::OK
    );
    let lite = request(
        &app,
        Some(cookie),
        Method::GET,
        &format!("{root}/lite"),
        Value::Null,
    )
    .await;
    assert_eq!(lite.0, StatusCode::OK);
    assert_eq!(lite.1["mine"], true);
    assert_eq!(lite.1["useBalance"], true);
    assert_eq!(lite.1["rollingUsage"]["usage"], 250_000_000);
    assert_eq!(lite.1["rollingUsage"]["usagePercent"], 50.0);
    let details = request(
        &app,
        Some(cookie),
        Method::GET,
        &format!("{root}/lite/usage?window=weekly"),
        Value::Null,
    )
    .await;
    assert_eq!(details.0, StatusCode::OK);
    assert_eq!(details.1["rows"][0]["name"], "Fixture Model");
    assert_eq!(details.1["rows"][0]["quotaCost"], 250_000_000.0);
    assert_eq!(details.1["rows"][0]["contributionPercent"], 25.0);
    assert!(
        request(
            &app,
            Some(cookie),
            Method::GET,
            "/workspaces/wrk_not_mine/billing",
            Value::Null
        )
        .await
        .0 != StatusCode::OK
    );
    let cross = app
        .oneshot(
            Request::post(format!("{root}/monthly-limit"))
                .header("cookie", cookie)
                .header("origin", "https://evil.example.test")
                .body(Body::from("{\"limit\":1}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cross.status(), StatusCode::FORBIDDEN);
    let summary = BillingStore::new(fixture.db.clone())
        .summary(&workspace)
        .await
        .unwrap();
    assert_eq!(summary.balance, 50_000_000_000);
    assert_eq!(summary.monthly_limit, Some(0));
    drop(tenant);
    fixture.drop_database().await.unwrap();
}
