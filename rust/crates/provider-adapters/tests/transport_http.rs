//! Purpose: contract tests for the `http-json` transport: request shape on the wire, SSE and
//! AWS event-stream framing over real HTTP, no retry after a 2xx body starts, body read failures,
//! and cancellation by drop.
//! Scope: fixture-based (local wiremock server and raw TCP fixtures), no live provider calls.
//! Uses:
//! rust/crates/
//! |-- provider-adapters/src/protocol.rs
//! |-- provider-adapters/src/route.rs
//! |-- provider-adapters/src/transport/event_stream.rs
//! `-- provider-adapters/src/transport/http.rs
//! External:
//! |-- tokio::net::TcpListener
//! `-- wiremock::{Mock, MockServer, ResponseTemplate}
//! Used by:
//! `-- none, entrypoint
//! Invariants: mirrors packages/llm/test/executor.test.ts "does not retry after a successful
//! response reaches stream parsing" and route/transport/http.ts read-error mapping.
//! Contract tests: provider-adapters/tests/transport_http.rs

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use opencode_llm_api::{LlmError, LlmErrorReason, LlmEvent, LlmRequest, Model, RouteClient};
use opencode_provider_adapters::protocol::{Frame, Protocol, ProtocolState};
use opencode_provider_adapters::route::{Endpoint, HttpRequestParts, ProtocolRoute, Transport};
use opencode_provider_adapters::transport::event_stream::encode_frame;
use opencode_provider_adapters::transport::{Framing, HttpExecutor, HttpJsonTransport};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use wiremock::matchers::{body_string, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn parts(url: String) -> HttpRequestParts {
    HttpRequestParts {
        method: "POST".into(),
        url,
        headers: [
            ("x-route".to_string(), "r".to_string()),
            ("authorization".to_string(), "Bearer sk-test".to_string()),
            ("content-type".to_string(), "application/json".to_string()),
        ]
        .into_iter()
        .collect(),
        body: r#"{"model":"m","stream":true}"#.into(),
        json_body: json!({"model": "m", "stream": true}),
    }
}

async fn collect(transport: &HttpJsonTransport, url: String) -> Vec<Result<Frame, LlmError>> {
    transport
        .frames(parts(url), "prov/route".into())
        .collect()
        .await
}

#[tokio::test]
async fn sends_prepared_request_and_frames_sse() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat"))
        .and(query_param("api-version", "1"))
        .and(header("authorization", "Bearer sk-test"))
        .and(header("x-route", "r"))
        .and(header("content-type", "application/json"))
        .and(body_string(r#"{"model":"m","stream":true}"#))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string("data: {\"a\":1}\n\n: keepalive\n\ndata: \n\ndata: {\"b\":2}\r\n\r\ndata: [DONE]\n\n"),
        )
        .expect(1)
        .mount(&server)
        .await;
    let transport = HttpJsonTransport::sse(HttpExecutor::new().unwrap());
    assert_eq!(transport.id(), "http-json");
    let frames = collect(
        &transport,
        format!("{}/v1/chat?api-version=1", server.uri()),
    )
    .await;
    let frames: Vec<Frame> = frames.into_iter().map(Result::unwrap).collect();
    assert_eq!(
        frames,
        [
            Frame::Text(r#"{"a":1}"#.into()),
            Frame::Text(r#"{"b":2}"#.into())
        ]
    );
}

#[tokio::test]
async fn sse_retry_field_ends_stream() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string("data: a\n\nretry: 100\n\ndata: b\n\n"),
        )
        .mount(&server)
        .await;
    let transport = HttpJsonTransport::sse(HttpExecutor::new().unwrap());
    let frames = collect(&transport, format!("{}/x", server.uri())).await;
    // The retry directive and the event after it arrive in the same chunk; the baseline emits
    // the chunk's buffered events then ends.
    let texts: Vec<String> = frames.into_iter().map(|f| f.unwrap().raw()).collect();
    assert_eq!(texts.first().map(String::as_str), Some("a"));
    assert!(texts.len() <= 2);
}

#[tokio::test]
async fn aws_event_stream_framing_filters_and_projects() {
    let mut body = Vec::new();
    body.extend(encode_frame(
        &[
            (":message-type", "event"),
            (":event-type", "messageStart"),
            (":content-type", "application/json"),
        ],
        br#"{"role":"assistant","p":"abcdefgh"}"#,
    ));
    body.extend(encode_frame(
        &[
            (":message-type", "exception"),
            (":exception-type", "throttlingException"),
        ],
        br#"{"message":"slow"}"#,
    ));
    body.extend(encode_frame(&[(":message-type", "event")], br#"{"x":1}"#));
    body.extend(encode_frame(
        &[(":message-type", "event"), (":event-type", "ping")],
        b"",
    ));
    body.extend(encode_frame(
        &[
            (":event-type", "contentBlockDelta"),
            (":message-type", "event"),
        ],
        br#"{"delta":{"text":"hi"},"contentBlockIndex":0}"#,
    ));
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/vnd.amazon.eventstream")
                .set_body_bytes(body),
        )
        .mount(&server)
        .await;
    let transport = HttpJsonTransport::sse(HttpExecutor::new().unwrap())
        .with_framing(Framing::aws_event_stream("bedrock-converse"));
    let frames: Vec<Frame> = collect(
        &transport,
        format!("{}/model/m/converse-stream", server.uri()),
    )
    .await
    .into_iter()
    .map(Result::unwrap)
    .collect();
    assert_eq!(
        frames,
        [
            Frame::Json(json!({"messageStart": {"role": "assistant"}})),
            Frame::Json(
                json!({"contentBlockDelta": {"delta": {"text": "hi"}, "contentBlockIndex": 0}})
            ),
        ]
    );
}

#[tokio::test]
async fn aws_event_stream_crc_failure_is_invalid_output() {
    let mut frame = encode_frame(&[(":message-type", "event"), (":event-type", "x")], b"{}");
    let last = frame.len() - 1;
    frame[last] ^= 0xff;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(frame))
        .mount(&server)
        .await;
    let transport = HttpJsonTransport::new(
        HttpExecutor::new().unwrap(),
        Framing::aws_event_stream("bedrock-converse"),
    );
    let frames = collect(&transport, format!("{}/x", server.uri())).await;
    let error = frames.into_iter().find_map(Result::err).expect("error");
    let LlmErrorReason::InvalidProviderOutput { route, message, .. } = error.reason.as_ref() else {
        panic!("{error:?}");
    };
    assert_eq!(route.as_deref(), Some("bedrock-converse"));
    assert!(
        message.starts_with(
            "Failed to decode Bedrock Converse event-stream frame: The message checksum"
        ),
        "{message}"
    );
}

#[tokio::test]
async fn status_error_surfaces_from_frames() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(401).set_body_string("bad key"))
        .mount(&server)
        .await;
    let transport = HttpJsonTransport::sse(HttpExecutor::new().unwrap());
    let frames = collect(&transport, format!("{}/x", server.uri())).await;
    assert_eq!(frames.len(), 1);
    let error = frames.into_iter().next().unwrap().unwrap_err();
    assert_eq!(error.reason.tag(), "Authentication");
}

