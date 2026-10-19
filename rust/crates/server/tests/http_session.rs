//! Purpose: socket tests for current location, Session and message handlers over fixture
//! services: admission, error mapping, cursor paging, query decoding and session placement.
//! Scope: live local listener, fixture `SessionService` (not a production adapter).
//! Uses:
//! rust/crates/
//! |-- server/src/lifecycle.rs
//! `-- server/tests/support/mod.rs
//! External:
//! |-- serde_json::Value
//! |-- serde_json::json
//! `-- tokio::test
//! Used by:
//! `-- none, entrypoint
//! Invariants: fixture-based; real Session behavior is covered by Phase 04 tests.
//! Contract tests: this file.

mod support;

use std::sync::Arc;

use serde_json::{Value, json};

async fn embedded() -> (
    opencode_server::lifecycle::Listener,
    Arc<support::FakeSessions>,
) {
    let sessions = Arc::new(support::FakeSessions::default());
    let server = support::start(support::session_router(support::services(sessions.clone()))).await;
    (server, sessions)
}

async fn get(url: String) -> (u16, Value) {
    let response = support::client().get(url).send().await.unwrap();
    let status = response.status().as_u16();
    (status, response.json().await.unwrap_or(Value::Null))
}

#[tokio::test]
async fn empty_session_list_preserves_recorded_null_cursor_keys() {
    let sessions = Arc::new(support::FakeSessions {
        empty_lists: true,
        ..support::FakeSessions::default()
    });
    let server = support::start(support::session_router(support::services(sessions))).await;
    let (status, body) = get(format!("{}/api/session", server.url())).await;
    assert_eq!(status, 200);
    assert_eq!(
        body,
        json!({"data":[],"cursor":{"previous":null,"next":null}})
    );
    server.stop(false).await;
}

#[tokio::test]
async fn location_uses_bracketed_query_then_header() {
    let (server, _) = embedded().await;
    let (status, body) = get(format!(
        "{}/api/location?location%5Bdirectory%5D=/tmp/x&location%5Bworkspace%5D=wrk_1",
        server.url()
    ))
    .await;
    assert_eq!(status, 200);
    assert_eq!(
        body,
        json!({"directory":"/tmp/x","workspaceID":"wrk_1","project":{"id":"global","directory":"/"}})
    );
    let response = support::client()
        .get(format!("{}/api/location", server.url()))
        .header("x-opencode-directory", "/a%20b")
        .send()
        .await
        .unwrap();
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["directory"], "/a b");
    server.stop(false).await;
}

#[tokio::test]
async fn prompt_admits_through_session_service() {
    let (server, sessions) = embedded().await;
    let response = support::client()
        .post(format!("{}/api/session/ses_known/prompt", server.url()))
        .json(&json!({ "id": "msg_x", "prompt": { "text": "hello" }, "resume": false }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["data"]["id"], "msg_x");
    assert_eq!(body["data"]["sessionID"], "ses_known");
    let recorded = sessions.prompts.lock().unwrap().clone();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].resume, Some(false));
    assert_eq!(recorded[0].prompt.text, "hello");
    server.stop(false).await;
}

