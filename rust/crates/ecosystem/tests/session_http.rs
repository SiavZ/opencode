//! Purpose: fixture tests for the legacy-HTTP session port against a local fake opencode
//! server (axum). Fixture-only, no live server.
//! Scope: test.
//! Uses:
//! rust/crates/
//! `-- ecosystem/src/session.rs
//! External:
//! |-- axum::Json
//! |-- axum::Router
//! |-- axum::extract::Path
//! |-- axum::http::StatusCode::BAD_REQUEST
//! |-- axum::routing::get
//! |-- axum::routing::post
//! |-- axum::serve
//! |-- serde_json::Value
//! |-- serde_json::json
//! |-- tokio::net::TcpListener::bind
//! |-- tokio::spawn
//! |-- tokio::test
//! `-- tokio_util::sync::CancellationToken
//! Used by:
//! `-- none, entrypoint
//! Invariants: wire bodies match the baseline SDK shapes.
//! Contract tests: this file.

use axum::{Json, Router, extract::Path, routing::get, routing::post};
use opencode_ecosystem::session::{HttpSessionPort, PromptRequest, SessionPort};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

async fn serve(router: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    format!("http://{addr}")
}

#[tokio::test]
async fn fixture_create_share_prompt_and_events() {
    let router = Router::new()
        .route(
            "/session",
            post(|Json(body): Json<Value>| async move {
                Json(json!({ "id": "ses_abc12345", "title": body["title"], "version": "1.0.0" }))
            }),
        )
        .route(
            "/session/{id}/share",
            post(|Path(id): Path<String>| async move {
                Json(json!({ "id": id, "title": "t", "version": "1", "share": { "url": "https://opncd.ai/s/abc12345" } }))
            }),
        )
        .route(
            "/session/{id}/message",
            post(|Json(body): Json<Value>| async move {
                assert_eq!(body["agent"], "build");
                assert_eq!(body["model"]["providerID"], "anthropic");
                Json(json!({ "info": {}, "parts": [
                    { "type": "text", "text": "first" },
                    { "type": "tool", "tool": "bash" },
                    { "type": "text", "text": "last" }
                ]}))
            }),
        )
        .route(
            "/event",
            get(|| async {
                "data: {\"type\":\"message.part.updated\",\"properties\":{\"part\":{\"type\":\"tool\"}}}\n\ndata: not-json\n\n"
            }),
        );
    let port = HttpSessionPort::new(serve(router).await);
    let session = port.create(Some("Slack thread 1")).await.unwrap();
    assert_eq!(session.id, "ses_abc12345");
    assert_eq!(session.title, "Slack thread 1");
    let shared = port.share(&session.id).await.unwrap();
    assert_eq!(shared.share.unwrap().url, "https://opncd.ai/s/abc12345");

    let mut request = PromptRequest::text("hi");
    request.agent = Some("build".into());
    request.model = Some(opencode_ecosystem::session::ModelRef {
        provider_id: "anthropic".into(),
        model_id: "claude".into(),
    });
    let response = port.prompt(&session.id, &request).await.unwrap();
    assert_eq!(response.last_text(), Some("last"));
    assert_eq!(response.joined_text(), "first\nlast");

    let mut events = port.subscribe(CancellationToken::new()).await.unwrap();
    let event = events.recv().await.unwrap();
    assert_eq!(event.kind, "message.part.updated");
    assert!(
        events.recv().await.is_none(),
        "malformed frames are skipped"
    );
}

#[tokio::test]
async fn fixture_error_status_is_typed() {
    let router = Router::new().route(
        "/session",
        post(|| async { (axum::http::StatusCode::BAD_REQUEST, "bad") }),
    );
    let port = HttpSessionPort::new(serve(router).await);
    let err = port.create(None).await.unwrap_err();
    assert!(matches!(
        err,
        opencode_ecosystem::session::SessionError::Status { status: 400, .. }
    ));
}
