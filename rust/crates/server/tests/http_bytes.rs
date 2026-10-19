//! Purpose: binary HTTP response acceptance for filesystem service ports.
//! Scope: real loopback listener with an explicitly labeled byte-service fixture.
//! Uses:
//! rust/crates/
//! |-- server/src/services/legacy.rs
//! `-- server/tests/support/mod.rs
//! External:
//! |-- async_trait::async_trait
//! `-- serde_json::{Map, Value}
//! Used by:
//! `-- none, entrypoint
//! Invariants: bytes are never decoded to UTF-8 or enclosed in location/data JSON.
//! Contract tests: this file; runtime owns actual filesystem adapter acceptance.

mod support;

use opencode_server::middleware::legacy_routing::RouteContext;
use opencode_server::services::legacy::{LegacyBytesResponse, LegacyError, LegacySessions};
use serde_json::{Map, Value};
use std::sync::Arc;

struct Files;
#[async_trait::async_trait]
impl LegacySessions for Files {
    async fn handle_bytes(
        &self,
        operation: &str,
        _: &RouteContext,
        _: &Map<String, Value>,
        _: &Map<String, Value>,
        _: Option<Value>,
    ) -> Result<LegacyBytesResponse, LegacyError> {
        assert_eq!(operation, "v2.fs.read");
        Ok(LegacyBytesResponse {
            body: vec![0, 255, 128, 65],
            status: 200,
            headers: vec![("content-type".into(), "application/octet-stream".into())],
        })
    }
    async fn handle(
        &self,
        _: &str,
        _: &RouteContext,
        _: &Map<String, Value>,
        _: &Map<String, Value>,
        _: Option<Value>,
    ) -> Result<Value, LegacyError> {
        panic!("binary endpoint must not invoke JSON service path")
    }
}

#[tokio::test]
async fn filesystem_binary_response_preserves_every_octet() {
    let mut services = support::services(Arc::new(support::FakeSessions::default()));
    services.legacy_global = Some(Arc::new(Files));
    let server = support::start(opencode_server::app::embedded_router(services)).await;
    let response = support::client()
        .get(format!("{}/api/fs/read/binary.dat", server.url()))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.headers()["content-type"],
        "application/octet-stream"
    );
    assert_eq!(response.bytes().await.unwrap().as_ref(), &[0, 255, 128, 65]);
    server.stop(false).await;
}
