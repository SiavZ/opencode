//! Purpose: lower the resolved legacy configuration into its own live provider service.
//! Scope: pure V1 normalization after the shared legacy loader has completed remote/local inputs.
//! Uses:
//! rust/crates/
//! |-- config/src/migrate.rs
//! |-- provider-adapters/src/catalog/refresh.rs
//! `-- runtime/src/provider_services.rs
//! External:
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! `-- runtime/src/location.rs
//! Invariants: V1 inline/remote configuration never changes the V2 CurrentConfig document contract.
//! Contract tests: runtime/tests/provider_services.rs

use crate::provider_services::ProviderServices;
use opencode_config::Environment;
use opencode_provider_adapters::catalog::ModelsService;
use opencode_storage_api::Database;
use serde_json::Value;
use std::sync::Arc;

pub struct Inputs {
    pub port: Arc<dyn opencode_server::services::legacy::LegacySessions>,
    pub environment: Environment,
    pub models: Arc<ModelsService>,
    pub database: Arc<dyn Database>,
}

impl Inputs {
    pub async fn resolve(
        &self,
        context: &opencode_server::middleware::legacy_routing::RouteContext,
    ) -> Result<Arc<ProviderServices>, opencode_server::services::legacy::LegacyError> {
        let config = self
            .port
            .handle(
                "config.get",
                context,
                &serde_json::Map::new(),
                &serde_json::Map::new(),
                None,
            )
            .await?;
        Ok(provider_services(
            config,
            &self.environment,
            self.models.clone(),
            self.database.clone(),
        ))
    }
}

pub fn provider_services(
    config: Value,
    environment: &Environment,
    models: Arc<ModelsService>,
    database: Arc<dyn Database>,
) -> Arc<ProviderServices> {
    Arc::new(ProviderServices::new(
        database,
        models,
        opencode_config::migrate::migrate(&config),
        environment
            .vars
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    ))
}
