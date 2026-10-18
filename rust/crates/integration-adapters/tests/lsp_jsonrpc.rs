//! Purpose: fixture tests for the JSON-RPC transport: Content-Length framing, request/response
//! correlation, server requests (handled and -32601), notifications, close semantics.
//! Scope: in-memory duplex pipes, no processes.
//! Uses:
//! rust/crates/
//! `-- integration-adapters/src/lsp/jsonrpc.rs
//! External:
//! `-- tokio::io::duplex
//! Used by:
//! `-- none, entrypoint
//! Invariants: fixture-only (no live server).
//! Contract tests: integration-adapters/tests/lsp_jsonrpc.rs

use std::sync::{Arc, Mutex};
use std::time::Duration;

use opencode_integration_adapters::lsp::jsonrpc::{Connection, Handler, encode_frame, read_frame};
use opencode_integration_api::IntegrationError;
use serde_json::{Value, json};
use tokio::io::{AsyncWriteExt, BufReader};

#[derive(Default)]
struct Recorder {
    notes: Mutex<Vec<(String, Value)>>,
}

impl Handler for Recorder {
    fn request(&self, method: &str, _params: &Value) -> Option<Value> {
        (method == "known").then(|| json!({"ok": true}))
    }
    fn notification(&self, method: &str, params: &Value) {
        self.notes
            .lock()
            .unwrap()
            .push((method.to_string(), params.clone()));
    }
}

#[tokio::test]
async fn frame_roundtrip_handles_case_and_extra_headers() {
    let body = br#"{"a":"\u00e9"}"#;
    let mut raw =
        b"content-length: 14\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8\r\n\r\n"
            .to_vec();
    raw.extend_from_slice(body);
    raw.extend_from_slice(&encode_frame(b"{}"));
    let mut reader = BufReader::new(&raw[..]);
    assert_eq!(read_frame(&mut reader).await.unwrap().unwrap(), body);
    assert_eq!(read_frame(&mut reader).await.unwrap().unwrap(), b"{}");
    assert!(read_frame(&mut reader).await.unwrap().is_none());
    assert_eq!(
        encode_frame("é".as_bytes()),
        b"Content-Length: 2\r\n\r\n\xc3\xa9"
    );
}

#[tokio::test]
async fn request_response_and_server_traffic() {
    let (client_io, server_io) = tokio::io::duplex(1 << 16);
    let (cr, cw) = tokio::io::split(client_io);
    let (sr, mut sw) = tokio::io::split(server_io);
    let recorder = Arc::new(Recorder::default());
    let conn = Connection::start(BufReader::new(cr), cw, recorder.clone());
    let mut server_reader = BufReader::new(sr);

    let pending = tokio::spawn({
        let conn = conn.clone();
        async move {
            conn.request("textDocument/hover", json!({"x": 1}), None)
                .await
        }
    });
    let req: Value =
        serde_json::from_slice(&read_frame(&mut server_reader).await.unwrap().unwrap()).unwrap();
    assert_eq!(req["id"], 0);
    assert_eq!(req["method"], "textDocument/hover");

    // Server-initiated traffic before the response.
    for msg in [
        json!({"jsonrpc":"2.0","method":"note","params":{"n":1}}),
        json!({"jsonrpc":"2.0","id":"s1","method":"known","params":{}}),
        json!({"jsonrpc":"2.0","id":7,"method":"unknown/thing"}),
        json!({"jsonrpc":"2.0","id":0,"result":{"contents":"hi"}}),
    ] {
        sw.write_all(&encode_frame(msg.to_string().as_bytes()))
            .await
            .unwrap();
    }
    assert_eq!(pending.await.unwrap().unwrap(), json!({"contents":"hi"}));
    let r1: Value =
        serde_json::from_slice(&read_frame(&mut server_reader).await.unwrap().unwrap()).unwrap();
    assert_eq!(r1, json!({"jsonrpc":"2.0","id":"s1","result":{"ok":true}}));
    let r2: Value =
        serde_json::from_slice(&read_frame(&mut server_reader).await.unwrap().unwrap()).unwrap();
    assert_eq!(r2["id"], 7);
    assert_eq!(r2["error"]["code"], -32601);
    assert_eq!(recorder.notes.lock().unwrap()[0].0, "note");

    // Error response maps to Failed, timeout maps to Timeout.
    let err_req = tokio::spawn({
        let conn = conn.clone();
        async move { conn.request("bad", json!(null), None).await }
    });
    let req: Value =
        serde_json::from_slice(&read_frame(&mut server_reader).await.unwrap().unwrap()).unwrap();
    let reply = json!({"jsonrpc":"2.0","id":req["id"],"error":{"code":-32603,"message":"boom"}});
    sw.write_all(&encode_frame(reply.to_string().as_bytes()))
        .await
        .unwrap();
    assert!(matches!(
        err_req.await.unwrap(),
        Err(IntegrationError::Failed { .. })
    ));
    let timed = conn
        .request("slow", json!(null), Some(Duration::from_millis(30)))
        .await;
    assert_eq!(timed, Err(IntegrationError::Timeout { ms: 30 }));

    // Peer EOF resolves pending requests as Closed and close() joins the reader.
    let waiting = tokio::spawn({
        let conn = conn.clone();
        async move { conn.request("never", json!(null), None).await }
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    drop(sw);
    drop(server_reader);
    assert!(matches!(
        waiting.await.unwrap(),
        Err(IntegrationError::Closed(_))
    ));
    conn.close().await;
    assert!(matches!(
        conn.notify("x", json!(null)).await,
        Err(IntegrationError::Closed(_))
    ));
}
