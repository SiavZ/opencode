//! Purpose: verify real inference billing against local baseline-migrated MySQL.
//! Scope: pricing, source precedence, tenant boundaries, durable retries and quota producers.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/db.rs
//! |-- hosted-adapters/src/console/formats.rs
//! `-- hosted-adapters/src/console/gateway/usage.rs
//! External:
//! |-- mysql_async::prelude::Queryable
//! `-- serde_json::{Value, json}
//! Used by:
//! `-- none, entrypoint
//! Invariants: all persistence uses the public implementation; fixtures never call live providers or payments.
//! Contract tests: this file.

use mysql_async::prelude::Queryable;
use opencode_hosted_adapters::console::db::TestDatabase;
use opencode_hosted_adapters::console::formats::UsageInfo;
use opencode_hosted_adapters::console::gateway::usage::{BillingSource, UsageService};
use serde_json::{Value, json};

fn limits() -> Value {
    json!({"lite":{"rollingLimit":1,"rollingWindow":5,"weeklyLimit":2,"monthlyLimit":3},"black":{"20":{"fixedLimit":2,"rollingLimit":1,"rollingWindow":5}}})
}

fn model() -> Value {
    json!({"cost":{"input":0.000001,"output":0.000002},"costMultiplier":1.5})
}

async fn fixture(label: &str) -> TestDatabase {
    let fixture = TestDatabase::create(label).await.unwrap();
    let mut conn = fixture.db.conn().await.unwrap();
    conn.query_drop(
        "INSERT INTO workspace (id,name) VALUES ('wrk_usage_a','A'),('wrk_usage_b','B')",
    )
    .await
    .unwrap();
    conn.query_drop("INSERT INTO billing (id,workspace_id,balance,payment_method_id) VALUES ('bil_usage_a','wrk_usage_a',1000000000,'pm_a'),('bil_usage_b','wrk_usage_b',1000000000,'pm_b')").await.unwrap();
    conn.query_drop("INSERT INTO user (id,workspace_id,name,role) VALUES ('usr_usage','wrk_usage_a','A','admin'),('usr_usage','wrk_usage_b','B','admin')").await.unwrap();
    conn.query_drop("INSERT INTO `key` (id,workspace_id,user_id,name,`key`) VALUES ('key_usage_a','wrk_usage_a','usr_usage','A','secret_a'),('key_usage_b','wrk_usage_b','usr_usage','B','secret_b')").await.unwrap();
    drop(conn);
    fixture
}

#[tokio::test]
async fn fixture_durable_usage_retries_conflicts_and_tenant_isolation() {
    let fixture = fixture("usage_retry").await;
    let service = UsageService::new(fixture.db.clone(), limits());
    let model = model();
    let a = service
        .authenticate(Some("secret_a"), "model", &model, false)
        .await
        .unwrap();
    let b = service
        .authenticate(Some("secret_b"), "model", &model, false)
        .await
        .unwrap();
    assert_eq!(a.source, BillingSource::Balance);
    assert_eq!(a.subscription_plan(), None);
    let usage = UsageInfo {
        input_tokens: 3,
        output_tokens: 2,
        reasoning_tokens: Some(1),
        cache_read_tokens: Some(4),
        ..Default::default()
    };
    let first = service
        .record_with_id(
            &a,
            "a-session-that-is-longer-than-thirty-characters",
            "model",
            &model,
            "provider",
            &usage,
            "usg_retry",
        )
        .await
        .unwrap();
    assert_eq!(first.cost_in_microcents, 700);
    assert_eq!(first.occurred_cost, "0.00000700");
    let replay = service
        .record_with_id(
            &a,
            "a-session-that-is-longer-than-thirty-characters",
            "model",
            &model,
            "provider",
            &usage,
            "usg_retry",
        )
        .await
        .unwrap();
    assert_eq!(first, replay);
    let error = service
        .record_with_id(
            &a,
            "different",
            "model",
            &model,
            "provider",
            &usage,
            "usg_retry",
        )
        .await
        .unwrap_err();
    assert_eq!(error.status, 409);
    // A durable identifier is scoped to its workspace, not globally.
    service
        .record_with_id(&b, "b", "model", &model, "provider", &usage, "usg_retry")
        .await
        .unwrap();
    let mut conn = fixture.db.conn().await.unwrap();
    let rows: Vec<(String, i64, i64)> = conn
        .query("SELECT workspace_id,balance,monthly_usage FROM billing ORDER BY workspace_id")
        .await
        .unwrap();
    assert_eq!(
        rows,
        vec![
            ("wrk_usage_a".into(), 999999300, 700),
            ("wrk_usage_b".into(), 999999300, 700)
        ]
    );
    let count: Option<u64> = conn
        .query_first("SELECT COUNT(*) FROM `usage`")
        .await
        .unwrap();
    assert_eq!(count, Some(2));
    let session: Option<String> = conn
        .query_first("SELECT session_id FROM `usage` WHERE workspace_id='wrk_usage_a'")
        .await
        .unwrap();
    assert_eq!(session.unwrap().len(), 30);
    let users: Vec<i64> = conn
        .query("SELECT monthly_usage FROM user ORDER BY workspace_id")
        .await
        .unwrap();
    assert_eq!(users, vec![700, 700]);
    drop(conn);
    fixture.drop_database().await.unwrap();
}

