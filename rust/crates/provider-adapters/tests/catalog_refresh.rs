//! Purpose: verify remote catalog refresh through controlled loopback HTTP.
//! Scope: public ModelsService and real temporary cache lifecycle.
//! Uses:
//! rust/crates/
//! `-- provider-adapters/src/catalog/refresh.rs
//! External:
//! |-- serde_json::json
//! `-- wiremock::MockServer
//! Used by:
//! `-- none, entrypoint
//! Invariants: no live provider network or real credentials.
//! Contract tests: this file

use opencode_provider_adapters::catalog::ModelsService;
use serde_json::json;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{header, method, path},
};

fn inventory(id: &str) -> serde_json::Value {
    json!({"local":{"id":"local","name":"Local","env":[],"models":{
        id:{"id":id,"name":id,"release_date":"2026-10-01","tool_call":true,"limit":{"context":1000,"output":100}}
    }}})
}

#[tokio::test]
async fn fetch_cache_ttl_force_and_invalidation() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("nested/models.json");
    let service = ModelsService::new(
        reqwest::Client::new(),
        server.uri(),
        cache.clone(),
        "catalog-test".into(),
    );
    Mock::given(method("GET"))
        .and(path("/api.json"))
        .and(header("user-agent", "catalog-test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(inventory("first")))
        .expect(1)
        .mount(&server)
        .await;
    assert_eq!(
        service
            .get()
            .await
            .unwrap()
            .default_model()
            .unwrap()
            .id
            .as_str(),
        "first"
    );
    assert!(cache.exists());
    assert!(!service.refresh(false).await.unwrap());
    assert_eq!(
        service
            .get()
            .await
            .unwrap()
            .default_model()
            .unwrap()
            .id
            .as_str(),
        "first"
    );
    server.verify().await;
    server.reset().await;
    Mock::given(path("/api.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(inventory("second")))
        .expect(1)
        .mount(&server)
        .await;
    assert!(service.refresh(true).await.unwrap());
    assert_eq!(
        service
            .get()
            .await
            .unwrap()
            .default_model()
            .unwrap()
            .id
            .as_str(),
        "second"
    );
}

#[tokio::test]
async fn invalid_and_failed_remote_preserve_last_good_cache() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("models.json");
    let original = serde_json::to_vec(&inventory("good")).unwrap();
    std::fs::write(&cache, &original).unwrap();
    let service = ModelsService::new(
        reqwest::Client::new(),
        server.uri(),
        cache.clone(),
        "test".into(),
    );
    assert_eq!(
        service
            .get()
            .await
            .unwrap()
            .default_model()
            .unwrap()
            .id
            .as_str(),
        "good"
    );
    Mock::given(path("/api.json"))
        .respond_with(ResponseTemplate::new(200).set_body_string("{broken"))
        .expect(1)
        .mount(&server)
        .await;
    assert!(service.refresh(true).await.is_err());
    assert_eq!(std::fs::read(&cache).unwrap(), original);
    server.verify().await;
    server.reset().await;
    Mock::given(path("/api.json"))
        .respond_with(ResponseTemplate::new(503))
        .expect(3)
        .mount(&server)
        .await;
    assert!(service.refresh(true).await.is_err());
    assert_eq!(std::fs::read(&cache).unwrap(), original);
    assert_eq!(
        service
            .get()
            .await
            .unwrap()
            .default_model()
            .unwrap()
            .id
            .as_str(),
        "good"
    );
}
