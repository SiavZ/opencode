//! Purpose: verify settings access and month normalization edge cases through HTTP.
//! Scope: real MySQL fixture and shared CSRF policy.
//! Uses:
//! rust/bins/
//! `-- hosted/src/console/workspace/settings/tests.rs
//! External:
//! |-- axum::http::Request
//! `-- tower::ServiceExt
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/workspace/settings/tests.rs
//! Invariants: denied writes leave provider state untouched; invalid dates cannot panic.
//! Contract tests: this file

use super::*;
use axum::body::Body;
use axum::http::Request;
use tower::ServiceExt;

#[tokio::test]
async fn settings_deny_cross_workspace_members_and_cross_site_fetch_metadata() {
    let fixture = Fixture::new("settings_edges").await;
    for endpoint in [
        "providers",
        "models",
        "usage/graph?year=2026&month=9&tzOffset=%2B00%3A00",
    ] {
        let path = format!("/workspaces/{}/{endpoint}", fixture.other);
        assert!(
            !fixture
                .request("GET", &path, Some(&fixture.member), None, Value::Null)
                .await
                .0
                .is_success()
        );
    }
    let path = format!("/workspaces/{}/providers", fixture.workspace);
    let response = fixture
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&path)
                .header("cookie", &fixture.owner)
                .header("origin", ORIGIN)
                .header("sec-fetch-site", "cross-site")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"provider":"openai","credentials":"sk-edge-fixture"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        fixture
            .request("GET", &path, Some(&fixture.owner), None, Value::Null)
            .await
            .1,
        json!([])
    );
    // Origin-less native clients follow the existing shared CSRF contract.
    assert_eq!(
        fixture
            .request(
                "POST",
                &path,
                Some(&fixture.owner),
                None,
                json!({"provider":"openai","credentials":"sk-edge-fixture"})
            )
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    fixture.database.drop_database().await.unwrap();
}

#[tokio::test]
async fn graph_normalizes_overflow_months_and_rejects_unrepresentable_dates() {
    let fixture = Fixture::new("settings_graph_edges").await;
    let base = format!("/workspaces/{}/usage/graph", fixture.workspace);
    for query in [
        "year=2026&month=12&tzOffset=%2B00%3A00",
        "year=2027&month=-1&tzOffset=-05%3A30",
        "year=2026&month=9&tzOffset=invalid",
    ] {
        let (status, graph) = fixture
            .request(
                "GET",
                &format!("{base}?{query}"),
                Some(&fixture.member),
                None,
                Value::Null,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{graph}");
        assert_eq!(graph["usage"], json!([]));
        assert_eq!(graph["keys"].as_array().unwrap().len(), 1);
    }
    for query in [
        "year=2147483647&month=0&tzOffset=%2B00%3A00",
        "year=-2147483648&month=-1&tzOffset=-02%3A00",
        "year=2026&month=9",
    ] {
        assert_eq!(
            fixture
                .request(
                    "GET",
                    &format!("{base}?{query}"),
                    Some(&fixture.member),
                    None,
                    Value::Null
                )
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
    }
    fixture.database.drop_database().await.unwrap();
}
