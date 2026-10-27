//! Purpose: normalize Zen catalog composite provider keys for native routing.
//! Scope: catalog models and provider definitions, preserving public aliases.
//! Uses:
//! `-- none
//! External:
//! `-- serde_json::Value
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/zen/legacy.rs
//! Invariants: only the first composite provider expands model entries, matching ZenData.list.
//! Contract tests: hosted-adapters/tests/console_gateway_limits.rs

use serde_json::{Map, Value, json};

pub fn normalize(
    catalog: &Value,
    lite: bool,
    model_id: &str,
    format: &str,
) -> Option<(Value, Map<String, Value>)> {
    let providers = catalog.get("providers")?.as_object()?;
    let model = catalog
        .get(if lite { "liteModels" } else { "zenModels" })?
        .get(model_id)?;
    let mut model = if let Some(models) = model.as_array() {
        models.iter().find(|m| m["formatFilter"] == format)?.clone()
    } else {
        model.clone()
    };
    let keys = providers
        .iter()
        .map(|(id, p)| {
            let keys = if let Some(keys) = p["apiKey"].as_object() {
                keys.iter()
                    .map(|(key, value)| (format!("{id}.{key}"), value.clone()))
                    .collect::<Vec<_>>()
            } else {
                vec![(id.clone(), p["apiKey"].clone())]
            };
            (id.clone(), keys)
        })
        .collect::<std::collections::HashMap<_, _>>();
    let expanded = providers
        .iter()
        .flat_map(|(id, provider)| {
            keys[id].iter().map(|(id, key)| {
                let mut provider = provider.clone();
                provider["apiKey"] = key.clone();
                (id.clone(), provider)
            })
        })
        .collect::<Map<_, _>>();
    let configured = model.get("providers")?.as_array()?;
    let composite = configured.iter().find_map(|p| {
        let id = p["id"].as_str()?;
        (keys.get(id)?.len() > 1).then_some(id.to_owned())
    });
    let multiplier = composite.as_ref().map(|id| keys[id].len()).unwrap_or(1);
    let normalized = configured
        .iter()
        .flat_map(|provider| {
            let id = provider["id"].as_str().unwrap_or_default();
            if composite.as_deref() == Some(id) {
                return keys[id]
                    .iter()
                    .map(|(id, _)| {
                        let mut p = provider.clone();
                        p["id"] = json!(id);
                        if p.get("weight").is_none() {
                            p["weight"] = json!(1);
                        }
                        p
                    })
                    .collect::<Vec<_>>();
            }
            let mut provider = provider.clone();
            provider["weight"] =
                json!(provider["weight"].as_u64().unwrap_or(1) * multiplier as u64);
            vec![provider]
        })
        .collect::<Vec<_>>();
    model["providers"] = json!(normalized);
    if let Some(trial) = model["trialProvider"].as_str() {
        model["trialProvider"] = if composite.as_deref() == Some(trial) {
            json!(keys[trial].iter().map(|(id, _)| id).collect::<Vec<_>>())
        } else {
            json!([trial])
        };
    }
    if model.get("costMultiplier").is_none() {
        model["costMultiplier"] = json!(1);
    }
    Some((model, expanded))
}
