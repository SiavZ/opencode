//! Purpose: verify workspace usage graph grouping, timezone cutoffs and historical key labels.
//! Scope: public HTTP graph endpoint against disposable migrated MySQL.
//! Uses:
//! rust/bins/
//! `-- hosted/src/console/workspace/settings/tests.rs
//! External:
//! `-- mysql_async::prelude::Queryable
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/workspace/settings/tests.rs
//! Invariants: every grouping dimension and tenant boundary uses real persisted usage rows.
//! Contract tests: this file

use super::*;

#[tokio::test]
async fn usage_graph_groups_month_offset_model_key_plan_and_historical_key_labels() {
    let fixture = Fixture::new("settings_graph").await;
    let mut conn = fixture.database.db.conn().await.unwrap();
    let owner: String = conn
        .exec_first(
            "SELECT `id` FROM `user` WHERE `workspace_id` = ? AND `role` = 'admin'",
            (&fixture.workspace,),
        )
        .await
        .unwrap()
        .unwrap();
    for (id, name, deleted) in [
        ("key_live", "Alpha", false),
        ("key_deleted", "Old", true),
        ("key_unused_deleted", "Gone", true),
    ] {
        conn.exec_drop("INSERT INTO `key` (`id`,`workspace_id`,`user_id`,`name`,`key`,`time_deleted`) VALUES (?, ?, ?, ?, ?, ?)", (id, &fixture.workspace, &owner, name, format!("sk-fixture-{id}"), deleted.then_some(time::OffsetDateTime::now_utc().date().midnight()))).await.unwrap();
    }
    // October +02:00 starts Sep 30 22:00 UTC and ends Oct 31 22:00 UTC.
    for (id, workspace, date, model, key, plan, cost) in [
        (
            "usg_before",
            &fixture.workspace,
            "2026-09-30 21:59:59",
            "gpt-z",
            Some("key_deleted"),
            Some("sub"),
            900,
        ),
        (
            "usg_start",
            &fixture.workspace,
            "2026-09-30 22:00:00",
            "gpt-z",
            Some("key_deleted"),
            Some("sub"),
            10,
        ),
        (
            "usg_sum",
            &fixture.workspace,
            "2026-10-01 01:00:00",
            "gpt-z",
            Some("key_deleted"),
            Some("sub"),
            15,
        ),
        (
            "usg_plan",
            &fixture.workspace,
            "2026-10-01 01:00:00",
            "gpt-z",
            Some("key_deleted"),
            Some("lite"),
            30,
        ),
        (
            "usg_byok",
            &fixture.workspace,
            "2026-10-01 01:00:00",
            "gpt-z",
            Some("key_live"),
            Some("byok"),
            40,
        ),
        (
            "usg_model",
            &fixture.workspace,
            "2026-10-01 01:00:00",
            "claude-a",
            Some("key_live"),
            None,
            50,
        ),
        (
            "usg_null",
            &fixture.workspace,
            "2026-10-01 01:00:00",
            "gpt-z",
            None,
            None,
            60,
        ),
        (
            "usg_end",
            &fixture.workspace,
            "2026-10-31 21:59:59",
            "gpt-z",
            Some("key_live"),
            None,
            70,
        ),
        (
            "usg_after",
            &fixture.workspace,
            "2026-10-31 22:00:00",
            "gpt-z",
            Some("key_deleted"),
            Some("sub"),
            800,
        ),
        (
            "usg_other",
            &fixture.other,
            "2026-10-01 01:00:00",
            "gpt-z",
            Some("key_deleted"),
            Some("sub"),
            700,
        ),
    ] {
        conn.exec_drop("INSERT INTO `usage` (`id`,`workspace_id`,`time_created`,`model`,`provider`,`input_tokens`,`output_tokens`,`cost`,`key_id`,`enrichment`) VALUES (?, ?, ?, ?, 'fixture', 1, 1, ?, ?, ?)", (id, workspace, date, model, cost, key, plan.map(|plan| json!({"plan":plan}).to_string()))).await.unwrap();
    }
    drop(conn);
    let path = format!(
        "/workspaces/{}/usage/graph?year=2026&month=9&tzOffset=%2B02%3A00",
        fixture.workspace
    );
    let (status, graph) = fixture
        .request("GET", &path, Some(&fixture.member), None, Value::Null)
        .await;
    assert_eq!(status, StatusCode::OK, "{graph}");
    let usage = graph["usage"].as_array().unwrap();
    assert_eq!(usage.len(), 6);
    assert_eq!(
        usage
            .iter()
            .map(|row| row["totalCost"].as_i64().unwrap())
            .sum::<i64>(),
        275
    );
    assert!(usage.contains(&json!({"date":"2026-10-01","model":"gpt-z","totalCost":25,"keyId":"key_deleted","plan":"sub"})));
    assert!(
        usage
            .iter()
            .any(|row| row["date"] == "2026-10-31" && row["totalCost"] == 70)
    );
    let keys = graph["keys"].as_array().unwrap();
    assert!(keys.contains(
        &json!({"id":"key_deleted","displayName":"owner@example.test - Old","deleted":true})
    ));
    assert!(keys.contains(
        &json!({"id":"key_live","displayName":"owner@example.test - Alpha","deleted":false})
    ));
    assert!(!keys.iter().any(|key| key["id"] == "key_unused_deleted"));
    assert!(
        keys.windows(2)
            .all(|pair| pair[0]["displayName"].as_str() <= pair[1]["displayName"].as_str())
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
    let empty = format!(
        "/workspaces/{}/usage/graph?year=2026&month=10&tzOffset=%2B02%3A00",
        fixture.workspace
    );
    let graph = fixture
        .request("GET", &empty, Some(&fixture.member), None, Value::Null)
        .await
        .1;
    assert_eq!(graph["usage"].as_array().unwrap().len(), 1);
    let negative = format!(
        "/workspaces/{}/usage/graph?year=2026&month=9&tzOffset=-02%3A00",
        fixture.workspace
    );
    let graph = fixture
        .request("GET", &negative, Some(&fixture.member), None, Value::Null)
        .await
        .1;
    assert_eq!(
        graph["usage"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["totalCost"].as_i64().unwrap())
            .sum::<i64>(),
        870
    );
    fixture.database.drop_database().await.unwrap();
}
