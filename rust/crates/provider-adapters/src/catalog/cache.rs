//! Purpose: Decode the supplied models.dev cache into canonical catalog inventory.
//! Scope: Local file loading and models.dev to schema projection; no network or ambient paths.
//! Uses:
//! rust/crates/
//! |-- provider-adapters/src/catalog/mod.rs
//! |-- schema/src/ids.rs
//! `-- schema/src/provider.rs
//! External:
//! |-- serde_json::Value
//! |-- serde_json::from_slice
//! |-- serde_json::from_value
//! |-- serde_json::json
//! |-- serde_json::to_value
//! |-- str::to_owned
//! |-- time::Date::from_calendar_date
//! `-- time::Month::try_from
//! Used by:
//! rust/crates/
//! `-- provider-adapters/src/catalog/mod.rs
//! Invariants: IDs come only from supplied inventory; malformed input leaves the catalog unchanged.
//! Contract tests: provider-adapters/tests/catalog.rs

use super::CatalogStore;
use opencode_schema::ids::{IntegrationID, ModelID, ProviderID};
use opencode_schema::provider::{ModelInfo, ProviderApi, ProviderInfo};
use serde_json::{Value, json};
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("cannot read model cache: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid model cache: {0}")]
    Invalid(String),
    #[error("cannot fetch model cache: {0}")]
    Fetch(String),
}

pub(super) fn load(store: &mut CatalogStore, path: &Path) -> Result<(), CatalogError> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    decode(store, &bytes)
}

