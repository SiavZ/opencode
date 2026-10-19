//! Purpose: compare current and legacy model selection and mode discovery public observations.
//! Scope: pure catalogs and explicit fixture time, no network or environment.
//! Uses:
//! rust/crates/
//! |-- provider-adapters/src/catalog/mod.rs
//! `-- provider-adapters/src/catalog/selection.rs
//! External:
//! `-- serde_json::json
//! Used by:
//! `-- none, entrypoint
//! Invariants: generations keep distinct default ordering and exact explicit identities.
//! Contract tests: this file

use opencode_provider_adapters::catalog::{CatalogStore, SelectionError};
use opencode_schema::ids::{ModelID, ProviderID};
use serde_json::json;

#[test]
fn legacy_priority_recent_and_explicit_defaults_are_not_v2_release_order() {
    let catalog = CatalogStore::from_config(&json!({"providers":{"first":{"models":{
        "gpt-5-z":{},"gemini-3-pro-z":{},"gemini-3-pro-latest":{},"zz-new":{}
    }},"second":{"models":{"custom/slash":{}}}}}));
    assert_eq!(
        catalog.legacy_default(&[], &[]).unwrap(),
        (
            ProviderID::new("first"),
            ModelID::new("gemini-3-pro-latest")
        )
    );
    assert_eq!(
        catalog
            .legacy_default(&[ProviderID::new("second")], &[])
            .unwrap()
            .1,
        ModelID::new("custom/slash")
    );
    assert_eq!(
        catalog
            .legacy_default(
                &[],
                &[
                    (ProviderID::new("missing"), ModelID::new("missing")),
                    (ProviderID::new("first"), ModelID::new("gpt-5-z"))
                ]
            )
            .unwrap()
            .1,
        ModelID::new("gpt-5-z")
    );
    let mut explicit = catalog;
    explicit.set_default("absent", "exact/model");
    assert_eq!(
        explicit.legacy_default(&[], &[]).unwrap(),
        (ProviderID::new("absent"), ModelID::new("exact/model"))
    );
    assert!(explicit.default_model().is_some());
    assert_eq!(
        CatalogStore::new().legacy_default(&[], &[]),
        Err(SelectionError::NoProviders)
    );
    let empty = CatalogStore::from_config(&json!({"providers":{"empty":{}}}));
    assert_eq!(
        empty.legacy_default(&[], &[]),
        Err(SelectionError::NoModels(ProviderID::new("empty")))
    );
}

#[test]
fn models_dev_modes_preserve_base_api_and_merge_cost_tiers_and_wire_request() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("models.json");
    std::fs::write(&path,serde_json::to_vec(&json!({"local":{"id":"local","name":"Local","env":[],"npm":"@ai-sdk/openai","models":{
        "base":{"id":"base","name":"Base","release_date":"2026-10-01","limit":{"context":1000,"output":100},"cost":{"input":1,"output":2,"tiers":[{"input":3,"output":4,"tier":{"type":"context","size":100}}]},"experimental":{"modes":{"fast":{"cost":{"input":2,"output":3,"tiers":[{"input":5,"output":6,"tier":{"type":"context","size":100}}]},"provider":{"body":{"reasoning_effort":"low"},"headers":{"x-mode":"fast"}}}}}}
    }}})).unwrap()).unwrap();
    let mut catalog = CatalogStore::new();
    catalog.load_cache(&path).unwrap();
    let fast = catalog
        .model(&ProviderID::new("local"), &ModelID::new("base-fast"))
        .unwrap();
    assert_eq!(fast.name, "Base Fast");
    assert_eq!(fast.api.id.as_str(), "base");
    assert_eq!(fast.request.body["reasoning_effort"], "low");
    assert_eq!(fast.request.headers["x-mode"], "fast");
    assert_eq!(fast.cost.len(), 2);
    assert_eq!(fast.cost[0].input.get(), 2.0);
    assert_eq!(fast.cost[1].input.get(), 5.0);
}

#[test]
fn small_model_prefers_small_names_then_weighted_cost_and_age() {
    let mut catalog = CatalogStore::from_config(&json!({"providers":{"local":{"models":{
        "cheap-large":{"name":"Large","cost":{"input":0.01,"output":0.01},"capabilities":{"tools":true,"input":["text/plain"],"output":["text"]}},
        "mini":{"name":"Mini","cost":{"input":1,"output":1},"capabilities":{"tools":true,"input":["text"],"output":["text"]}},
        "free-mini":{"name":"Free mini","cost":{"input":0,"output":0},"capabilities":{"tools":true,"input":["text"],"output":["text"]}}
    }},"azure":{"models":{"mini":{"cost":{"input":1,"output":1}}}}}}));
    // ModelInfo.empty has release time zero; the explicit clock keeps these fixture models young.
    assert_eq!(
        catalog
            .small_model(&ProviderID::new("local"), 1000.0)
            .unwrap()
            .id
            .as_str(),
        "mini"
    );
    assert!(
        catalog
            .small_model(&ProviderID::new("azure"), 1000.0)
            .is_none()
    );
    assert!(
        catalog
            .small_model(
                &ProviderID::new("local"),
                19.0 * 30.0 * 24.0 * 60.0 * 60.0 * 1000.0
            )
            .is_none()
    );
    catalog.apply_config(&json!({"providers":{"local":{"models":{"mini":{"disabled":true}}}}}));
    assert_eq!(
        catalog
            .small_model(&ProviderID::new("local"), 1000.0)
            .unwrap()
            .id
            .as_str(),
        "cheap-large"
    );
}
