//! Purpose: serve current provider, model and credential operations from concrete runtime services.
//! Scope: raw data boundary; HTTP host adds the resolved Location envelope.
//! Uses:
//! rust/crates/
//! |-- runtime/src/provider_services.rs
//! |-- server/src/services/legacy.rs
//! `-- storage-sqlite/src/repositories/credentials.rs
//! External:
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! `-- runtime/src/provider_services.rs
//! Invariants: no credential secret is returned by catalog endpoints; current mutations use credential IDs.
//! Contract tests: runtime/tests/provider_services.rs

use super::ProviderServices;
use opencode_schema::ids::ProviderID;
use opencode_server::services::legacy::LegacyError;
use opencode_storage_api::{DatabaseExt, TxOptions};
use opencode_storage_sqlite::repositories::credentials;
use serde_json::{Map, Value, json};

impl ProviderServices {
    pub(super) async fn current(
        &self,
        operation: &str,
        params: &Map<String, Value>,
        payload: Option<Value>,
    ) -> Result<Value, LegacyError> {
        match operation {
            "v2.provider.list" => serde_json::to_value(
                self.catalog()
                    .await
                    .map_err(LegacyError::Internal)?
                    .available_providers(),
            )
            .map_err(|error| LegacyError::Internal(error.to_string())),
            "v2.model.list" => serde_json::to_value(
                self.catalog()
                    .await
                    .map_err(LegacyError::Internal)?
                    .available_models(),
            )
            .map_err(|error| LegacyError::Internal(error.to_string())),
            "v2.provider.get" => {
                let id = params
                    .get("providerID")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let catalog = self.catalog().await.map_err(LegacyError::Internal)?;
                let provider = catalog.provider(&ProviderID::new(id)).ok_or_else(|| LegacyError::Response {status:404,body:json!({"_tag":"ProviderNotFoundError","providerID":id,"message":format!("Provider not found: {id}")})})?;
                serde_json::to_value(provider)
                    .map_err(|error| LegacyError::Internal(error.to_string()))
            }
            "v2.credential.update" | "v2.credential.remove" => {
                let id = params
                    .get("credentialID")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let label = payload
                    .as_ref()
                    .and_then(|value| value.get("label"))
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                let remove = operation == "v2.credential.remove";
                self.database
                    .transaction(TxOptions::default(), move |sql| {
                        if remove {
                            return credentials::remove(sql, &id);
                        }
                        credentials::update(sql, &id, label.as_deref(), None)
                    })
                    .await
                    .map_err(|error| LegacyError::Internal(error.to_string()))?;
                Ok(Value::Null)
            }
            _ => Err(LegacyError::Unavailable(format!(
                "provider operation not composed: {operation}"
            ))),
        }
    }
}
