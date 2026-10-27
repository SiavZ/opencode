//! Purpose: verify referral attribution, summary and reward lifecycle against real local MySQL.
//! Scope: public referral repository and baseline migrated schema fixtures.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/db.rs
//! |-- hosted-adapters/src/console/referral.rs
//! `-- hosted-adapters/src/console/tenant.rs
//! External:
//! |-- mysql_async::prelude::Queryable
//! `-- serde_json::json
//! Used by:
//! `-- none, entrypoint
//! Invariants: fixture-only data; concurrent subscription completion cannot grant duplicate rewards; reward failure rolls back consumption.
//! Contract tests: this file.

use mysql_async::prelude::Queryable;
use opencode_hosted_adapters::console::{
    db::TestDatabase, referral::ReferralStore, tenant::TenantStore,
};
use opencode_hosted_domain::console::common::Actor;
use opencode_hosted_domain::console::referral::{RewardSource, RewardStatus};
use serde_json::json;

#[tokio::test]
async fn fixture_referral_concurrent_distinct_workspaces_grant_one_invitee_reward() {
    let fixture = TestDatabase::create("referral_distinct_race")
        .await
        .unwrap();
    let mut conn = fixture.db.conn().await.unwrap();
    conn.query_drop("INSERT INTO account(id) VALUES ('acc_race_invitee')")
        .await
        .unwrap();
    conn.query_drop("INSERT INTO workspace(id,name) VALUES ('wrk_race_inviter','Inviter'),('wrk_race_a','A'),('wrk_race_b','B')").await.unwrap();
    conn.query_drop("INSERT INTO user(id,workspace_id,account_id,name,role) VALUES ('usr_race_a','wrk_race_a','acc_race_invitee','','admin'),('usr_race_b','wrk_race_b','acc_race_invitee','','admin')").await.unwrap();
    conn.query_drop("INSERT INTO referral(id,workspace_id,invitee_account_id) VALUES ('ref_distinct_race','wrk_race_inviter','acc_race_invitee')").await.unwrap();
    let mut blocker = fixture
        .db
        .pool()
        .start_transaction(mysql_async::TxOpts::default())
        .await
        .unwrap();
    let locked: Option<String> = blocker
        .query_first("SELECT id FROM referral WHERE id='ref_distinct_race' FOR UPDATE")
        .await
        .unwrap();
    assert_eq!(locked.as_deref(), Some("ref_distinct_race"));
    let first = ReferralStore::new(fixture.db.clone());
    let second = first.clone();
    let a = tokio::spawn(async move {
        first
            .complete_from_lite_subscription("wrk_race_a", "usr_race_a")
            .await
    });
    let b = tokio::spawn(async move {
        second
            .complete_from_lite_subscription("wrk_race_b", "usr_race_b")
            .await
    });
    // Observe both transactions blocked on the attribution row after their initial
    // consistent account reads. A sleep alone would not establish the stale-snapshot race.
    let observed = tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let waiting: Option<u64> = conn.query_first("SELECT COUNT(DISTINCT w.REQUESTING_THREAD_ID) FROM performance_schema.data_lock_waits w JOIN performance_schema.data_locks l ON l.ENGINE_LOCK_ID=w.REQUESTING_ENGINE_LOCK_ID AND l.ENGINE=w.ENGINE WHERE l.OBJECT_SCHEMA=DATABASE() AND l.OBJECT_NAME='referral'").await?;
            if waiting.is_some_and(|count| count >= 2) { return Ok::<_, mysql_async::Error>(()); }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await;
    if !matches!(&observed, Ok(Ok(()))) {
        a.abort();
        b.abort();
        let _ = tokio::join!(a, b);
        blocker.rollback().await.unwrap();
        drop(conn);
        fixture.drop_database().await.unwrap();
        panic!("both completion transactions must block on the held referral row: {observed:?}");
    }
    blocker.commit().await.unwrap();
    a.await.unwrap().unwrap();
    b.await.unwrap().unwrap();
    let rewards: Vec<String> = conn.query("SELECT workspace_id FROM referral_reward WHERE referral_id='ref_distinct_race' AND time_deleted IS NULL ORDER BY workspace_id").await.unwrap();
    assert_eq!(
        rewards.len(),
        2,
        "one inviter and exactly one invitee workspace"
    );
    assert!(rewards.iter().any(|id| id == "wrk_race_inviter"));
    assert_eq!(
        rewards
            .iter()
            .filter(|id| id.as_str() != "wrk_race_inviter")
            .count(),
        1
    );
    let store = ReferralStore::new(fixture.db.clone());
    store
        .complete_from_lite_subscription("wrk_race_a", "usr_race_a")
        .await
        .unwrap();
    store
        .complete_from_lite_subscription("wrk_race_b", "usr_race_b")
        .await
        .unwrap();
    let after: Vec<String> = conn.query("SELECT workspace_id FROM referral_reward WHERE referral_id='ref_distinct_race' AND time_deleted IS NULL ORDER BY workspace_id").await.unwrap();
    assert_eq!(after, rewards);
    drop(conn);
    drop(store);
    fixture.drop_database().await.unwrap();
}

