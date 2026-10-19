//! Purpose: Exercise canonical config, inventory, availability and model projection.
//! Scope: Public CatalogStore API using local models.dev cache files.
//! Uses:
//! rust/crates/
//! |-- provider-adapters/src/catalog/mod.rs
//! |-- provider-adapters/src/credentials/mod.rs
//! |-- schema/src/ids.rs
//! `-- schema/src/provider.rs
//! External:
//! |-- serde_json::json
//! |-- serde_json::to_vec
//! `-- tempfile::tempdir
//! Used by:
//! `-- none, entrypoint
//! Invariants: no invented inventory, no unknown-selection fallback, model overrides win.
//! Contract tests: provider-adapters/tests/catalog.rs

use opencode_provider_adapters::{catalog::CatalogStore, credentials::CredentialStore};
use opencode_schema::ids::{ModelID, ProviderID};
use opencode_schema::provider::ProviderApi;
use serde_json::json;

#[test]
fn configured_env_requires_connection_but_empty_cache_env_does_not() {
    let catalog = CatalogStore::from_config(
        &json!({"providers":{"secured":{"env":["TOKEN"],"models":{"configured":{}}}}}),
    );
    assert!(catalog.available_models().is_empty());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.json");
    std::fs::write(&path, serde_json::to_vec(&json!({"ollama":{"id":"ollama","name":"Ollama","env":[],"models":{
        "llama3.2":{"id":"llama3.2","name":"Llama 3.2","release_date":"2024-09-25","tool_call":true,"limit":{"context":131072,"output":8192}}
    }}})).unwrap()).unwrap();
    let mut catalog = CatalogStore::new();
    catalog.load_cache(&path).unwrap();
    let mut credentials = CredentialStore::new();
    credentials.load_cache(&path).unwrap();
    catalog.apply_credentials(&credentials).unwrap();
    assert_eq!(catalog.default_model().unwrap().id.as_str(), "llama3.2");
}

#[test]
fn cost_objects_and_variant_overlays_follow_config_contract() {
    let mut catalog =
        CatalogStore::from_config(&json!({"providers":{"local":{"models":{"configured":{
            "cost":{"input":1,"output":2},
            "variants":[{"id":"fast","headers":{"first":"one"},"body":{"effort":"low"}}]
        }}}}}));
    catalog.apply_config(&json!({"providers":{"local":{"models":{"configured":{
        "variants":[{"id":"fast","headers":{"second":"two"},"body":{"effort":"medium"}}]
    }}}}}));
    let model = catalog.default_model().unwrap();
    assert_eq!(model.cost[0].input.get(), 1.0);
    assert_eq!(model.cost[0].cache.read.get(), 0.0);
    assert_eq!(model.variants.len(), 1);
    assert_eq!(model.variants[0].headers["first"], "one");
    assert_eq!(model.variants[0].headers["second"], "two");
    assert_eq!(model.variants[0].body["effort"], "medium");
}

#[test]
fn missing_cache_keeps_explicit_local_models_usable() {
    let dir = tempfile::tempdir().unwrap();
    let mut catalog =
        CatalogStore::from_config(&json!({"providers":{"local":{"models":{"configured":{}}}}}));
    catalog.load_cache(&dir.path().join("models.json")).unwrap();
    let mut credentials = CredentialStore::new();
    credentials
        .load_cache(&dir.path().join("models.json"))
        .unwrap();
    catalog.apply_credentials(&credentials).unwrap();
    assert_eq!(catalog.default_model().unwrap().id.as_str(), "configured");
    assert!(CatalogStore::new().available_models().is_empty());
    assert!(
        catalog
            .model(&ProviderID::new("local"), &ModelID::new("unknown"))
            .is_none()
    );
}

