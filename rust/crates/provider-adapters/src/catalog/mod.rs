//! Purpose: Canonical provider and model catalog for runtime model resolution.
//! Scope: Configuration-backed provider inventory, availability, projection, and defaults.
//! Uses:
//! rust/crates/
//! |-- provider-adapters/src/catalog/cache.rs
//! |-- provider-adapters/src/credentials/mod.rs
//! |-- schema/src/ids.rs
//! `-- schema/src/provider.rs
//! External:
//! |-- indexmap::IndexMap
//! |-- serde_json::Value
//! |-- serde_json::from_value
//! |-- serde_json::json
//! |-- serde_json::to_value
//! `-- str::to_owned
//! Used by:
//! rust/crates/
//! |-- provider-adapters/src/catalog/cache.rs
//! |-- provider-adapters/src/lib.rs
//! |-- provider-adapters/tests/catalog.rs
//! `-- runtime/src/location_models.rs
//! Invariants: never invent model inventory; disabled models are excluded; explicit defaults require availability.
//! Contract tests: provider-adapters/tests/catalog.rs

use crate::credentials::{CredentialError, CredentialStore};
use indexmap::IndexMap;
use opencode_schema::ids::{IntegrationID, ModelID, ProviderID};
use std::collections::HashSet;
use std::path::Path;

mod cache;
mod refresh;
mod selection;
pub use cache::CatalogError;
use opencode_schema::provider::{ModelInfo, ProviderApi, ProviderInfo};
pub use refresh::{ModelsService, cache_filename};
pub use selection::SelectionError;
use serde_json::Value;

#[derive(Debug, Clone, Default)]
pub struct CatalogStore {
    providers: IndexMap<ProviderID, ProviderInfo>,
    models: IndexMap<(ProviderID, ModelID), ModelInfo>,
    default: Option<(ProviderID, ModelID)>,
    connected: HashSet<IntegrationID>,
    integrations: HashSet<IntegrationID>,
    environment: IndexMap<IntegrationID, Vec<String>>,
    metadata: serde_json::Map<String, Value>,
}