pub(super) fn decode(store: &mut CatalogStore, bytes: &[u8]) -> Result<(), CatalogError> {
    let value: Value =
        serde_json::from_slice(bytes).map_err(|error| CatalogError::Invalid(error.to_string()))?;
    let entries = value
        .as_object()
        .ok_or_else(|| CatalogError::Invalid("expected provider object".into()))?;
    let mut next = store.clone();
    next.metadata.extend(entries.clone());
    for (provider_id, entry) in entries {
        let mut provider = ProviderInfo::empty(ProviderID::new(provider_id));
        provider.name = entry
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| CatalogError::Invalid("missing provider name".into()))?
            .to_owned();
        provider.api = api(entry, None)?;
        if entry
            .get("env")
            .and_then(Value::as_array)
            .is_some_and(|names| !names.is_empty())
        {
            next.integrations.insert(IntegrationID::new(provider_id));
        }
        if let Some(names) = entry.get("env").and_then(Value::as_array) {
            next.environment.insert(
                IntegrationID::new(provider_id),
                names
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect(),
            );
        }
        let models = entry
            .get("models")
            .and_then(Value::as_object)
            .ok_or_else(|| CatalogError::Invalid("missing models object".into()))?;
        for (id, entry) in models {
            let mut value =
                serde_json::to_value(ModelInfo::empty(provider.id.clone(), ModelID::new(id)))
                    .expect("canonical model serializes");
            value["name"] = entry
                .get("name")
                .cloned()
                .ok_or_else(|| CatalogError::Invalid("missing model name".into()))?;
            if let Some(family) = entry.get("family") {
                value["family"] = family.clone();
            }
            if let Some(status) = entry.get("status") {
                value["status"] = status.clone();
            }
            value["limit"] = entry
                .get("limit")
                .cloned()
                .ok_or_else(|| CatalogError::Invalid("missing model limits".into()))?;
            value["capabilities"] = json!({
                "tools": entry.get("tool_call").and_then(Value::as_bool).unwrap_or(false),
                "input": entry.pointer("/modalities/input").cloned().unwrap_or_else(|| json!([])),
                "output": entry.pointer("/modalities/output").cloned().unwrap_or_else(|| json!([]))
            });
            if let Some(date) = entry.get("release_date").and_then(Value::as_str) {
                let parts: Vec<_> = date.split('-').collect();
                if parts.len() == 3 {
                    let date = parts[0]
                        .parse::<i32>()
                        .ok()
                        .zip(parts[1].parse::<u8>().ok())
                        .zip(parts[2].parse::<u8>().ok())
                        .and_then(|((year, month), day)| {
                            time::Month::try_from(month).ok().and_then(|month| {
                                time::Date::from_calendar_date(year, month, day).ok()
                            })
                        });
                    if let Some(date) = date {
                        value["time"]["released"] =
                            json!(date.midnight().assume_utc().unix_timestamp() * 1000);
                    }
                }
            }
            if let Some(overrides) = entry.get("provider") {
                let model_api = api(overrides, Some(&provider.api))?;
                value["api"] = serde_json::to_value(model_api).expect("canonical api serializes");
                value["api"]["id"] = json!(id);
            }
            let cost = entry.get("cost").cloned().unwrap_or_else(|| json!({}));
            {
                let cost = &cost;
                let mut costs = vec![project_cost(cost)];
                if let Some(tiers) = cost.get("tiers").and_then(Value::as_array) {
                    costs.extend(tiers.iter().map(project_cost));
                }
                if let Some(high) = cost.get("context_over_200k") {
                    let mut high = project_cost(high);
                    high["tier"] = json!({"type":"context","size":200000});
                    costs.push(high);
                }
                value["cost"] = json!(costs);
            }
            let model: ModelInfo = serde_json::from_value(value)
                .map_err(|error| CatalogError::Invalid(error.to_string()))?;
            next.insert_model(model.clone());
            for (mode, options) in entry
                .pointer("/experimental/modes")
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
            {
                let mut variant = model.clone();
                variant.id = ModelID::new(format!("{id}-{mode}"));
                let mut letters = mode.chars();
                variant.name = format!(
                    "{} {}{}",
                    model.name,
                    letters
                        .next()
                        .map(|letter| letter.to_uppercase().to_string())
                        .unwrap_or_default(),
                    letters.as_str()
                );
                if let Some(cost) = options.get("cost") {
                    variant.cost[0] = serde_json::from_value(project_cost(cost))
                        .map_err(|error| CatalogError::Invalid(error.to_string()))?;
                    for cost in cost
                        .get("tiers")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                    {
                        let cost: opencode_schema::provider::ModelCost =
                            serde_json::from_value(project_cost(cost))
                                .map_err(|error| CatalogError::Invalid(error.to_string()))?;
                        if let Some(existing) = variant
                            .cost
                            .iter_mut()
                            .find(|existing| existing.tier == cost.tier)
                        {
                            *existing = cost;
                        } else {
                            variant.cost.push(cost);
                        }
                    }
                }
                if let Some(body) = options.pointer("/provider/body").and_then(Value::as_object) {
                    variant.request.body.extend(body.clone());
                }
                if let Some(headers) = options
                    .pointer("/provider/headers")
                    .and_then(Value::as_object)
                {
                    variant
                        .request
                        .headers
                        .extend(headers.iter().filter_map(|(key, value)| {
                            value.as_str().map(|value| (key.clone(), value.to_owned()))
                        }));
                }
                next.insert_model(variant);
            }
        }
        next.insert_provider(provider);
    }
    *store = next;
    Ok(())
}

fn api(entry: &Value, base: Option<&ProviderApi>) -> Result<ProviderApi, CatalogError> {
    let package = entry.get("npm").and_then(Value::as_str);
    let url = entry.get("api").and_then(Value::as_str).map(str::to_owned);
    match (package, base) {
        (Some(package), _) => Ok(ProviderApi::Aisdk {
            package: package.to_owned(),
            url,
            settings: None,
        }),
        (
            None,
            Some(ProviderApi::Aisdk {
                package,
                url: original,
                settings,
            }),
        ) => Ok(ProviderApi::Aisdk {
            package: package.clone(),
            url: url.or_else(|| original.clone()),
            settings: settings.clone(),
        }),
        _ => Ok(ProviderApi::Native {
            url,
            settings: Default::default(),
        }),
    }
}

fn project_cost(cost: &Value) -> Value {
    let mut projected = json!({
        "input": cost.get("input").cloned().unwrap_or_else(|| json!(0)),
        "output": cost.get("output").cloned().unwrap_or_else(|| json!(0)),
        "cache": {"read":cost.get("cache_read").cloned().unwrap_or_else(|| json!(0)),"write":cost.get("cache_write").cloned().unwrap_or_else(|| json!(0))}
    });
    if let Some(tier) = cost.get("tier") {
        projected["tier"] = tier.clone();
    }
    projected
}
