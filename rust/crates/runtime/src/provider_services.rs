//! Purpose: expose durable provider credentials and refreshed canonical catalogs to runtime hosts.
//! Scope: Location configuration/environment with process database and models cache.
//! Uses:
//! rust/crates/
//! |-- provider-adapters/src/catalog/mod.rs
//! |-- provider-adapters/src/credentials/mod.rs
//! |-- schema/src/ids.rs
//! |-- server/src/services/legacy.rs
//! |-- storage-api/src/lib.rs
//! `-- storage-sqlite/src/repositories/credentials.rs
//! External:
//! |-- indexmap::IndexMap
//! `-- serde_json::Value
//! Used by:
//! `-- none, entrypoint
//! Invariants: credentials use canonical integration identity and never enter request bodies.
//! Contract tests: runtime/tests/provider_services.rs

mod attempts;
mod current;
mod legacy;
mod registry;

use indexmap::IndexMap;
use opencode_provider_adapters::catalog::{CatalogStore, ModelsService};
use opencode_provider_adapters::credentials::CredentialStore;
use opencode_server::middleware::legacy_routing::RouteContext;
use opencode_server::services::legacy::{LegacyError, LegacySessions};
use opencode_storage_api::{Database, DatabaseExt};
use opencode_storage_sqlite::repositories::credentials;
use serde_json::{Map, Value, json};
use std::sync::Arc;

pub struct ProviderServices {
    attempts: attempts::State,
    database: Arc<dyn Database>,
    models: Arc<ModelsService>,
    documents: Vec<Value>,
    environment: IndexMap<String, String>,
    plugins: tokio::sync::RwLock<Vec<Arc<opencode_integration_adapters::plugins::Instance>>>,
}

impl ProviderServices {
    pub fn new(
        database: Arc<dyn Database>,
        models: Arc<ModelsService>,
        config: Value,
        environment: IndexMap<String, String>,
    ) -> Self {
        Self {
            attempts: attempts::State::default(),
            database,
            models,
            documents: vec![config],
            environment,
            plugins: tokio::sync::RwLock::new(Vec::new()),
        }
    }

    pub fn with_documents(mut self, documents: Vec<Value>) -> Self {
        self.documents = documents;
        self
    }

    pub fn with_plugins(
        mut self,
        plugins: Vec<Arc<opencode_integration_adapters::plugins::Instance>>,
    ) -> Self {
        self.plugins = tokio::sync::RwLock::new(plugins);
        self
    }

    pub async fn set_plugins(
        &self,
        plugins: Vec<Arc<opencode_integration_adapters::plugins::Instance>>,
    ) {
        *self.plugins.write().await = plugins;
    }

    pub async fn auth_methods(&self) -> Result<Value, LegacyError> {
        let mut methods = Map::new();
        let plugins = self.plugins.read().await.clone();
        for plugin in plugins {
            let result = plugin
                .host
                .request("auth.methods", json!({"handle":plugin.handle}))
                .await
                .map_err(plugin_error)?;
            if let Some(provider) = result.get("provider").and_then(Value::as_str) {
                methods.insert(provider.to_owned(), result["methods"].clone());
            }
        }
        Ok(Value::Object(methods))
    }

    pub async fn oauth_authorize(
        &self,
        provider: &str,
        payload: Value,
    ) -> Result<Value, LegacyError> {
        let plugin = self
            .auth_plugin(provider)
            .await
            .map_err(|error| match error {
                LegacyError::Response { .. } => {
                    LegacyError::Internal(format!("OAuth provider hook not found: {provider}"))
                }
                error => error,
            })?;
        plugin.host.request("auth.authorize", json!({"handle":plugin.handle,"method":payload["method"],"inputs":payload.get("inputs")})).await.map_err(plugin_error)
    }