/// Minimal JSON protocol: each frame must parse as JSON and becomes one text delta.
#[derive(Debug)]
struct JsonEcho;

struct JsonEchoState;

impl ProtocolState for JsonEchoState {
    fn step(&mut self, route: &str, frame: Frame) -> Result<Vec<LlmEvent>, LlmError> {
        let value: Value = frame.json(route, "Invalid echo chunk")?;
        Ok(vec![LlmEvent::TextDelta {
            id: "t".into(),
            text: value["text"].as_str().unwrap_or_default().to_owned(),
            provider_metadata: None,
        }])
    }
}

impl Protocol for JsonEcho {
    fn id(&self) -> &'static str {
        "json-echo"
    }

    fn body(&self, request: &LlmRequest) -> Result<Value, LlmError> {
        Ok(json!({"model": request.model.id.as_str(), "stream": true}))
    }

    fn initial(&self, _request: &LlmRequest) -> Box<dyn ProtocolState> {
        Box::new(JsonEchoState)
    }
}

#[tokio::test]
async fn does_not_retry_after_successful_response_reaches_stream_parsing() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string("data: {\"text\":\"Hello\"}\n\ndata: not-json\n\n"),
        )
        .mount(&server)
        .await;
    let route = Arc::new(ProtocolRoute::new(
        "json-echo",
        Arc::new(JsonEcho),
        Endpoint::new(Some(&server.uri()), "/v1/chat"),
        Arc::new(HttpJsonTransport::sse(HttpExecutor::new().unwrap())),
    ));
    let request = LlmRequest::new(Model::new("m1", "prov", route));
    let error = opencode_llm_api::generate(&RouteClient, request)
        .await
        .unwrap_err();
    let LlmErrorReason::InvalidProviderOutput { route, raw, .. } = error.reason.as_ref() else {
        panic!("{error:?}");
    };
    assert_eq!(route.as_deref(), Some("prov/json-echo"));
    assert_eq!(raw.as_deref(), Some("not-json"));
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

