//! Purpose: request-shape tests for cloud-account providers resolved through the registry:
//! Google Vertex (express key, ADC auth injection, Anthropic publisher `streamRawPredict` with
//! `anthropic_version`), Amazon Bedrock (bearer, SigV4 env credentials, Mantle) and Snowflake
//! Cortex (`max_completion_tokens`, `role:""` stream repair, credential errors).
//! Scope: fixture-based over wiremock; no live calls and no real cloud credentials.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- provider-adapters/src/auth.rs
//! |-- provider-adapters/src/providers/cloud.rs
//! |-- provider-adapters/src/providers/inventory.rs
//! |-- provider-adapters/src/providers/registry.rs
//! `-- provider-adapters/tests/providers_support/mod.rs
//! External:
//! |-- serde_json::json
//! `-- wiremock::MockServer
//! Used by:
//! `-- none, entrypoint
//! Invariants: credential-chain modes without supplied auth fail with the typed
//! `NoRoute (not yet ported)` error and never reach the network.
//! Contract tests: provider-adapters/tests/providers_cloud.rs

mod providers_support;

use opencode_llm_api::{GenerationOptions, LlmRequest, Message, RouteClient, generate};
use opencode_provider_adapters::auth::bearer_value;
use opencode_provider_adapters::providers::{CatalogRoute, ProviderRegistry};
use pretty_assertions::assert_eq;
use providers_support::*;
use serde_json::json;
use wiremock::MockServer;

fn registry() -> ProviderRegistry {
    ProviderRegistry::new(transports(), "1.0.0")
}

#[tokio::test]
async fn vertex_anthropic_stream_raw_predict_with_vertex_version() {
    let server = MockServer::start().await;
    mount(
        &server,
        "/v1/models/claude-sonnet-4-5@20250929:streamRawPredict",
        cassette_body("anthropic-messages/streams-text"),
    )
    .await;
    let mut route = CatalogRoute::aisdk(
        "google-vertex-anthropic",
        " claude-sonnet-4-5@20250929 ",
        "@ai-sdk/google-vertex/anthropic",
        Some(&format!("{}/v1/models", server.uri())),
    );
    route.env = env(&[]);
    route.auth = Some(bearer_value("adc-token"));
    let model = registry().resolve(&route).unwrap();
    assert_eq!(model.id.as_str(), "claude-sonnet-4-5@20250929");
    say_hello(model).await;
    let request = only_request(&server).await;
    assert_eq!(
        header(&request, "authorization").as_deref(),
        Some("Bearer adc-token")
    );
    let body = body(&request);
    assert_eq!(body.get("model"), None);
    assert_eq!(body["anthropic_version"], "vertex-2023-10-16");
    assert_eq!(body["stream"], true);
}

#[test]
fn vertex_urls_project_location_and_typed_adc_error() {
    let mut route = CatalogRoute::aisdk(
        "google-vertex-anthropic",
        "claude-sonnet-4-5",
        "@ai-sdk/google-vertex/anthropic",
        None,
    );
    route.env = env(&[
        ("GOOGLE_CLOUD_PROJECT", "proj"),
        ("GOOGLE_CLOUD_LOCATION", "eu"),
    ]);
    let err = registry().resolve(&route).unwrap_err();
    assert_eq!(err.reason.tag(), "NoRoute");
    assert!(
        err.reason
            .message()
            .contains("ADC access token (not yet ported)")
    );
    route.auth = Some(bearer_value("t"));
    let model = registry().resolve(&route).unwrap();
    assert!(format!("{:?}", model.route).contains(
        "https://aiplatform.eu.rep.googleapis.com/v1/projects/proj/locations/eu/publishers/anthropic/models"
    ));
    let mut gemini = CatalogRoute::aisdk(
        "google-vertex",
        "gemini-2.5-pro",
        "@ai-sdk/google-vertex",
        None,
    );
    gemini.env = env(&[("GOOGLE_VERTEX_PROJECT", "p1")]);
    gemini.auth = Some(bearer_value("t"));
    let model = registry().resolve(&gemini).unwrap();
    assert!(format!("{:?}", model.route).contains(
        "https://us-central1-aiplatform.googleapis.com/v1beta1/projects/p1/locations/us-central1/publishers/google"
    ));
}

#[tokio::test]
async fn vertex_express_mode_uses_goog_api_key() {
    let server = MockServer::start().await;
    mount(
        &server,
        "/v1/publishers/google/models/gemini-2.5-flash:streamGenerateContent",
        cassette_body("gemini/streams-text"),
    )
    .await;
    let mut route = CatalogRoute::aisdk(
        "google-vertex",
        "gemini-2.5-flash",
        "@ai-sdk/google-vertex",
        Some(&format!("{}/v1/publishers/google", server.uri())),
    );
    route.env = env(&[("GOOGLE_VERTEX_API_KEY", "vx")]);
    say_hello(registry().resolve(&route).unwrap()).await;
    let request = only_request(&server).await;
    assert_eq!(header(&request, "x-goog-api-key").as_deref(), Some("vx"));
}

