//! Purpose: verify persisted quota reset boundaries through public usage recording.
//! Scope: subscription and Lite producers over baseline-migrated MySQL.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/db.rs
//! |-- hosted-adapters/src/console/formats.rs
//! `-- hosted-adapters/src/console/gateway/usage.rs
//! External:
//! |-- mysql_async::prelude::Queryable
//! `-- serde_json::json
//! Used by:
//! `-- none, entrypoint
//! Invariants: expired counters reset; future fixed periods are never overwritten by delayed writes.
//! Contract tests: this file.

use mysql_async::prelude::Queryable;
use opencode_hosted_adapters::console::{
    db::TestDatabase, formats::UsageInfo, gateway::usage::UsageService,
};
use serde_json::json;

#[tokio::test]
async fn fixture_expired_and_future_lite_periods_and_default_multiplier() {
    let fixture = TestDatabase::create("usage_windows").await.unwrap();
    let mut conn = fixture.db.conn().await.unwrap();
    conn.query_drop("INSERT INTO workspace (id,name) VALUES ('wrk_window','Window')")
        .await
        .unwrap();
    conn.query_drop("INSERT INTO billing (id,workspace_id,balance,lite) VALUES ('bil_window','wrk_window',1000000000,JSON_OBJECT('useBalance',false))").await.unwrap();
    conn.query_drop("INSERT INTO user (id,workspace_id,name,role) VALUES ('usr_window','wrk_window','Window','admin')").await.unwrap();
    conn.query_drop("INSERT INTO `key` (id,workspace_id,user_id,name,`key`) VALUES ('key_window','wrk_window','usr_window','Window','window_key')").await.unwrap();
    conn.query_drop("INSERT INTO lite (id,workspace_id,user_id,rolling_usage,weekly_usage,monthly_usage,time_rolling_updated,time_weekly_updated,time_monthly_updated) VALUES ('lit_window','wrk_window','usr_window',900000000,900000000,900000000,TIMESTAMPADD(YEAR,-1,NOW(3)),TIMESTAMPADD(YEAR,-1,NOW(3)),TIMESTAMPADD(YEAR,-1,NOW(3)))").await.unwrap();
    drop(conn);
    let limits =
        json!({"lite":{"rollingLimit":1,"rollingWindow":5,"weeklyLimit":1,"monthlyLimit":1}});
    let service = UsageService::new(fixture.db.clone(), limits);
    let model = json!({"cost":{"input":0.000001,"output":0}});
    let context = service
        .authenticate(Some("window_key"), "model", &model, true)
        .await
        .unwrap();
    let usage = UsageInfo {
        input_tokens: 1,
        ..Default::default()
    };
    service
        .record_with_id(
            &context,
            "s",
            "model",
            &model,
            "p",
            &usage,
            "usg_window_reset",
        )
        .await
        .unwrap();
    let mut conn = fixture.db.conn().await.unwrap();
    let reset: Option<(i64, i64, i64)> = conn
        .query_first("SELECT rolling_usage,weekly_usage,monthly_usage FROM lite")
        .await
        .unwrap();
    assert_eq!(reset, Some((100, 100, 100)));
    conn.query_drop("UPDATE lite SET rolling_usage=7,weekly_usage=8,monthly_usage=9,time_rolling_updated=TIMESTAMPADD(YEAR,1,NOW(3)),time_weekly_updated=TIMESTAMPADD(YEAR,1,NOW(3)),time_monthly_updated=TIMESTAMPADD(YEAR,1,NOW(3))").await.unwrap();
    drop(conn);
    let context = service
        .authenticate(Some("window_key"), "model", &model, true)
        .await
        .unwrap();
    service
        .record_with_id(
            &context,
            "s",
            "model",
            &model,
            "p",
            &usage,
            "usg_window_future",
        )
        .await
        .unwrap();
    let mut conn = fixture.db.conn().await.unwrap();
    let future: Option<(i64,i64,i64,bool,bool,bool)> = conn.query_first("SELECT rolling_usage,weekly_usage,monthly_usage,time_rolling_updated>NOW(3),time_weekly_updated>NOW(3),time_monthly_updated>NOW(3) FROM lite").await.unwrap();
    assert_eq!(future, Some((107, 8, 9, true, true, true)));
    let enrichment: Option<String> = conn
        .query_first("SELECT enrichment FROM `usage` WHERE id='usg_window_reset'")
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&enrichment.unwrap()).unwrap(),
        json!({"plan":"lite","costMultiplier":1})
    );
    drop(conn);
    fixture.drop_database().await.unwrap();
}
