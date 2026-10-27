//! Purpose: prove native HTTP/2 response cancellation without closing the client connection.
//! Scope: existing native fixture, one-accept TCP relay, partial usage and follow-up settlement.
//! Uses:
//! rust/bins/
//! |-- hosted/src/console/zen/acceptance.rs
//! `-- hosted/src/transport.rs
//! rust/crates/
//! `-- hosted-adapters/src/console/db.rs
//! External:
//! |-- futures::StreamExt
//! |-- mysql_async::prelude::Queryable
//! |-- reqwest::Client
//! `-- tokio::io::copy_bidirectional
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/zen/acceptance.rs
//! Invariants: only one downstream TCP connection is accepted; cancellation never settles partial usage.
//! Contract tests: parent fixture_native_sse_records_usage_after_eof_and_disconnect_drops_provider.

use axum::Router;
use futures::{FutureExt, StreamExt};
use mysql_async::prelude::Queryable;
use opencode_hosted_adapters::console::db::ConsoleDb;
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;

#[test]
fn fixture_http2_absolute_and_http1_relative_urls_preserve_enriched_metrics() {
    use opencode_hosted_adapters::console::{
        formats::UsageInfo, gateway::usage::UsageRecord, workers::logs,
    };
    let path = "/zen/v1/chat/completions?fixture=once%2Fonly";
    for scheme in ["http", "https"] {
        let expected = format!("{scheme}://console.example.test{path}");
        for absolute in [false, true] {
            if !absolute && scheme == "https" {
                continue;
            }
            let mut request = axum::http::Request::post(if absolute { &expected } else { path })
                .header("authorization", "Bearer caller-secret")
                .header("x-api-key", "provider-secret")
                .body(())
                .unwrap();
            if !absolute {
                request
                    .headers_mut()
                    .insert("host", "console.example.test".parse().unwrap());
            }
            let (parts, ()) = request.into_parts();
            let mut trace = super::super::metrics::GatewayTrace::new(&parts, None);
            assert_eq!(trace.metric["request"]["url"], expected);
            trace.metric["provider"] = json!("endpoint-local");
            trace.metric["provider.model"] = json!("upstream-local");
            trace.metric["shallowProvider"] = json!("console.local");
            let now = time::OffsetDateTime::now_utc();
            trace
                .usage(
                    &json!({"cost":{"input":0.000001,"output":0.000002}}),
                    &UsageInfo {
                        input_tokens: 3,
                        output_tokens: 2,
                        reasoning_tokens: None,
                        cache_read_tokens: None,
                        cache_write5m_tokens: None,
                        cache_write1h_tokens: None,
                    },
                    &UsageRecord {
                        occurred_cost: "0.00000700".into(),
                        cost_in_microcents: 700,
                        reload_claimed: false,
                        raw_cost_in_cents: 0.0007,
                        tracked_at: time::PrimitiveDateTime::new(now.date(), now.time()),
                    },
                )
                .unwrap();
            let request = trace
                .metric
                .as_object_mut()
                .unwrap()
                .remove("request")
                .unwrap();
            let envelope = json!({"event":{"request":request,"response":{"status":200}},"eventTimestamp":now.unix_timestamp()*1000,"logs":[{"message":[format!("_metric:{}",trace.metric)]}]});
            let enriched = logs::enrich(&[envelope], now).unwrap();
            assert_eq!(enriched["console"].as_array().unwrap().len(), 1);
            let metric = &enriched["console"][0];
            assert_eq!(metric["tokens.input"], 3);
            assert_eq!(metric["tokens.output"], 2);
            assert_eq!(metric["cost.input.microcents"], 300);
            assert_eq!(metric["cost.output.microcents"], 400);
            assert_eq!(metric["cost.total.microcents"], 700);
            assert_eq!(metric["provider"], "endpoint-local");
            assert_eq!(metric["provider.model"], "upstream-local");
            assert_eq!(metric["shallowProvider"], "console.local");
            assert!(!enriched.to_string().contains("caller-secret"));
            assert!(!enriched.to_string().contains("provider-secret"));
        }
    }
}

#[test]
fn fixture_http2_uri_credentials_ipv6_ports_and_encoded_query_are_safe() {
    let path = "/zen/v1/chat/completions?once=a%2Fb%40c&literal=%252F";
    for (uri, host, expected) in [
        (
            format!("https://uri-user:uri-secret@console.example.test:8443{path}"),
            "ignored.example.test",
            format!("https://console.example.test:8443{path}"),
        ),
        (
            format!("http://uri-user:uri-secret@[::1]:8787{path}"),
            "ignored.example.test",
            format!("http://[::1]:8787{path}"),
        ),
        (
            path.into(),
            "host-user:host-secret@[::1]:8787",
            format!("http://[::1]:8787{path}"),
        ),
    ] {
        let (parts, ()) = axum::http::Request::post(uri)
            .header("host", host)
            .body(())
            .unwrap()
            .into_parts();
        let trace = super::super::metrics::GatewayTrace::new(&parts, None);
        assert_eq!(trace.metric["request"]["url"], expected);
        let url = url::Url::parse(trace.metric["request"]["url"].as_str().unwrap()).unwrap();
        assert_eq!(url.path(), "/zen/v1/chat/completions");
        assert_eq!(url.query(), Some("once=a%2Fb%40c&literal=%252F"));
        assert!(url.username().is_empty());
        assert!(url.password().is_none());
        assert!(!trace.metric.to_string().contains("uri-secret"));
        assert!(!trace.metric.to_string().contains("host-secret"));
    }
}

