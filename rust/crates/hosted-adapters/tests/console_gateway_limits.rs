//! Purpose: accept Redis caps and MySQL routing counters through public adapters.
//! Scope: fake Upstash HTTP service, real migrated MySQL and provider selection rules.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/gateway/catalog.rs
//! `-- hosted-adapters/src/console/limits.rs
//! External:
//! |-- axum::Router
//! |-- mysql_async::prelude::Queryable
//! `-- serde_json::json
//! Used by:
//! `-- none, entrypoint
//! Invariants: fixture-only service, no production Redis or provider traffic.
//! Contract tests: this file.

use axum::{Json, Router, extract::State, routing::post};
use mysql_async::prelude::Queryable;
use opencode_hosted_adapters::console::{
    db::TestDatabase,
    formats::UsageInfo,
    gateway::catalog::normalize,
    limits::{
        LimitError, RedisCounters,
        provider::select,
        store::{ProviderCounters, RoutingSnapshot},
    },
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use time::{Date, Month};

#[tokio::test]
async fn fixture_redis_key_ip_lifetime_and_checkout_caps() {
    let counts = Arc::new(Mutex::new(HashMap::<String, i64>::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route(
                    "/pipeline",
                    post(
                        |State(counts): State<Arc<Mutex<HashMap<String, i64>>>>,
                         Json(commands): Json<Vec<Vec<Value>>>| async move {
                            let mut counts = counts.lock().unwrap();
                            Json(
                                commands
                                    .iter()
                                    .map(|command| {
                                        let value = match command[0].as_str().unwrap() {
                                            "MGET" => json!(
                                                command[1..]
                                                    .iter()
                                                    .map(|key| counts
                                                        .get(key.as_str().unwrap())
                                                        .map(|v| v.to_string()))
                                                    .collect::<Vec<_>>()
                                            ),
                                            "INCR" => {
                                                let count = counts
                                                    .entry(command[1].as_str().unwrap().into())
                                                    .or_default();
                                                *count += 1;
                                                json!(count)
                                            }
                                            "EXPIRE" => json!(1),
                                            other => panic!("unexpected Redis command {other}"),
                                        };
                                        json!({"result":value})
                                    })
                                    .collect::<Vec<_>>(),
                            )
                        },
                    ),
                )
                .with_state(counts),
        )
        .await
        .unwrap()
    });
    let redis =
        RedisCounters::new(&format!("http://{address}"), "fixture-token", "fixture").unwrap();
    let now = Date::from_calendar_date(2026, Month::October, 27)
        .unwrap()
        .with_hms(0, 0, 0)
        .unwrap()
        .assume_utc();
    let limit = redis
        .request("model", Some(1), false, "key", 3, now)
        .await
        .unwrap();
    limit.track().await.unwrap();
    assert!(matches!(
        redis.request("model", Some(1), false, "key", 3, now).await,
        Err(LimitError::Exceeded {
            kind: "RateLimitError",
            retry_after: 60
        })
    ));
    for _ in 0..6 {
        redis
            .request("model", None, true, "ip", 3, now)
            .await
            .unwrap()
            .track()
            .await
            .unwrap();
    }
    assert!(matches!(
        redis.request("model", None, true, "ip", 3, now).await,
        Err(LimitError::Exceeded {
            kind: "FreeUsageLimitError",
            retry_after: 86400
        })
    ));
    for _ in 0..5 {
        redis.checkout("account").await.unwrap();
    }
    assert!(matches!(
        redis.checkout("account").await,
        Err(LimitError::Exceeded {
            kind: "CheckoutRateLimitError",
            ..
        })
    ));
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn fixture_mysql_trial_sticky_tpm_tps_track_real_usage() {
    let fixture = TestDatabase::create("gateway_counters").await.unwrap();
    let now = Date::from_calendar_date(2026, Month::October, 27)
        .unwrap()
        .with_hms(0, 0, 0)
        .unwrap()
        .assume_utc();
    let counters = ProviderCounters::new(fixture.db.clone(), now);
    let provider = json!({"id":"p","model":"m","tpmLimit":1,"tpsGoal":5});
    let model = json!({"providers":[provider],"trialProvider":["p"],"stickyProvider":"prefer"});
    let before = counters
        .snapshot(&model, "public", "session", "ip", 10)
        .await
        .unwrap();
    assert!(before.trial);
    assert!(before.sticky.is_none());
    counters.sticky("public", "session", "p").await.unwrap();
    counters
        .track(
            &provider,
            "ip",
            true,
            &UsageInfo {
                input_tokens: 3,
                output_tokens: 20,
                reasoning_tokens: Some(2),
                cache_read_tokens: Some(4),
                ..Default::default()
            },
            1000,
            3000,
        )
        .await
        .unwrap();
    let after = counters
        .snapshot(&model, "public", "session", "ip", 10)
        .await
        .unwrap();
    assert!(!after.trial);
    assert_eq!(after.sticky.as_deref(), Some("p"));
    assert_eq!(after.tpm["p/m"], 3);
    assert_eq!(after.tps["p/m/5"], (1, 0));
    let mut conn = fixture.db.conn().await.unwrap();
    let tokens: Option<i64> = conn
        .query_first("SELECT `usage` FROM ip WHERE ip='ip'")
        .await
        .unwrap();
    assert_eq!(tokens, Some(29));
    drop(conn);
    fixture.drop_database().await.unwrap();
}

#[tokio::test]
async fn fixture_provider_budget_reserves_previous_priority_and_tracks_contribution() {
    use opencode_hosted_adapters::console::limits::provider::ProviderBudget;
    let counts = Arc::new(Mutex::new(HashMap::<String, i64>::from([
        (
            "fixture:ratelimit:provider-budget:p:1:202610270000".into(),
            10_000_000,
        ),
        (
            "fixture:ratelimit:provider-budget:p:2:202610270000".into(),
            20_000_000,
        ),
        (
            "fixture:ratelimit:provider-budget:p:1:202610262359".into(),
            60_000_000,
        ),
    ])));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener,Router::new().route("/pipeline",post(|State(counts):State<Arc<Mutex<HashMap<String,i64>>>>,Json(commands):Json<Vec<Vec<Value>>>|async move {
        let mut counts=counts.lock().unwrap();
        Json(commands.iter().map(|command|json!({"result":match command[0].as_str().unwrap() {
            "MGET"=>json!(command[1..].iter().map(|key|counts.get(key.as_str().unwrap()).map(i64::to_string)).collect::<Vec<_>>()),
            "INCRBY"=>{let value=counts.entry(command[1].as_str().unwrap().into()).or_default();*value+=command[2].as_i64().unwrap();json!(value)},
            "EXPIRE"=>json!(1),
            other=>panic!("unexpected Redis budget command {other}"),
        }})).collect::<Vec<_>>())
    })).with_state(counts)).await.unwrap()
    });
    let now = Date::from_calendar_date(2026, Month::October, 27)
        .unwrap()
        .with_hms(0, 0, 0)
        .unwrap()
        .assume_utc();
    let redis = RedisCounters::new(&format!("http://{address}"), "local", "fixture").unwrap();
    let provider = json!({"id":"p","budget":1,"budgetPriority":2,"budgetContribution":0.5});
    let tracker = ProviderBudget::new(redis, std::slice::from_ref(&provider), now).unwrap();
    let before = tracker.snapshot().await.unwrap();
    assert!(before.qualify("p", 1));
    assert!(before.qualify("p", 2));
    assert!(before.prefer("p", 2));
    tracker.track(&provider, 30.0).await.unwrap();
    let after = tracker.snapshot().await.unwrap();
    assert!(after.qualify("p", 1));
    assert!(!after.qualify("p", 2));
    server.abort();
    let _ = server.await;
}

