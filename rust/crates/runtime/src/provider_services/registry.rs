//! Purpose: expose real canonical integration methods and non-secret connection metadata.
//! Scope: ModelsDev/config/installed plugin registry plus credential-backed key connections.
//! Uses:
//! rust/crates/
//! |-- runtime/src/provider_services.rs
//! |-- runtime/src/provider_services/attempts.rs
//! `-- storage-sqlite/src/repositories/credentials.rs
//! External:
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! `-- runtime/src/provider_services/current.rs
//! Invariants: unavailable methods fail, environment is captured explicitly, secrets never leave this API.
//! Contract tests: runtime/tests/provider_services.rs

use super::ProviderServices;
use opencode_server::services::legacy::LegacyError;
use opencode_storage_api::{DatabaseExt, TxOptions};
use opencode_storage_sqlite::repositories::credentials;
use serde_json::{Map, Value, json};

impl ProviderServices {
    pub(super) async fn integration_list(&self) -> Result<Vec<Value>, LegacyError> {
        let catalog = self.catalog().await.map_err(LegacyError::Internal)?;
        let names = catalog.environment_names();
        let mut entries = Map::new();
        for provider in catalog.providers() {
            let id = self.integration(provider.id.as_str());
            let configured = self.documents.iter().rev().find_map(|document| {
                document
                    .get("providers")
                    .and_then(|providers| providers.get(provider.id.as_str()))
            });
            let env = configured
                .and_then(|value| value.get("env"))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_else(|| {
                    names
                        .get(&opencode_schema::ids::IntegrationID::new(
                            provider.id.as_str(),
                        ))
                        .into_iter()
                        .flatten()
                        .map(|name| json!(name))
                        .collect()
                });
            if env.is_empty() && configured.and_then(|value| value.get("env")).is_none() {
                continue;
            }
            let name = configured
                .and_then(|value| value.get("name"))
                .and_then(Value::as_str)
                .unwrap_or(&provider.name);
            entries.insert(id.clone(), json!({"id":id,"name":name,"methods":[{"type":"key"},{"type":"env","names":env}],"connections":[]}));
        }
        let plugins = self.plugins.read().await.clone();
        for plugin in plugins {
            let auth = plugin
                .host
                .request("auth.methods", json!({"handle":plugin.handle}))
                .await
                .map_err(super::plugin_error)?;
            let Some(provider) = auth.get("provider").and_then(Value::as_str) else {
                continue;
            };
            let id = self.integration(provider);
            let entry = entries
                .entry(id.clone())
                .or_insert_with(|| json!({"id":id,"name":provider,"methods":[],"connections":[]}));
            let methods = entry["methods"].as_array_mut().expect("registry methods");
            for (index, method) in auth
                .get("methods")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .enumerate()
            {
                if method["type"] == "api" {
                    if !methods.iter().any(|method| method["type"] == "key") {
                        methods.push(json!({"type":"key","label":method["label"]}));
                    }
                    continue;
                }
                let mut method = method.clone();
                method["id"] = json!(index.to_string());
                if let Some(existing) = methods
                    .iter_mut()
                    .find(|existing| existing["id"] == method["id"])
                {
                    *existing = method;
                } else {
                    methods.push(method);
                }
            }
        }
        let records = self
            .database
            .read(credentials::all)
            .await
            .map_err(|error| LegacyError::Internal(error.to_string()))?;
        for (id, entry) in entries.iter_mut() {
            let mut connections: Vec<_> = records
                .iter()
                .rev()
                .filter(|record| record.integration_id == *id)
                .map(|record| json!({"type":"credential","id":record.id,"label":record.label}))
                .collect();
            for method in entry["methods"].as_array().expect("registry methods") {
                if method["type"] != "env" {
                    continue;
                }
                connections.extend(
                    method["names"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .filter(|name| {
                            self.environment
                                .get(*name)
                                .is_some_and(|value| !value.is_empty())
                        })
                        .map(|name| json!({"type":"env","name":name})),
                );
            }
            entry["connections"] = json!(connections);
        }
        let mut result: Vec<_> = entries.into_values().collect();
        result.sort_by(|left, right| left["name"].as_str().cmp(&right["name"].as_str()));
        Ok(result)
    }

    pub(super) async fn integration_operation(
        &self,
        operation: &str,
        params: &Map<String, Value>,
        payload: Option<Value>,
    ) -> Result<Value, LegacyError> {
        if operation.starts_with("v2.integration.attempt.") {
            return self.attempt_operation(operation, params, payload).await;
        }
        let id = params
            .get("integrationID")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let integrations = self.integration_list().await?;
        let integration = integrations.iter().find(|entry| entry["id"] == id);
        match operation {
            "v2.integration.list" => Ok(json!(integrations)),
            "v2.integration.get" => Ok(integration.cloned().unwrap_or(Value::Null)),
            "v2.integration.connect.key" => {
                if !integration
                    .and_then(|entry| entry["methods"].as_array())
                    .is_some_and(|methods| methods.iter().any(|method| method["type"] == "key"))
                {
                    return Err(LegacyError::Internal(format!("Key method not found: {id}")));
                }
                let payload = payload.unwrap_or(Value::Null);
                let key = payload
                    .get("key")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let label = payload
                    .get("label")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                let id = id.to_owned();
                self.database
                    .transaction(TxOptions::default(), move |sql| {
                        credentials::create(
                            sql,
                            &id,
                            &credentials::CredentialValue::Key {
                                key,
                                metadata: None,
                            },
                            label.as_deref(),
                        )
                        .map(|_| ())
                    })
                    .await
                    .map_err(|error| LegacyError::Internal(error.to_string()))?;
                Ok(Value::Null)
            }
            "v2.integration.connect.oauth" => {
                self.start_attempt(id, payload.unwrap_or(Value::Null)).await
            }
            _ => Err(LegacyError::Unavailable(format!(
                "integration operation not composed: {operation}"
            ))),
        }
    }
}