pub(super) async fn cancel_and_complete(
    app: Router,
    db: &ConsoleDb,
    ended: Arc<AtomicBool>,
    captured: &mut tokio::sync::mpsc::Receiver<Value>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        crate::transport::serve(listener, app).await.unwrap();
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay_address = listener.local_addr().unwrap();
    let relay = tokio::spawn(async move {
        let (mut downstream, _) = listener.accept().await.unwrap();
        // Closing the listener makes a pooled reconnect fail instead of hiding a lost connection.
        drop(listener);
        let mut upstream = TcpStream::connect(address).await.unwrap();
        tokio::io::copy_bidirectional(&mut downstream, &mut upstream).await
    });
    let result = std::panic::AssertUnwindSafe(async {
    let client = reqwest::Client::builder()
        .no_proxy()
        .http2_prior_knowledge()
        .build()
        .unwrap();
    let request = |content: &str| {
        client
            .post(format!("http://{relay_address}/zen/v1/chat/completions"))
            .header("authorization", "Bearer sk_native")
            .header("x-opencode-session", "session-native")
            .json(&json!({"model":"public-model","stream":true,"messages":[{"role":"user","content":content}]}))
    };
    ended.store(false, Ordering::SeqCst);
    let response = timeout(Duration::from_secs(5), request("h2cancel").send())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.version(), reqwest::Version::HTTP_2);
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let mut stream = response.bytes_stream();
    let partial = timeout(Duration::from_secs(5), async {
        let mut bytes = Vec::new();
        while !bytes.windows(2).any(|pair| pair == b"\n\n") {
            bytes.extend_from_slice(&stream.next().await.unwrap().unwrap());
        }
        bytes
    })
    .await
    .unwrap();
    let event = std::str::from_utf8(&partial)
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("data: "))
        .unwrap();
    let event: Value = serde_json::from_str(event).unwrap();
    assert_eq!(event["usage"]["prompt_tokens"], 11);
    assert_eq!(event["usage"]["completion_tokens"], 13);
    // Drop only this HTTP/2 response, leaving the client and its sole TCP connection alive.
    drop(stream);
    timeout(Duration::from_secs(5), async {
        while !ended.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        !relay.is_finished(),
        "stream reset must not close the TCP relay"
    );
    let mut conn = db.conn().await.unwrap();
    let count: Option<i64> = conn
        .query_first("SELECT COUNT(*) FROM `usage`")
        .await
        .unwrap();
    let balance: Option<i64> = conn
        .query_first("SELECT balance FROM billing WHERE workspace_id='wrk_native'")
        .await
        .unwrap();
    assert_eq!(count, Some(1), "partial usage must not produce settlement");
    assert_eq!(
        balance,
        Some(999999300),
        "partial usage must not debit balance"
    );
    assert!(
        timeout(Duration::from_millis(150), captured.recv())
            .await
            .is_err(),
        "cancelled partial usage must not emit a completion tail"
    );
    let response = timeout(Duration::from_secs(5), request("complete").send())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.version(), reqwest::Version::HTTP_2);
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let bytes = timeout(Duration::from_secs(5), response.bytes())
        .await
        .unwrap()
        .unwrap();
    assert!(bytes.ends_with(b"data: {\"choices\":[],\"cost\":\"0.00000700\"}\n\n"));
    assert!(
        !relay.is_finished(),
        "follow-up must reuse the still-open TCP connection"
    );
    let count: Option<i64> = conn
        .query_first("SELECT COUNT(*) FROM `usage`")
        .await
        .unwrap();
    let balance: Option<i64> = conn
        .query_first("SELECT balance FROM billing WHERE workspace_id='wrk_native'")
        .await
        .unwrap();
    assert_eq!(count, Some(2));
    assert_eq!(balance, Some(999998600));
    let trace = timeout(Duration::from_secs(3), captured.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(trace["console"][0]["tokens.input"], 3);
    assert_eq!(trace["console"][0]["tokens.output"], 2);
    assert_eq!(trace["console"][0]["cost.total.microcents"], 700);
    assert!(
        timeout(Duration::from_millis(150), captured.recv())
            .await
            .is_err(),
        "only the EOF-completed follow-up may emit a completion tail"
    );
    drop(conn);
    drop(client);
    }).catch_unwind().await;
    relay.abort();
    let _ = relay.await;
    server.abort();
    let _ = server.await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
