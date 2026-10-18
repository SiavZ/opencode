//! Purpose: request-shape tests for the packages/llm provider facades: OpenAI (Responses/Chat,
//! GPT-5 defaults, env auth), OpenAI-compatible profiles, Anthropic, Google, xAI, Azure, GitHub
//! Copilot and Amazon Bedrock (URL, headers, auth, body).
//! Scope: fixture-based over wiremock (recorded cassette SSE bodies); no live calls.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- provider-adapters/src/providers/azure.rs
//! |-- provider-adapters/src/providers/bedrock.rs
//! |-- provider-adapters/src/providers/compatible.rs
//! |-- provider-adapters/src/providers/config.rs
//! |-- provider-adapters/src/providers/native.rs
//! |-- provider-adapters/src/providers/openai.rs
//! `-- provider-adapters/tests/providers_support/mod.rs
//! External:
//! |-- serde_json::json
//! `-- wiremock::MockServer
//! Used by:
//! `-- none, entrypoint
//! Invariants: expectations mirror packages/llm/src/providers/*.ts and their TS tests.
//! Contract tests: provider-adapters/tests/providers_facades.rs

mod providers_support;

use opencode_llm_api::{JsonObject, ProviderOptions, RouteDefaults};
use opencode_provider_adapters::providers::azure::{
    self, AzureConfig, CopilotEndpoint, should_use_responses_api,
};
use opencode_provider_adapters::providers::bedrock::{self, BedrockConfig, resolve_model_id};
use opencode_provider_adapters::providers::compatible::{self, PROFILES};
use opencode_provider_adapters::providers::native;
use opencode_provider_adapters::providers::openai;
use opencode_provider_adapters::providers::{ProviderConfig, Transports};
use pretty_assertions::assert_eq;
use providers_support::*;
use serde_json::json;
use wiremock::MockServer;

fn cfg(server: &MockServer, suffix: &str) -> ProviderConfig {
    ProviderConfig::default()
        .with_env(env(&[]))
        .with_base_url(format!("{}{suffix}", server.uri()))
}

#[tokio::test]
async fn openai_responses_sends_bearer_and_gpt5_defaults() {
    let server = MockServer::start().await;
    mount(
        &server,
        "/v1/responses",
        cassette_body("openai-responses/gpt-5-5-streams-text"),
    )
    .await;
    let provider = openai::configure(&cfg(&server, "/v1").with_api_key("sk-test"), &transports());
    let response = say_hello(provider.model("gpt-5.5")).await;
    assert!(!response.text().is_empty());
    let request = only_request(&server).await;
    assert_eq!(
        header(&request, "authorization").as_deref(),
        Some("Bearer sk-test")
    );
    let body = body(&request);
    assert_eq!(body["model"], "gpt-5.5");
    assert_eq!(body["store"], false);
    assert_eq!(
        body["reasoning"],
        json!({ "effort": "medium", "summary": "auto" })
    );
    assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));
    assert_eq!(body["text"]["verbosity"], "low");
}

#[tokio::test]
async fn openai_reads_env_key_at_request_time_and_chat_has_no_text_verbosity() {
    let server = MockServer::start().await;
    mount(
        &server,
        "/v1/chat/completions",
        cassette_body("openai-chat/streams-text"),
    )
    .await;
    let config = cfg(&server, "/v1").with_env(env(&[("OPENAI_API_KEY", "sk-env")]));
    let provider = openai::configure(&config, &transports());
    say_hello(provider.chat("gpt-4o-mini")).await;
    let request = only_request(&server).await;
    assert_eq!(
        header(&request, "authorization").as_deref(),
        Some("Bearer sk-env")
    );
    assert_eq!(body(&request)["store"], false);
}

