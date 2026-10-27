//! Purpose: prove durable support quota resets, referral rewards and diagnostic lookup.
//! Scope: isolated local MySQL acceptance tests using baseline migrations.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/db.rs
//! |-- hosted-adapters/src/console/support.rs
//! |-- hosted-adapters/src/console/tenant.rs
//! `-- hosted-domain/src/console/quotas.rs
//! External:
//! |-- mysql_async::prelude::Queryable
//! `-- serde_json::json
//! Used by:
//! `-- none, entrypoint
//! Invariants: fixtures only; no production database or external service calls.
//! Contract tests: this file.

use mysql_async::prelude::Queryable;
use opencode_hosted_adapters::console::db::TestDatabase;
use opencode_hosted_adapters::console::support::{SupportStore, authorized, secure_equal};
use opencode_hosted_adapters::console::tenant::TenantStore;
use opencode_hosted_domain::console::common::Actor;
use opencode_hosted_domain::console::quotas::Plan;
use serde_json::json;

#[test]
fn bearer_is_exact_and_configuration_fails_closed() {
    assert!(authorized(
        Some("Bearer fixture-secret"),
        Some("fixture-secret")
    ));
    for header in [
        None,
        Some("fixture-secret"),
        Some("bearer fixture-secret"),
        Some("Bearer fixture-secreu"),
        Some("Bearer fixture-secret "),
        Some(""),
    ] {
        assert!(!authorized(header, Some("fixture-secret")));
    }
    assert!(!authorized(Some("Bearer "), Some("")));
    assert!(!authorized(Some("Bearer fixture-secret"), None));
    assert!(secure_equal("webhook-secret", "webhook-secret"));
    assert!(!secure_equal("webhook-secret-x", "webhook-secret"));
}

async fn workspace(store: &TenantStore, email: &str) -> (String, String, String) {
    let account = store.create_account(None).await.unwrap();
    store
        .upsert_auth_links(
            &account,
            opencode_hosted_domain::console::identity::AuthProvider::Github,
            &format!("gh-{email}"),
            email,
        )
        .await
        .unwrap();
    let workspace = store
        .create_workspace(
            &Actor::Account {
                account_id: account.clone(),
                email: email.into(),
            },
            "Support fixture",
        )
        .await
        .unwrap();
    let user = store
        .admission_member(&workspace, std::slice::from_ref(&account))
        .await
        .unwrap()
        .unwrap()
        .user_id;
    (workspace, account, user)
}

