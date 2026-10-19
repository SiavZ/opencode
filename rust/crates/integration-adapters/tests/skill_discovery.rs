//! Purpose: verify native indexed skill downloads against a controlled loopback HTTP service.
//! Scope: version refresh, retries, malformed paths and preservation of an existing cache.
//! Uses:
//! rust/crates/
//! `-- integration-adapters/src/skills.rs
//! External:
//! |-- axum::Router
//! `-- tempfile::tempdir
//! Used by:
//! `-- none, entrypoint
//! Invariants: no external network, environment mutation, installation or user data.
//! Contract tests: this file

use axum::{
    Router,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
};
use opencode_integration_adapters::skills::discover;
use serde_json::json;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Fixture {
    version: u64,
    fail: bool,
    attempts: u64,
}

async fn serve(
    State(state): State<Arc<Mutex<Fixture>>>,
    Path(path): Path<String>,
) -> impl IntoResponse {
    let mut state = state.lock().unwrap();
    match path.as_str() {
        "index.json" => (StatusCode::OK, json!({"skills":[
            {"name":"valid","version":state.version.to_string(),"files":["SKILL.md","detail.txt"]},
            {"name":"../escape","files":["SKILL.md"]},
            {"name":"traversal","files":["SKILL.md","%2e%2e/escape"]},
            {"name":"absolute","files":["SKILL.md","https://outside.invalid/escape"]}
        ]}).to_string()),
        "valid/SKILL.md" => (StatusCode::OK, format!("---\nname: valid\ndescription: Fixture\n---\nversion {}",state.version)),
        "valid/detail.txt" => {
            state.attempts += 1;
            if state.fail || state.attempts == 1 { return (StatusCode::SERVICE_UNAVAILABLE, "retry".into()); }
            (StatusCode::OK, format!("detail {}", state.version))
        }
        _ => (StatusCode::NOT_FOUND,"missing".into()),
    }
}

#[tokio::test]
async fn indexed_skill_cache_retries_refreshes_and_preserves_failed_version() {
    let root = tempfile::tempdir().unwrap();
    let state = Arc::new(Mutex::new(Fixture {
        version: 1,
        ..Default::default()
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new()
        .route("/{*path}", get(serve))
        .with_state(state.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let paths = discover(root.path(), &url).await.unwrap();
    assert_eq!(paths.len(), 1);
    assert_eq!(
        std::fs::read_to_string(paths[0].join("detail.txt")).unwrap(),
        "detail 1"
    );
    assert_eq!(state.lock().unwrap().attempts, 2);
    {
        let mut state = state.lock().unwrap();
        state.version = 2;
        state.fail = true;
    }
    let failed = discover(root.path(), &url).await.unwrap();
    assert_eq!(failed, paths);
    assert!(
        std::fs::read_to_string(paths[0].join("SKILL.md"))
            .unwrap()
            .ends_with("version 1")
    );
    state.lock().unwrap().fail = false;
    assert_eq!(discover(root.path(), &url).await.unwrap(), paths);
    assert_eq!(
        std::fs::read_to_string(paths[0].join("detail.txt")).unwrap(),
        "detail 2"
    );
    assert_eq!(
        std::fs::read_to_string(paths[0].join(".opencode-version")).unwrap(),
        "2"
    );
    assert!(!root.path().join("escape").exists());
    server.abort();
    let _ = server.await;
}
