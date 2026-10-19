//! Purpose: fixture tests for the enterprise sharing adapters. They replay baseline captures from
//! `fixtures/sharing/baseline.json` (produced by `capture.ts` running the real TS modules) against
//! the Rust share service over a local fake S3 server that speaks the S3 HTTP subset used.
//! Scope: fixture. Local loopback HTTP only, no cloud credentials.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/sharing/objects.rs
//! |-- hosted-adapters/src/sharing/service.rs
//! |-- hosted-adapters/src/sharing/sigv4.rs
//! `-- hosted-domain/src/sharing/share.rs
//! External:
//! |-- axum::Router
//! |-- axum::body::Bytes
//! |-- axum::extract::Path
//! |-- axum::extract::Query
//! |-- axum::extract::State
//! |-- axum::http::HeaderMap
//! |-- axum::http::Method
//! |-- axum::http::StatusCode
//! |-- axum::routing::any
//! |-- axum::serve
//! |-- reqwest::Client::new
//! |-- serde_json::Value
//! |-- serde_json::from_str
//! |-- serde_json::from_value
//! |-- serde_json::to_value
//! |-- tokio::net::TcpListener::bind
//! |-- tokio::spawn
//! |-- tokio::test
//! `-- url::Url::parse
//! Used by:
//! `-- none, entrypoint
//! Invariants: the fake server records every operation so object-level traces can be compared
//! with the baseline trace exactly.
//! Contract tests: this file

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::routing::any;
use opencode_hosted_adapters::sharing::objects::{ListOptions, ObjectStore, S3Store, extract_keys};
use opencode_hosted_adapters::sharing::service::ShareService;
use opencode_hosted_adapters::sharing::sigv4::{Credentials, region_for, sign};
use opencode_hosted_domain::sharing::share::ShareData;
use serde_json::Value;

#[derive(Default)]
struct Fake {
    objects: BTreeMap<String, String>,
    trace: Vec<String>,
    unsigned: usize,
}

type Shared = Arc<Mutex<Fake>>;

async fn handle(
    State(fake): State<Shared>,
    method: Method,
    path: Option<Path<String>>,
    Query(query): Query<BTreeMap<String, String>>,
    headers: HeaderMap,
    body: Bytes,
) -> (StatusCode, String) {
    let mut fake = fake.lock().expect("lock");
    if !headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("AWS4-HMAC-SHA256 "))
    {
        fake.unsigned += 1;
    }
    let key = path.map(|p| p.0).unwrap_or_default();
    if method == Method::GET && query.get("list-type").map(String::as_str) == Some("2") {
        let mut pairs: Vec<String> = Vec::new();
        for name in ["list-type", "prefix", "max-keys", "start-after"] {
            if let Some(v) = query.get(name) {
                pairs.push(format!(
                    "{name}={}",
                    url::form_urlencoded::byte_serialize(v.as_bytes()).collect::<String>()
                ));
            }
        }
        fake.trace.push(format!("LIST {}", pairs.join("&")));
        let prefix = query.get("prefix").cloned().unwrap_or_default();
        let after = query.get("start-after").cloned().unwrap_or_default();
        let max: usize = query
            .get("max-keys")
            .and_then(|m| m.parse().ok())
            .unwrap_or(1000);
        let keys: Vec<String> = fake
            .objects
            .keys()
            .filter(|k| k.starts_with(&prefix) && **k > after)
            .take(max)
            .cloned()
            .collect();
        let xml: String = keys
            .iter()
            .map(|k| format!("<Contents><Key>{k}</Key></Contents>"))
            .collect();
        return (
            StatusCode::OK,
            format!("<ListBucketResult>{xml}</ListBucketResult>"),
        );
    }
    fake.trace.push(format!("{method} {key}"));
    match method {
        Method::GET => match fake.objects.get(&key) {
            Some(v) => (StatusCode::OK, v.clone()),
            None => (StatusCode::NOT_FOUND, String::new()),
        },
        Method::PUT => {
            fake.objects
                .insert(key, String::from_utf8_lossy(&body).into_owned());
            (StatusCode::OK, String::new())
        }
        Method::DELETE => {
            fake.objects.remove(&key);
            (StatusCode::NO_CONTENT, String::new())
        }
        _ => (StatusCode::METHOD_NOT_ALLOWED, String::new()),
    }
}

