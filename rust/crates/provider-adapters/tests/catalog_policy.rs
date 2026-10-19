//! Purpose: observe disabled-fetch, read-path, refresh events and cross-instance cache locking.
//! Scope: public ModelsService with explicit paths and controlled HTTP.
//! Uses:
//! rust/crates/
//! `-- provider-adapters/src/catalog/refresh.rs
//! External:
//! |-- serde_json::json
//! `-- wiremock::MockServer
//! Used by:
//! `-- none, entrypoint
//! Invariants: no live provider network, detached tasks or global environment writes.
//! Contract tests: this file

use opencode_provider_adapters::catalog::ModelsService;
use serde_json::json;
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};

#[test]
fn source_cache_names_match_typescript_sha1() {
    use opencode_provider_adapters::catalog::cache_filename;
    assert_eq!(cache_filename("https://models.opencode.ai"), "models.json");
    assert_eq!(
        cache_filename("abc"),
        "models-a9993e364706816aba3e25717850c26c9cd0d89d.json"
    );
    assert_ne!(cache_filename("abc"), cache_filename("abc/"));
}

fn inventory(id: &str) -> serde_json::Value {
    json!({"local":{"name":"Local","env":[],"models":{id:{"id":id,"name":id,"limit":{"context":10,"output":1}}}}})
}

#[tokio::test]
async fn disabled_population_and_override_are_not_remote_write_targets() {
    let server = MockServer::start().await;
    let root = tempfile::tempdir().unwrap();
    let cache = root.path().join("models.json");
    let override_path = root.path().join("supplied.json");
    let service = ModelsService::new(
        reqwest::Client::new(),
        server.uri(),
        cache.clone(),
        "test".into(),
    )
    .with_policy(true, None);
    assert!(service.get().await.unwrap().all_models().is_empty());
    assert!(server.received_requests().await.unwrap().is_empty());
    std::fs::write(
        &override_path,
        serde_json::to_vec(&inventory("supplied")).unwrap(),
    )
    .unwrap();
    let service = ModelsService::new(
        reqwest::Client::new(),
        server.uri(),
        cache.clone(),
        "test".into(),
    )
    .with_policy(true, None)
    .with_read_path(Some(override_path.clone()));
    let mut events = service.subscribe();
    assert_eq!(
        service
            .get()
            .await
            .unwrap()
            .default_model()
            .unwrap()
            .id
            .as_str(),
        "supplied"
    );
    Mock::given(path("/api.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(inventory("remote")))
        .expect(1)
        .mount(&server)
        .await;
    assert!(service.refresh(true).await.unwrap());
    events.try_recv().unwrap();
    assert_eq!(
        service
            .get()
            .await
            .unwrap()
            .default_model()
            .unwrap()
            .id
            .as_str(),
        "supplied"
    );
    assert!(
        String::from_utf8(std::fs::read(&cache).unwrap())
            .unwrap()
            .contains("remote")
    );
    assert!(
        String::from_utf8(std::fs::read(&override_path).unwrap())
            .unwrap()
            .contains("supplied")
    );
}

#[tokio::test]
async fn independent_services_coalesce_nonforced_refresh_under_file_lock() {
    let server = MockServer::start().await;
    let root = tempfile::tempdir().unwrap();
    let cache = root.path().join("models.json");
    Mock::given(path("/api.json"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(std::time::Duration::from_millis(50))
                .set_body_json(inventory("remote")),
        )
        .expect(1)
        .mount(&server)
        .await;
    let first = ModelsService::new(
        reqwest::Client::new(),
        server.uri(),
        cache.clone(),
        "test".into(),
    );
    let second = ModelsService::new(reqwest::Client::new(), server.uri(), cache, "test".into());
    let (first, second) = tokio::join!(first.refresh(false), second.refresh(false));
    assert_ne!(first.unwrap(), second.unwrap());
}

#[tokio::test]
async fn malformed_cache_falls_back_to_snapshot_or_remote_without_partial_inventory() {
    let server = MockServer::start().await;
    let root = tempfile::tempdir().unwrap();
    let cache = root.path().join("models.json");
    std::fs::write(&cache, "{broken").unwrap();
    Mock::given(path("/api.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(inventory("recovered")))
        .expect(1)
        .mount(&server)
        .await;
    let service = ModelsService::new(reqwest::Client::new(), server.uri(), cache, "test".into());
    assert_eq!(
        service
            .get()
            .await
            .unwrap()
            .default_model()
            .unwrap()
            .id
            .as_str(),
        "recovered"
    );
}