#[tokio::test]
async fn fixture_referral_summary_skips_empty_email_on_earliest_inviter_admin() {
    let fixture = TestDatabase::create("referral_empty_email").await.unwrap();
    let mut conn = fixture.db.conn().await.unwrap();
    conn.query_drop("INSERT INTO account(id) VALUES ('acc_empty_admin'),('acc_valid_admin'),('acc_email_invitee')").await.unwrap();
    conn.query_drop("INSERT INTO workspace(id,name) VALUES ('wrk_email_inviter','Inviter'),('wrk_email_invitee','Invitee')").await.unwrap();
    conn.query_drop("INSERT INTO user(id,workspace_id,account_id,name,role,time_created) VALUES ('usr_empty_admin','wrk_email_inviter','acc_empty_admin','','admin','2026-01-01'),('usr_valid_admin','wrk_email_inviter','acc_valid_admin','','admin','2026-01-02')").await.unwrap();
    conn.query_drop("INSERT INTO auth(id,account_id,provider,subject) VALUES ('aut_empty_admin','acc_empty_admin','email',''),('aut_valid_admin','acc_valid_admin','email','valid@example.test')").await.unwrap();
    conn.query_drop("INSERT INTO referral(id,workspace_id,invitee_account_id) VALUES ('ref_empty_email','wrk_email_inviter','acc_email_invitee')").await.unwrap();
    drop(conn);
    let summary = ReferralStore::new(fixture.db.clone())
        .summary("wrk_email_invitee", "acc_email_invitee")
        .await
        .unwrap();
    assert_eq!(summary.rewards.len(), 1);
    assert_eq!(
        summary.rewards[0].email.as_deref(),
        Some("valid@example.test")
    );
    fixture.drop_database().await.unwrap();
}

