//! Purpose: adapt refreshed provider catalogs and durable credential reads to Session resolver ports.
//! Scope: Location services sharing one ProviderServices instance with CLI and HTTP hosts.
//! Uses:
//! rust/crates/
//! |-- config/src/current.rs
//! |-- provider-adapters/src/catalog/refresh.rs
//! |-- runtime/src/provider_services.rs
//! |-- schema/src/ids.rs
//! |-- schema/src/provider.rs
//! |-- session-runtime/src/runner/model.rs
//! `-- storage-api/src/lib.rs
//! External:
//! `-- async_trait::async_trait
//! Used by:
//! rust/crates/
//! `-- runtime/src/location.rs
//! Invariants: refresh and auth rotation are observed on the next resolution, never stale snapshots.
//! Contract tests: runtime/tests/provider_services.rs, runtime/tests/production.rs

use crate::provider_services::ProviderServices;
use async_trait::async_trait;
use opencode_config::CurrentConfig;
use opencode_provider_adapters::catalog::ModelsService;
use opencode_schema::ids::{IntegrationID, ProviderID};
use opencode_schema::provider::{ModelInfo, ProviderInfo};
use opencode_session_runtime::runner::model::{
    CredentialValue, IntegrationCredentials, ModelCatalog,
};
use opencode_storage_api::Database;
use std::sync::Arc;

pub struct Catalog(pub Arc<ProviderServices>);
pub struct Credentials(pub Arc<ProviderServices>);

pub async fn load(
    config: &CurrentConfig,
    environment: &opencode_config::Environment,
    models: Arc<ModelsService>,
    database: Arc<dyn Database>,
) -> Result<(Arc<Catalog>, Arc<Credentials>, Arc<ProviderServices>), String> {
    let services = Arc::new(
        ProviderServices::new(
            database,
            models,
            serde_json::json!({}),
            environment
                .vars
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        )
        .with_documents(
            config
                .documents()
                .map(|document| document.info.clone())
                .collect(),
        ),
    );
    // Opening validates the initial snapshot. Later failures are logged because ModelCatalog's
    // existing optional/infallible ports cannot transport a typed catalog I/O error.
    services.catalog().await?;
    Ok((
        Arc::new(Catalog(services.clone())),
        Arc::new(Credentials(services.clone())),
        services,
    ))
}

#[async_trait]
impl ModelCatalog for Catalog {
    async fn default_model(&self) -> Option<ModelInfo> {
        match self.0.catalog().await {
            Ok(catalog) => catalog.default_model(),
            Err(error) => {
                tracing::error!(%error, "provider catalog unavailable");
                None
            }
        }
    }
    async fn available_models(&self) -> Vec<ModelInfo> {
        match self.0.catalog().await {
            Ok(catalog) => catalog.available_models(),
            Err(error) => {
                tracing::error!(%error, "provider catalog unavailable");
                Vec::new()
            }
        }
    }
    async fn provider(&self, id: &ProviderID) -> Option<ProviderInfo> {
        match self.0.catalog().await {
            Ok(catalog) => catalog.provider(id),
            Err(error) => {
                tracing::error!(%error, "provider catalog unavailable");
                None
            }
        }
    }
}

#[async_trait]
impl IntegrationCredentials for Credentials {
    async fn active_credential(
        &self,
        id: &IntegrationID,
    ) -> Result<Option<CredentialValue>, String> {
        self.0
            .credentials()
            .await?
            .active_credential(id)
            .map_err(|error| error.to_string())?
            .map(|value| {
                serde_json::to_value(value)
                    .and_then(serde_json::from_value)
                    .map_err(|error| error.to_string())
            })
            .transpose()
    }
}
