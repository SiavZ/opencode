//! Purpose: local temp fixture for health check testing.
//! Scope: spike only. Provides a minimal HTTP server with /api/health and /global/health.
//! Uses:
//! rust/crates/
//! `-- none
//! External:
//! |-- axum
//! `-- tokio
//! Used by:
//! `-- tests/spawn_health_stop.rs
//! Invariants: binds to 127.0.0.1, no production data, auto-shutdown on drop.
//! Contract tests: tests/spawn_health_stop.rs

use axum::{Router, routing::get, http::StatusCode};

pub fn make_health_router() -> Router {
    Router::new()
        .route("/api/health", get(|| async { StatusCode::OK }))
        .route("/global/health", get(|| async { StatusCode::OK }))
}

pub async fn spawn_temp_server() -> (String, tokio::task::JoinHandle<()>) {
    let app = make_health_router();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{}", addr);
    
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    
    (url, handle)
}
