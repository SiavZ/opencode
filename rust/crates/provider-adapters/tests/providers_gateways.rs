//! Purpose: request-shape tests for gateway providers: OpenRouter (`openrouter-chat` body
//! options, profile URL, attribution headers via the registry) and Cloudflare AI Gateway /
//! Workers AI (compat URLs, `cf-aig-authorization`, upstream bearer), mirroring
//! packages/llm/test/provider/{openrouter,cloudflare}.test.ts.
//! Scope: fixture-based over wiremock; no live calls.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- provider-adapters/src/providers/gateways.rs
//! |-- provider-adapters/src/providers/mod.rs
//! `-- provider-adapters/tests/providers_support/mod.rs
//! External:
//! |-- gateways::ai_gateway
//! |-- gateways::openrouter
//! |-- pretty_assertions::assert_eq
//! |-- serde_json::json
//! |-- str::to_owned
//! |-- tokio::test
//! `-- wiremock::MockServer
//! Used by:
//! `-- none, entrypoint
//! Invariants: expectations copied from the TS tests and plugin sources, not from Rust output.
//! Contract tests: provider-adapters/tests/providers_gateways.rs

mod providers_support;

use opencode_llm_api::{LlmRequest, Message, ProviderOptions};
use opencode_provider_adapters::providers::gateways::{self, CloudflareConfig};
use opencode_provider_adapters::providers::{CatalogRoute, ProviderConfig, ProviderRegistry};
use pretty_assertions::assert_eq;
use providers_support::*;
use serde_json::json;
use wiremock::MockServer;

#[test]
fn openrouter_prepare_spreads_body_options() {
    let provider =
        gateways::openrouter(&ProviderConfig::default().with_env(env(&[])), &transports());
    assert_eq!(
        provider.route.endpoint.base_url.as_deref(),
        Some("https://openrouter.ai/api/v1")
    );
    let model = provider.model("openai/gpt-4o-mini");
    let mut request = LlmRequest::new(model.clone());
    request.messages = vec![Message::user("Say hello.")];
    request.provider_options = Some(ProviderOptions::from([(
        "openrouter".to_owned(),
        json!({ "usage": true, "reasoning": { "effort": "high" }, "promptCacheKey": "k1" })
            .as_object()
            .cloned()
            .unwrap(),
    )]));
    let prepared = model.route.prepare(&request).unwrap();
    assert_eq!(prepared.route, "openrouter");
    assert_eq!(prepared.protocol, "openrouter-chat");
    assert_eq!(prepared.body["usage"], json!({ "include": true }));
    assert_eq!(prepared.body["reasoning"], json!({ "effort": "high" }));
    assert_eq!(prepared.body["prompt_cache_key"], "k1");
    assert_eq!(prepared.body["stream"], true);
}

#[tokio::test]
async fn openrouter_via_registry_sends_attribution_headers() {
    let server = MockServer::start().await;
    mount(&server, "/api/v1/chat/completions", chat_hello()).await;
    let mut route = CatalogRoute::aisdk(
        "openrouter",
        "anthropic/claude-sonnet-4",
        "@openrouter/ai-sdk-provider",
        Some(&format!("{}/api/v1", server.uri())),
    );
    route.env = env(&[("OPENROUTER_API_KEY", "or-key")]);
    let model = ProviderRegistry::new(transports(), "1.0.0")
        .resolve(&route)
        .unwrap();
    assert_eq!(say_hello(model).await.text(), "Hello");
    let request = only_request(&server).await;
    assert_eq!(
        header(&request, "authorization").as_deref(),
        Some("Bearer or-key")
    );
    assert_eq!(
        header(&request, "http-referer").as_deref(),
        Some("https://opencode.ai/")
    );
    assert_eq!(header(&request, "x-title").as_deref(), Some("opencode"));
}

fn gateway(account: &str, gateway: Option<&str>, key: Option<&str>) -> CloudflareConfig {
    CloudflareConfig {
        account_id: Some(account.into()),
        gateway_id: gateway.map(str::to_owned),
        gateway_api_key: key.map(str::to_owned),
    }
}