async fn fake_s3() -> (Shared, S3Store) {
    let fake: Shared = Arc::default();
    let app = Router::new()
        .route("/bucket", any(handle))
        .route("/bucket/{*key}", any(handle))
        .with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    let creds = Credentials {
        access_key_id: "AKIDEXAMPLE".into(),
        secret_access_key: "secret".into(),
    };
    let store = S3Store::new(
        reqwest::Client::new(),
        creds,
        &format!("http://{addr}"),
        "bucket",
        "auto",
    );
    (fake, store)
}

fn baseline() -> Value {
    serde_json::from_str(include_str!("fixtures/sharing/baseline.json")).expect("baseline fixture")
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .expect("array")
        .iter()
        .map(|v| v.as_str().expect("string").to_string())
        .collect()
}

fn data(value: &Value) -> Vec<ShareData> {
    serde_json::from_value(value.clone()).expect("share data")
}

#[tokio::test]
async fn sync_merge_and_trace_match_baseline() {
    let fixture = baseline();
    let (fake, store) = fake_s3().await;
    let service = ShareService {
        store: &store,
        node_env_test: false,
    };
    let info = service
        .create("ses_0123456789abcdefGHIJ")
        .await
        .expect("create");
    assert_eq!(info.id, fixture["create"]["id"]);
    assert_eq!(
        info.secret.len() as u64,
        fixture["create"]["secretLength"].as_u64().expect("len")
    );
    fake.lock().expect("lock").trace.clear();
    for batch in fixture["syncInputs"].as_array().expect("batches") {
        service
            .sync(&info.id, &info.secret, &data(batch))
            .await
            .expect("sync");
    }
    assert_eq!(
        fake.lock().expect("lock").trace,
        strings(&fixture["syncTrace"])
    );
    let out = serde_json::to_value(service.data(&info.id).await.expect("data")).expect("json");
    assert_eq!(out, fixture["data"]);
    assert_eq!(fake.lock().expect("lock").unsigned, 0);
}

#[tokio::test]
async fn legacy_import_matches_baseline() {
    let fixture = baseline();
    let (fake, store) = fake_s3().await;
    {
        let mut fake = fake.lock().expect("lock");
        for (k, v) in fixture["legacyObjects"].as_object().expect("objects") {
            if !k.starts_with("share_snapshot/") {
                fake.objects
                    .insert(k.clone(), v.as_str().expect("string").to_string());
            }
        }
        // Restore the pre-import compaction object captured by the TS run's inputs.
        fake.objects.insert(
            "share_compaction/legacy01.json".into(),
            r#"{"event":"evt_002","data":[{"type":"message","data":{"id":"msg_1","sessionID":"ses_0123456789abcdefGHIJ","role":"user"}}]}"#.into(),
        );
    }
    let service = ShareService {
        store: &store,
        node_env_test: false,
    };
    let out = serde_json::to_value(service.data("legacy01").await.expect("data")).expect("json");
    assert_eq!(out, fixture["legacyData"]);
    let fake = fake.lock().expect("lock");
    assert_eq!(fake.trace, strings(&fixture["legacyTrace"]));
    for (k, v) in fixture["legacyObjects"].as_object().expect("objects") {
        assert_eq!(
            fake.objects.get(k).map(String::as_str),
            v.as_str(),
            "object {k}"
        );
    }
}

