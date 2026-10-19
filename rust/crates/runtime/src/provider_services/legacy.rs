//! Purpose: project canonical inventory into the legacy provider/model JSON shape.
//! Scope: serialization only; discovery and credential access remain owned by ProviderServices.
//! Uses:
//! rust/crates/
//! |-- provider-adapters/src/catalog/mod.rs
//! |-- runtime/src/provider_services.rs
//! `-- schema/src/provider.rs
//! External:
//! `-- serde_json::json
//! Used by:
//! rust/crates/
//! `-- runtime/src/provider_services.rs
//! Invariants: V1 flags retained from catalog metadata never become outbound request fields.
//! Contract tests: runtime/tests/provider_services.rs

use super::ProviderServices;
use opencode_provider_adapters::catalog::CatalogStore;
use opencode_schema::provider::{ModelInfo, ProviderApi};
use serde_json::{Map, Value, json};

pub(super) async fn list(service: &ProviderServices) -> Result<Value, String> {
    let catalog = service.catalog().await?;
    let active = catalog.available_providers();
    let credentials = service.auth_list().await?;
    let names = catalog.environment_names();
    let mut defaults = Map::new();
    let all: Vec<_> = catalog.providers().into_iter().filter(|provider| provider.disabled != Some(true)).map(|provider| {
        let configured = service.documents.iter().any(|document| document.get("providers").and_then(|providers|providers.get(provider.id.as_str())).is_some());
        let integration = service.integration(provider.id.as_str());
        let env = service.documents.iter().rev().find_map(|document| document.pointer(&format!("/providers/{}/env", provider.id.as_str())).and_then(Value::as_array).cloned())
            .unwrap_or_else(||names.get(&opencode_schema::ids::IntegrationID::new(provider.id.as_str())).into_iter().flatten().map(|name|json!(name)).collect());
        let from_env = env.iter().filter_map(Value::as_str).any(|name|service.environment.get(name).is_some_and(|value|!value.is_empty()));
        let credential = credentials.get(&integration);
        let source = if configured { "config" } else if credential.is_some_and(|value|value["type"] == "api") { "api" } else if from_env { "env" } else { "custom" };
        let models: Map<_, _> = catalog.models_for(&provider.id).into_iter().filter(|model|model.enabled).map(|model| (model.id.as_str().to_owned(), project(&catalog, &model))).collect();
        if let Some(model) = catalog.legacy_default_for(&provider.id) { defaults.insert(provider.id.as_str().to_owned(),json!(model.id)); }
        let mut result = json!({"id":provider.id,"name":provider.name,"source":source,"env":env,"options":provider.request.body,"models":models});
        if let Some(key) = credential.and_then(|value|value.get("key")) { result["key"] = key.clone(); }
        else if env.len() == 1 && let Some(key) = env[0].as_str().and_then(|name|service.environment.get(name)).filter(|value|!value.is_empty()) { result["key"] = json!(key); }
        result
    }).collect();
    Ok(
        json!({"all":all,"default":defaults,"connected":active.into_iter().map(|provider|provider.id).collect::<Vec<_>>()}),
    )
}

fn project(catalog: &CatalogStore, model: &ModelInfo) -> Value {
    let raw = catalog
        .metadata(&model.provider_id)
        .and_then(|provider| provider.get("models"))
        .and_then(|models| models.get(model.api.id.as_str()));
    let modalities = |items: &[String]| {
        ["text", "audio", "image", "video", "pdf"]
            .into_iter()
            .map(|kind| {
                (
                    kind.to_owned(),
                    json!(items.iter().any(|item| item == kind)),
                )
            })
            .collect::<Map<_, _>>()
    };
    let (npm, url) = match &model.api.api {
        ProviderApi::Aisdk { package, url, .. } => {
            (package.as_str(), url.as_deref().unwrap_or_default())
        }
        ProviderApi::Native { url, .. } => (
            "@ai-sdk/openai-compatible",
            url.as_deref().unwrap_or_default(),
        ),
    };
    let flag = |name: &str| {
        raw.and_then(|raw| raw.get(name))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    };
    let mut interleaved = raw
        .and_then(|raw| raw.get("interleaved"))
        .cloned()
        .unwrap_or(json!(false));
    if interleaved.is_string() {
        interleaved = json!({"field":interleaved});
    }
    let cost = model
        .cost
        .first()
        .map(|cost| serde_json::to_value(cost).expect("cost serializes"))
        .unwrap_or(json!({"input":0,"output":0,"cache":{"read":0,"write":0}}));
    let mut result = json!({
        "id":model.id,"providerID":model.provider_id,"api":{"id":model.api.id,"url":url,"npm":npm},"name":model.name,
        "capabilities":{"temperature":flag("temperature"),"reasoning":flag("reasoning"),"attachment":flag("attachment"),"toolcall":model.capabilities.tools,
            "input":modalities(&model.capabilities.input),"output":modalities(&model.capabilities.output),"interleaved":interleaved},
        "cost":cost,"limit":model.limit,"status":model.status,"options":model.request.body,"headers":model.request.headers,
        "release_date":raw.and_then(|raw|raw.get("release_date")).and_then(Value::as_str).unwrap_or_default(),
        "variants":model.variants.iter().map(|variant|(variant.id.as_str().to_owned(),json!(variant.body))).collect::<Map<_,_>>()
    });
    if let Some(family) = &model.family {
        result["family"] = json!(family);
    }
    if raw.is_none() && model.family.is_none() {
        result["family"] = json!("");
    }
    let tiers: Vec<_> = model
        .cost
        .iter()
        .skip(1)
        .map(|cost| serde_json::to_value(cost).expect("cost serializes"))
        .collect();
    if !tiers.is_empty() {
        result["cost"]["tiers"] = json!(tiers);
    }
    result
}