#[tokio::test]
async fn bedrock_registry_bearer_env_and_region_prefix() {
    let server = MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .respond_with(wiremock::ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let mut route = CatalogRoute::aisdk(
        "amazon-bedrock",
        "anthropic.claude-sonnet-4-5",
        "@ai-sdk/amazon-bedrock",
        None,
    );
    route.options = json!({ "region": "eu-west-1", "endpoint": server.uri() })
        .as_object()
        .cloned()
        .unwrap();
    route.env = env(&[("AWS_BEARER_TOKEN_BEDROCK", "brk")]);
    let model = registry().resolve(&route).unwrap();
    assert_eq!(model.id.as_str(), "eu.anthropic.claude-sonnet-4-5");
    let _ = say_hello_err(model).await;
    let request = only_request(&server).await;
    assert_eq!(
        request.url.path(),
        "/model/eu.anthropic.claude-sonnet-4-5/converse-stream"
    );
    assert_eq!(
        header(&request, "authorization").as_deref(),
        Some("Bearer brk")
    );
}

#[tokio::test]
async fn bedrock_registry_sigv4_from_env_keys_and_typed_chain_error() {
    let server = MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .respond_with(wiremock::ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let mut route = CatalogRoute::aisdk(
        "amazon-bedrock",
        "amazon.titan-text",
        "@ai-sdk/amazon-bedrock",
        Some(&server.uri()),
    );
    route.env = env(&[
        ("AWS_ACCESS_KEY_ID", "AKIDEXAMPLE"),
        ("AWS_SECRET_ACCESS_KEY", "secret"),
        ("AWS_REGION", "us-west-2"),
    ]);
    let _ = say_hello_err(registry().resolve(&route).unwrap()).await;
    let request = only_request(&server).await;
    let auth = header(&request, "authorization").unwrap();
    assert!(
        auth.starts_with("AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/"),
        "{auth}"
    );
    assert!(auth.contains("/us-west-2/bedrock/aws4_request"), "{auth}");
    route.env = env(&[]);
    let err = registry().resolve(&route).unwrap_err();
    assert_eq!(err.reason.tag(), "NoRoute");
    assert!(err.reason.message().contains("AWS credential chain"));
}

#[tokio::test]
async fn bedrock_mantle_routes_responses_and_safeguard_chat() {
    let mut route = CatalogRoute::aisdk(
        "amazon-bedrock",
        "openai.gpt-oss-120b",
        "@ai-sdk/amazon-bedrock/mantle",
        None,
    );
    route.env = env(&[
        ("AWS_BEARER_TOKEN_BEDROCK", "m"),
        ("AWS_REGION", "us-west-2"),
    ]);
    let model = registry().resolve(&route).unwrap();
    assert_eq!(model.route.id(), "openai-responses");
    assert!(format!("{:?}", model.route).contains("https://bedrock-mantle.us-west-2.api.aws/v1"));
    route.model_id = "openai.gpt-oss-safeguard-20b".into();
    assert_eq!(
        registry().resolve(&route).unwrap().route.id(),
        "openai-chat"
    );
    route.env = env(&[]);
    assert_eq!(
        registry().resolve(&route).unwrap_err().reason.tag(),
        "NoRoute"
    );
}

#[tokio::test]
async fn snowflake_renames_max_tokens_and_repairs_empty_role() {
    let server = MockServer::start().await;
    let stream = sse(&[
        json!({ "id": "c", "choices": [{ "delta": { "role": "", "content": "Hi" }, "finish_reason": null }] }),
        json!({ "id": "c", "choices": [{ "delta": {}, "finish_reason": "stop" }] }),
    ])
    .replace(r#""role":"""#, r#""role" : """#);
    mount(&server, "/api/v2/cortex/v1/chat/completions", stream).await;
    let mut route = CatalogRoute::aisdk(
        "snowflake-cortex",
        "claude-3-5-sonnet",
        "@ai-sdk/openai-compatible",
        Some(&format!("{}/api/v2/cortex/v1", server.uri())),
    );
    route.env = env(&[("SNOWFLAKE_CORTEX_PAT", "pat")]);
    let model = registry().resolve(&route).unwrap();
    let mut request = LlmRequest::new(model);
    request.messages = vec![Message::user("Say hello.")];
    request.generation = Some(GenerationOptions {
        max_tokens: Some(64.0),
        ..GenerationOptions::default()
    });
    let response = generate(&RouteClient, request).await.unwrap();
    assert_eq!(response.text(), "Hi");
    let request = only_request(&server).await;
    assert_eq!(
        header(&request, "authorization").as_deref(),
        Some("Bearer pat")
    );
    let body = body(&request);
    assert_eq!(body["max_completion_tokens"], 64);
    assert_eq!(body.get("max_tokens"), None);
}

#[test]
fn snowflake_missing_credentials_message() {
    let mut route = CatalogRoute::aisdk("snowflake-cortex", "m", "@ai-sdk/openai-compatible", None);
    route.env = env(&[]);
    assert_eq!(
        registry().resolve(&route).unwrap_err().reason.message(),
        "Snowflake Cortex: missing credentials (SNOWFLAKE_ACCOUNT, SNOWFLAKE_CORTEX_TOKEN). Provide a bearer token (OAuth, JWT, or PAT) via env var, opencode auth, or provider options."
    );
    route.env = env(&[
        ("SNOWFLAKE_ACCOUNT", "acme"),
        ("SNOWFLAKE_CORTEX_TOKEN", "t"),
    ]);
    let model = registry().resolve(&route).unwrap();
    assert!(
        format!("{:?}", model.route)
            .contains("https://acme.snowflakecomputing.com/api/v2/cortex/v1")
    );
}