#[test]
fn fixture_composite_weights_byok_sticky_and_tpm_fallback_selection() {
    let catalog = json!({"providers":{"p":{"apiKey":{"a":"a","b":"b"}},"q":{"apiKey":"q"}},"zenModels":{"m":{"providers":[{"id":"p","model":"native","priority":1},{"id":"q","model":"native","priority":1}],"trialProvider":"p","fallbackProvider":"q"}},"liteModels":{}});
    let (model, providers) = normalize(&catalog, false, "m", "oa-compat").unwrap();
    assert_eq!(providers.len(), 3);
    assert_eq!(model["providers"][2]["weight"], 2);
    assert_eq!(model["trialProvider"], json!(["p.a", "p.b"]));
    let snapshot = RoutingSnapshot {
        sticky: Some("q".into()),
        ..Default::default()
    };
    assert_eq!(
        select(&model, "session", false, &snapshot, None).unwrap()["id"],
        "q"
    );
    let model = json!({"providers":[{"id":"p","model":"m","tpmLimit":1,"priority":1},{"id":"q","model":"m","weight":0}],"fallbackProvider":"q","byokProvider":"p"});
    let snapshot = RoutingSnapshot {
        tpm: HashMap::from([("p/m".into(), 1_000_000)]),
        ..Default::default()
    };
    assert_eq!(
        select(&model, "session", false, &snapshot, None).unwrap()["id"],
        "q"
    );
    assert_eq!(
        select(&model, "session", true, &snapshot, None).unwrap()["id"],
        "p"
    );
}
