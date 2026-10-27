//! Purpose: verify native stat, trace and operator workers using captured baseline and local transports.
//! Scope: fixture-only adapter acceptance tests with real isolated MySQL and HTTP listeners.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/db.rs
//! |-- hosted-adapters/src/console/workers.rs
//! |-- hosted-adapters/src/console/workers/logs.rs
//! |-- hosted-adapters/src/console/workers/operators.rs
//! `-- hosted-adapters/src/console/workers/stat.rs
//! External:
//! |-- axum::{Router, Json}
//! |-- mysql_async::prelude::Queryable
//! |-- serde_json::{Value, json}
//! `-- time::OffsetDateTime
//! Used by:
//! `-- none, entrypoint
//! Invariants: no production endpoints or data; fixtures fail visibly if local MySQL is unavailable.
//! Contract tests: this file.

use axum::{Json, Router, routing::post};
use mysql_async::prelude::Queryable;
use opencode_hosted_adapters::console::{
    db::TestDatabase,
    workers::{
        Capture, WorkerError, logs, operators,
        stat::{StatStore, intervals},
    },
};
use serde_json::{Value, json};
use time::OffsetDateTime;

async fn run_op(
    command: &str,
    input: &Value,
    db: &opencode_hosted_adapters::console::db::ConsoleDb,
) -> Result<Value, WorkerError> {
    let mut input = input.clone();
    input
        .as_object_mut()
        .unwrap()
        .insert("confirmDatabase".into(), json!(db.database()));
    operators::run(command, &input, db).await
}

#[test]
fn captured_baseline_log_parity_and_ip_prefixes() {
    let fixture: Value =
        serde_json::from_str(include_str!("console_workers_fixture.json")).unwrap();
    let now = OffsetDateTime::from_unix_timestamp_nanos(
        i128::from(fixture["now"].as_i64().unwrap()) * 1_000_000,
    )
    .unwrap();
    assert_eq!(
        logs::enrich(fixture["traces"].as_array().unwrap(), now).unwrap(),
        fixture["expected"]
    );
    for (input, expected) in [
        ("203.0.113.45", Some("203.0.113.45/32")),
        ("2001:0DB8:00ab:0000:1234::1", Some("2001:db8:ab:0::/64")),
        ("::1", Some("0:0:0:0::/64")),
        ("", None),
        ("invalid", None),
        ("1:2:3:4:5:6:7:8:9", None),
    ] {
        assert_eq!(logs::ip_prefix(input).as_deref(), expected, "{input}");
    }
}

