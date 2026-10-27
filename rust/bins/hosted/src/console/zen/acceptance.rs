//! Purpose: accept public Zen inference routes through native adapters and real local MySQL.
//! Scope: fake provider, fake Upstash, SSE completion accounting and disconnect ownership.
//! Uses:
//! rust/bins/
//! |-- hosted/src/console/zen.rs
//! `-- hosted/src/console/zen/acceptance/h2.rs
//! rust/crates/
//! `-- hosted-adapters/src/console/db.rs
//! External:
//! |-- axum::Router
//! |-- mysql_async::prelude::Queryable
//! `-- tower::ServiceExt
//! Used by:
//! `-- none, entrypoint
//! Invariants: all network calls terminate locally, every spawned server is aborted and joined.
//! Contract tests: this file.

use super::*;
use axum::{
    Json, Router,
    body::{Body, Bytes, to_bytes},
    extract::Request,
    http::StatusCode,
    response::IntoResponse,
    routing::post,
};
use futures::{FutureExt, StreamExt};
use mysql_async::prelude::Queryable;
use opencode_hosted_adapters::console::{db::TestDatabase, limits::RedisCounters};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tower::ServiceExt;

mod h2;

#[tokio::test]
async fn fixture_migrated_routes_preserve_provider_status_usage_and_google_raw_body() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().fallback(axum::routing::any(|request: Request| async move {
                let path = request.uri().path().to_owned();
                assert_eq!(request.headers()["authorization"], "Bearer oc_sk_fixture");
                assert!(request.headers().get("cf-access-client-secret").is_none());
                let bytes = to_bytes(request.into_body(), usize::MAX).await.unwrap();
                if path == "/go/v1/usage" {
                    return (
                        StatusCode::OK,
                        [("content-type", "application/json"), ("retry-after", "9")],
                        "{\"usage\":{\"destination\":true}}".to_owned(),
                    );
                }
                if path == "/google/v1beta/models/gemini:generateContent" {
                    assert_eq!(bytes, "opaque-google-body");
                } else {
                    assert_eq!(path, "/openai/v1/chat/completions");
                    let body: Value = serde_json::from_slice(&bytes).unwrap();
                    assert_eq!(body["model"], "not-in-legacy-catalog");
                }
                (
                    StatusCode::TOO_MANY_REQUESTS,
                    [("content-type", "text/event-stream"), ("retry-after", "9")],
                    "data: destination-quota\n\n".to_owned(),
                )
            })),
        )
        .await
        .unwrap()
    });
    let config = super::super::state::ConsoleConfig {
        zen: Config {
            migration: Some(
                MigrationGateway::new(&format!("http://{address}"), "trusted").unwrap(),
            ),
            ..Default::default()
        },
        ..Default::default()
    };
    let app = routes().with_state(ConsoleState {
        config: Arc::new(config),
        db: opencode_hosted_adapters::console::db::ConsoleDb::connect(
            "mysql://unused:unused@127.0.0.1:1/unused",
        )
        .unwrap(),
    });
    for (path, header, body) in [
        (
            "/zen/v1/chat/completions",
            "authorization",
            json!({"model":"not-in-legacy-catalog","stream":true}).to_string(),
        ),
        (
            "/zen/v1/models/gemini:generateContent",
            "x-goog-api-key",
            "opaque-google-body".into(),
        ),
    ] {
        let response = app
            .clone()
            .oneshot(
                axum::http::Request::post(path)
                    .header(
                        header,
                        if header == "authorization" {
                            "Bearer oc_sk_fixture"
                        } else {
                            "oc_sk_fixture"
                        },
                    )
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response.headers()["retry-after"], "9");
        assert_eq!(
            to_bytes(response.into_body(), usize::MAX).await.unwrap(),
            "data: destination-quota\n\n"
        );
    }
    let response = app
        .oneshot(
            axum::http::Request::get("/zen/go/v1/usage")
                .header("authorization", "Bearer oc_sk_fixture")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(body, json!({"usage":{"destination":true}}));
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn fixture_native_disconnect_before_provider_headers_cancels_upstream_without_usage() {
    use tokio::io::AsyncWriteExt;
    let fixture = TestDatabase::create("zen_disconnect").await.unwrap();
    let mut conn = fixture.db.conn().await.unwrap();
    conn.query_drop("INSERT INTO workspace (id,name) VALUES ('wrk_disconnect','Disconnect')")
        .await
        .unwrap();
    conn.query_drop("INSERT INTO billing (id,workspace_id,balance,payment_method_id) VALUES ('bil_disconnect','wrk_disconnect',1000000000,'pm_local')").await.unwrap();
    conn.query_drop("INSERT INTO user (id,workspace_id,name,role) VALUES ('usr_disconnect','wrk_disconnect','Disconnect','admin')").await.unwrap();
    conn.query_drop("INSERT INTO `key` (id,workspace_id,user_id,name,`key`) VALUES ('key_disconnect','wrk_disconnect','usr_disconnect','Disconnect','sk_disconnect')").await.unwrap();
    let entered = Arc::new(tokio::sync::Notify::new());
    let ended = Arc::new(AtomicBool::new(false));
    let provider_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider_address = provider_listener.local_addr().unwrap();
    let started = entered.clone();
    let dropped = ended.clone();
    let provider = tokio::spawn(async move {
        crate::transport::serve(provider_listener,Router::new()
        .route("/pipeline",post(|Json(commands):Json<Vec<Value>>| async move { Json(commands.iter().map(|command| if command[0] == "MGET" { json!({"result":vec![Value::Null;command.as_array().unwrap().len()-1]}) } else { json!({"result":1}) }).collect::<Vec<_>>()) }))
        .route("/provider/chat/completions",post(move |request:Request| { let started = started.clone(); let dropped = dropped.clone(); async move {
            let _bytes = to_bytes(request.into_body(),usize::MAX).await.unwrap();
            let _guard = Ended(dropped); started.notify_one();
            futures::future::pending::<()>().await;
            StatusCode::OK
        }}))).await.unwrap()
    });
    let mut zen = Config::from_catalog(json!({"zenModels":{"public-model":{"cost":{"input":0.000001,"output":0.000002},"providers":[{"id":"local","model":"native"}]}},"liteModels":{},"providers":{"local":{"api":format!("http://{provider_address}/provider"),"apiKey":"local","format":"oa-compat"}}})).unwrap();
    zen.limits = Some(json!({"free":{"promoTokens":100,"dailyRequests":10}}));
    zen.redis = Some(
        RedisCounters::new(&format!("http://{provider_address}"), "local", "fixture").unwrap(),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let state = ConsoleState {
        db: fixture.db.clone(),
        config: Arc::new(super::super::state::ConsoleConfig {
            zen,
            ..Default::default()
        }),
    };
    let server = tokio::spawn(async move {
        crate::transport::serve(listener, routes().with_state(state))
            .await
            .unwrap()
    });
    let body = json!({"model":"public-model","stream":true,"messages":[]}).to_string();
    let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
    socket.write_all(format!("POST /zen/v1/chat/completions HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer sk_disconnect\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified())
        .await
        .unwrap();
    drop(socket);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !ended.load(Ordering::SeqCst) {
            tokio::task::yield_now().await
        }
    })
    .await
    .unwrap();
    let count: Option<i64> = conn
        .query_first("SELECT COUNT(*) FROM `usage`")
        .await
        .unwrap();
    let balance: Option<i64> = conn
        .query_first("SELECT balance FROM billing WHERE workspace_id='wrk_disconnect'")
        .await
        .unwrap();
    assert_eq!(count, Some(0));
    assert_eq!(balance, Some(1000000000));
    drop(conn);
    server.abort();
    let _ = server.await;
    provider.abort();
    let _ = provider.await;
    fixture.drop_database().await.unwrap();
}

struct Ended(Arc<AtomicBool>);
impl Drop for Ended {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn fixture_native_sse_records_usage_after_eof_and_disconnect_drops_provider() {
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    let fixture = TestDatabase::create("zen_native").await.unwrap();
    let mut conn = fixture.db.conn().await.unwrap();
    conn.query_drop("INSERT INTO workspace (id,name) VALUES ('wrk_native','Native')")
        .await
        .unwrap();
    conn.query_drop("INSERT INTO billing (id,workspace_id,balance,payment_method_id) VALUES ('bil_native','wrk_native',1000000000,'pm_local')").await.unwrap();
    conn.query_drop("INSERT INTO user (id,workspace_id,name,role) VALUES ('usr_native','wrk_native','Native','admin')").await.unwrap();
    conn.query_drop("INSERT INTO `key` (id,workspace_id,user_id,name,`key`) VALUES ('key_native','wrk_native','usr_native','Native','sk_native')").await.unwrap();
    drop(conn);
    let (captured, mut captured_rx) = tokio::sync::mpsc::channel::<Value>(4);
    let capture_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let capture_address = capture_listener.local_addr().unwrap();
    let capture_server = tokio::spawn(async move {
        axum::serve(
            capture_listener,
            Router::new().route(
                "/capture",
                post(move |Json(value): Json<Value>| {
                    let captured = captured.clone();
                    async move {
                        captured.send(value).await.unwrap();
                        StatusCode::ACCEPTED
                    }
                }),
            ),
        )
        .await
        .unwrap()
    });
    let tail_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let tail_address = tail_listener.local_addr().unwrap();
    let capture = opencode_hosted_adapters::console::workers::Capture::Http {
        client: reqwest::Client::new(),
        endpoint: url::Url::parse(&format!("http://{capture_address}/capture")).unwrap(),
        bearer: None,
    };
    let tail_server = tokio::spawn(async move {
        axum::serve(
            tail_listener,
            super::super::workers::tail_router("tail-secret".into(), capture, 1024 * 1024),
        )
        .await
        .unwrap()
    });
    let ended = Arc::new(AtomicBool::new(false));
    let dropped = ended.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, Router::new()
        .route("/pipeline",post(|Json(commands):Json<Vec<Value>>| async move { Json(commands.iter().map(|command| if command[0] == "MGET" { json!({"result":vec![Value::Null;command.as_array().unwrap().len()-1]}) } else { json!({"result":1}) }).collect::<Vec<_>>()) }))
        .route("/provider/chat/completions",post(move |request: Request| { let dropped = dropped.clone(); async move {
            assert_eq!(request.headers()["authorization"],"Bearer provider-local");
            assert_eq!(request.headers()["x-opencode-session"],"session-native");
            assert_eq!(request.headers()["x-zen-model"],"public-model");
            let bytes = to_bytes(request.into_body(),usize::MAX).await.unwrap();
            let body:Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(body["model"],"native-provider-model");
            assert_eq!(body["stream_options"]["include_usage"],true);
            let partial_usage = body["messages"][0]["content"] == "h2cancel";
            let endless = partial_usage || body["messages"][0]["content"] == "cancel";
            let guard = Ended(dropped);
            let stream = futures::stream::unfold((0,guard), move |(index,guard)| async move {
                if index == 0 {
                    let bytes = if partial_usage { Bytes::from_static(b"data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}],\"usage\":{\"prompt_tokens\":11,\"completion_tokens\":13}}\n\n") } else { Bytes::from_static(b"data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n") };
                    return Some((Ok::<_,std::io::Error>(bytes),(1,guard)));
                }
                if endless { futures::future::pending::<()>().await; }
                if index == 1 { return Some((Ok(Bytes::from_static(b"data: {\"choices\":[],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":2}}\n\ndata: [DONE]\n\n")),(2,guard))); }
                None
            });
            ([("content-type","text/event-stream"),("x-private-provider","must-not-leak"),("x-opencode-endpoint-id","endpoint-local"),("x-opencode-upstream-model-id","upstream-local")],Body::from_stream(stream)).into_response()
        }}))).await.unwrap()
    });
    let mut zen = Config::from_catalog(json!({"zenModels":{"public-model":{"name":"Public","cost":{"input":0.000001,"output":0.000002},"providers":[{"id":"console.local","model":"native-provider-model"}]}},"liteModels":{},"providers":{"console.local":{"api":format!("http://{address}/provider"),"apiKey":"provider-local","format":"oa-compat"}}})).unwrap();
    zen.limits = Some(
        json!({"free":{"promoTokens":100,"dailyRequests":10},"lite":{"rollingLimit":1,"rollingWindow":5,"weeklyLimit":2,"monthlyLimit":3}}),
    );
    zen.redis =
        Some(RedisCounters::new(&format!("http://{address}"), "local-token", "fixture").unwrap());
    zen.tail = Some(
        super::super::workers::producer::TailEmitter::with_token(
            &format!("http://{tail_address}/tail"),
            "tail-secret",
        )
        .unwrap(),
    );
    let state = ConsoleState {
        db: fixture.db.clone(),
        config: Arc::new(super::super::state::ConsoleConfig {
            zen,
            ..Default::default()
        }),
    };
    let app = routes().with_state(state);
    let request = |content: &str| {
        axum::http::Request::post("/zen/v1/chat/completions").header("authorization","Bearer sk_native").header("content-type","application/json").header("x-opencode-session","session-native").body(Body::from(json!({"model":"public-model","stream":true,"messages":[{"role":"user","content":content}]}).to_string())).unwrap()
    };
    let response = app.clone().oneshot(request("complete")).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().get("x-private-provider").is_none());
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(text.contains("hello"));
    assert!(text.ends_with("data: {\"choices\":[],\"cost\":\"0.00000700\"}\n\n"));
    let mut conn = fixture.db.conn().await.unwrap();
    let count: Option<i64> = conn
        .query_first("SELECT COUNT(*) FROM `usage`")
        .await
        .unwrap();
    assert_eq!(count, Some(1));
    let balance: Option<i64> = conn
        .query_first("SELECT balance FROM billing WHERE workspace_id='wrk_native'")
        .await
        .unwrap();
    assert_eq!(balance, Some(999999300));
    let trace = tokio::time::timeout(std::time::Duration::from_secs(3), captured_rx.recv())
        .await
        .unwrap()
        .unwrap();
    let metric = &trace["console"][0];
    assert_eq!(metric["model"], "public-model");
    assert_eq!(metric["provider"], "endpoint-local");
    assert_eq!(metric["provider.model"], "upstream-local");
    assert_eq!(metric["shallowProvider"], "console.local");
    assert_eq!(metric["shallowProvider.model"], "native-provider-model");
    assert_eq!(metric["source"], "balance");
    assert_eq!(metric["workspace"], "wrk_native");
    assert_eq!(metric["tokens.input"], 3);
    assert_eq!(metric["tokens.output"], 2);
    assert_eq!(metric["cost.total.microcents"], 700);
    assert_eq!(metric["cost.input.microcents"], 300);
    assert_eq!(metric["cost.output.microcents"], 400);
    assert_eq!(metric["cost.input"], 0);
    assert_eq!(metric["cost.output"], 0);
    assert!(metric.get("rawCents").is_none());
    assert!(metric.get("cost.cache_read.microcents").is_none());
    assert_eq!(metric["status"], 200);
    assert!(metric["timestamp.first_byte"].as_i64().unwrap() > 0);
    assert!(!trace.to_string().contains("provider-local"));
    assert!(!trace.to_string().contains("sk_native"));
    assert!(!trace.to_string().contains("tail-secret"));
    ended.store(false, Ordering::SeqCst);
    let response = app.clone().oneshot(request("cancel")).await.unwrap();
    let mut body = response.into_body().into_data_stream();
    assert!(body.next().await.unwrap().unwrap().starts_with(b"data:"));
    drop(body);
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while !ended.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let count: Option<i64> = conn
        .query_first("SELECT COUNT(*) FROM `usage`")
        .await
        .unwrap();
    assert_eq!(
        count,
        Some(1),
        "cancelled streams must not record complete usage"
    );
    drop(conn);
    let h2_result = std::panic::AssertUnwindSafe(h2::cancel_and_complete(
        app,
        &fixture.db,
        ended,
        &mut captured_rx,
    ))
    .catch_unwind()
    .await;
    server.abort();
    let _ = server.await;
    tail_server.abort();
    let _ = tail_server.await;
    capture_server.abort();
    let _ = capture_server.await;
    fixture.drop_database().await.unwrap();
    if let Err(panic) = h2_result {
        std::panic::resume_unwind(panic);
    }
}