#[tokio::test]
async fn openai_missing_key_is_typed_authentication_error() {
    let server = MockServer::start().await;
    let provider = openai::configure(&cfg(&server, "/v1"), &transports());
    let message = say_hello_err(provider.model("gpt-4o")).await;
    assert_eq!(message, "Missing auth credential: OPENAI_API_KEY");
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[test]
fn gpt5_default_options_match_baseline_rules() {
    assert_eq!(
        openai::gpt5_default_options("gpt-5-chat-latest", true),
        None
    );
    assert_eq!(openai::gpt5_default_options("gpt-5-pro", true), None);
    assert_eq!(openai::gpt5_default_options("gpt-4o", true), None);
    let codex = openai::gpt5_default_options("gpt-5.1-codex", true).unwrap();
    assert!(!codex["openai"].contains_key("textVerbosity"));
    let plain = openai::gpt5_default_options("gpt-5", true).unwrap();
    assert!(!plain["openai"].contains_key("textVerbosity"));
    let user = ProviderOptions::from([(
        "openai".to_owned(),
        JsonObject::from_iter([("store".to_owned(), json!(true))]),
    )]);
    let merged = openai::with_openai_options(
        "gpt-4o",
        &RouteDefaults {
            provider_options: Some(user),
            ..RouteDefaults::default()
        },
        false,
    );
    assert_eq!(merged.provider_options.unwrap()["openai"]["store"], true);
}

#[tokio::test]
async fn compatible_profiles_and_generic_configure() {
    assert_eq!(PROFILES.len(), 9);
    assert_eq!(
        compatible::profile("deepinfra").unwrap().base_url,
        "https://api.deepinfra.com/v1/openai"
    );
    let missing = compatible::configure(None, &ProviderConfig::default(), &transports());
    assert_eq!(
        missing.unwrap_err().reason.message(),
        "Route.model(openai-compatible-chat) requires an endpoint baseURL — configure it on the route first"
    );
    let server = MockServer::start().await;
    mount(&server, "/v1/chat/completions", chat_hello()).await;
    let groq = compatible::profile("groq").unwrap();
    let provider = compatible::configure_profile(
        groq,
        &cfg(&server, "/v1").with_api_key("gsk"),
        &transports(),
    );
    let model = provider.model("llama-3.3-70b");
    assert_eq!(model.provider.as_str(), "groq");
    assert_eq!(say_hello(model).await.text(), "Hello");
    let request = only_request(&server).await;
    assert_eq!(
        header(&request, "authorization").as_deref(),
        Some("Bearer gsk")
    );
    assert_eq!(
        body(&request)["stream_options"],
        json!({ "include_usage": true })
    );
}

#[tokio::test]
async fn anthropic_sends_x_api_key_and_version() {
    let server = MockServer::start().await;
    mount(
        &server,
        "/v1/messages",
        cassette_body("anthropic-messages/streams-text"),
    )
    .await;
    let config = cfg(&server, "/v1").with_env(env(&[("ANTHROPIC_API_KEY", "ant-env")]));
    let provider = native::anthropic(&config, &transports());
    say_hello(provider.model("claude-haiku-4-5")).await;
    let request = only_request(&server).await;
    assert_eq!(header(&request, "x-api-key").as_deref(), Some("ant-env"));
    assert_eq!(
        header(&request, "anthropic-version").as_deref(),
        Some("2023-06-01")
    );
    assert_eq!(header(&request, "authorization"), None);
}

#[tokio::test]
async fn google_sends_x_goog_api_key_to_stream_generate_content() {
    let server = MockServer::start().await;
    mount(
        &server,
        "/v1beta/models/gemini-2.5-flash:streamGenerateContent",
        cassette_body("gemini/streams-text"),
    )
    .await;
    let provider = native::google(
        &cfg(&server, "/v1beta").with_api_key("g-key"),
        &transports(),
    );
    say_hello(provider.model("gemini-2.5-flash")).await;
    let request = only_request(&server).await;
    assert_eq!(header(&request, "x-goog-api-key").as_deref(), Some("g-key"));
    assert_eq!(request.url.query(), Some("alt=sse"));
}

#[tokio::test]
async fn xai_defaults_to_responses_with_profile_url() {
    let provider = native::xai(&ProviderConfig::default().with_env(env(&[])), &transports());
    assert_eq!(
        provider.responses.route.endpoint.base_url.as_deref(),
        Some("https://api.x.ai/v1")
    );
    assert_eq!(provider.model("grok-4").route.id(), "openai-responses");
    assert_eq!(
        provider.chat.model("grok-4").route.id(),
        "openai-compatible-chat"
    );
}

#[tokio::test]
async fn azure_resource_url_api_version_and_api_key_header() {
    let server = MockServer::start().await;
    mount(
        &server,
        "/openai/v1/responses",
        cassette_body("openai-responses/gpt-5-5-streams-text"),
    )
    .await;
    let config = cfg(&server, "/openai/v1").with_api_key("az-key");
    let az = AzureConfig {
        api_version: Some("2025-04-01-preview".into()),
        ..AzureConfig::default()
    };
    let provider = azure::azure(&config, &az, &transports()).unwrap();
    let model = provider.model("gpt-5.5");
    assert_eq!(model.route.id(), "azure-openai-responses");
    say_hello(model).await;
    let request = only_request(&server).await;
    assert_eq!(
        path_and_query(&request),
        "/openai/v1/responses?api-version=2025-04-01-preview"
    );
    assert_eq!(header(&request, "api-key").as_deref(), Some("az-key"));
    assert_eq!(header(&request, "authorization"), None);
    assert_eq!(
        azure::azure_resource_base_url(" myres "),
        "https://myres.openai.azure.com/openai/v1"
    );
    let missing = azure::azure(
        &ProviderConfig::default(),
        &AzureConfig::default(),
        &transports(),
    );
    assert!(missing.is_err());
    let chat = azure::azure(
        &config,
        &AzureConfig {
            use_completion_urls: true,
            ..AzureConfig::default()
        },
        &transports(),
    )
    .unwrap();
    assert_eq!(chat.model("gpt-4o").route.id(), "azure-openai-chat");
}

#[tokio::test]
async fn copilot_routes_gpt5_to_responses_except_mini() {
    assert!(should_use_responses_api("gpt-5", None));
    assert!(should_use_responses_api("gpt-10-preview", None));
    assert!(!should_use_responses_api("gpt-5-mini", None));
    assert!(!should_use_responses_api("gpt-4.1", None));
    assert!(!should_use_responses_api("claude-sonnet-4", None));
    assert!(should_use_responses_api(
        "gpt-4.1",
        Some(CopilotEndpoint::Responses)
    ));
    assert!(!should_use_responses_api(
        "gpt-5",
        Some(CopilotEndpoint::Chat)
    ));
    let server = MockServer::start().await;
    mount(&server, "/chat/completions", chat_hello()).await;
    let provider =
        azure::copilot(&cfg(&server, "").with_api_key("gho"), None, &transports()).unwrap();
    assert_eq!(provider.model("gpt-5").route.id(), "openai-responses");
    say_hello(provider.model("gpt-5-mini")).await;
    let request = only_request(&server).await;
    assert_eq!(
        header(&request, "authorization").as_deref(),
        Some("Bearer gho")
    );
    assert!(azure::copilot(&ProviderConfig::default(), None, &transports()).is_err());
}

#[tokio::test]
async fn bedrock_bearer_auth_and_region_url() {
    let t: Transports = transports();
    let provider = bedrock::configure(&ProviderConfig::default(), &BedrockConfig::default(), &t);
    assert_eq!(provider.region, "us-east-1");
    assert_eq!(
        provider.route.endpoint.base_url.as_deref(),
        Some("https://bedrock-runtime.us-east-1.amazonaws.com")
    );
    let server = MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .respond_with(wiremock::ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let config = cfg(&server, "").with_api_key("bedrock-key");
    let provider = bedrock::configure(&config, &BedrockConfig::default(), &t);
    let _ = say_hello_err(provider.catalog_model("anthropic.claude-sonnet-4-5")).await;
    let request = only_request(&server).await;
    assert_eq!(
        request.url.path(),
        "/model/us.anthropic.claude-sonnet-4-5/converse-stream"
    );
    assert_eq!(
        header(&request, "authorization").as_deref(),
        Some("Bearer bedrock-key")
    );
}

#[test]
fn bedrock_cross_region_prefixes() {
    let cases = [
        (
            "anthropic.claude-sonnet-4-5",
            None,
            "us.anthropic.claude-sonnet-4-5",
        ),
        (
            "anthropic.claude-sonnet-4-5",
            Some("us-gov-west-1"),
            "anthropic.claude-sonnet-4-5",
        ),
        ("amazon.titan-text", Some("us-east-1"), "amazon.titan-text"),
        ("arn:aws:bedrock:x", Some("eu-west-1"), "arn:aws:bedrock:x"),
        (
            "global.anthropic.claude",
            Some("eu-west-1"),
            "global.anthropic.claude",
        ),
        ("meta.llama3-70b", Some("eu-west-1"), "eu.meta.llama3-70b"),
        ("meta.llama3-70b", Some("eu-central-2"), "meta.llama3-70b"),
        (
            "anthropic.claude-haiku-4-5",
            Some("ap-southeast-2"),
            "au.anthropic.claude-haiku-4-5",
        ),
        (
            "amazon.nova-pro-v1:0",
            Some("ap-northeast-1"),
            "jp.amazon.nova-pro-v1:0",
        ),
        (
            "amazon.nova-pro-v1:0",
            Some("ap-south-1"),
            "apac.amazon.nova-pro-v1:0",
        ),
        (
            "amazon.nova-pro-v1:0",
            Some("ca-central-1"),
            "amazon.nova-pro-v1:0",
        ),
    ];
    for (model, region, expected) in cases {
        assert_eq!(
            resolve_model_id(model, region),
            expected,
            "{model} {region:?}"
        );
    }
    assert!(bedrock::mantle_uses_chat("openai.gpt-oss-safeguard-20b"));
    assert!(!bedrock::mantle_uses_chat("openai.gpt-oss-120b"));
}