#[tokio::test]
async fn prompt_conflict_maps_to_conflict_error() {
    let (server, _) = embedded().await;
    let response = support::client()
        .post(format!("{}/api/session/ses_known/prompt", server.url()))
        .json(&json!({ "id": "msg_conflict", "prompt": { "text": "x" } }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 409);
    let body: Value = response.json().await.unwrap();
    assert_eq!(
        body,
        json!({"_tag":"ConflictError","message":"Prompt message ID conflicts with an existing durable record: msg_conflict","resource":"msg_conflict"})
    );
    server.stop(false).await;
}

#[tokio::test]
async fn session_location_rejects_bad_and_unknown_ids() {
    let (server, _) = embedded().await;
    let (status, body) = get(format!("{}/api/session/bogus/message", server.url())).await;
    assert_eq!(status, 400);
    assert_eq!(
        body,
        json!({"_tag":"InvalidRequestError","message":"Invalid session ID","field":"sessionID"})
    );
    let (status, body) = get(format!("{}/api/session/ses_missing/message", server.url())).await;
    assert_eq!(status, 404);
    assert_eq!(
        body,
        json!({"_tag":"SessionNotFoundError","sessionID":"ses_missing","message":"Session not found: ses_missing"})
    );
    server.stop(false).await;
}

#[tokio::test]
async fn messages_page_with_cursors_and_reject_cursor_plus_order() {
    let (server, sessions) = embedded().await;
    let (status, body) = get(format!(
        "{}/api/session/ses_known/message?limit=2",
        server.url()
    ))
    .await;
    assert_eq!(status, 200);
    assert_eq!(body["data"].as_array().unwrap().len(), 2);
    let next = body["cursor"]["next"].as_str().unwrap().to_string();
    assert_eq!(sessions.messages.lock().unwrap()[0].limit, Some(2));

    let (status, _) = get(format!(
        "{}/api/session/ses_known/message?cursor={next}",
        server.url()
    ))
    .await;
    assert_eq!(status, 200);
    let last = sessions.messages.lock().unwrap().last().cloned().unwrap();
    assert_eq!(last.cursor.unwrap().0.as_str(), "msg_1");

    let (status, body) = get(format!(
        "{}/api/session/ses_known/message?cursor={next}&order=asc",
        server.url()
    ))
    .await;
    assert_eq!(status, 400);
    assert_eq!(
        body,
        json!({"_tag":"InvalidCursorError","message":"Cursor cannot be combined with order"})
    );
    let (status, body) = get(format!(
        "{}/api/session/ses_known/message?cursor=zzz",
        server.url()
    ))
    .await;
    assert_eq!(status, 400);
    assert_eq!(body["message"], "Invalid cursor");
    server.stop(false).await;
}

#[tokio::test]
async fn number_from_string_query_rejects_out_of_range() {
    let (server, _) = embedded().await;
    let (status, body) = get(format!(
        "{}/api/session/ses_known/message?limit=abc",
        server.url()
    ))
    .await;
    assert_eq!(status, 400);
    assert_eq!(body["_tag"], "InvalidRequestError");
    assert_eq!(body["kind"], "Query");
    server.stop(false).await;
}

#[tokio::test]
async fn list_sessions_returns_cursors_and_active_map() {
    let (server, sessions) = embedded().await;
    let (status, body) = get(format!(
        "{}/api/session?directory=/work&limit=2",
        server.url()
    ))
    .await;
    assert_eq!(status, 200);
    assert_eq!(body["data"][0]["id"], "ses_b");
    let next = body["cursor"]["next"].as_str().unwrap().to_string();
    let (status, _) = get(format!("{}/api/session?cursor={next}", server.url())).await;
    assert_eq!(status, 200);
    let requests = sessions.lists.lock().unwrap().clone();
    assert_eq!(requests[1].anchor.as_ref().unwrap().id.as_str(), "ses_a");
    assert_eq!(requests[1].scope, requests[0].scope);
    let (status, body) = get(format!("{}/api/session/active", server.url())).await;
    assert_eq!(status, 200);
    assert_eq!(body, json!({"data":{"ses_run":{"type":"running"}}}));
    server.stop(false).await;
}

#[tokio::test]
async fn unavailable_operation_maps_to_service_unavailable() {
    let (server, _) = embedded().await;
    let response = support::client()
        .post(format!("{}/api/session/ses_known/compact", server.url()))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 503);
    let body: Value = response.json().await.unwrap();
    assert_eq!(
        body,
        json!({"_tag":"ServiceUnavailableError","message":"Session compact is not available yet","service":"session.compact"})
    );
    server.stop(false).await;
}

#[tokio::test]
async fn payload_rejections_and_media_type() {
    let (server, _) = embedded().await;
    let url = format!("{}/api/session/ses_known/prompt", server.url());
    let response = support::client()
        .post(&url)
        .json(&json!({ "prompt": 5 }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["_tag"], "InvalidRequestError");
    assert_eq!(body["kind"], "Payload");

    let response = support::client()
        .post(&url)
        .header("content-type", "text/plain")
        .body("x")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 415);
    assert_eq!(
        response.text().await.unwrap(),
        "Unsupported content-type: text/plain"
    );
    server.stop(false).await;
}