#[test]
fn utc_minutes_cross_calendar_boundaries_and_local_guard_fails_closed() {
    let now = OffsetDateTime::parse(
        "2026-01-01T01:00:30+01:00",
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap();
    let minutes = intervals(now);
    assert_eq!(minutes[0], 202601010000);
    assert_eq!(minutes[1], 202512312359);
    assert_eq!(minutes[29], 202512312331);
    assert!(
        operators::validate_local(
            "mysql://root@127.0.0.1/p12_fixture",
            "p12_fixture",
            "p12_fixture"
        )
        .is_ok()
    );
    for (url, db, confirm) in [
        (
            "mysql://root@production.test/p12_fixture",
            "p12_fixture",
            "p12_fixture",
        ),
        (
            "mysql://root@127.0.0.1/production",
            "production",
            "production",
        ),
        ("mysql://root@127.0.0.1/p12_fixture", "p12_fixture", ""),
    ] {
        assert!(operators::validate_local(url, db, confirm).is_err());
    }
}

#[tokio::test]
async fn fixture_http_and_durable_file_capture_and_failure_propagation() {
    let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
    let router = Router::new().route(
        "/capture",
        post(move |Json(value): Json<Value>| {
            let sender = sender.clone();
            async move {
                sender.send(value).await.unwrap();
                axum::http::StatusCode::ACCEPTED
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let sink = Capture::Http {
        client: reqwest::Client::new(),
        endpoint: format!("http://{addr}/capture").parse().unwrap(),
        bearer: Some("fixture".into()),
    };
    let value = json!({"events":[{"data":{"ip.prefix":"203.0.113.1/32"}}]});
    sink.send(&value).await.unwrap();
    assert_eq!(receiver.recv().await.unwrap(), value);
    let failing = Capture::Http {
        client: reqwest::Client::new(),
        endpoint: format!("http://{addr}/absent").parse().unwrap(),
        bearer: None,
    };
    assert!(failing.send(&value).await.is_err());
    task.abort();
    let _ = task.await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("capture.jsonl");
    let sink = Capture::JsonLines(path.clone());
    sink.send(&value).await.unwrap();
    sink.send(&value).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>(),
        vec![value.clone(), value]
    );
}

#[tokio::test]
async fn fixture_mysql_stat_buckets_and_native_operators_are_durable() {
    let fixture = TestDatabase::create("workers").await.unwrap();
    let now = OffsetDateTime::parse(
        "2026-01-01T00:00:30Z",
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap();
    let mut conn = fixture.db.conn().await.unwrap();
    conn.query_drop("INSERT INTO model_tps_rate_limit (id,`interval`,`qualify`,`unqualify`) VALUES ('provider/model/20',202601010000,7,3),('provider/model/20',202512312359,4,5),('provider/model/20',202512312330,99,99),('other',202601010000,90,90)").await.unwrap();
    assert!(
        operators::run(
            "create-coupon",
            &json!({"email":"blocked@example.test","type":"BUILDATHON"}),
            &fixture.db
        )
        .await
        .is_err()
    );
    let stats = StatStore::new(fixture.db.clone());
    let result = stats
        .aggregate(&["provider/model/20".into(), "missing".into()], now)
        .await
        .unwrap();
    let baseline: Value =
        serde_json::from_str(include_str!("console_workers_stat_fixture.json")).unwrap();
    assert_eq!(result["provider/model/20"], baseline["expected"]);
    assert_eq!(result["provider/model/20"].as_array().unwrap().len(), 30);
    assert_eq!(
        result["provider/model/20"][0],
        json!({"interval":202601010000_i64,"qualify":7,"unqualify":3})
    );
    assert_eq!(result["provider/model/20"][1]["qualify"], 4);
    assert_eq!(
        result["provider/model/20"][29],
        json!({"interval":202512312331_i64,"qualify":0,"unqualify":0})
    );
    assert!(
        result["missing"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["qualify"] == 0 && row["unqualify"] == 0)
    );
    assert_eq!(stats.aggregate(&[], now).await.unwrap(), json!({}));
    let dir = tempfile::tempdir().unwrap();
    let capture = Capture::JsonLines(dir.path().join("stat.jsonl"));
    stats
        .capture(&["provider/model/20".into()], now, &capture)
        .await
        .unwrap();
    let cancel = tokio_util::sync::CancellationToken::new();
    cancel.cancel();
    stats.scheduled(&[], &capture, cancel).await.unwrap();
    let key = run_op(
        "create-api-key",
        &json!({"email":"workers@example.test","balanceDollars":12.5}),
        &fixture.db,
    )
    .await
    .unwrap();
    let workspace = key["workspaceID"].as_str().unwrap();
    assert!(key["secretKey"].as_str().unwrap().starts_with("sk-"));
    assert_eq!(key["secretKey"].as_str().unwrap().len(), 67);
    let repeat = run_op(
        "create-api-key",
        &json!({"email":"workers@example.test","balanceDollars":1}),
        &fixture.db,
    )
    .await
    .unwrap();
    assert_eq!(repeat["workspaceID"], workspace);
    assert_ne!(repeat["secretKey"], key["secretKey"]);
    assert_eq!(
        conn.exec_first::<i64, _, _>(
            "SELECT balance FROM billing WHERE workspace_id = ?",
            (workspace,)
        )
        .await
        .unwrap(),
        Some(1_250_000_000)
    );
    run_op(
        "credit-workspace",
        &json!({"workspaceID":workspace,"dollarAmount":2.5}),
        &fixture.db,
    )
    .await
    .unwrap();
    assert_eq!(
        conn.exec_first::<i64, _, _>(
            "SELECT balance FROM billing WHERE workspace_id = ?",
            (workspace,)
        )
        .await
        .unwrap(),
        Some(1_500_000_000)
    );
    run_op(
        "freeze-workspace",
        &json!({"workspaceID":workspace}),
        &fixture.db,
    )
    .await
    .unwrap();
    assert_eq!(
        conn.exec_first::<i64, _, _>(
            "SELECT balance FROM billing WHERE workspace_id = ?",
            (workspace,)
        )
        .await
        .unwrap(),
        Some(0)
    );
    assert_eq!(
        conn.exec_first::<i64, _, _>(
            "SELECT CAST(SUM(amount) AS SIGNED) FROM payment WHERE workspace_id = ?",
            (workspace,)
        )
        .await
        .unwrap(),
        Some(-1_250_000_000)
    );
    conn.exec_drop("UPDATE billing SET reload = TRUE, subscription_plan = '20', time_subscription_booked = NOW(3) WHERE workspace_id = ?",(workspace,)).await.unwrap();
    assert_eq!(
        run_op(
            "disable-reload",
            &json!({"workspaceID":workspace}),
            &fixture.db
        )
        .await
        .unwrap()["alreadyDisabled"],
        false
    );
    assert_eq!(
        run_op(
            "disable-reload",
            &json!({"workspaceID":workspace}),
            &fixture.db
        )
        .await
        .unwrap()["alreadyDisabled"],
        true
    );
    assert_eq!(
        run_op(
            "black-select-workspaces",
            &json!({"plan":"20"}),
            &fixture.db
        )
        .await
        .unwrap()["workspaceIDs"],
        json!([workspace])
    );
    run_op(
        "black-onboard-waitlist",
        &json!({"workspaceID":workspace}),
        &fixture.db,
    )
    .await
    .unwrap();
    assert_eq!(
        run_op(
            "black-select-workspaces",
            &json!({"plan":"20"}),
            &fixture.db
        )
        .await
        .unwrap()["workspaceIDs"],
        json!([])
    );
    run_op(
        "black-cancel-waitlist",
        &json!({"workspaceID":workspace}),
        &fixture.db,
    )
    .await
    .unwrap();
    assert!(
        run_op(
            "black-cancel-waitlist",
            &json!({"workspaceID":workspace}),
            &fixture.db
        )
        .await
        .is_err()
    );
    run_op(
        "create-coupon",
        &json!({"email":"workers@example.test","type":"BUILDATHON"}),
        &fixture.db,
    )
    .await
    .unwrap();
    assert!(
        run_op(
            "create-coupon",
            &json!({"email":"workers@example.test","type":"BUILDATHON"}),
            &fixture.db
        )
        .await
        .is_err()
    );
    assert!(matches!(
        run_op("black-gift", &json!({}), &fixture.db).await,
        Err(WorkerError::Validation(_))
    ));
    assert!(
        run_op(
            "create-api-key",
            &json!({"email":"rollback@example.test","workspaceID":"wrk_missing"}),
            &fixture.db
        )
        .await
        .is_err()
    );
    assert_eq!(
        conn.exec_first::<u64, _, _>(
            "SELECT COUNT(*) FROM auth WHERE subject = 'rollback@example.test'",
            ()
        )
        .await
        .unwrap(),
        Some(0)
    );
    assert!(
        run_op("reset-db", &json!({"stage":"production"}), &fixture.db)
            .await
            .is_err()
    );
    run_op("reset-db", &json!({"stage":"frank"}), &fixture.db)
        .await
        .unwrap();
    assert_eq!(
        conn.query_first::<u64, _>("SELECT COUNT(*) FROM workspace")
            .await
            .unwrap(),
        Some(0)
    );
    assert_eq!(
        conn.query_first::<u64, _>("SELECT COUNT(*) FROM payment")
            .await
            .unwrap(),
        Some(0)
    );
    // Baseline reset intentionally leaves auth, coupons and TPS tables alone.
    assert_eq!(
        conn.query_first::<u64, _>("SELECT COUNT(*) FROM coupon")
            .await
            .unwrap(),
        Some(1)
    );
    assert_eq!(
        conn.query_first::<u64, _>("SELECT COUNT(*) FROM model_tps_rate_limit")
            .await
            .unwrap(),
        Some(4)
    );
    drop(conn);
    fixture.drop_database().await.unwrap();
}
