//! Purpose: verify all baseline secret operator commands against a durable marked fixture store.
//! Scope: temporary filesystem only, no SST commands or live promotion.
//! Uses:
//! rust/crates/
//! `-- hosted-adapters/src/console/workers/operators/config.rs
//! External:
//! |-- serde_json::{Value,json}
//! `-- tempfile::tempdir
//! Used by:
//! `-- none, entrypoint
//! Invariants: failed validation leaves destination unchanged; source chunks and unrelated secrets persist.
//! Contract tests: this file.

use opencode_hosted_adapters::console::workers::operators::config;
use serde_json::{Value, json};

fn chunks(value: &Value, count: usize) -> Value {
    let text = serde_json::to_string(value).unwrap();
    let size = text.len().div_ceil(count);
    Value::Object(
        (0..count)
            .map(|i| {
                (
                    format!("ZEN_MODELS{}", i + 1),
                    json!(text.chars().skip(i * size).take(size).collect::<String>()),
                )
            })
            .collect(),
    )
}

#[tokio::test]
async fn fixture_config_store_updates_pulls_and_promotes_all_five_commands() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::write(
        root.join(".console-worker-fixture"),
        "p12-console-config-v1\n",
    )
    .unwrap();
    let mut models = json!({"zenModels":{"model":{"name":"Model","cost":{"input":1,"output":2},"providers":[{"id":"p","model":"native"}]}},"liteModels":{},"providers":{"p":{"api":"https://example.test","apiKey":"capture-secret"}}});
    models["providers"]["p"]["apiKey"] = json!("fixture-secret".repeat(100));
    std::fs::write(
        root.join("frank.json"),
        serde_json::to_vec(&chunks(&models, 30)).unwrap(),
    )
    .unwrap();
    std::fs::write(
        root.join("upstream.json"),
        serde_json::to_vec(&chunks(&models, 20)).unwrap(),
    )
    .unwrap();
    std::fs::write(root.join("local.json"), r#"{"UNRELATED":"keep"}"#).unwrap();
    let limits = json!({"free":{"promoTokens":10,"dailyRequests":20,"dailyRequestsFallback":5,"checkHeaders":{"x-test":"yes"}},"lite":{"rollingLimit":1,"rollingWindow":2,"weeklyLimit":3,"monthlyLimit":4},"black":{"20":{"fixedLimit":20,"rollingLimit":2,"rollingWindow":5},"100":{"fixedLimit":100,"rollingLimit":10,"rollingWindow":5},"200":{"fixedLimit":200,"rollingLimit":20,"rollingWindow":5}}});
    std::fs::write(
        root.join("fallback.json"),
        serde_json::to_vec(&json!({"ZEN_LIMITS":limits.to_string(),"OTHER":"yes"})).unwrap(),
    )
    .unwrap();
    let mut input = json!({"configStore":root,"confirmConfigStore":root,"stage":"promoted","localStage":"local"});
    let result = config::run("promote-models", &input).await.unwrap();
    assert_eq!(result["keys"].as_array().unwrap().len(), 30);
    let promoted: Value =
        serde_json::from_slice(&std::fs::read(root.join("promoted.json")).unwrap()).unwrap();
    assert_eq!(promoted, chunks(&models, 30));
    input["stage"] = json!("upstream");
    config::run("pull-models", &input).await.unwrap();
    let local: Value =
        serde_json::from_slice(&std::fs::read(root.join("local.json")).unwrap()).unwrap();
    assert_eq!(local["UNRELATED"], "keep");
    assert_eq!(local["ZEN_MODELS20"], chunks(&models, 20)["ZEN_MODELS20"]);
    let mut edited = models.clone();
    edited["zenModels"]["model"]["name"] = json!("Changed");
    input["edited"] = edited.clone();
    config::run("update-models", &input).await.unwrap();
    let changed: Value =
        serde_json::from_slice(&std::fs::read(root.join("frank.json")).unwrap()).unwrap();
    let combined = (1..=30)
        .map(|i| changed[format!("ZEN_MODELS{i}")].as_str().unwrap())
        .collect::<String>();
    assert_eq!(serde_json::from_str::<Value>(&combined).unwrap(), edited);
    input["edited"] = limits.clone();
    input["edited"]["free"]["promoTokens"] = json!(11);
    config::run("update-limits", &input).await.unwrap();
    input["stage"] = json!("promoted");
    config::run("promote-limits", &input).await.unwrap();
    let promoted: Value =
        serde_json::from_slice(&std::fs::read(root.join("promoted.json")).unwrap()).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(promoted["ZEN_LIMITS"].as_str().unwrap()).unwrap()["free"]["promoTokens"],
        11
    );
    let before = std::fs::read(root.join("fallback.json")).unwrap();
    input["edited"]["free"]["promoTokens"] = json!(1.5);
    assert!(config::run("update-limits", &input).await.is_err());
    assert_eq!(std::fs::read(root.join("fallback.json")).unwrap(), before);
    input["stage"] = json!("../escape");
    assert!(config::run("promote-models", &input).await.is_err());
    input["confirmConfigStore"] = json!("wrong");
    assert!(config::run("promote-models", &input).await.is_err());
    assert!(!root.join(".lock").exists());
    input["confirmConfigStore"] = json!(root);
    input["stage"] = json!("promoted");
    std::fs::write(root.join(".lock"), "held").unwrap();
    assert!(config::run("promote-limits", &input).await.is_err());
    assert_eq!(std::fs::read_to_string(root.join(".lock")).unwrap(), "held");
    std::fs::remove_file(root.join(".lock")).unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(root.join("frank.json"), root.join("symlink.json")).unwrap();
        input["stage"] = json!("symlink");
        assert!(config::run("pull-models", &input).await.is_err());
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(root.join("promoted.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}