#[tokio::test]
async fn fixture_referral_attribution_completion_preview_and_atomic_apply() {
    let fixture = TestDatabase::create("commerce_referral").await.unwrap();
    let tenant = TenantStore::new(fixture.db.clone());
    let inviter_account = tenant.create_account(None).await.unwrap();
    let invitee_account = tenant.create_account(None).await.unwrap();
    let inviter = tenant
        .create_workspace(
            &Actor::Account {
                account_id: inviter_account.clone(),
                email: "inviter@example.test".into(),
            },
            "Inviter",
        )
        .await
        .unwrap();
    let invitee = tenant
        .create_workspace(
            &Actor::Account {
                account_id: invitee_account.clone(),
                email: "invitee@example.test".into(),
            },
            "Invitee",
        )
        .await
        .unwrap();
    let mut conn = fixture.db.conn().await.unwrap();
    conn.exec_drop("INSERT INTO auth(id,account_id,provider,subject) VALUES ('aut_ref_inviter',?,'email','inviter@example.test'),('aut_ref_invitee',?,'email','invitee@example.test')",(&inviter_account,&invitee_account)).await.unwrap();
    let user: String = conn
        .exec_first(
            "SELECT id FROM user WHERE workspace_id=? AND account_id=?",
            (&invitee, &invitee_account),
        )
        .await
        .unwrap()
        .unwrap();
    drop(conn);
    let store = ReferralStore::new(fixture.db.clone());
    let empty = store.summary(&inviter, &inviter_account).await.unwrap();
    assert_eq!(empty.referral_code.len(), 10);
    assert!(!empty.has_referral);
    assert_eq!(empty.reward_amount, 500);
    assert_eq!(
        store
            .summary(&inviter, &inviter_account)
            .await
            .unwrap()
            .referral_code,
        empty.referral_code
    );
    assert!(
        store
            .create_from_account(&invitee_account, Some("INVALID"))
            .await
            .is_err()
    );
    assert!(
        store
            .create_from_account(&inviter_account, Some(&empty.referral_code))
            .await
            .is_err()
    );
    store
        .create_from_account(
            &invitee_account,
            Some(&format!(" {} ", empty.referral_code.to_lowercase())),
        )
        .await
        .unwrap();
    assert!(
        store
            .create_from_account(&invitee_account, Some(&empty.referral_code))
            .await
            .is_err()
    );
    let pending = store.summary(&inviter, &inviter_account).await.unwrap();
    assert_eq!(pending.rewards.len(), 1);
    assert_eq!(pending.rewards[0].status, RewardStatus::Pending);
    let pending_invitee = store.summary(&invitee, &invitee_account).await.unwrap();
    assert_eq!(pending_invitee.rewards[0].source, RewardSource::Invitee);
    assert_eq!(
        pending_invitee.rewards[0].email.as_deref(),
        Some("inviter@example.test")
    );
    let (a, b) = tokio::join!(
        store.complete_from_lite_subscription(&invitee, &user),
        store.complete_from_lite_subscription(&invitee, &user)
    );
    a.unwrap();
    b.unwrap();
    let summary = store.summary(&invitee, &invitee_account).await.unwrap();
    assert_eq!(summary.rewards.len(), 1);
    assert_eq!(summary.rewards[0].status, RewardStatus::Available);
    assert_eq!(summary.rewards[0].amount, 500);
    let referral = &summary.rewards[0].id;
    let failure = store.apply_reward(&invitee, referral).await.unwrap_err();
    assert_eq!(
        failure.to_string(),
        "Subscribe to Go before applying referral rewards"
    );
    assert_eq!(
        store
            .summary(&invitee, &invitee_account)
            .await
            .unwrap()
            .rewards[0]
            .status,
        RewardStatus::Available
    );
    let mut conn = fixture.db.conn().await.unwrap();
    conn.exec_drop("INSERT INTO lite(id,workspace_id,user_id,rolling_usage,weekly_usage,monthly_usage,time_rolling_updated,time_weekly_updated,time_monthly_updated) VALUES ('lit_ref_fixture',?,?,900000000,700000000,100000000,NOW(3),NOW(3),NOW(3))",(&invitee,&user)).await.unwrap();
    drop(conn);
    let limits = json!({"rollingLimit":10,"rollingWindow":5,"weeklyLimit":20,"monthlyLimit":40});
    let preview = store
        .usage_preview(&invitee, referral, &limits)
        .await
        .unwrap();
    assert_eq!(preview["rollingUsage"]["beforePercent"], 90);
    assert_eq!(preview["rollingUsage"]["afterPercent"], 40);
    let (a, b) = tokio::join!(
        store.apply_reward(&invitee, referral),
        store.apply_reward(&invitee, referral)
    );
    assert!(matches!(
        (a.is_ok(), b.is_ok()),
        (true, false) | (false, true)
    ));
    let applied = store.summary(&invitee, &invitee_account).await.unwrap();
    assert_eq!(applied.rewards[0].status, RewardStatus::Applied);
    assert!(applied.rewards[0].time_applied.is_some());
    assert!(
        store
            .usage_preview(&invitee, referral, &limits)
            .await
            .unwrap()
            .is_null()
    );
    let mut conn = fixture.db.conn().await.unwrap();
    let counters: Option<(i64, i64, i64)> = conn
        .exec_first(
            "SELECT rolling_usage,weekly_usage,monthly_usage FROM lite WHERE workspace_id=?",
            (&invitee,),
        )
        .await
        .unwrap();
    assert_eq!(counters, Some((400_000_000, 200_000_000, 0)));
    let rewards: Option<i64> = conn
        .exec_first(
            "SELECT COUNT(*) FROM referral_reward WHERE referral_id=?",
            (referral,),
        )
        .await
        .unwrap();
    assert_eq!(rewards, Some(2));
    let balance: Option<i64> = conn
        .exec_first(
            "SELECT balance FROM billing WHERE workspace_id=?",
            (&invitee,),
        )
        .await
        .unwrap();
    assert_eq!(balance, Some(0));
    drop(conn);
    let created = store.create(&inviter, &invitee).await.unwrap();
    assert_eq!(created["createdReferral"], false);
    assert_eq!(created["createdRewards"], 0);
    drop(store);
    drop(tenant);
    fixture.drop_database().await.unwrap();
}