    pub async fn oauth_callback(&self, provider: &str, payload: Value) -> Result<(), LegacyError> {
        let plugin = self.auth_plugin(provider).await?;
        let result = plugin
            .host
            .request(
                "auth.callback",
                json!({"handle":plugin.handle,"code":payload.get("code")}),
            )
            .await
            .map_err(plugin_error)?;
        if let Some(key) = result.get("key") {
            self.auth_set(
                provider,
                json!({"type":"api","key":key,"metadata":result.get("metadata")}),
            )
            .await
            .map_err(LegacyError::Internal)?;
        }
        if result.get("refresh").is_some() {
            let mut value = result;
            value["type"] = json!("oauth");
            self.auth_set(provider, value)
                .await
                .map_err(LegacyError::Internal)?;
        }
        Ok(())
    }

    async fn auth_plugin(
        &self,
        provider: &str,
    ) -> Result<Arc<opencode_integration_adapters::plugins::Instance>, LegacyError> {
        // Last declaration wins, exactly as Record.fromEntries in the TS hook registry.
        let plugins = self.plugins.read().await.clone();
        for plugin in plugins.into_iter().rev() {
            let result = plugin
                .host
                .request("auth.methods", json!({"handle":plugin.handle}))
                .await
                .map_err(plugin_error)?;
            if result.get("provider").and_then(Value::as_str) == Some(provider) {
                return Ok(plugin);
            }
        }
        Err(LegacyError::Response {
            status: 400,
            body: json!({"name":"ProviderAuthOauthMissing","data":{"providerID":provider}}),
        })
    }

