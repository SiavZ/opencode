//! Purpose: replay every enterprise API exchange captured from the baseline Hono route
//! (crates/hosted-adapters/tests/fixtures/sharing/http.json) through the Rust router, plus public
//! share API route checks.
//! Scope: fixture. In-memory object store, tower oneshot, no network.
//! Uses:
//! rust/bins/
//! |-- hosted/src/sharing/enterprise.rs
//! |-- hosted/src/sharing/mod.rs
//! `-- hosted/src/sharing/public.rs
//! rust/crates/
//! |-- hosted-adapters/src/sharing/objects.rs
//! `-- hosted-adapters/src/sharing/public.rs
//! External:
//! |-- async_trait::async_trait
//! |-- axum::Router
//! |-- axum::body::Body
//! |-- axum::body::to_bytes
//! |-- axum::http::Request
//! |-- serde_json::Value
//! |-- serde_json::from_str
//! |-- serde_json::json
//! |-- tokio::test
//! |-- tower::ServiceExt
//! `-- usize::MAX
//! Used by:
//! rust/bins/
//! `-- hosted/src/sharing/mod.rs
//! Invariants: secrets returned by create are substituted for `<secret>` in later requests and
//! replaced by `<secret>` in responses, exactly like the capture script.
//! Contract tests: this file

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::body::{Body, to_bytes};
use axum::http::Request;
use opencode_hosted_adapters::sharing::objects::{ListOptions, ObjectStore, StoreError};
use opencode_hosted_adapters::sharing::public::SyncNamespace;
use serde_json::Value;
use tower::ServiceExt;

use crate::sharing::enterprise::EnterpriseState;
use crate::sharing::public::PublicState;
use crate::sharing::{enterprise_router, public_router};

#[derive(Default)]
struct Memory(Mutex<BTreeMap<String, String>>);

#[async_trait]
impl ObjectStore for Memory {
    async fn read(&self, path: &str) -> Result<Option<String>, StoreError> {
        Ok(self.0.lock().expect("lock").get(path).cloned())
    }
    async fn write(&self, path: &str, value: String) -> Result<(), StoreError> {
        self.0.lock().expect("lock").insert(path.into(), value);
        Ok(())
    }
    async fn remove(&self, path: &str) -> Result<(), StoreError> {
        self.0.lock().expect("lock").remove(path);
        Ok(())
    }
    async fn list(&self, options: ListOptions) -> Result<Vec<String>, StoreError> {
        Ok(self
            .0
            .lock()
            .expect("lock")
            .keys()
            .filter(|k| k.starts_with(&options.prefix))
            .cloned()
            .collect())
    }
}

fn fixture() -> Vec<Value> {
    let text = include_str!("../../../../crates/hosted-adapters/tests/fixtures/sharing/http.json");
    serde_json::from_str(text).expect("http fixture")
}