#[tokio::test]
async fn fixture_referral_account_history_and_deleted_inviter_eligibility() {
    let fixture = TestDatabase::create("commerce_referral_eligibility")
        .await
        .unwrap();
    let mut conn = fixture.db.conn().await.unwrap();
    conn.exec_drop(
        "INSERT INTO account(id) VALUES ('acc_ref_history'),('acc_ref_empty')",
        (),
    )
    .await
    .unwrap();
    conn.exec_drop("INSERT INTO workspace(id,name) VALUES ('wrk_ref_inviter','Inviter'),('wrk_ref_history','History')",()).await.unwrap();
    conn.exec_drop("INSERT INTO user(id,workspace_id,account_id,name,role) VALUES ('usr_ref_history','wrk_ref_history','acc_ref_history','','admin')",()).await.unwrap();
    conn.exec_drop(
        "INSERT INTO referral_code(workspace_id,code) VALUES ('wrk_ref_inviter','REFHISTORY')",
        (),
    )
    .await
    .unwrap();
    conn.exec_drop("INSERT INTO payment(id,workspace_id,amount,enrichment) VALUES ('pay_ref_history','wrk_ref_history',0,JSON_OBJECT('type','lite'))",()).await.unwrap();
    drop(conn);
    let store = ReferralStore::new(fixture.db.clone());
    store
        .create_from_account("acc_ref_empty", Some("REFHISTORY"))
        .await
        .unwrap();
    store
        .create_from_account("acc_ref_history", Some("REFHISTORY"))
        .await
        .unwrap();
    let mut conn = fixture.db.conn().await.unwrap();
    let count: Option<i64> = conn
        .exec_first("SELECT COUNT(*) FROM referral", ())
        .await
        .unwrap();
    assert_eq!(count, Some(0));
    conn.exec_drop(
        "UPDATE workspace SET time_deleted=NOW(3) WHERE id='wrk_ref_inviter'",
        (),
    )
    .await
    .unwrap();
    drop(conn);
    assert!(
        store
            .create_from_account("acc_ref_history", Some("REFHISTORY"))
            .await
            .is_err()
    );
    assert!(
        store
            .complete_from_lite_subscription("wrk_ref_history", "usr_missing")
            .await
            .is_err()
    );
    drop(store);
    fixture.drop_database().await.unwrap();
}