    pub async fn refresh_models(&self, force: bool) -> Result<bool, String> {
        self.models
            .refresh(force)
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn catalog(&self) -> Result<CatalogStore, String> {
        let mut catalog = self.models.get().await.map_err(|error| error.to_string())?;
        for document in &self.documents {
            catalog.apply_config(document);
        }
        catalog
            .apply_credentials(&self.credentials().await?)
            .map_err(|error| error.to_string())?;
        Ok(catalog)
    }

    pub async fn credentials(&self) -> Result<CredentialStore, String> {
        let mut store = CredentialStore::new().with_environment(self.environment.clone());
        let records = self.database.read(|sql| {
            credentials::all(sql)?.into_iter().map(|record| {
                Ok(json!({"integrationID":record.integration_id,"value":serde_json::to_value(record.value.expose()).map_err(|_| opencode_storage_api::StorageError::Validation("invalid credential".into()))?}))
            }).collect::<opencode_storage_api::Result<Vec<_>>>()
        }).await.map_err(|error| error.to_string())?;
        store
            .load_records(&Value::Array(records))
            .map_err(|error| error.to_string())?;
        // Inventory-declared environment names must be registered without inspecting process env.
        for (id, names) in self
            .models
            .environment_names()
            .await
            .map_err(|error| error.to_string())?
        {
            store.register_env(
                opencode_schema::ids::IntegrationID::new(self.integration(id.as_str())),
                names,
            );
        }
        for document in &self.documents {
            store.apply_config(document);
        }
        Ok(store)
    }

    fn integration(&self, provider: &str) -> String {
        self.documents
            .iter()
            .rev()
            .find_map(|document| {
                document
                    .get("providers")
                    .and_then(|providers| providers.get(provider))
                    .and_then(|provider| provider.get("integrationID"))
                    .and_then(Value::as_str)
            })
            .unwrap_or(provider)
            .trim_end_matches('/')
            .to_owned()
    }

    pub async fn auth_list(&self) -> Result<Value, String> {
        self.database
            .read(|sql| {
                let mut result = Map::new();
                for record in credentials::all(sql)? {
                    let mut value = serde_json::to_value(record.value.expose()).map_err(|_| {
                        opencode_storage_api::StorageError::Validation("invalid credential".into())
                    })?;
                    if value["type"] == "key" {
                        value["type"] = json!("api");
                    }
                    result.entry(record.integration_id).or_insert(value);
                }
                Ok::<_, opencode_storage_api::StorageError>(Value::Object(result))
            })
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn auth_set(&self, provider: &str, mut info: Value) -> Result<(), String> {
        let integration = self.integration(provider);
        if info["type"] == "api" {
            info["type"] = json!("key");
        }
        if info["type"] == "oauth" && info.get("methodID").is_none() {
            info["methodID"] = json!("oauth");
            let metadata: Map<String, Value> = ["accountId", "enterpriseUrl"]
                .into_iter()
                .filter_map(|key| info.get(key).cloned().map(|value| (key.to_owned(), value)))
                .collect();
            if !metadata.is_empty() {
                info["metadata"] = Value::Object(metadata);
            }
        }
        let value: credentials::CredentialValue =
            serde_json::from_value(info).map_err(|_| "invalid auth credential".to_owned())?;
        self.database
            .transaction(opencode_storage_api::TxOptions::default(), move |sql| {
                credentials::create(sql, &integration, &value, None).map(|_| ())
            })
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn auth_remove(&self, provider: &str) -> Result<(), String> {
        let integration = self.integration(provider);
        self.database
            .transaction(opencode_storage_api::TxOptions::default(), move |sql| {
                sql.execute(
                    "DELETE FROM credential WHERE integration_id = ?1",
                    &[integration.into()],
                )?;
                Ok::<_, opencode_storage_api::StorageError>(())
            })
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn provider_list(&self) -> Result<Value, String> {
        legacy::list(self).await
    }
}

fn plugin_error(error: opencode_integration_api::IntegrationError) -> LegacyError {
    if let opencode_integration_api::IntegrationError::Rpc {
        data: Some(Value::Object(data)),
        ..
    } = &error
    {
        let mut data = data.clone();
        if let Some(Value::String(name)) = data.remove("name") {
            return LegacyError::Response {
                status: 400,
                body: json!({"name":name,"data":data}),
            };
        }
    }
    LegacyError::Internal(error.to_string())
}
#[async_trait::async_trait]
impl LegacySessions for ProviderServices {
    async fn handle(
        &self,
        operation: &str,
        _context: &RouteContext,
        params: &Map<String, Value>,
        _query: &Map<String, Value>,
        payload: Option<Value>,
    ) -> Result<Value, LegacyError> {
        let provider = params
            .get("providerID")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match operation {
            operation if operation.starts_with("v2.") => {
                self.current(operation, params, payload).await
            }
            "provider.list" => self.provider_list().await.map_err(LegacyError::Internal),
            "config.providers" => {
                let list = self.provider_list().await.map_err(LegacyError::Internal)?;
                let connected = list["connected"].as_array().cloned().unwrap_or_default();
                let providers: Vec<_> = list["all"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|provider| connected.contains(&provider["id"]))
                    .cloned()
                    .collect();
                let defaults: Map<_, _> = list["default"]
                    .as_object()
                    .into_iter()
                    .flatten()
                    .filter(|(id, _)| {
                        connected
                            .iter()
                            .any(|provider| provider.as_str() == Some(id.as_str()))
                    })
                    .map(|(id, value)| (id.clone(), value.clone()))
                    .collect();
                Ok(json!({"providers":providers,"default":defaults}))
            }
            "auth.set" => {
                self.auth_set(
                    provider,
                    payload
                        .ok_or_else(|| LegacyError::BadRequest("missing auth credential".into()))?,
                )
                .await
                .map_err(LegacyError::BadRequest)?;
                Ok(json!(true))
            }
            "auth.remove" => {
                self.auth_remove(provider)
                    .await
                    .map_err(LegacyError::Internal)?;
                Ok(json!(true))
            }
            "provider.auth" => self.auth_methods().await,
            "provider.oauth.authorize" => {
                self.oauth_authorize(provider, payload.unwrap_or(Value::Null))
                    .await
            }
            "provider.oauth.callback" => {
                self.oauth_callback(provider, payload.unwrap_or(Value::Null))
                    .await?;
                Ok(json!(true))
            }
            _ => Err(LegacyError::Unavailable(format!(
                "provider operation not composed: {operation}"
            ))),
        }
    }
}