#[test]
fn cached_inventory_config_and_environment_are_composed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.json");
    std::fs::write(&path, serde_json::to_vec(&json!({"openai":{
        "id":"openai", "name":"OpenAI", "env":["OPENAI_API_KEY"], "npm":"@ai-sdk/openai", "api":"https://api.openai.com/v1",
        "models":{
            "gpt-4o":{"id":"gpt-4o","name":"GPT-4o","release_date":"2024-05-13","tool_call":true,"limit":{"context":128000,"output":16384},"modalities":{"input":["text","image"],"output":["text"]},"cost":{"input":2.5,"output":10,"cache_read":1.25}},
            "gpt-4o-mini":{"id":"gpt-4o-mini","name":"GPT-4o mini","release_date":"2024-07-18","tool_call":true,"limit":{"context":128000,"output":16384}}
        }
    }})).unwrap()).unwrap();
    let mut catalog = CatalogStore::new();
    catalog.load_cache(&path).unwrap();
    assert!(catalog.available_models().is_empty());
    let mut credentials = CredentialStore::new()
        .with_environment([("OPENAI_API_KEY".into(), "snapshot-key".into())].into());
    credentials.load_cache(&path).unwrap();
    catalog.apply_credentials(&credentials).unwrap();
    assert_eq!(catalog.default_model().unwrap().id.as_str(), "gpt-4o-mini");
    let config = json!({"model":"openai/gpt-4o","providers":{"openai":{
        "request":{"headers":{"x-base":"provider","x-conflict":"provider"},"body":{"temperature":0.3,"baseURL":"https://proxy.test/v1"}},
        "models":{"gpt-4o":{"request":{"headers":{"x-conflict":"model"},"body":{"temperature":0.7}}},"gpt-4o-mini":{"disabled":true}}
    }}});
    catalog.apply_config(&config);
    catalog.apply_config(&json!({"providers":{"openai":{"models":{"gpt-4o":{"request":{"headers":{"x-next":"next"},"body":{"seed":3}}}}}}}));
    let model = catalog.default_model().unwrap();
    assert_eq!(model.id.as_str(), "gpt-4o");
    assert_eq!(model.request.headers["x-base"], "provider");
    assert_eq!(model.request.headers["x-conflict"], "model");
    assert_eq!(model.request.headers["x-next"], "next");
    assert_eq!(model.request.body["temperature"], json!(0.7));
    assert_eq!(model.request.body["seed"], json!(3));
    assert!(!model.request.body.contains_key("baseURL"));
    assert!(
        matches!(model.api.api, ProviderApi::Aisdk { url: Some(ref url), .. } if url == "https://proxy.test/v1")
    );
    assert_eq!(model.cost[0].input.get(), 2.5);
    assert_eq!(model.cost[0].cache.read.get(), 1.25);
    assert!(model.capabilities.tools);
    assert_eq!(model.capabilities.input, ["text", "image"]);
    assert_eq!(catalog.available_models().len(), 1);
    catalog.apply_config(&json!({"providers":{"openai":{"disabled":true}}}));
    assert!(catalog.default_model().is_none());
}

#[test]
fn integration_credentials_enable_provider_but_disabled_always_wins() {
    let mut catalog = CatalogStore::from_config(&json!({"providers":{
        "linked":{"integrationID":"account","models":{"known":{}}},
        "disabled":{"integrationID":"account","disabled":true,"request":{"body":{"apiKey":"configured"}},"models":{"hidden":{}}}
    }}));
    assert!(catalog.available_models().is_empty());
    let mut credentials = CredentialStore::new();
    credentials.load_records(&json!([{"integrationID":"account","value":{"type":"oauth","methodID":"oauth","access":"access","refresh":"refresh","expires":1}}])).unwrap();
    catalog.apply_credentials(&credentials).unwrap();
    assert_eq!(catalog.available_models().len(), 1);
    assert_eq!(
        catalog.default_model().unwrap().provider_id.as_str(),
        "linked"
    );
    assert!(
        catalog
            .model(&ProviderID::new("linked"), &ModelID::new("missing"))
            .is_none()
    );
}

#[test]
fn malformed_cache_is_an_error_and_does_not_partially_change_inventory() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.json");
    std::fs::write(
        &path,
        r#"{"a":{"name":"valid","models":{}},"b":{"name":"broken"}}"#,
    )
    .unwrap();
    let mut catalog = CatalogStore::new();
    assert!(catalog.load_cache(&path).is_err());
    assert!(catalog.providers().is_empty());
}

#[test]
fn model_base_url_and_api_settings_overlay_are_projected() {
    let catalog = CatalogStore::from_config(&json!({"providers":{"local":{
        "api":{"type":"aisdk","package":"@ai-sdk/openai-compatible","url":"https://provider.test","settings":{"base":true,"same":"base"}},
        "models":{"configured":{"api":{"type":"aisdk","package":"@ai-sdk/openai-compatible","settings":{"same":"model"}},"request":{"body":{"baseURL":"https://model.test"}}}}
    }}}));
    let model = catalog.default_model().unwrap();
    match model.api.api {
        ProviderApi::Aisdk {
            url,
            settings: Some(settings),
            ..
        } => {
            assert_eq!(url.as_deref(), Some("https://model.test"));
            assert_eq!(settings["base"], true);
            assert_eq!(settings["same"], "model");
        }
        _ => panic!("expected AISDK API"),
    }
    assert!(!model.request.body.contains_key("baseURL"));
}