/// Raw HTTP/1.1 server: writes `response` after the request head, then (when `hold_open`)
/// reports whether the client hung up within 5s (`usize::MAX` = it did not).
async fn raw_server(
    response: &'static [u8],
    hold_open: bool,
) -> (String, tokio::sync::oneshot::Receiver<usize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 8192];
        let mut seen = Vec::new();
        while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = socket.read(&mut buf).await.unwrap();
            seen.extend_from_slice(&buf[..n]);
        }
        socket.write_all(response).await.unwrap();
        socket.flush().await.unwrap();
        if !hold_open {
            return;
        }
        // Wait for the client to hang up; report how many extra bytes it sent first.
        let mut extra = 0;
        loop {
            match tokio::time::timeout(Duration::from_secs(5), socket.read(&mut buf)).await {
                Ok(Ok(0)) | Ok(Err(_)) => break,
                Ok(Ok(n)) => extra += n,
                Err(_) => {
                    extra = usize::MAX;
                    break;
                }
            }
        }
        let _ = tx.send(extra);
    });
    (format!("http://{addr}/x"), rx)
}

#[tokio::test]
async fn body_read_failure_maps_to_failed_to_read_stream() {
    let (url, _rx) = raw_server(
        b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: 1000\r\n\r\ndata: {\"a\":1}\n\n",
        false,
    )
    .await;
    let transport = HttpJsonTransport::sse(HttpExecutor::new().unwrap());
    let frames = collect(&transport, url).await;
    assert_eq!(frames.len(), 2, "{frames:?}");
    assert_eq!(
        frames[0].as_ref().unwrap(),
        &Frame::Text(r#"{"a":1}"#.into())
    );
    let error = frames[1].as_ref().unwrap_err();
    let LlmErrorReason::InvalidProviderOutput { route, message, .. } = error.reason.as_ref() else {
        panic!("{error:?}");
    };
    assert_eq!(route.as_deref(), Some("prov/route"));
    assert_eq!(message, "Failed to read prov/route stream");
}

#[tokio::test]
async fn dropping_the_stream_closes_the_connection() {
    let (url, rx) = raw_server(
        b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n10\r\ndata: {\"a\":1}\n\n\r\n",
        true,
    )
    .await;
    let transport = HttpJsonTransport::sse(HttpExecutor::new().unwrap());
    let mut stream = transport.frames(parts(url), "prov/route".into());
    let first = stream.next().await.unwrap().unwrap();
    assert_eq!(first, Frame::Text(r#"{"a":1}"#.into()));
    drop(stream);
    let extra = tokio::time::timeout(Duration::from_secs(6), rx)
        .await
        .expect("server observed hangup")
        .unwrap();
    assert_ne!(extra, usize::MAX, "connection stayed open after drop");
}

#[tokio::test]
async fn sse_event_over_limit_fails() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(format!("data: {}\n\n", "x".repeat(64))),
        )
        .mount(&server)
        .await;
    let transport = HttpJsonTransport::new(
        HttpExecutor::new().unwrap(),
        Framing::Sse {
            max_event_bytes: 16,
        },
    );
    let frames = collect(&transport, format!("{}/x", server.uri())).await;
    let error = frames.into_iter().find_map(Result::err).expect("error");
    assert_eq!(error.reason.message(), "Failed to read prov/route stream");
}
