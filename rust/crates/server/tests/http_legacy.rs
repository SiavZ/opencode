//! Purpose: actual loopback HTTP tests for legacy registry, health, decoding and middleware.
//! Scope: real server listener with intentionally absent domain services, no source mocks.
//! Uses:
//! rust/crates/
//! |-- server/src/app.rs
//! |-- server/src/lifecycle.rs
//! `-- server/src/services/mod.rs
//! External:
//! |-- reqwest::Client
//! `-- serde_json::json
//! Used by:
//! `-- none, entrypoint
//! Invariants: absent capabilities are typed 503, never UI fallback or fabricated domain success.
//! Contract tests: this file; persisted Session/event integration is in runtime/tests/process.rs.

use opencode_server::app::{ServerConfig, router};
use opencode_server::lifecycle::{ListenOptions, Listener, listen};
use opencode_server::middleware::auth::AuthConfig;
use opencode_server::middleware::cors::CorsConfig;
use opencode_server::services::Services;
use serde_json::{Value, json};

async fn start(password: Option<&str>) -> Listener {
    listen(
        router(
            Services {
                version: "release-test".into(),
                ..Services::default()
            },
            ServerConfig::full_host(
                AuthConfig {
                    username: "opencode".into(),
                    password: password.map(str::to_owned),
                },
                CorsConfig::default(),
            ),
        ),
        ListenOptions {
            hostname: "127.0.0.1".into(),
            port: 0,
        },
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn legacy_health_is_versioned_and_authenticated_with_cors() {
    let server = start(Some("local-secret")).await;
    let client = reqwest::Client::new();
    let url = format!("{}/global/health", server.url());
    let rejected = client.get(&url).send().await.unwrap();
    assert_eq!(rejected.status(), 401);
    assert_eq!(
        rejected.headers()["www-authenticate"],
        "Basic realm=\"Secure Area\""
    );
    assert!(rejected.bytes().await.unwrap().is_empty());
    let preflight = client
        .request(reqwest::Method::OPTIONS, &url)
        .header("origin", "http://localhost:3000")
        .header("access-control-request-method", "GET")
        .send()
        .await
        .unwrap();
    assert!(preflight.status().is_success());
    assert_eq!(
        preflight.headers()["access-control-allow-origin"],
        "http://localhost:3000"
    );
    let healthy = client
        .get(&url)
        .basic_auth("opencode", Some("local-secret"))
        .header("origin", "http://localhost:3000")
        .send()
        .await
        .unwrap();
    assert_eq!(healthy.status(), 200);
    assert_eq!(
        healthy.headers()["access-control-allow-origin"],
        "http://localhost:3000"
    );
    assert_eq!(
        healthy.json::<Value>().await.unwrap(),
        json!({"healthy":true,"version":"release-test"})
    );
    server.stop(false).await;
}

#[tokio::test]
async fn legacy_authorization_precedes_session_decode_and_stream_subscription() {
    let server = start(Some("local-secret")).await;
    let client = reqwest::Client::new();
    for path in [
        "/session",
        "/session/ses_missing",
        "/event",
        "/global/event",
    ] {
        let url = format!("{}{path}", server.url());
        let rejected = client.get(&url).send().await.unwrap();
        assert_eq!(rejected.status(), 401, "{path}");
        assert!(rejected.bytes().await.unwrap().is_empty());
        let admitted = client
            .get(&url)
            .basic_auth("opencode", Some("local-secret"))
            .send()
            .await
            .unwrap();
        assert_eq!(admitted.status(), 503, "{path}");
        assert_ne!(admitted.json::<Value>().await.unwrap()["service"], "ui");
    }
    let url = format!("{}/session", server.url());
    let rejected = client.post(&url).body("{").send().await.unwrap();
    assert_eq!(rejected.status(), 401);
    assert!(rejected.bytes().await.unwrap().is_empty());
    let admitted = client
        .post(&url)
        .basic_auth("opencode", Some("local-secret"))
        .body("{")
        .send()
        .await
        .unwrap();
    assert_eq!(admitted.status(), 400);
    assert_eq!(
        admitted.json::<Value>().await.unwrap(),
        json!({"_tag":"BadRequest"})
    );
    server.stop(false).await;
}

#[tokio::test]
async fn registered_legacy_routes_fail_typed_without_runtime_services() {
    let server = start(None).await;
    let client = reqwest::Client::new();
    for path in [
        "/session",
        "/session/ses_missing",
        "/session/status",
        "/global/event",
        "/event",
    ] {
        let response = client
            .get(format!("{}{path}", server.url()))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 503, "{path}");
        let value = response.json::<Value>().await.unwrap();
        assert_ne!(value["service"], "ui", "{path}: {value}");
        assert_eq!(value["_tag"], "ServiceUnavailableError");
    }
    for body in ["", " \n\t", "\u{00a0}\u{feff}\u{3000}", "{}"] {
        let response = client
            .post(format!("{}/session", server.url()))
            .body(body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 503);
        assert_eq!(
            response.json::<Value>().await.unwrap()["service"],
            "session.create"
        );
    }
    server.stop(false).await;
}

#[tokio::test]
async fn legacy_create_preserves_raw_decode_errors_and_complete_input_schema() {
    let server = start(None).await;
    let client = reqwest::Client::new();
    let url = format!("{}/session", server.url());
    for body in [
        "{",
        "null",
        "[]",
        "\u{0085}",
        "\u{feff}{}",
        "{\"title\":12}",
        "{\"model\":{\"providerID\":\"p\",\"modelID\":\"wrong-v2-key\"}}",
        "{\"permission\":[{\"permission\":\"read\",\"pattern\":\"*\",\"action\":\"invalid\"}]}",
    ] {
        let response = client.post(&url).body(body).send().await.unwrap();
        assert_eq!(response.status(), 400, "{body}");
        assert_eq!(
            response.json::<Value>().await.unwrap(),
            json!({"_tag":"BadRequest"})
        );
    }
    let valid = json!({"parentID":"ses_parent", "title":"A title", "agent":"build",
        "model":{"id":"model","providerID":"provider","variant":"fast"},
        "metadata":{"nested":{"enabled":true}}, "workspaceID":"wrk_local",
        "permission":[{"permission":"read","pattern":"*","action":"allow"}]});
    let response = client.post(&url).json(&valid).send().await.unwrap();
    assert_eq!(response.status(), 503);
    let malformed = client
        .get(format!("{}/session/not-a-session", server.url()))
        .send()
        .await
        .unwrap();
    assert_eq!(malformed.status(), 500);
    let value = malformed.json::<Value>().await.unwrap();
    assert_eq!(value["name"], "UnknownError");
    assert!(value["data"]["ref"].as_str().unwrap().starts_with("err_"));
    let invalid_query = client
        .get(format!("{}/session?limit=banana", server.url()))
        .send()
        .await
        .unwrap();
    assert_eq!(invalid_query.status(), 400);
    server.stop(false).await;
}
