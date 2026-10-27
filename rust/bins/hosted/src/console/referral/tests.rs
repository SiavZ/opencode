//! Purpose: accept referral HTTP attribution, summary, preview and reward application lifecycle.
//! Scope: real migrated MySQL, sealed auth sessions and Axum routes.
//! Uses:
//! rust/bins/
//! `-- hosted/src/console/referral.rs
//! rust/crates/
//! |-- hosted-adapters/src/console/db.rs
//! `-- hosted-adapters/src/console/tenant.rs
//! External:
//! |-- mysql_async::prelude::Queryable
//! `-- tower::ServiceExt
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/referral.rs
//! Invariants: disposable local database only; no Stripe, email or production side effects.
//! Contract tests: this file

use super::*;
use crate::console::state::ConsoleConfig;
use axum::body::{Body, to_bytes};
use axum::http::Request;
use mysql_async::prelude::Queryable;
use opencode_hosted_adapters::console::auth::SessionCodec;
use opencode_hosted_adapters::console::db::TestDatabase;
use opencode_hosted_adapters::console::tenant::TenantStore;
use opencode_hosted_domain::console::identity::AuthProvider;
use std::sync::Arc;
use tower::ServiceExt;

const ORIGIN: &str = "https://console.opencode.ai";
const SECRET: &str = "referral-http-fixture-session-secret-at-least-32-bytes";

