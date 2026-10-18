//! Purpose: load the TypeScript baseline fixtures (tests/fixtures/baseline/*.json) and the
//! expected-failure ledger for the replay harness.
//! Scope: test-only.
//! Uses:
//! rust/crates/
//! |-- server/tests/support/replay/matcher.rs
//! `-- server/tests/support/replay/run.rs
//! External:
//! `-- serde::Deserialize
//! Used by:
//! rust/crates/
//! `-- server/tests/replay_baseline.rs
//! Invariants: cases replay in capture order (`seq`), because later cases reuse ids bound by
//! earlier ones. The ledger maps case id -> mismatch category; every non-passing case must be
//! ledgered and every ledgered case must still fail (stale entries are errors).
//! Contract tests: server/tests/replay_baseline.rs
#![allow(dead_code)]

pub mod matcher;
pub mod run;

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, Deserialize)]
pub struct Family {
    pub family: String,
    pub cases: Vec<Case>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Case {
    pub seq: u64,
    pub id: String,
    #[serde(rename = "operationId")]
    pub operation_id: Option<String>,
    /// `nopass` or `pass` (OPENCODE_SERVER_PASSWORD set).
    pub server: String,
    pub request: Req,
    pub response: Resp,
    pub note: Option<String>,
    #[serde(skip)]
    pub family: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Req {
    pub method: String,
    pub path: String,
    /// `none`, `invalid`, `valid`, `query-valid`.
    pub auth: String,
    #[serde(default)]
    pub query: Vec<(String, String)>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    pub body: Option<Value>,
    pub raw_body: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Resp {
    pub status: Option<u16>,
    pub headers: BTreeMap<String, String>,
    /// `json`, `text`, `empty`, `sse`, `timeout`, `json-digest`, `text-digest`.
    pub kind: String,
    pub body: Option<Value>,
    #[serde(default)]
    pub frames: Vec<Frame>,
    pub digest: Option<Digest>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Frame {
    pub shape: String,
    pub fields: Vec<String>,
    pub data: Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Digest {
    pub bytes: usize,
    pub sha256: String,
    #[serde(default)]
    pub paths: Vec<String>,
}

pub fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

pub fn baseline_dir() -> PathBuf {
    manifest_dir().join("tests/fixtures/baseline")
}

pub fn ledger_path() -> PathBuf {
    manifest_dir().join("tests/fixtures/replay-ledger.json")
}

pub fn repo_root() -> PathBuf {
    manifest_dir().join("../../..")
}

/// Every case of every family, in capture order.
pub fn load() -> Vec<Case> {
    let mut cases = Vec::new();
    let mut entries: Vec<_> = std::fs::read_dir(baseline_dir())
        .expect("baseline fixtures dir")
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    entries.sort();
    for path in entries {
        let text = std::fs::read_to_string(&path).unwrap();
        let family: Family =
            serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        for mut case in family.cases {
            case.family = family.family.clone();
            cases.push(case);
        }
    }
    cases.sort_by_key(|c| c.seq);
    cases
}

/// One expected mismatch: category (compared) and the first difference seen when blessed
/// (informational, may contain run-specific ids).
#[derive(Debug, Clone, Deserialize, serde::Serialize, PartialEq)]
pub struct Expected {
    pub category: String,
    pub detail: String,
}

/// Ledger: case id -> expected mismatch (see README). Missing file = empty ledger.
pub fn ledger() -> BTreeMap<String, Expected> {
    match std::fs::read_to_string(ledger_path()) {
        Ok(text) => serde_json::from_str(&text).expect("replay-ledger.json"),
        Err(_) => BTreeMap::new(),
    }
}

/// `(method, path, operationId)` for every operation in packages/sdk/openapi.json.
pub fn openapi_operations() -> Vec<(String, String, String)> {
    let text = std::fs::read_to_string(repo_root().join("packages/sdk/openapi.json"))
        .expect("packages/sdk/openapi.json");
    let spec: Value = serde_json::from_str(&text).unwrap();
    let mut out = Vec::new();
    for (path, item) in spec["paths"].as_object().unwrap() {
        for (method, op) in item.as_object().unwrap() {
            if ["get", "post", "put", "patch", "delete"].contains(&method.as_str()) {
                out.push((
                    method.to_uppercase(),
                    path.clone(),
                    op["operationId"].as_str().unwrap_or_default().to_string(),
                ));
            }
        }
    }
    out
}