impl CatalogStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_config(config: &Value) -> Self {
        let mut store = Self::new();
        store.apply_config(config);
        store
    }

    pub fn apply_config(&mut self, config: &Value) {
        if let Some(model) = config.get("model").and_then(Value::as_str)
            && let Some((provider, model)) = model.split_once('/')
        {
            self.set_default(provider, model);
        }
        let Some(providers) = config.get("providers").and_then(Value::as_object) else {
            return;
        };
        for (id, entry) in providers {
            let provider_id = ProviderID::new(id);
            if entry.get("env").is_some() {
                self.integrations.insert(IntegrationID::new(
                    entry
                        .get("integrationID")
                        .and_then(Value::as_str)
                        .unwrap_or(id),
                ));
            }
            let mut provider = self
                .providers
                .get(&provider_id)
                .cloned()
                .unwrap_or_else(|| ProviderInfo::empty(provider_id.clone()));
            if let Some(disabled) = entry.get("disabled").and_then(Value::as_bool) {
                provider.disabled = Some(disabled);
            }
            if let Some(integration) = entry.get("integrationID") {
                provider.integration_id = integration.as_str().map(IntegrationID::new);
            }
            if let Some(name) = entry.get("name").and_then(Value::as_str) {
                provider.name = name.to_owned();
            }
            if let Some(api) = entry.get("api")
                && let Ok(value) = serde_json::from_value::<ProviderApi>(api.clone())
            {
                provider.api = value;
            }
            if let Some(request) = entry.get("request") {
                if let Some(headers) = request.get("headers").and_then(Value::as_object) {
                    provider.request.headers.extend(
                        headers
                            .iter()
                            .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_owned()))),
                    );
                }
                if let Some(body) = request.get("body").and_then(Value::as_object) {
                    provider
                        .request
                        .body
                        .extend(body.iter().map(|(k, v)| (k.clone(), v.clone())));
                }
            }
            if let Some(url) = provider
                .request
                .body
                .shift_remove("baseURL")
                .and_then(|v| v.as_str().map(str::to_owned))
            {
                match &mut provider.api {
                    ProviderApi::Aisdk { url: current, .. }
                    | ProviderApi::Native { url: current, .. } => *current = Some(url),
                }
            }
            self.providers.insert(provider_id.clone(), provider);
            if let Some(models) = entry.get("models").and_then(Value::as_object) {
                for (model_id, model_config) in models {
                    let id = ModelID::new(model_id);
                    let key = (provider_id.clone(), id.clone());
                    let mut model = self
                        .models
                        .get(&key)
                        .cloned()
                        .unwrap_or_else(|| ModelInfo::empty(provider_id.clone(), id.clone()));
                    let mut value =
                        serde_json::to_value(&model).expect("canonical model serializes");
                    if let (Some(dst), Some(src)) =
                        (value.as_object_mut(), model_config.as_object())
                    {
                        for (field, patch) in src {
                            match field.as_str() {
                                "name" | "family" | "capabilities" => {
                                    dst.insert(field.clone(), patch.clone());
                                }
                                "cost" => {
                                    let costs = patch
                                        .as_array()
                                        .cloned()
                                        .unwrap_or_else(|| vec![patch.clone()]);
                                    dst.insert(
                                        "cost".into(),
                                        Value::Array(
                                            costs
                                                .into_iter()
                                                .map(|mut cost| {
                                                    if let Some(cost) = cost.as_object_mut() {
                                                        let cache =
                                                            cost.entry("cache").or_insert_with(
                                                                || serde_json::json!({}),
                                                            );
                                                        if let Some(cache) = cache.as_object_mut() {
                                                            cache
                                                                .entry("read")
                                                                .or_insert(Value::from(0));
                                                            cache
                                                                .entry("write")
                                                                .or_insert(Value::from(0));
                                                        }
                                                    }
                                                    cost
                                                })
                                                .collect(),
                                        ),
                                    );
                                }
                                "variants" => {
                                    if let (Some(target), Some(source)) = (
                                        dst.get_mut("variants").and_then(Value::as_array_mut),
                                        patch.as_array(),
                                    ) {
                                        for variant in source {
                                            if let Some(existing) =
                                                target.iter_mut().find(|existing| {
                                                    existing.get("id") == variant.get("id")
                                                })
                                            {
                                                for field in ["headers", "body"] {
                                                    if let (Some(target), Some(source)) = (
                                                        existing
                                                            .get_mut(field)
                                                            .and_then(Value::as_object_mut),
                                                        variant
                                                            .get(field)
                                                            .and_then(Value::as_object),
                                                    ) {
                                                        target.extend(source.clone());
                                                    }
                                                }
                                            } else {
                                                let mut value =
                                                    serde_json::json!({"headers":{},"body":{}});
                                                if let (Some(target), Some(source)) =
                                                    (value.as_object_mut(), variant.as_object())
                                                {
                                                    target.extend(source.clone());
                                                }
                                                target.push(value);
                                            }
                                        }
                                    }
                                }
                                "disabled" => {
                                    dst.insert(
                                        "enabled".into(),
                                        Value::Bool(!patch.as_bool().unwrap_or(false)),
                                    );
                                }
                                "request" => {
                                    for field in ["headers", "body"] {
                                        if let (Some(target), Some(source)) = (
                                            dst.get_mut("request")
                                                .and_then(|request| request.get_mut(field))
                                                .and_then(Value::as_object_mut),
                                            patch.get(field).and_then(Value::as_object),
                                        ) {
                                            target.extend(source.clone());
                                        }
                                    }
                                    if let Some(variant) = patch.get("variant") {
                                        dst["request"]["variant"] = variant.clone();
                                    }
                                }
                                "limit" | "api" => {
                                    if let (Some(target), Some(source)) = (
                                        dst.get_mut(field).and_then(Value::as_object_mut),
                                        patch.as_object(),
                                    ) {
                                        for (k, v) in source {
                                            target.insert(k.clone(), v.clone());
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    if let Ok(parsed) = serde_json::from_value(value) {
                        model = parsed;
                    }
                    if let Some(url) = model
                        .request
                        .body
                        .shift_remove("baseURL")
                        .and_then(|v| v.as_str().map(str::to_owned))
                    {
                        if matches!(&model.api.api, ProviderApi::Native { url: None, settings } if settings.is_empty())
                        {
                            model.api.api = self.providers[&provider_id].api.clone();
                        }
                        match &mut model.api.api {
                            ProviderApi::Aisdk { url: current, .. }
                            | ProviderApi::Native { url: current, .. } => *current = Some(url),
                        }
                    }
                    self.models.insert(key, model);
                }
            }
        }
    }

    /// Read the caller-supplied models.dev cache file; never accesses the network.
    pub fn load_cache(&mut self, path: &Path) -> Result<(), CatalogError> {
        cache::load(self, path)
    }
    pub fn apply_credentials(
        &mut self,
        credentials: &CredentialStore,
    ) -> Result<(), CredentialError> {
        let mut connected = HashSet::new();
        for provider in self.providers.values() {
            let id = provider
                .integration_id
                .clone()
                .unwrap_or_else(|| IntegrationID::new(provider.id.as_str()));
            if credentials.has_integration(&id) {
                self.integrations.insert(id.clone());
            }
            if credentials.active_credential(&id)?.is_some() {
                connected.insert(id);
            }
        }
        self.connected = connected;
        Ok(())
    }
    /// Exact explicit model lookup. An unknown selection never resolves to a default.
    pub fn model(&self, provider: &ProviderID, model: &ModelID) -> Option<ModelInfo> {
        self.available_models()
            .into_iter()
            .find(|m| &m.provider_id == provider && &m.id == model)
    }
    pub fn insert_provider(&mut self, provider: ProviderInfo) {
        self.providers.insert(provider.id.clone(), provider);
    }
    pub fn insert_model(&mut self, model: ModelInfo) {
        self.models
            .insert((model.provider_id.clone(), model.id.clone()), model);
    }
    pub fn set_default(&mut self, provider: &str, model: &str) {
        self.default = Some((ProviderID::new(provider), ModelID::new(model)));
    }
    pub fn provider(&self, id: &ProviderID) -> Option<ProviderInfo> {
        self.providers.get(id).cloned()
    }
    pub fn metadata(&self, provider: &ProviderID) -> Option<&Value> {
        self.metadata.get(provider.as_str())
    }
    pub fn environment_names(&self) -> IndexMap<IntegrationID, Vec<String>> {
        self.environment.clone()
    }
    pub fn models_for(&self, provider: &ProviderID) -> Vec<ModelInfo> {
        self.all_models()
            .into_iter()
            .filter(|model| &model.provider_id == provider)
            .collect()
    }
    pub fn providers(&self) -> Vec<ProviderInfo> {
        self.providers.values().cloned().collect()
    }
    pub fn available_providers(&self) -> Vec<ProviderInfo> {
        self.providers
            .values()
            .filter(|p| {
                let id = p
                    .integration_id
                    .clone()
                    .unwrap_or_else(|| IntegrationID::new(p.id.as_str()));
                p.disabled != Some(true)
                    && (p
                        .request
                        .body
                        .get("apiKey")
                        .and_then(Value::as_str)
                        .is_some()
                        || self.connected.contains(&id)
                        || (p.integration_id.is_none() && !self.integrations.contains(&id)))
            })
            .cloned()
            .collect()
    }
    pub fn available_models(&self) -> Vec<ModelInfo> {
        let allowed: std::collections::HashSet<_> = self
            .available_providers()
            .into_iter()
            .map(|p| p.id)
            .collect();
        self.all_models()
            .into_iter()
            .filter(|model| model.enabled && allowed.contains(&model.provider_id))
            .collect()
    }
    pub fn all_models(&self) -> Vec<ModelInfo> {
        let mut models: Vec<_> = self.models.values().cloned().map(|mut model| {
            if let Some(provider) = self.providers.get(&model.provider_id) {
                if matches!(&model.api.api, ProviderApi::Native { url: None, settings } if settings.is_empty()) {
                    model.api.api = provider.api.clone();
                }
                if let (ProviderApi::Aisdk { url, settings, .. }, ProviderApi::Aisdk { url: base_url, settings: base_settings, .. }) = (&mut model.api.api, &provider.api) {
                    if url.is_none() { *url = base_url.clone(); }
                    let mut merged = base_settings.clone().unwrap_or_default();
                    merged.extend(settings.clone().unwrap_or_default());
                    if !merged.is_empty() { *settings = Some(merged); }
                }
                let mut headers = provider.request.headers.clone();
                headers.extend(model.request.headers);
                model.request.headers = headers;
                let mut body = provider.request.body.clone();
                body.extend(model.request.body);
                model.request.body = body;
            }
            model
        }).collect();
        models.sort_by(|a, b| {
            b.time
                .released
                .get()
                .partial_cmp(&a.time.released.get())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        models
    }
    pub fn default_model(&self) -> Option<ModelInfo> {
        let available = self.available_models();
        self.default
            .as_ref()
            .and_then(|key| {
                available
                    .iter()
                    .find(|m| (&m.provider_id, &m.id) == (&key.0, &key.1))
                    .cloned()
            })
            .or_else(|| available.into_iter().next())
    }
}