fn session(account: &str) -> String {
    let codec = SessionCodec::new(SECRET);
    let now = (time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as i64;
    let mut loaded = codec.load(None, now);
    loaded.data.login(account, "fixture@example.test");
    codec
        .set_cookie(&loaded, now)
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string()
}

struct Fixture {
    db: TestDatabase,
    router: Router,
    inviter: String,
    invitee: String,
    account: String,
    member: String,
    outsider: String,
}

impl Fixture {
    async fn new(label: &str) -> Self {
        let db = TestDatabase::create(label).await.unwrap();
        let store = TenantStore::new(db.db.clone());
        let owner = store.create_account(None).await.unwrap();
        store
            .upsert_auth_links(
                &owner,
                AuthProvider::Github,
                "referral-owner",
                "inviter@example.test",
            )
            .await
            .unwrap();
        let account = store.create_account(None).await.unwrap();
        store
            .upsert_auth_links(
                &account,
                AuthProvider::Github,
                "referral-invitee",
                "invitee@example.test",
            )
            .await
            .unwrap();
        let inviter = store
            .create_workspace(
                &Actor::Account {
                    account_id: owner,
                    email: "inviter@example.test".into(),
                },
                "Inviter",
            )
            .await
            .unwrap();
        let invitee = store
            .create_workspace(
                &Actor::Account {
                    account_id: account.clone(),
                    email: "invitee@example.test".into(),
                },
                "Invitee",
            )
            .await
            .unwrap();
        let member = store.create_account(None).await.unwrap();
        let outsider = store.create_account(None).await.unwrap();
        let mut conn = db.db.conn().await.unwrap();
        conn.exec_drop("UPDATE billing SET subscription_id=CASE WHEN workspace_id=? THEN 'sub_referral_a' ELSE 'sub_referral_b' END WHERE workspace_id IN (?,?)", (&inviter,&inviter,&invitee)).await.unwrap();
        conn.exec_drop(
            "INSERT INTO user(id,workspace_id,account_id,name,role) VALUES ('usr_referral_member',?,?,'','member')",
            (&invitee, &member),
        )
        .await
        .unwrap();
        conn.exec_drop(
            "INSERT INTO referral_code(workspace_id,code) VALUES (?,'INVITE1234')",
            (&inviter,),
        )
        .await
        .unwrap();
        drop(conn);
        let limits = json!({"lite":{"rollingLimit":10,"rollingWindow":5,"weeklyLimit":20,"monthlyLimit":30}});
        let config = ConsoleConfig {
            auth: auth::Config {
                session_secret: Some(SECRET.into()),
                public_origin: Some(ORIGIN.into()),
                console_url: Some(ORIGIN.into()),
                ..Default::default()
            },
            zen: crate::console::zen::Config::default().with_billing(limits, None),
            ..Default::default()
        };
        let router = routes().with_state(ConsoleState {
            config: Arc::new(config),
            db: db.db.clone(),
        });
        Self {
            db,
            router,
            inviter,
            invitee,
            account,
            member: session(&member),
            outsider: session(&outsider),
        }
    }

    async fn request(
        &self,
        method: &str,
        path: &str,
        cookie: Option<&str>,
        origin: Option<&str>,
        body: Value,
    ) -> (StatusCode, HeaderMap, Value) {
        let mut builder = Request::builder()
            .method(method)
            .uri(path)
            .header("host", "console.opencode.ai")
            .header("content-type", "application/json");
        if let Some(cookie) = cookie {
            builder = builder.header("cookie", cookie);
        }
        if let Some(origin) = origin {
            builder = builder.header("origin", origin);
        }
        let response = self
            .router
            .clone()
            .oneshot(builder.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (
            status,
            headers,
            serde_json::from_slice(&body).unwrap_or(Value::Null),
        )
    }
}

#[tokio::test]
async fn referral_summary_captures_cookie_and_clears_failed_attribution_without_admin_guard() {
    let fixture = Fixture::new("referral_http_cookie").await;
    let cookie = format!("{}; oc_referral=invite-1234", session(&fixture.account));
    let path = format!("/workspaces/{}/referral", fixture.invitee);
    let (status, headers, summary) = fixture
        .request("GET", &path, Some(&cookie), None, Value::Null)
        .await;
    assert_eq!(status, StatusCode::OK, "{summary}");
    assert_eq!(summary["rewardAmount"], 500);
    assert_eq!(summary["referralCode"].as_str().unwrap().len(), 10);
    assert_eq!(summary["hasReferral"], true);
    assert_eq!(summary["rewards"][0]["source"], "invitee");
    assert_eq!(summary["rewards"][0]["status"], "pending");
    assert!(
        headers
            .get_all(header::SET_COOKIE)
            .iter()
            .any(|value| value.to_str().unwrap() == CLEAR_COOKIE)
    );
    let mut conn = fixture.db.db.conn().await.unwrap();
    let count: Option<u64> = conn
        .exec_first(
            "SELECT COUNT(*) FROM referral WHERE workspace_id=? AND invitee_account_id=?",
            (&fixture.inviter, &fixture.account),
        )
        .await
        .unwrap();
    assert_eq!(count, Some(1));
    drop(conn);
    // Same cookie repeated fails attribution internally but must not fail summary or retain cookie.
    let (status, headers, _) = fixture
        .request("GET", &path, Some(&cookie), None, Value::Null)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers
            .get_all(header::SET_COOKIE)
            .iter()
            .any(|value| value.to_str().unwrap() == CLEAR_COOKIE)
    );
    assert_eq!(
        fixture
            .request("GET", &path, Some(&fixture.member), None, Value::Null)
            .await
            .0,
        StatusCode::OK
    );
    for cookie in [None, Some(fixture.outsider.as_str())] {
        assert!(
            !fixture
                .request("GET", &path, cookie, None, Value::Null)
                .await
                .0
                .is_success()
        );
    }
    let wrong = format!("/workspaces/{}/referral", fixture.inviter);
    assert!(
        !fixture
            .request("GET", &wrong, Some(&fixture.member), None, Value::Null)
            .await
            .0
            .is_success()
    );
    fixture.db.drop_database().await.unwrap();
}

#[tokio::test]
async fn referral_preview_and_member_apply_update_all_usage_counters_once() {
    let fixture = Fixture::new("referral_http_reward").await;
    let mut conn = fixture.db.db.conn().await.unwrap();
    conn.exec_drop(
        "INSERT INTO referral(id,workspace_id,invitee_account_id) VALUES ('ref_http_reward',?,?)",
        (&fixture.inviter, &fixture.account),
    )
    .await
    .unwrap();
    conn.exec_drop(
        "INSERT INTO referral_reward(workspace_id,referral_id,amount) VALUES (?,'ref_http_reward',500000000)",
        (&fixture.invitee,),
    )
    .await
    .unwrap();
    let user: String = conn
        .exec_first(
            "SELECT id FROM user WHERE workspace_id=? AND role='admin'",
            (&fixture.invitee,),
        )
        .await
        .unwrap()
        .unwrap();
    conn.exec_drop("INSERT INTO lite(id,workspace_id,user_id,rolling_usage,weekly_usage,monthly_usage,time_rolling_updated,time_weekly_updated,time_monthly_updated) VALUES ('lit_referral_http',?,?,900000000,1000000000,400000000,NOW(3),NOW(3),NOW(3))",(&fixture.invitee,&user)).await.unwrap();
    drop(conn);
    let preview = format!(
        "/workspaces/{}/referral/preview?referralID=ref_http_reward",
        fixture.invitee
    );
    let (status, _, before) = fixture
        .request("GET", &preview, Some(&fixture.member), None, Value::Null)
        .await;
    assert_eq!(status, StatusCode::OK, "{before}");
    assert!(
        before["rollingUsage"]["beforePercent"].as_i64().unwrap()
            > before["rollingUsage"]["afterPercent"].as_i64().unwrap()
    );
    let apply = format!("/workspaces/{}/referral/apply", fixture.invitee);
    let input = json!({"referralID":"ref_http_reward"});
    assert_eq!(
        fixture
            .request(
                "POST",
                &apply,
                Some(&fixture.member),
                Some("https://evil.example"),
                input.clone()
            )
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert!(
        !fixture
            .request(
                "POST",
                &apply,
                Some(&fixture.outsider),
                Some(ORIGIN),
                input.clone()
            )
            .await
            .0
            .is_success()
    );
    let (status, _, applied) = fixture
        .request(
            "POST",
            &apply,
            Some(&fixture.member),
            Some(ORIGIN),
            input.clone(),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{applied}");
    assert_eq!(applied, json!({"amount":500}));
    assert_eq!(
        fixture
            .request("POST", &apply, Some(&fixture.member), Some(ORIGIN), input)
            .await
            .0,
        StatusCode::CONFLICT
    );
    let mut conn = fixture.db.db.conn().await.unwrap();
    let counters: Option<(i64, i64, i64)> = conn
        .exec_first(
            "SELECT rolling_usage,weekly_usage,monthly_usage FROM lite WHERE workspace_id=?",
            (&fixture.invitee,),
        )
        .await
        .unwrap();
    assert_eq!(counters, Some((400000000, 500000000, 0)));
    drop(conn);
    assert_eq!(
        fixture
            .request("GET", &preview, Some(&fixture.member), None, Value::Null)
            .await
            .2,
        Value::Null
    );
    let missing = format!("/workspaces/{}/referral/preview", fixture.invitee);
    assert_eq!(
        fixture
            .request("GET", &missing, None, None, Value::Null)
            .await
            .2,
        Value::Null
    );
    fixture.db.drop_database().await.unwrap();
}

#[tokio::test]
async fn referral_account_capture_rejects_cross_origin_and_preserves_baseline_failure_clear() {
    let fixture = Fixture::new("referral_http_capture").await;
    let cookie = session(&fixture.account);
    assert_eq!(
        fixture
            .request(
                "POST",
                "/referral/capture",
                Some(&cookie),
                Some("https://evil.example"),
                json!({"code":"INVITE1234"})
            )
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let (status, headers, _) = fixture
        .request(
            "POST",
            "/referral/capture",
            Some(&cookie),
            Some(ORIGIN),
            json!({"code":"INVITE1234"}),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(
        headers
            .get_all(header::SET_COOKIE)
            .iter()
            .any(|value| value.to_str().unwrap() == CLEAR_COOKIE)
    );
    let (status, headers, _) = fixture
        .request(
            "POST",
            "/referral/capture",
            Some(&cookie),
            Some(ORIGIN),
            json!({"code":"invalid"}),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(
        headers
            .get_all(header::SET_COOKIE)
            .iter()
            .any(|value| value.to_str().unwrap() == CLEAR_COOKIE)
    );
    fixture.db.drop_database().await.unwrap();
}