#[test]
fn ai_gateway_urls_default_gateway_and_errors() {
    let cf = gateway("test-account", Some("test-gateway"), None);
    assert_eq!(
        gateways::ai_gateway_base_url(None, &cf).unwrap(),
        "https://gateway.ai.cloudflare.com/v1/test-account/test-gateway/compat"
    );
    let blank = gateway("test-account", Some(" "), None);
    assert_eq!(
        gateways::ai_gateway_base_url(None, &blank).unwrap(),
        "https://gateway.ai.cloudflare.com/v1/test-account/default/compat"
    );
    assert_eq!(
        gateways::ai_gateway_base_url(None, &CloudflareConfig::default())
            .unwrap_err()
            .reason
            .message(),
        "CloudflareAIGateway.configure requires accountId unless baseURL is supplied"
    );
    assert_eq!(
        gateways::workers_ai_base_url(None, Some("acct")).unwrap(),
        "https://api.cloudflare.com/client/v4/accounts/acct/ai/v1"
    );
    assert!(gateways::workers_ai_base_url(None, None).is_err());
}

#[tokio::test]
async fn ai_gateway_api_key_is_upstream_bearer() {
    let server = MockServer::start().await;
    mount(&server, "/compat/chat/completions", chat_hello()).await;
    let config = ProviderConfig::default()
        .with_env(env(&[]))
        .with_api_key("test-token")
        .with_base_url(format!("{}/compat", server.uri()));
    let provider = gateways::ai_gateway(&config, &gateway("a", None, None), &transports()).unwrap();
    let model = provider.model("openai/gpt-4o-mini");
    assert_eq!(model.route.id(), "cloudflare-ai-gateway");
    assert_eq!(say_hello(model).await.text(), "Hello");
    let request = only_request(&server).await;
    assert_eq!(
        header(&request, "authorization").as_deref(),
        Some("Bearer test-token")
    );
    assert_eq!(header(&request, "cf-aig-authorization"), None);
    assert_eq!(body(&request)["model"], "openai/gpt-4o-mini");
}

#[tokio::test]
async fn ai_gateway_gateway_key_plus_upstream_key() {
    let server = MockServer::start().await;
    mount(&server, "/compat/chat/completions", chat_hello()).await;
    let config = ProviderConfig::default()
        .with_env(env(&[]))
        .with_api_key("provider-token")
        .with_base_url(format!("{}/compat", server.uri()));
    let provider = gateways::ai_gateway(
        &config,
        &gateway("a", None, Some("gateway-token")),
        &transports(),
    )
    .unwrap();
    say_hello(provider.model("openai/gpt-4o-mini")).await;
    let request = only_request(&server).await;
    assert_eq!(
        header(&request, "cf-aig-authorization").as_deref(),
        Some("Bearer gateway-token")
    );
    assert_eq!(
        header(&request, "authorization").as_deref(),
        Some("Bearer provider-token")
    );
}

#[tokio::test]
async fn ai_gateway_env_token_only_sets_gateway_header() {
    let server = MockServer::start().await;
    mount(&server, "/compat/chat/completions", chat_hello()).await;
    let config = ProviderConfig::default()
        .with_env(env(&[("CF_AIG_TOKEN", "env-gw")]))
        .with_base_url(format!("{}/compat", server.uri()));
    let provider = gateways::ai_gateway(&config, &gateway("a", None, None), &transports()).unwrap();
    say_hello(provider.model("workers-ai/@cf/meta/llama-3.3-70b-instruct")).await;
    let request = only_request(&server).await;
    assert_eq!(
        header(&request, "cf-aig-authorization").as_deref(),
        Some("Bearer env-gw")
    );
    assert_eq!(header(&request, "authorization"), None);
}