#[tokio::test]
async fn errors_and_remove_match_baseline() {
    let fixture = baseline();
    let (fake, store) = fake_s3().await;
    let service = ShareService {
        store: &store,
        node_env_test: false,
    };
    let info = service
        .create("ses_0123456789abcdefGHIJ")
        .await
        .expect("create");
    let errors = &fixture["errors"];
    assert_eq!(
        service
            .create("ses_0123456789abcdefGHIJ")
            .await
            .expect_err("dup")
            .to_string(),
        errors["duplicate"]
    );
    assert_eq!(
        service
            .sync(&info.id, "nope", &[])
            .await
            .expect_err("secret")
            .to_string(),
        errors["syncBadSecret"]
    );
    assert_eq!(
        service
            .sync("missing0", "x", &[])
            .await
            .expect_err("missing")
            .to_string(),
        errors["syncMissing"]
    );
    assert_eq!(
        service
            .remove(&info.id, "nope")
            .await
            .expect_err("secret")
            .to_string(),
        errors["removeBadSecret"]
    );
    fake.lock()
        .expect("lock")
        .objects
        .insert(format!("share_data/{}/session.json", info.id), "{}".into());
    fake.lock().expect("lock").trace.clear();
    service
        .remove(&info.id, &info.secret)
        .await
        .expect("remove");
    let fake = fake.lock().expect("lock");
    assert_eq!(fake.trace, strings(&fixture["removeTrace"]));
    // Faithful defect: the snapshot object survives removal (prefix `share_snapshot/<id>/`).
    let remaining: Vec<String> = fake
        .objects
        .keys()
        .filter(|k| k.contains(&info.id))
        .cloned()
        .collect();
    assert_eq!(remaining, strings(&fixture["remainingAfterRemove"]));
}

#[tokio::test]
async fn list_reads_a_single_page_and_filters_before() {
    let (fake, store) = fake_s3().await;
    {
        let mut fake = fake.lock().expect("lock");
        for i in 0..1005 {
            fake.objects.insert(format!("p/{i:05}.json"), "[]".into());
        }
    }
    let all = store
        .list(ListOptions {
            prefix: "p/".into(),
            ..Default::default()
        })
        .await
        .expect("list");
    assert_eq!(
        all.len(),
        1000,
        "baseline reads one page, objects after 1000 are invisible"
    );
    let before = store
        .list(ListOptions {
            prefix: "p/".into(),
            before: Some("00003".into()),
            ..Default::default()
        })
        .await
        .expect("list");
    assert_eq!(before, ["p/00000.json", "p/00001.json", "p/00002.json"]);
    let after = store
        .list(ListOptions {
            prefix: "p/".into(),
            after: Some("01002".into()),
            limit: Some(2),
            ..Default::default()
        })
        .await
        .expect("list");
    assert_eq!(after, ["p/01003.json", "p/01004.json"]);
}

#[test]
fn key_extraction_does_not_unescape_xml() {
    assert_eq!(
        extract_keys("<Key>a&amp;b</Key><Key></Key><Key>c</Key>"),
        ["a&amp;b", "c"]
    );
}

#[test]
fn sigv4_matches_aws4fetch_vectors() {
    let fixture = baseline();
    let creds = Credentials {
        access_key_id: "AKIDEXAMPLE".into(),
        secret_access_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".into(),
    };
    for vector in fixture["sigv4"].as_array().expect("vectors") {
        let url = url::Url::parse(vector["signedUrl"].as_str().expect("url")).expect("parse");
        let method = vector["method"].as_str().expect("method");
        let headers = match vector["body"] {
            Value::Null => Vec::new(),
            _ => vec![("Content-Type".to_string(), "application/json".to_string())],
        };
        let signed = sign(
            &creds,
            method,
            &url,
            &headers,
            &region_for(&url, "us-east-1"),
            "20261003T120000Z",
        );
        let auth = signed
            .iter()
            .find(|(k, _)| k == "authorization")
            .map(|(_, v)| v.as_str());
        assert_eq!(auth, vector["authorization"].as_str(), "{method} {url}");
    }
}