#[tokio::test]
async fn enterprise_routes_match_baseline_exchanges() {
    let store: Arc<dyn ObjectStore> = Arc::new(Memory::default());
    let app = enterprise_router(EnterpriseState {
        store,
        node_env_test: false,
        support_api_key: "support-key".into(),
    });
    let mut secret = String::new();
    for exchange in fixture() {
        let method = exchange["method"].as_str().expect("method");
        let path = exchange["path"].as_str().expect("path");
        let mut builder = Request::builder().method(method).uri(path);
        for (k, v) in exchange["requestHeaders"].as_object().expect("headers") {
            builder = builder.header(k, v.as_str().expect("header"));
        }
        let body = match &exchange["request"] {
            Value::Null => Body::empty(),
            Value::String(s) if s == "no content-type" => {
                Body::from(r#"{"sessionID":"ses_plain0001"}"#)
            }
            Value::String(s) => {
                builder = builder.header("content-type", "application/json");
                Body::from(s.clone())
            }
            other => {
                builder = builder.header("content-type", "application/json");
                Body::from(other.to_string().replace("<secret>", &secret))
            }
        };
        if method == "OPTIONS" {
            builder = builder
                .header("origin", "https://a.example")
                .header("access-control-request-method", "POST")
                .header("access-control-request-headers", "content-type,x-custom");
        }
        let response = app
            .clone()
            .oneshot(builder.body(body).expect("request"))
            .await
            .expect("response");
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let mut text = String::from_utf8(bytes.to_vec()).expect("utf8");
        if secret.is_empty() && path == "/api/share" && status == 200 {
            let parsed: Value = serde_json::from_str(&text).expect("json");
            secret = parsed["secret"].as_str().expect("secret").to_string();
        }
        if !secret.is_empty() {
            text = text.replace(&secret, "<secret>");
        }
        // The no-content-type create succeeds with a fresh secret the capture redacted by regex.
        if exchange["request"] == "no content-type" && status == 200 {
            let mut parsed: Value = serde_json::from_str(&text).expect("json");
            parsed["secret"] = Value::String("<secret>".into());
            text = parsed.to_string();
        }
        let label = format!("{method} {path} {}", exchange["request"]);
        assert_eq!(
            status,
            exchange["status"].as_u64().expect("status") as u16,
            "{label}"
        );
        assert_eq!(text, exchange["body"].as_str().expect("body"), "{label}");
        for (k, v) in exchange["headers"].as_object().expect("headers") {
            assert_eq!(
                headers.get(k).and_then(|h| h.to_str().ok()),
                v.as_str(),
                "{label} header {k}"
            );
        }
        for k in ["content-type", "cache-control"] {
            if exchange["headers"].get(k).is_none() {
                assert!(headers.get(k).is_none(), "{label} unexpected header {k}");
            }
        }
    }
}

fn public_app() -> (axum::Router, Arc<Memory>) {
    let bucket = Arc::new(Memory::default());
    let state = PublicState {
        namespace: SyncNamespace::new(bucket.clone()),
        web_domain: "opencode.ai".into(),
        admin_secret: "admin".into(),
    };
    (public_router(state), bucket)
}

async fn call(app: &axum::Router, method: &str, uri: &str, body: Option<Value>) -> (u16, String) {
    let request = Request::builder().method(method).uri(uri);
    let request = match body {
        Some(b) => request
            .header("content-type", "application/json")
            .body(Body::from(b.to_string())),
        None => request.body(Body::empty()),
    };
    let response = app
        .clone()
        .oneshot(request.expect("request"))
        .await
        .expect("response");
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    (status, String::from_utf8(bytes.to_vec()).expect("utf8"))
}

#[tokio::test]
async fn public_share_flow() {
    let (app, bucket) = public_app();
    let sid = "ses_0123456789abcdefGHIJ";
    let (status, body) = call(
        &app,
        "POST",
        "/share_create",
        Some(serde_json::json!({ "sessionID": sid })),
    )
    .await;
    assert_eq!(status, 200);
    let created: Value = serde_json::from_str(&body).expect("json");
    assert_eq!(created["url"], "https://opencode.ai/s/cdefGHIJ");
    let secret = created["secret"].as_str().expect("secret").to_string();
    let (_, again) = call(
        &app,
        "POST",
        "/share_create",
        Some(serde_json::json!({ "sessionID": sid })),
    )
    .await;
    assert_eq!(
        serde_json::from_str::<Value>(&again).expect("json")["secret"],
        secret.as_str()
    );

    let publish = |key: &str, content: Value| serde_json::json!({ "sessionID": sid, "secret": secret, "key": key, "content": content });
    for (key, content) in [
        (
            format!("session/info/{sid}"),
            serde_json::json!({ "id": sid }),
        ),
        (
            format!("session/message/{sid}/msg_1"),
            serde_json::json!({ "id": "msg_1" }),
        ),
        (
            format!("session/part/{sid}/msg_1/prt_1"),
            serde_json::json!({ "id": "prt_1", "messageID": "msg_1" }),
        ),
        (
            "session/info/other".to_string(),
            serde_json::json!({ "bad": true }),
        ),
    ] {
        assert_eq!(
            call(&app, "POST", "/share_sync", Some(publish(&key, content))).await,
            (200, "{}".to_string())
        );
    }
    let bad = serde_json::json!({ "sessionID": sid, "secret": "nope", "key": "k", "content": 1 });
    assert_eq!(call(&app, "POST", "/share_sync", Some(bad)).await.0, 500);
    let (status, data) = call(&app, "GET", "/share_data?id=cdefGHIJ", None).await;
    assert_eq!(status, 200);
    assert_eq!(
        data,
        r#"{"info":{"id":"ses_0123456789abcdefGHIJ"},"messages":{"msg_1":{"parts":[{"id":"prt_1","messageID":"msg_1"}],"id":"msg_1"}}}"#
    );
    assert!(
        bucket
            .0
            .lock()
            .expect("lock")
            .contains_key(&format!("share/session/info/{sid}.json"))
    );
    assert_eq!(
        call(&app, "GET", "/share_data", None).await,
        (400, "Error: Share ID is required".to_string())
    );
    assert_eq!(
        call(&app, "GET", "/share_poll?id=x", None).await,
        (426, "Error: Upgrade header is required".to_string())
    );

    let admin = serde_json::json!({ "sessionShortName": "cdefGHIJ", "adminSecret": "wrong" });
    assert_eq!(
        call(&app, "POST", "/share_delete_admin", Some(admin))
            .await
            .0,
        500
    );
    let delete = serde_json::json!({ "sessionID": sid, "secret": secret });
    assert_eq!(
        call(&app, "POST", "/share_delete", Some(delete)).await,
        (200, "{}".to_string())
    );
    assert_eq!(
        call(&app, "GET", "/share_data?id=cdefGHIJ", None).await.1,
        r#"{"messages":{}}"#
    );
    // Faithful defect: clear never deletes the `share/...json` objects publish wrote.
    assert_eq!(bucket.0.lock().expect("lock").len(), 3);
}