#[tokio::test]
async fn registry_ai_gateway_options_headers_and_workers_upstream_token() {
    let server = MockServer::start().await;
    mount(&server, "/compat/chat/completions", chat_hello()).await;
    let mut route = CatalogRoute::aisdk(
        "cloudflare-ai-gateway",
        "workers-ai/@cf/meta/llama",
        "ai-gateway-provider",
        Some(&format!("{}/compat", server.uri())),
    );
    route.env = env(&[("CLOUDFLARE_API_TOKEN", "cf-tok")]);
    route.options = json!({ "cacheTtl": 60, "skipCache": true, "metadata": { "a": 1 } })
        .as_object()
        .cloned()
        .unwrap();
    let model = ProviderRegistry::new(transports(), "1.2.3")
        .resolve(&route)
        .unwrap();
    say_hello(model).await;
    let request = only_request(&server).await;
    assert_eq!(
        header(&request, "cf-aig-authorization").as_deref(),
        Some("Bearer cf-tok")
    );
    assert_eq!(
        header(&request, "authorization").as_deref(),
        Some("Bearer cf-tok")
    );
    assert_eq!(header(&request, "cf-aig-cache-ttl").as_deref(), Some("60"));
    assert_eq!(
        header(&request, "cf-aig-skip-cache").as_deref(),
        Some("true")
    );
    assert_eq!(
        header(&request, "cf-aig-metadata").as_deref(),
        Some(r#"{"a":1}"#)
    );
    let ua = header(&request, "user-agent").unwrap();
    assert!(
        ua.starts_with("opencode/1.2.3 cloudflare-ai-gateway ("),
        "{ua}"
    );
}

#[test]
fn registry_ai_gateway_missing_ids_and_token_errors() {
    let registry = ProviderRegistry::new(transports(), "1.0.0");
    let mut route = CatalogRoute::aisdk(
        "cloudflare-ai-gateway",
        "openai/gpt-4o",
        "ai-gateway-provider",
        None,
    );
    route.env = env(&[]);
    assert_eq!(
        registry.resolve(&route).unwrap_err().reason.message(),
        "CLOUDFLARE_ACCOUNT_ID and CLOUDFLARE_GATEWAY_ID missing. Set with: export CLOUDFLARE_ACCOUNT_ID=<value> && export CLOUDFLARE_GATEWAY_ID=<value>"
    );
    route.env = env(&[
        ("CLOUDFLARE_ACCOUNT_ID", "a"),
        ("CLOUDFLARE_GATEWAY_ID", "g"),
    ]);
    assert!(
        registry
            .resolve(&route)
            .unwrap_err()
            .reason
            .message()
            .starts_with("CLOUDFLARE_API_TOKEN (or CF_AIG_TOKEN) is required")
    );
}

#[tokio::test]
async fn registry_workers_ai_account_url_and_key() {
    let server = MockServer::start().await;
    mount(
        &server,
        "/client/v4/accounts/acct/ai/v1/chat/completions",
        chat_hello(),
    )
    .await;
    let mut route = CatalogRoute::aisdk(
        "cloudflare-workers-ai",
        "@cf/meta/llama-3.1-8b-instruct",
        "@ai-sdk/openai-compatible",
        Some(&format!(
            "{}/client/v4/accounts/${{CLOUDFLARE_ACCOUNT_ID}}/ai/v1",
            server.uri()
        )),
    );
    route.env = env(&[
        ("CLOUDFLARE_ACCOUNT_ID", "acct"),
        ("CLOUDFLARE_API_KEY", "wk"),
    ]);
    let model = ProviderRegistry::new(transports(), "1.0.0")
        .resolve(&route)
        .unwrap();
    say_hello(model).await;
    let request = only_request(&server).await;
    assert_eq!(
        header(&request, "authorization").as_deref(),
        Some("Bearer wk")
    );
    assert!(
        header(&request, "user-agent")
            .unwrap()
            .contains("cloudflare-workers-ai")
    );
    route.api = opencode_provider_adapters::providers::Api::Aisdk {
        package: "@ai-sdk/openai-compatible".into(),
        url: None,
    };
    route.env = env(&[]);
    assert_eq!(
        ProviderRegistry::new(transports(), "1.0.0")
            .resolve(&route)
            .unwrap_err()
            .reason
            .message(),
        "CLOUDFLARE_ACCOUNT_ID is missing. Set it with: export CLOUDFLARE_ACCOUNT_ID=<your-account-id>"
    );
}
