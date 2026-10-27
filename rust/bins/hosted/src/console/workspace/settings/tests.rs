//! Purpose: verify workspace settings HTTP lifecycle against migrated fixture MySQL.
//! Scope: provider/model authorization, persistence, graph grouping and historical keys.
//! Uses:
//! rust/bins/
//! |-- hosted/src/console/workspace/settings.rs
//! |-- hosted/src/console/workspace/settings/tests/edges.rs
//! |-- hosted/src/console/workspace/settings/tests/fixture.rs
//! `-- hosted/src/console/workspace/settings/tests/graph.rs
//! External:
//! `-- mysql_async::prelude::Queryable
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/workspace/settings.rs
//! Invariants: no external provider calls; only disposable local MySQL and signed fixture sessions.
//! Contract tests: this file

use super::*;
use mysql_async::prelude::Queryable;

mod edges;
mod fixture;
mod graph;
use fixture::Fixture;

const ORIGIN: &str = "https://console.opencode.ai";

#[tokio::test]
async fn provider_lifecycle_is_workspace_scoped_admin_only_and_same_origin() {
    let fixture = Fixture::new("settings_providers").await;
    let path = format!("/workspaces/{}/providers", fixture.workspace);
    let payload = json!({"provider":"openai","credentials":"sk-fixture-first"});
    for origin in [Some("https://evil.example"), Some("null")] {
        assert_eq!(
            fixture
                .request("POST", &path, Some(&fixture.owner), origin, payload.clone())
                .await
                .0,
            StatusCode::FORBIDDEN
        );
    }
    for cookie in [
        None,
        Some(fixture.member.as_str()),
        Some(fixture.outsider.as_str()),
    ] {
        assert!(
            !fixture
                .request("POST", &path, cookie, Some(ORIGIN), payload.clone())
                .await
                .0
                .is_success()
        );
    }
    for payload in [
        json!({"provider":"","credentials":"key"}),
        json!({"provider":"openai","credentials":""}),
        json!({"provider":"x".repeat(65),"credentials":"key"}),
    ] {
        assert_eq!(
            fixture
                .request("POST", &path, Some(&fixture.owner), Some(ORIGIN), payload)
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        fixture
            .request("POST", &path, Some(&fixture.owner), Some(ORIGIN), payload)
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    let (status, rows) = fixture
        .request("GET", &path, Some(&fixture.member), None, Value::Null)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rows[0]["credentials"], "sk-fixture-first");
    assert_eq!(rows[0]["workspaceID"], fixture.workspace);
    let id = rows[0]["id"].as_str().unwrap().to_string();
    assert!(rows[0]["timeCreated"].is_string());
    let mut conn = fixture.database.db.conn().await.unwrap();
    conn.exec_drop(
        "UPDATE `provider` SET `time_deleted` = now() WHERE `workspace_id` = ?",
        (&fixture.workspace,),
    )
    .await
    .unwrap();
    drop(conn);
    assert_eq!(
        fixture
            .request("GET", &path, Some(&fixture.owner), None, Value::Null)
            .await
            .1,
        json!([])
    );
    assert_eq!(
        fixture
            .request(
                "POST",
                &path,
                Some(&fixture.owner),
                Some(ORIGIN),
                json!({"provider":"openai","credentials":"sk-fixture-replacement"})
            )
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    let rows = fixture
        .request("GET", &path, Some(&fixture.owner), None, Value::Null)
        .await
        .1;
    assert_eq!(rows.as_array().unwrap().len(), 1);
    assert_eq!(rows[0]["id"], id);
    assert_eq!(rows[0]["credentials"], "sk-fixture-replacement");
    assert!(rows[0]["timeDeleted"].is_null());
    let other = format!("/workspaces/{}/providers", fixture.other);
    assert_eq!(
        fixture
            .request("GET", &other, Some(&fixture.owner), None, Value::Null)
            .await
            .1,
        json!([])
    );
    let remove = format!("{path}/openai");
    assert_eq!(
        fixture
            .request(
                "DELETE",
                &remove,
                Some(&fixture.member),
                Some(ORIGIN),
                Value::Null
            )
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .request(
                "DELETE",
                &remove,
                Some(&fixture.owner),
                Some("https://evil.example"),
                Value::Null
            )
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .request(
                "DELETE",
                &remove,
                Some(&fixture.owner),
                Some(ORIGIN),
                Value::Null
            )
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        fixture
            .request("GET", &path, Some(&fixture.owner), None, Value::Null)
            .await
            .1,
        json!([])
    );
    let mut conn = fixture.database.db.conn().await.unwrap();
    let count: Option<u64> = conn
        .exec_first("SELECT COUNT(*) FROM `provider` WHERE `id` = ?", (&id,))
        .await
        .unwrap();
    assert_eq!(count, Some(0));
    drop(conn);
    fixture.database.drop_database().await.unwrap();
}

#[tokio::test]
async fn model_info_and_toggle_preserve_baseline_current_state_semantics() {
    let fixture = Fixture::new("settings_models").await;
    let path = format!("/workspaces/{}/models", fixture.workspace);
    let (status, info) = fixture
        .request("GET", &path, Some(&fixture.member), None, Value::Null)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        info["all"]
            .as_array()
            .unwrap()
            .iter()
            .map(|model| model["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["big-pickle", "claude-a", "claude-b", "gpt-z", "unknown"]
    );
    assert_eq!(info["disabled"], json!([]));
    for cookie in [None, Some(fixture.outsider.as_str())] {
        assert!(
            !fixture
                .request("GET", &path, cookie, None, Value::Null)
                .await
                .0
                .is_success()
        );
    }
    let toggle = format!("{path}/toggle");
    let payload = json!({"model":"gpt-z","enabled":true});
    assert_eq!(
        fixture
            .request(
                "POST",
                &toggle,
                Some(&fixture.member),
                Some(ORIGIN),
                payload.clone()
            )
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .request(
                "POST",
                &toggle,
                Some(&fixture.owner),
                Some("https://evil.example"),
                payload.clone()
            )
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    for invalid in [
        json!({"model":"","enabled":true}),
        json!({"model":null,"enabled":true}),
        json!({"model":"gpt-z","enabled":"true"}),
    ] {
        assert!(
            fixture
                .request("POST", &toggle, Some(&fixture.owner), Some(ORIGIN), invalid)
                .await
                .0
                .is_client_error()
        );
    }
    for _ in 0..2 {
        assert_eq!(
            fixture
                .request(
                    "POST",
                    &toggle,
                    Some(&fixture.owner),
                    Some(ORIGIN),
                    payload.clone()
                )
                .await
                .0,
            StatusCode::NO_CONTENT
        );
    }
    assert_eq!(
        fixture
            .request("GET", &path, Some(&fixture.member), None, Value::Null)
            .await
            .1["disabled"],
        json!(["gpt-z"])
    );
    let other = format!("/workspaces/{}/models", fixture.other);
    assert_eq!(
        fixture
            .request("GET", &other, Some(&fixture.owner), None, Value::Null)
            .await
            .1["disabled"],
        json!([])
    );
    let mut conn = fixture.database.db.conn().await.unwrap();
    conn.exec_drop(
        "UPDATE `model` SET `time_deleted` = now() WHERE `workspace_id` = ?",
        (&fixture.workspace,),
    )
    .await
    .unwrap();
    drop(conn);
    // Model.listDisabled includes soft-deleted rows, unlike gateway key model filtering.
    assert_eq!(
        fixture
            .request("GET", &path, Some(&fixture.member), None, Value::Null)
            .await
            .1["disabled"],
        json!(["gpt-z"])
    );
    assert_eq!(
        fixture
            .request("POST", &toggle, Some(&fixture.owner), Some(ORIGIN), payload)
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    let mut conn = fixture.database.db.conn().await.unwrap();
    let live: Option<u64> = conn
        .exec_first(
            "SELECT COUNT(*) FROM `model` WHERE `workspace_id` = ? AND `time_deleted` IS NULL",
            (&fixture.workspace,),
        )
        .await
        .unwrap();
    assert_eq!(live, Some(1));
    drop(conn);
    assert_eq!(
        fixture
            .request(
                "POST",
                &toggle,
                Some(&fixture.owner),
                Some(ORIGIN),
                json!({"model":"gpt-z","enabled":false})
            )
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        fixture
            .request("GET", &path, Some(&fixture.member), None, Value::Null)
            .await
            .1["disabled"],
        json!([])
    );
    fixture.database.drop_database().await.unwrap();
}
