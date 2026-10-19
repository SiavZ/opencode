//! Purpose: retain live catalog failures as typed resolver errors instead of missing-model errors.
//! Scope: public LocationModels resolver with a failed catalog port.
//! Uses:
//! rust/crates/
//! |-- schema/src/provider.rs
//! |-- session-runtime/src/runner/model.rs
//! `-- session-runtime/tests/support/model.rs
//! External:
//! `-- async_trait::async_trait
//! Used by:
//! `-- none, entrypoint
//! Invariants: catalog failure stops before model lookup or credential resolution.
//! Contract tests: this file

#[path = "support/model.rs"]
mod support;

use async_trait::async_trait;
use opencode_schema::ids::{IntegrationID, ProviderID};
use opencode_schema::provider::{ModelInfo, ProviderInfo};
use opencode_session_runtime::runner::model::{
    CredentialValue, IntegrationCredentials, LocationModels, ModelCatalog, ModelError,
    SessionRunnerModel,
};
use std::sync::Arc;

struct FailedCatalog;

#[async_trait]
impl ModelCatalog for FailedCatalog {
    async fn check(&self) -> Result<(), String> {
        Err("catalog database unavailable".into())
    }
    async fn default_model(&self) -> Option<ModelInfo> {
        panic!("failed catalog must not be queried")
    }
    async fn available_models(&self) -> Vec<ModelInfo> {
        panic!("failed catalog must not be queried")
    }
    async fn provider(&self, _: &ProviderID) -> Option<ProviderInfo> {
        panic!("failed catalog must not be queried")
    }
}

struct UnusedCredentials;

#[async_trait]
impl IntegrationCredentials for UnusedCredentials {
    async fn active_credential(
        &self,
        _: &IntegrationID,
    ) -> Result<Option<CredentialValue>, String> {
        panic!("catalog failure precedes credentials")
    }
}

#[tokio::test]
async fn catalog_failure_does_not_become_unavailable_model_or_authorization() {
    let resolver = LocationModels {
        catalog: Arc::new(FailedCatalog),
        integrations: Arc::new(UnusedCredentials),
        routes: Arc::new(support::Routes::default()),
    };
    let error = resolver
        .resolve(&support::session("ses_catalog", None))
        .await
        .err()
        .expect("catalog failure");
    assert_eq!(
        error,
        ModelError::Catalog("catalog database unavailable".into())
    );
    assert_eq!(error.tag(), "Catalog.Error");
}
