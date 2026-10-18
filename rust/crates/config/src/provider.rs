//! Purpose: provider config plugin (`packages/core/src/config/plugin/provider.ts:9-113`): applies
//! `providers` from current config documents to integrations (name, env method) and to the
//! catalog (default model, provider name/api/request, model overrides, variants, cost, limits).
//! Scope: pure transforms over a catalog draft.
//! Uses:
//! rust/crates/
//! |-- config/src/current.rs
//! `-- schema/src/provider.rs
//! External:
//! `-- serde_json::{Map, Value, json}
//! Used by:
//! rust/crates/
//! `-- config/src/lib.rs
//! Invariants: documents apply lowest to highest priority; header/body maps merge with
//! `Object.assign` semantics; `api` on models shallow-merges; cache costs default to 0; edits
//! that would produce an invalid `ProviderInfo`/`ModelInfo` are dropped rather than corrupting it.
//! Contract tests: config/tests/legacy_update.rs

use crate::current::{Entry, latest};
use opencode_schema::provider::{ModelInfo, ProviderInfo};
use serde_json::{Map, Value, json};

/// Integration registry operations the provider plugin needs.
pub trait IntegrationDraft {
    fn exists(&self, id: &str) -> bool;
    fn set_name(&mut self, id: &str, name: Option<&str>);
    fn set_env_method(&mut self, id: &str, names: &[String]);
}

/// Catalog draft operations (`ctx.catalog.transform`). `update_*` creates missing entries.
pub trait CatalogDraft {
    fn set_default(&mut self, provider: &str, model: &str);
    fn update_provider(&mut self, id: &str, edit: &mut dyn FnMut(&mut ProviderInfo));
    fn update_model(&mut self, provider: &str, model: &str, edit: &mut dyn FnMut(&mut ModelInfo));
}

fn providers(entries: &[Entry]) -> impl Iterator<Item = (&String, &Value)> {
    entries.iter().filter_map(Entry::document).flat_map(|doc| {
        doc.info
            .get("providers")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
    })
}

/// The integration transform.
pub fn apply_integrations(entries: &[Entry], draft: &mut dyn IntegrationDraft) {
    let configured: Vec<&String> = providers(entries)
        .filter(|(_, p)| p.get("env").is_some())
        .map(|(id, _)| id)
        .collect();
    for (id, item) in providers(entries) {
        if !configured.contains(&id) && !draft.exists(id) {
            continue;
        }
        draft.set_name(id, item.get("name").and_then(Value::as_str));
        if let Some(env) = item.get("env").and_then(Value::as_array) {
            let names: Vec<String> = env
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect();
            draft.set_env_method(id, &names);
        }
    }
}

/// `ModelV2.parse`: `provider/model/with/slashes`.
pub fn parse_model(input: &str) -> (String, String) {
    match input.split_once('/') {
        Some((provider, model)) => (provider.to_string(), model.to_string()),
        None => (input.to_string(), String::new()),
    }
}

fn assign(target: &mut Value, source: Option<&Value>) {
    let (Some(target), Some(Value::Object(source))) = (target.as_object_mut(), source) else {
        return;
    };
    for (key, value) in source {
        target.insert(key.clone(), value.clone());
    }
}

fn edit_via_json<T: serde::Serialize + serde::de::DeserializeOwned>(
    target: &mut T,
    edit: impl FnOnce(&mut Value),
) {
    let Ok(mut value) = serde_json::to_value(&*target) else {
        return;
    };
    edit(&mut value);
    if let Ok(next) = serde_json::from_value(value) {
        *target = next;
    }
}

/// The catalog transform.
pub fn apply_catalog(entries: &[Entry], catalog: &mut dyn CatalogDraft) {
    if let Some(model) = latest(entries, "model").and_then(Value::as_str) {
        let (provider, model) = parse_model(model);
        catalog.set_default(&provider, &model);
    }
    for (provider_id, item) in providers(entries) {
        catalog.update_provider(provider_id, &mut |provider| {
            edit_via_json(provider, |value| {
                if let Some(name) = item.get("name") {
                    value["name"] = name.clone();
                }
                if let Some(api) = item.get("api") {
                    value["api"] = api.clone();
                }
                if let Some(request) = item.get("request") {
                    assign(&mut value["request"]["headers"], request.get("headers"));
                    assign(&mut value["request"]["body"], request.get("body"));
                }
            });
        });
        for (model_id, config) in item
            .get("models")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            catalog.update_model(provider_id, model_id, &mut |model| {
                edit_via_json(model, |value| apply_model(value, config))
            });
        }
    }
}

fn apply_model(model: &mut Value, config: &Value) {
    for key in ["family", "name"] {
        if let Some(value) = config.get(key) {
            model[key] = value.clone();
        }
    }
    if let Some(api) = config.get("api") {
        assign(&mut model["api"], Some(api));
    }
    if let Some(capabilities) = config.get("capabilities") {
        model["capabilities"] = capabilities.clone();
    }
    if let Some(request) = config.get("request") {
        assign(&mut model["request"]["headers"], request.get("headers"));
        assign(&mut model["request"]["body"], request.get("body"));
        if let Some(variant) = request.get("variant") {
            model["request"]["variant"] = variant.clone();
        }
    }
    for variant in config
        .get("variants")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let variants = model["variants"].as_array_mut();
        let Some(variants) = variants else { continue };
        let index = variants
            .iter()
            .position(|item| item.get("id") == variant.get("id"))
            .unwrap_or_else(|| {
                variants.push(json!({ "id": variant.get("id"), "headers": {}, "body": {} }));
                variants.len() - 1
            });
        assign(&mut variants[index]["headers"], variant.get("headers"));
        assign(&mut variants[index]["body"], variant.get("body"));
    }
    if let Some(cost) = config.get("cost") {
        let list = match cost {
            Value::Array(items) => items.clone(),
            other => vec![other.clone()],
        };
        model["cost"] = Value::Array(
            list.iter()
                .map(|cost| {
                    let mut out = Map::new();
                    if let Some(tier) = cost.get("tier") {
                        out.insert("tier".into(), tier.clone());
                    }
                    out.insert(
                        "input".into(),
                        cost.get("input").cloned().unwrap_or(json!(0)),
                    );
                    out.insert(
                        "output".into(),
                        cost.get("output").cloned().unwrap_or(json!(0)),
                    );
                    let cache = cost.get("cache");
                    let pick =
                        |key: &str| cache.and_then(|c| c.get(key)).cloned().unwrap_or(json!(0));
                    out.insert(
                        "cache".into(),
                        json!({ "read": pick("read"), "write": pick("write") }),
                    );
                    Value::Object(out)
                })
                .collect(),
        );
    }
    if let Some(disabled) = config.get("disabled").and_then(Value::as_bool) {
        model["enabled"] = Value::Bool(!disabled);
    }
    if let Some(limit) = config.get("limit") {
        assign(&mut model["limit"], Some(limit));
    }
}
