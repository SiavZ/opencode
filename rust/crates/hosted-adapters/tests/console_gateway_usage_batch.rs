//! Purpose: verify hot workspace usage against real local Redis and migrated MySQL.
//! Scope: localhost Upstash protocol bridge forwards commands to Redis, without copied counter logic.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/db.rs
//! |-- hosted-adapters/src/console/formats.rs
//! |-- hosted-adapters/src/console/gateway/usage.rs
//! `-- hosted-adapters/src/console/limits.rs
//! External:
//! |-- axum::{Json, Router}
//! |-- mysql_async::prelude::Queryable
//! |-- redis::Client
//! `-- serde_json::{Value, json}
//! Used by:
//! `-- none, entrypoint
//! Invariants: unique Redis namespace and database; no production services or global flush operations.
//! Contract tests: this file; OPENCODE_P12_USAGE_REDIS_URL points to local fixture Redis.

use axum::{Json, Router, extract::State, routing::post};
use mysql_async::prelude::Queryable;
use opencode_hosted_adapters::console::{
    db::TestDatabase, formats::UsageInfo, gateway::usage::UsageService, limits::RedisCounters,
};
use serde_json::{Value, json};

#[tokio::test]
async fn fixture_real_redis_pending_flush_replay_and_hot_free_byok_deltas() {
    let fixture = TestDatabase::create("usage_hot").await.unwrap();
    let workspace = "wrk_01KJ8PX5CH50Y4YNGNS9ZR8YDC";
    let mut conn = fixture.db.conn().await.unwrap();
    conn.exec_drop(
        "INSERT INTO workspace (id,name) VALUES (?,'Hot fixture')",
        (workspace,),
    )
    .await
    .unwrap();
    conn.exec_drop("INSERT INTO billing (id,workspace_id,balance,payment_method_id) VALUES ('bil_hot',?,1000000000,'pm_local')",(workspace,)).await.unwrap();
    conn.exec_drop(
        "INSERT INTO user (id,workspace_id,name,role) VALUES ('usr_hot',?,'Hot','admin')",
        (workspace,),
    )
    .await
    .unwrap();
    conn.exec_drop("INSERT INTO `key` (id,workspace_id,user_id,name,`key`) VALUES ('key_hot',?,'usr_hot','Hot','hot_key')",(workspace,)).await.unwrap();
    drop(conn);
    let redis_url =
        std::env::var("OPENCODE_P12_USAGE_REDIS_URL").expect("local Redis fixture URL required");
    assert!(
        redis_url.starts_with("redis://127.0.0.1:"),
        "fixture must remain local"
    );
    let client = redis::Client::open(redis_url).unwrap();
    let app = Router::new().route("/pipeline",post(|State(client):State<redis::Client>,Json(commands):Json<Vec<Vec<Value>>>| async move {
        let mut conn = client.get_multiplexed_async_connection().await.unwrap();
        let mut results = Vec::new();
        for command in commands {
            let key = command[1].as_str().unwrap();
            let value = match command[0].as_str().unwrap() {
                "INCRBY" => json!(redis::cmd("INCRBY").arg(key).arg(command[2].as_i64().unwrap()).query_async::<i64>(&mut conn).await.unwrap()),
                "GETDEL" => json!(redis::cmd("GETDEL").arg(key).query_async::<Option<String>>(&mut conn).await.unwrap()),
                other => panic!("unexpected test transport command {other}"),
            };
            results.push(json!({"result":value}));
        }
        Json(results)
    })).with_state(client.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let stage = format!("usage_fixture_{}", uuid::Uuid::new_v4().simple());
    let redis = RedisCounters::new(&format!("http://{address}"), "fixture", &stage).unwrap();
    let service =
        UsageService::new(fixture.db.clone(), json!({})).with_usage_batcher(redis.clone());
    let pending = service.clone().with_flush_probability(0.0).unwrap();
    let flush = service.with_flush_probability(1.0).unwrap();
    let model = json!({"cost":{"input":0.000001,"output":0}});
    let context = pending
        .authenticate(Some("hot_key"), "model", &model, false)
        .await
        .unwrap();
    let usage = UsageInfo {
        input_tokens: 1,
        ..Default::default()
    };
    let (first, retry) = tokio::join!(
        pending.record_with_id(&context, "s", "model", &model, "p", &usage, "usg_pending"),
        pending.record_with_id(&context, "s", "model", &model, "p", &usage, "usg_pending")
    );
    assert_eq!(first.unwrap(), retry.unwrap());
    let mut conn = fixture.db.conn().await.unwrap();
    let before: Option<(i64, Option<i64>)> = conn
        .exec_first(
            "SELECT balance,monthly_usage FROM billing WHERE workspace_id=?",
            (workspace,),
        )
        .await
        .unwrap();
    assert_eq!(before, Some((1000000000, None)));
    drop(conn);
    let mut redis_conn = client.get_multiplexed_async_connection().await.unwrap();
    let workspace_key = redis.usage_key("wrk", workspace);
    let user_key = redis.usage_key("usr", &format!("{workspace}:usr_hot"));
    assert_eq!(
        redis::cmd("GET")
            .arg(&workspace_key)
            .query_async::<i64>(&mut redis_conn)
            .await
            .unwrap(),
        100
    );
    assert_eq!(
        redis::cmd("TTL")
            .arg(&workspace_key)
            .query_async::<i64>(&mut redis_conn)
            .await
            .unwrap(),
        -1
    );
    flush
        .record_with_id(&context, "s", "model", &model, "p", &usage, "usg_flush")
        .await
        .unwrap();
    let mut conn = fixture.db.conn().await.unwrap();
    let after: Option<(i64, i64)> = conn
        .exec_first(
            "SELECT balance,monthly_usage FROM billing WHERE workspace_id=?",
            (workspace,),
        )
        .await
        .unwrap();
    assert_eq!(after, Some((999999800, 200)));
    drop(conn);
    assert_eq!(
        redis::cmd("GET")
            .arg(&workspace_key)
            .query_async::<Option<String>>(&mut redis_conn)
            .await
            .unwrap(),
        None
    );
    let mut free_model = model.clone();
    free_model["allowAnonymous"] = json!(true);
    let free = flush
        .authenticate(Some("hot_key"), "model", &free_model, false)
        .await
        .unwrap();
    flush
        .record_with_id(
            &free,
            "s",
            "model",
            &free_model,
            "p",
            &usage,
            "usg_hot_free",
        )
        .await
        .unwrap();
    let mut conn = fixture.db.conn().await.unwrap();
    conn.exec_drop("INSERT INTO provider (id,workspace_id,provider,credentials) VALUES ('prv_hot',?,'anthropic','local_byok')",(workspace,)).await.unwrap();
    drop(conn);
    let mut byok_model = model.clone();
    byok_model["byokProvider"] = json!("anthropic");
    let byok = flush
        .authenticate(Some("hot_key"), "model", &byok_model, false)
        .await
        .unwrap();
    flush
        .record_with_id(
            &byok,
            "s",
            "model",
            &byok_model,
            "p",
            &usage,
            "usg_hot_byok",
        )
        .await
        .unwrap();
    let mut conn = fixture.db.conn().await.unwrap();
    let billing: Option<(i64, i64)> = conn
        .exec_first(
            "SELECT balance,monthly_usage FROM billing WHERE workspace_id=?",
            (workspace,),
        )
        .await
        .unwrap();
    let user: Option<i64> = conn
        .exec_first(
            "SELECT monthly_usage FROM user WHERE workspace_id=?",
            (workspace,),
        )
        .await
        .unwrap();
    assert_eq!(billing, Some((999999800, 200)));
    assert_eq!(user, Some(400));
    let count: Option<u64> = conn
        .query_first("SELECT COUNT(*) FROM `usage`")
        .await
        .unwrap();
    assert_eq!(count, Some(4));
    drop(conn);
    assert_eq!(
        redis::cmd("GET")
            .arg(&user_key)
            .query_async::<Option<String>>(&mut redis_conn)
            .await
            .unwrap(),
        None
    );
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
    fixture.drop_database().await.unwrap();
}