#[tokio::test]
async fn fixture_anonymous_free_and_byok_are_real_non_debiting_sources() {
    let fixture = fixture("usage_sources").await;
    let service = UsageService::new(fixture.db.clone(), limits());
    let mut model = model();
    assert_eq!(
        service
            .authenticate(None, "model", &model, false)
            .await
            .unwrap_err()
            .kind,
        "AuthError"
    );
    model["allowAnonymous"] = json!(true);
    let anonymous = service
        .authenticate(None, "model", &model, false)
        .await
        .unwrap();
    assert_eq!(anonymous.source, BillingSource::Anonymous);
    service
        .record(
            &anonymous,
            "s",
            "model",
            &model,
            "p",
            &UsageInfo {
                input_tokens: 1,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let free = service
        .authenticate(Some("secret_a"), "model", &model, false)
        .await
        .unwrap();
    assert_eq!(free.source, BillingSource::Free);
    service
        .record_with_id(
            &free,
            "s",
            "model",
            &model,
            "p",
            &UsageInfo {
                input_tokens: 1,
                ..Default::default()
            },
            "usg_free",
        )
        .await
        .unwrap();
    let mut conn = fixture.db.conn().await.unwrap();
    conn.query_drop("INSERT INTO provider (id,workspace_id,provider,credentials) VALUES ('prv_usage','wrk_usage_a','anthropic','byok-secret')").await.unwrap();
    drop(conn);
    model["byokProvider"] = json!("anthropic");
    let byok = service
        .authenticate(Some("secret_a"), "model", &model, false)
        .await
        .unwrap();
    assert_eq!(byok.source, BillingSource::Byok);
    assert_eq!(byok.provider_credentials.as_deref(), Some("byok-secret"));
    service
        .record_with_id(
            &byok,
            "s",
            "model",
            &model,
            "p",
            &UsageInfo {
                input_tokens: 2,
                ..Default::default()
            },
            "usg_byok",
        )
        .await
        .unwrap();
    let mut conn = fixture.db.conn().await.unwrap();
    let row: Option<(i64, i64)> = conn
        .query_first("SELECT balance,monthly_usage FROM billing WHERE workspace_id='wrk_usage_a'")
        .await
        .unwrap();
    assert_eq!(row, Some((1000000000, 300)));
    let count: Option<u64> = conn
        .query_first("SELECT COUNT(*) FROM `usage`")
        .await
        .unwrap();
    assert_eq!(count, Some(2));
    drop(conn);
    fixture.drop_database().await.unwrap();
}

#[tokio::test]
async fn fixture_lite_producer_multiplier_quota_fallback_and_go_readback() {
    let fixture = fixture("usage_lite").await;
    let mut conn = fixture.db.conn().await.unwrap();
    conn.query_drop(
        "UPDATE billing SET lite=JSON_OBJECT('useBalance',false) WHERE workspace_id='wrk_usage_a'",
    )
    .await
    .unwrap();
    conn.query_drop(
        "INSERT INTO lite (id,workspace_id,user_id) VALUES ('lit_usage','wrk_usage_a','usr_usage')",
    )
    .await
    .unwrap();
    drop(conn);
    let service = UsageService::new(fixture.db.clone(), limits());
    let model = model();
    let context = service
        .authenticate(Some("secret_a"), "model", &model, true)
        .await
        .unwrap();
    assert_eq!(context.source, BillingSource::Lite);
    assert_eq!(context.subscription_plan(), Some("lite"));
    let usage = UsageInfo {
        input_tokens: 1,
        ..Default::default()
    };
    service
        .record_with_id(&context, "s", "model", &model, "p", &usage, "usg_lite")
        .await
        .unwrap();
    let mut conn = fixture.db.conn().await.unwrap();
    let counters: Option<(i64,i64,i64)> = conn.query_first("SELECT rolling_usage,weekly_usage,monthly_usage FROM lite WHERE workspace_id='wrk_usage_a'").await.unwrap();
    assert_eq!(counters, Some((150, 150, 150)));
    let balance: Option<i64> = conn
        .query_first("SELECT balance FROM billing WHERE workspace_id='wrk_usage_a'")
        .await
        .unwrap();
    assert_eq!(balance, Some(1000000000));
    conn.query_drop("UPDATE lite SET weekly_usage=200000000,time_weekly_updated=NOW(3) WHERE workspace_id='wrk_usage_a'").await.unwrap();
    drop(conn);
    let error = service
        .authenticate(Some("secret_a"), "model", &model, true)
        .await
        .unwrap_err();
    assert_eq!(error.status, 429);
    assert_eq!(error.kind, "GoUsageLimitError");
    assert_eq!(
        error.response_json()["metadata"],
        json!({"workspace":"wrk_usage_a","limitName":"weekly"})
    );
    assert!(error.response_json().get("diagnostic").is_none());
    assert_eq!(
        *error.metadata,
        json!({"workspace":"wrk_usage_a","limitName":"weekly"})
    );
    assert!(error.retry_after.unwrap() > 0);
    let go = service.go_usage(Some("secret_a")).await.unwrap();
    assert_eq!(go["usage"]["weekly"]["status"], "rate-limited");
    assert_eq!(go["usage"]["weekly"]["percent"], 100);
    assert_eq!(
        service.go_usage(Some("secret_b")).await.unwrap_err().status,
        403
    );
    let mut conn = fixture.db.conn().await.unwrap();
    conn.query_drop(
        "UPDATE billing SET lite=JSON_OBJECT('useBalance',true) WHERE workspace_id='wrk_usage_a'",
    )
    .await
    .unwrap();
    drop(conn);
    assert_eq!(
        service
            .authenticate(Some("secret_a"), "model", &model, true)
            .await
            .unwrap()
            .source,
        BillingSource::Balance
    );
    let mut free_model = model.clone();
    free_model["cost"] = json!({"input":0,"output":0});
    assert_eq!(
        service
            .authenticate(Some("secret_a"), "model", &free_model, true)
            .await
            .unwrap()
            .source,
        BillingSource::Lite
    );
    fixture.drop_database().await.unwrap();
}

#[tokio::test]
async fn fixture_subscription_producer_and_monthly_credit_boundaries() {
    let fixture = fixture("usage_black").await;
    let mut conn = fixture.db.conn().await.unwrap();
    conn.query_drop("UPDATE billing SET subscription=JSON_OBJECT('plan','20','useBalance',false) WHERE workspace_id='wrk_usage_a'").await.unwrap();
    conn.query_drop("INSERT INTO subscription (id,workspace_id,user_id) VALUES ('sub_usage','wrk_usage_a','usr_usage')").await.unwrap();
    drop(conn);
    let service = UsageService::new(fixture.db.clone(), limits());
    let model = model();
    let context = service
        .authenticate(Some("secret_a"), "model", &model, false)
        .await
        .unwrap();
    assert_eq!(context.source, BillingSource::Subscription);
    assert_eq!(context.subscription_plan(), Some("20"));
    service
        .record_with_id(
            &context,
            "s",
            "model",
            &model,
            "p",
            &UsageInfo {
                input_tokens: 1,
                ..Default::default()
            },
            "usg_sub",
        )
        .await
        .unwrap();
    let mut conn = fixture.db.conn().await.unwrap();
    let counters: Option<(i64, i64)> = conn
        .query_first(
            "SELECT rolling_usage,fixed_usage FROM subscription WHERE workspace_id='wrk_usage_a'",
        )
        .await
        .unwrap();
    assert_eq!(counters, Some((100, 100)));
    conn.query_drop("UPDATE subscription SET rolling_usage=100000000,time_rolling_updated=NOW(3) WHERE workspace_id='wrk_usage_a'").await.unwrap();
    drop(conn);
    assert_eq!(
        service
            .authenticate(Some("secret_a"), "model", &model, false)
            .await
            .unwrap_err()
            .kind,
        "BlackUsageLimitError"
    );
    let mut conn = fixture.db.conn().await.unwrap();
    conn.query_drop("UPDATE billing SET subscription=NULL,monthly_limit=1,monthly_usage=99999999,time_monthly_usage_updated=NOW(3) WHERE workspace_id='wrk_usage_a'").await.unwrap();
    drop(conn);
    assert_eq!(
        service
            .authenticate(Some("secret_a"), "model", &model, false)
            .await
            .unwrap()
            .source,
        BillingSource::Balance
    );
    let mut conn = fixture.db.conn().await.unwrap();
    conn.query_drop("UPDATE billing SET monthly_usage=100000000 WHERE workspace_id='wrk_usage_a'")
        .await
        .unwrap();
    drop(conn);
    assert_eq!(
        service
            .authenticate(Some("secret_a"), "model", &model, false)
            .await
            .unwrap_err()
            .kind,
        "MonthlyLimitError"
    );
    let mut conn = fixture.db.conn().await.unwrap();
    conn.query_drop("UPDATE billing SET time_monthly_usage_updated=TIMESTAMPADD(YEAR,-1,NOW(3)) WHERE workspace_id='wrk_usage_a'").await.unwrap();
    conn.query_drop("UPDATE user SET monthly_limit=1,monthly_usage=100000000,time_monthly_usage_updated=NOW(3) WHERE workspace_id='wrk_usage_a'").await.unwrap();
    drop(conn);
    assert_eq!(
        service
            .authenticate(Some("secret_a"), "model", &model, false)
            .await
            .unwrap_err()
            .kind,
        "UserLimitError"
    );
    assert_eq!(
        service
            .authenticate(Some("secret_b"), "model", &model, false)
            .await
            .unwrap()
            .source,
        BillingSource::Balance
    );
    fixture.drop_database().await.unwrap();
}
