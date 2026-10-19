//! Purpose: fixture tests for the Feishu-to-Discord bridge: challenge echo, content
//! extraction, mention rewriting, thread suffix, empty-message success, Discord request shape,
//! upstream 502 and malformed input statuses.
//! Scope: test (fixture-only, local axum fake Discord).
//! Uses:
//! rust/crates/
//! |-- ecosystem/src/bridge.rs
//! `-- ecosystem/src/github/auth/handlers.rs
//! External:
//! |-- axum::Router
//! |-- axum::extract::Path
//! |-- axum::http::HeaderMap
//! |-- axum::http::StatusCode
//! |-- axum::routing::post
//! |-- axum::serve
//! |-- serde_json::Value
//! |-- serde_json::from_str
//! |-- serde_json::json
//! |-- tokio::net::TcpListener::bind
//! |-- tokio::spawn
//! `-- tokio::test
//! Used by:
//! `-- none, entrypoint
//! Invariants: no network beyond 127.0.0.1, bot token is a fixture string.
//! Contract tests: this file.

use std::sync::{Arc, Mutex};

use axum::extract::Path;
use axum::http::{HeaderMap, StatusCode};
use axum::{Router, routing::post};
use opencode_ecosystem::bridge::{FeishuAction, FeishuBridge, transform};
use opencode_ecosystem::github::auth::handlers::HttpRequest;
use serde_json::{Value, json};

fn msg(content: &str, root: Option<&str>, id: Option<&str>) -> Value {
    let mut m = json!({ "content": content, "chat_id": "c" });
    if let Some(r) = root {
        m["root_id"] = json!(r);
    }
    if let Some(i) = id {
        m["message_id"] = json!(i);
    }
    json!({ "event": { "message": m } })
}

#[test]
fn fixture_transform_matches_baseline() {
    assert_eq!(
        transform(&json!({"challenge": "abc"})),
        FeishuAction::Challenge(json!("abc"))
    );
    assert_eq!(transform(&json!({"challenge": ""})), FeishuAction::Ignore);
    assert_eq!(
        transform(&msg(r#"{"text":"@_user_1 hello"}"#, None, Some("om_1"))),
        FeishuAction::Post("hello [om_1]".into())
    );
    assert_eq!(
        transform(&msg(
            r#"{"text":"Aiden, can you look"}"#,
            Some("om_root"),
            Some("om_2")
        )),
        FeishuAction::Post("<@759257817772851260> can you look [om_root]".into())
    );
    assert_eq!(
        transform(&msg("  raw text  ", Some(""), Some("om_3"))),
        FeishuAction::Post("raw text [om_3]".into())
    );
    assert_eq!(
        transform(&msg(r#"{"text":"   "}"#, None, Some("x"))),
        FeishuAction::Ignore
    );
    assert_eq!(
        transform(&msg("@_user_12", None, Some("x"))),
        FeishuAction::Ignore
    );
    // JSON content without a text field falls back to the raw content string.
    assert_eq!(
        transform(&msg(r#"{"image_key":"k"}"#, None, None)),
        FeishuAction::Post(r#"{"image_key":"k"}"#.into())
    );
    assert_eq!(
        transform(&msg("{broken", None, None)),
        FeishuAction::Malformed
    );
    assert_eq!(transform(&json!({})), FeishuAction::Ignore);
    assert_eq!(
        transform(&msg("\u{0085}hello\u{0085}", None, None)),
        FeishuAction::Post("\u{0085}hello\u{0085}".into())
    );
    assert_eq!(
        transform(&msg("@_user_1\u{feff}hello", None, None)),
        FeishuAction::Post("hello".into())
    );
    // Only a leading aiden is rewritten.
    assert_eq!(
        transform(&msg("hi aiden", None, None)),
        FeishuAction::Post("hi aiden".into())
    );
}

type Log = Arc<Mutex<Vec<(String, String, Value)>>>;

async fn discord(status: StatusCode, log: Log) -> String {
    let router = Router::new().route(
        "/channels/{channel}/messages",
        post(
            move |Path(channel): Path<String>, headers: HeaderMap, body: String| {
                let log = log.clone();
                async move {
                    log.lock().unwrap().push((
                        channel,
                        headers["authorization"].to_str().unwrap().to_string(),
                        serde_json::from_str(&body).unwrap(),
                    ));
                    (status, "{\"message\":\"upstream\"}")
                }
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    format!("http://{addr}")
}

fn request(body: &str) -> HttpRequest {
    HttpRequest {
        method: "POST".into(),
        path: "/feishu".into(),
        body: body.as_bytes().to_vec(),
        ..Default::default()
    }
}

#[tokio::test]
async fn fixture_bridge_posts_to_discord() {
    let log = Log::default();
    let bridge = FeishuBridge::new(
        discord(StatusCode::OK, log.clone()).await,
        "chan-1",
        "bot-fixture",
    );
    let body = msg(r#"{"text":"@_user_1 help"}"#, None, Some("om_9")).to_string();
    let r = bridge.handle(&request(&body)).await.unwrap();
    assert_eq!((r.status, r.body.as_str()), (200, r#"{"ok":true}"#));
    assert_eq!(
        *log.lock().unwrap(),
        vec![(
            "chan-1".into(),
            "Bot bot-fixture".into(),
            json!({ "content": "help [om_9]" })
        )]
    );
    assert!(!format!("{bridge:?}").contains("bot-fixture"));
}

#[tokio::test]
async fn fixture_bridge_challenge_empty_and_errors() {
    let log = Log::default();
    let bridge = FeishuBridge::new(
        discord(StatusCode::BAD_REQUEST, log.clone()).await,
        "c",
        "t",
    );
    let challenge = bridge
        .handle(&request(r#"{"challenge":"xyz","type":"url_verification"}"#))
        .await
        .unwrap();
    assert_eq!(challenge.body, r#"{"challenge":"xyz"}"#);
    let empty = bridge
        .handle(&request(&msg("", None, None).to_string()))
        .await
        .unwrap();
    assert_eq!((empty.status, empty.body.as_str()), (200, r#"{"ok":true}"#));
    assert!(log.lock().unwrap().is_empty());

    for scalar in ["true", "123", "\"text\"", "[]"] {
        let r = bridge.handle(&request(scalar)).await.unwrap();
        assert_eq!((r.status, r.body.as_str()), (200, r#"{"ok":true}"#));
    }

    let failed = bridge
        .handle(&request(&msg("hello", None, None).to_string()))
        .await
        .unwrap();
    assert_eq!(
        (failed.status, failed.body.as_str()),
        (502, r#"{"error":"Discord bot message failed"}"#)
    );

    for bad in ["not json", "null", &msg("{x", None, None).to_string()] {
        let r = bridge.handle(&request(bad)).await.unwrap();
        assert_eq!(
            (r.status, r.body.as_str()),
            (500, "Internal Server Error"),
            "{bad}"
        );
    }
    let unreachable = FeishuBridge::new("http://127.0.0.1:9", "c", "t");
    assert_eq!(
        unreachable
            .handle(&request(&msg("hi", None, None).to_string()))
            .await
            .unwrap()
            .status,
        500
    );
    assert!(
        bridge
            .handle(&HttpRequest {
                method: "GET".into(),
                path: "/feishu".into(),
                ..Default::default()
            })
            .await
            .is_none()
    );
}
