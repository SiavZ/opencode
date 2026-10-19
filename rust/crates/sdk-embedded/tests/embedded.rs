//! Purpose: prove the embedded composition facade reports the missing component instead of
//! fabricating a router.
//! Scope: `OpenCode::create` behind the `embedded` feature.
//! Uses:
//! rust/crates/
//! |-- sdk-embedded/src/embedded.rs
//! `-- sdk-embedded/src/lib.rs
//! External:
//! `-- tokio::test
//! Used by:
//! `-- none, entrypoint
//! Invariants: live test of the real facade, no fakes.
//! Contract tests: self
#![cfg(feature = "embedded")]

use opencode_sdk_embedded::embedded::{Component, EMBEDDED_BASE_URL, REQUIRED_COMPONENTS};
use opencode_sdk_embedded::{EmbedError, OpenCode};

#[tokio::test]
async fn create_reports_first_missing_component() {
    let error = OpenCode::create()
        .await
        .expect_err("composition is not ported");
    assert_eq!(
        error,
        EmbedError::NotYetPorted {
            component: Component::RuntimeServices
        }
    );
    assert!(error.to_string().contains("opencode-runtime"));
}

#[test]
fn composition_contract_constants() {
    assert_eq!(EMBEDDED_BASE_URL, "http://opencode.local");
    assert_eq!(
        REQUIRED_COMPONENTS,
        [
            Component::RuntimeServices,
            Component::EmbeddedRouter,
            Component::InProcessClientTransport
        ]
    );
}
