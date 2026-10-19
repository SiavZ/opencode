//! Purpose: adapt canonical native catalog and credential stores to Session model resolver ports.
//! Scope: Location-scoped model discovery and credential lookup.
//! Uses:
//! rust/crates/
//! |-- config/src/lib.rs
//! |-- provider-adapters/src/catalog/mod.rs
//! |-- provider-adapters/src/credentials/mod.rs
//! |-- schema/src/ids.rs
//! |-- schema/src/provider.rs
//! |-- session-runtime/src/runner/model.rs
//! `-- storage-api/src/lib.rs
//! External:
//! |-- async_trait::async_trait
//! |-- serde_json::Value::Array
//! |-- serde_json::from_value
//! |-- serde_json::json
//! `-- serde_json::to_value
//! Used by:
//! rust/crates/
//! `-- runtime/src/location.rs
//! Invariants: native inventory is not fabricated; credential conversion preserves typed secrets.
//! Contract tests: runtime/tests/production.rs

use async_trait::async_trait;
use opencode_config::CurrentConfig;
use opencode_provider_adapters::{catalog::CatalogStore, credentials::CredentialStore};
use opencode_schema::ids::{IntegrationID, ProviderID};
use opencode_schema::provider::{ModelInfo, ProviderInfo};
use opencode_session_runtime::runner::model::{
    CredentialValue, IntegrationCredentials, ModelCatalog,
};
use opencode_storage_api::{Database, DatabaseExt};
use std::sync::Arc;

pub struct Catalog(pub CatalogStore);
pub struct Credentials(pub CredentialStore);

pub async fn load(
    config: &CurrentConfig,
    environment: &opencode_config::Environment,
    cache: &std::path::Path,
    database: Arc<dyn Database>,
) -> Result<(Arc<Catalog>, Arc<Credentials>), String> {
    let mut catalog = CatalogStore::new();
    let mut credentials = CredentialStore::new().with_environment(
        environment
            .vars
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    );
    let cache = cache.join("models.json");
    if cache.try_exists().map_err(|e| e.to_string())? {
        catalog.load_cache(&cache).map_err(|e| e.to_string())?;
        credentials.load_cache(&cache).map_err(|e| e.to_string())?;
    }
    for document in config.documents() {
        catalog.apply_config(&document.info);
        credentials.apply_config(&document.info);
    }
    let records = database.read(|sql| {
        sql.query("SELECT integration_id, value FROM credential WHERE integration_id IS NOT NULL ORDER BY time_created ASC", &[])?.into_iter().map(|row| {
            Ok(serde_json::json!({"integrationID": row.get::<String>("integration_id")?, "value": row.get_json::<serde_json::Value>("value")?}))
        }).collect::<opencode_storage_api::Result<Vec<_>>>()
    }).await.map_err(|e| e.to_string())?;
    credentials
        .load_records(&serde_json::Value::Array(records))
        .map_err(|e| e.to_string())?;
    catalog
        .apply_credentials(&credentials)
        .map_err(|e| e.to_string())?;
    Ok((
        Arc::new(Catalog(catalog)),
        Arc::new(Credentials(credentials)),
    ))
}

#[async_trait]
impl ModelCatalog for Catalog {
    async fn default_model(&self) -> Option<ModelInfo> {
        self.0.default_model()
    }
    async fn available_models(&self) -> Vec<ModelInfo> {
        self.0.available_models()
    }
    async fn provider(&self, id: &ProviderID) -> Option<ProviderInfo> {
        self.0.provider(id)
    }
}

#[async_trait]
impl IntegrationCredentials for Credentials {
    async fn active_credential(
        &self,
        id: &IntegrationID,
    ) -> Result<Option<CredentialValue>, String> {
        self.0
            .active_credential(id)
            .map_err(|e| e.to_string())?
            .map(|value| {
                serde_json::to_value(value)
                    .and_then(serde_json::from_value)
                    .map_err(|e| e.to_string())
            })
            .transpose()
    }
}