#[tokio::test]
async fn fixture_support_quota_referral_and_lookup_are_durable() {
    let fixture = TestDatabase::create("support").await.unwrap();
    let tenant = TenantStore::new(fixture.db.clone());
    let support = SupportStore::new(fixture.db.clone());
    let (inviter, _, _) = workspace(&tenant, "inviter@example.test").await;
    let (invitee, account, user) = workspace(&tenant, "invitee@example.test").await;
    let mut conn = fixture.db.conn().await.unwrap();
    conn.exec_drop("INSERT INTO lite (id,workspace_id,user_id,rolling_usage,weekly_usage,monthly_usage,time_rolling_updated,time_weekly_updated,time_monthly_updated) VALUES ('lit_support',?,?,123,456,789,'2026-10-26 04:00:00.123','2026-10-26 04:00:00.123','2026-10-26 04:00:00.123')",(&invitee,&user)).await.unwrap();
    conn.exec_drop("INSERT INTO lite (id,workspace_id,user_id,rolling_usage,time_deleted) VALUES ('lit_deleted',?,'usr_deleted',999,NOW())",(&invitee,)).await.unwrap();
    conn.exec_drop("INSERT INTO subscription (id,workspace_id,user_id,rolling_usage,fixed_usage,time_rolling_updated,time_fixed_updated) VALUES ('sub_support',?,?,321,654,'2026-10-26 04:00:00.123','2026-10-26 04:00:00.123')",(&invitee,&user)).await.unwrap();
    let result = support.reset_quota(&invitee, Plan::Lite).await.unwrap();
    assert_eq!(
        serde_json::to_value(result).unwrap(),
        json!({"plan":"lite","before":{"rollingUsage":123,"weeklyUsage":456,"monthlyUsage":789}})
    );
    let counters: (i64,i64,i64,String) = conn.exec_first("SELECT rolling_usage,weekly_usage,monthly_usage,DATE_FORMAT(time_rolling_updated,'%Y-%m-%d %H:%i:%s.%f') FROM lite WHERE id = 'lit_support'",()).await.unwrap().unwrap();
    assert_eq!(counters, (0, 0, 0, "2026-10-26 04:00:00.123000".into()));
    assert_eq!(
        conn.query_first::<i64, _>("SELECT rolling_usage FROM lite WHERE id = 'lit_deleted'")
            .await
            .unwrap(),
        Some(999)
    );
    let result = support
        .reset_quota(&invitee, Plan::Subscription)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(result).unwrap(),
        json!({"plan":"subscription","before":{"rollingUsage":321,"fixedUsage":654}})
    );
    assert_eq!(
        support
            .reset_quota("wrk_missing", Plan::Lite)
            .await
            .unwrap_err()
            .to_string(),
        "No lite usage counters found for workspace"
    );
    conn.exec_drop("UPDATE lite SET rolling_usage = 10, weekly_usage = 20, monthly_usage = 30 WHERE id = 'lit_support'",()).await.unwrap();
    let (first, second) = tokio::join!(
        support.reset_quota(&invitee, Plan::Lite),
        support.reset_quota(&invitee, Plan::Lite)
    );
    let mut before = [
        first.unwrap().before.rolling_usage,
        second.unwrap().before.rolling_usage,
    ];
    before.sort();
    assert_eq!(before, [0, 10]);
    let referral = support.create_referral(&inviter, &invitee).await.unwrap();
    assert_eq!(referral["createdReferral"], true);
    assert_eq!(referral["createdRewards"], 2);
    assert_eq!(referral["inviteeAccountID"], account);
    let again = support.create_referral(&inviter, &invitee).await.unwrap();
    assert_eq!(again["referralID"], referral["referralID"]);
    assert_eq!(again["createdReferral"], false);
    assert_eq!(again["createdRewards"], 0);
    assert_eq!(
        support
            .create_referral(&invitee, &invitee)
            .await
            .unwrap_err()
            .to_string(),
        "Self-referral workspace mismatch"
    );
    let (other, _, _) = workspace(&tenant, "other@example.test").await;
    assert!(
        support
            .create_referral(&other, &invitee)
            .await
            .unwrap_err()
            .to_string()
            .starts_with("Referral already belongs to")
    );
    let count: Option<(u64, i64)> = conn
        .query_first("SELECT COUNT(*),CAST(SUM(amount) AS SIGNED) FROM referral_reward")
        .await
        .unwrap();
    assert_eq!(count, Some((2, 1_000_000_000)));
    conn.exec_drop(
        "UPDATE referral_reward SET amount = 1 WHERE workspace_id = ?",
        (&inviter,),
    )
    .await
    .unwrap();
    assert!(
        support
            .create_referral(&inviter, &invitee)
            .await
            .unwrap_err()
            .to_string()
            .starts_with("Referral reward amount mismatch:")
    );
    conn.exec_drop(
        "UPDATE referral_reward SET amount = 500000000 WHERE workspace_id = ?",
        (&inviter,),
    )
    .await
    .unwrap();
    assert!(
        support
            .create_referral("wrk_notfound", &invitee)
            .await
            .unwrap_err()
            .to_string()
            .starts_with("Inviter workspace not found:")
    );
    assert!(
        support
            .create_referral(&inviter, &other)
            .await
            .unwrap_err()
            .to_string()
            .starts_with("Invitee Lite workspace owner not found:")
    );
    conn.exec_drop("INSERT INTO `usage` (id,workspace_id,model,provider,input_tokens,output_tokens,cost,enrichment) VALUES ('usg_support',?,'model','provider',1,2,150000000,JSON_OBJECT('plan','lite'))",(&invitee,)).await.unwrap();
    conn.exec_drop(
        "INSERT INTO model (id,workspace_id,model) VALUES ('mod_support',?,'disabled-model')",
        (&invitee,),
    )
    .await
    .unwrap();
    let limits = json!({"lite":{"rollingLimit":10,"rollingWindow":5,"weeklyLimit":50,"monthlyLimit":100},"black":{"20":{"fixedLimit":20,"rollingLimit":5,"rollingWindow":5}}});
    let result = support
        .lookup("invitee@example.test", &limits)
        .await
        .unwrap();
    assert_eq!(result["workspaces"][0]["workspaceID"], invitee);
    assert_eq!(result["workspaces"][0]["billing"]["balance"], "$0.00");
    assert_eq!(result["workspaces"][0]["usage"][0]["goCost"], "$1.50");
    assert_eq!(result["workspaces"][0]["usage"][1]["date"], "TOTAL");
    assert_eq!(result["workspaces"][0]["usage"][1]["goRequests"], 1);
    assert_eq!(
        result["workspaces"][0]["disabledModels"][0]["model"],
        "disabled-model"
    );
    let key: (String, String) = conn
        .exec_first(
            "SELECT id,`key` FROM `key` WHERE workspace_id = ?",
            (&invitee,),
        )
        .await
        .unwrap()
        .unwrap();
    // Fresh monthly windows cross a displayed day boundary one second after creation.
    // Compare identifier resolution at the same instant without dropping countdown fields.
    let lookup_time = time::OffsetDateTime::now_utc();
    let keyed = support
        .lookup_at(&key.1, &limits, lookup_time)
        .await
        .unwrap();
    assert_eq!(keyed["identifier"], "[redacted API key]");
    assert!(!keyed.to_string().contains(&key.1));
    assert_eq!(
        support
            .lookup_at(&key.0, &limits, lookup_time)
            .await
            .unwrap()["workspaces"],
        keyed["workspaces"]
    );
    assert_eq!(
        support
            .lookup("key_missing", &limits)
            .await
            .unwrap_err()
            .to_string(),
        "API key not found"
    );
    drop(conn);
    fixture.drop_database().await.unwrap();
}
