//! Purpose: registry coverage for every `ProviderPlugins` id (packages/core/src/plugin/provider.ts
//! :36-71): each resolves to a native route or fails with the typed `NoRoute (not yet ported)`
//! error; plus catalog attribution headers, opencode public key and Azure/Copilot catalog rules.
//! Scope: fixture-based over wiremock; no live calls.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- provider-adapters/src/providers/inventory.rs
//! |-- provider-adapters/src/providers/loaders.rs
//! |-- provider-adapters/src/providers/registry.rs
//! `-- provider-adapters/tests/providers_support/mod.rs
//! External:
//! |-- serde_json::json
//! `-- wiremock::MockServer
//! Used by:
//! `-- none, entrypoint
//! Invariants: the id list below is copied from the baseline file, not from `PLUGINS`.
//! Contract tests: provider-adapters/tests/providers_registry.rs

mod providers_support;

use opencode_llm_api::LlmErrorReason;
use opencode_provider_adapters::auth::bearer_value;
use opencode_provider_adapters::providers::{
    Api, CatalogRoute, PLUGINS, ProviderRegistry, Support,
};
use pretty_assertions::assert_eq;
use providers_support::*;
use serde_json::json;
use wiremock::MockServer;

/// `ProviderPlugins` ids, in baseline order (plugin `define({ id })` values).
const BASELINE_IDS: &[&str] = &[
    "alibaba",
    "amazon-bedrock",
    "anthropic",
    "azure-cognitive-services",
    "azure",
    "cerebras",
    "cloudflare-ai-gateway",
    "cloudflare-workers-ai",
    "cohere",
    "deepinfra",
    "gateway",
    "github-copilot",
    "gitlab",
    "google",
    "google-vertex-anthropic",
    "google-vertex",
    "groq",
    "kilo",
    "llmgateway",
    "mistral",
    "nvidia",
    "opencode",
    "snowflake-cortex",
    "openai-compatible",
    "openai",
    "openrouter",
    "perplexity",
    "sap-ai-core",
    "togetherai",
    "vercel",
    "venice",
    "xai",
    "zenmux",
    "dynamic-provider",
];

fn registry() -> ProviderRegistry {
    ProviderRegistry::new(transports(), "1.0.0")
}

/// A representative catalog entry for each plugin id, with credentials present.
fn catalog_for(id: &str) -> CatalogRoute {
    let (package, url): (&str, Option<&str>) = match id {
        "alibaba" => ("@ai-sdk/alibaba", None),
        "amazon-bedrock" => ("@ai-sdk/amazon-bedrock", None),
        "anthropic" => ("@ai-sdk/anthropic", None),
        "azure-cognitive-services" => ("@ai-sdk/openai-compatible", None),
        "azure" => ("@ai-sdk/azure", None),
        "cerebras" => ("@ai-sdk/cerebras", None),
        "cloudflare-ai-gateway" => ("ai-gateway-provider", None),
        "cloudflare-workers-ai" => ("@ai-sdk/openai-compatible", None),
        "cohere" => ("@ai-sdk/cohere", None),
        "deepinfra" => ("@ai-sdk/deepinfra", None),
        "gateway" => ("@ai-sdk/gateway", None),
        "github-copilot" => ("@ai-sdk/github-copilot", None),
        "gitlab" => ("gitlab-ai-provider", None),
        "google" => ("@ai-sdk/google", None),
        "google-vertex-anthropic" => ("@ai-sdk/google-vertex/anthropic", None),
        "google-vertex" => ("@ai-sdk/google-vertex", None),
        "groq" => ("@ai-sdk/groq", None),
        "kilo" => (
            "@ai-sdk/openai-compatible",
            Some("https://api.kilo.ai/api/gateway"),
        ),
        "llmgateway" => (
            "@ai-sdk/openai-compatible",
            Some("https://api.llmgateway.io/v1"),
        ),
        "mistral" => ("@ai-sdk/mistral", None),
        "nvidia" => (
            "@ai-sdk/openai-compatible",
            Some("https://integrate.api.nvidia.com/v1"),
        ),
        "opencode" => (
            "@ai-sdk/openai-compatible",
            Some("https://opencode.ai/zen/v1"),
        ),
        "snowflake-cortex" => ("@ai-sdk/openai-compatible", None),
        "openai-compatible" => ("@ai-sdk/openai-compatible", Some("https://example.test/v1")),
        "openai" => ("@ai-sdk/openai", None),
        "openrouter" => ("@openrouter/ai-sdk-provider", None),
        "perplexity" => ("@ai-sdk/perplexity", None),
        "sap-ai-core" => ("@jerome-benoit/sap-ai-provider-v2", None),
        "togetherai" => ("@ai-sdk/togetherai", None),
        "vercel" => ("@ai-sdk/vercel", None),
        "venice" => ("venice-ai-sdk-provider", None),
        "xai" => ("@ai-sdk/xai", None),
        "zenmux" => (
            "@ai-sdk/openai-compatible",
            Some("https://zenmux.ai/api/v1"),
        ),
        "dynamic-provider" => ("some-community-provider", None),
        other => panic!("no catalog fixture for {other}"),
    };
    let mut route = CatalogRoute::aisdk(id, "model-1", package, url);
    route.api_key = Some("key".into());
    route.env = env(&[
        ("AZURE_RESOURCE_NAME", "res"),
        ("AZURE_COGNITIVE_SERVICES_RESOURCE_NAME", "cog"),
        ("CLOUDFLARE_ACCOUNT_ID", "acct"),
        ("CLOUDFLARE_GATEWAY_ID", "gw"),
        ("CLOUDFLARE_API_TOKEN", "tok"),
        ("AWS_BEARER_TOKEN_BEDROCK", "brk"),
        ("GOOGLE_CLOUD_PROJECT", "proj"),
        ("SNOWFLAKE_ACCOUNT", "acme"),
    ]);
    if id.starts_with("google-vertex") {
        route.auth = Some(bearer_value("adc"));
    }
    route
}

#[test]
fn inventory_matches_baseline_plugin_list() {
    let ids: Vec<&str> = PLUGINS.iter().map(|p| p.id).collect();
    assert_eq!(ids, BASELINE_IDS);
}

#[test]
fn every_plugin_id_resolves_or_fails_typed() {
    let registry = registry();
    for plugin in PLUGINS {
        let result = registry.resolve(&catalog_for(plugin.id));
        match (plugin.support, result) {
            (Support::NotYetPorted, Ok(_)) => panic!("{} should not resolve", plugin.id),
            (Support::NotYetPorted, Err(e)) => match e.reason.as_ref() {
                LlmErrorReason::NoRoute {
                    route, provider, ..
                } => {
                    assert!(
                        route.ends_with("(not yet ported)"),
                        "{}: {route}",
                        plugin.id
                    );
                    assert_eq!(provider.as_str(), plugin.id);
                    assert!(!e.retryable());
                }
                other => panic!("{}: expected NoRoute, got {other:?}", plugin.id),
            },
            (_, Ok(model)) => assert_eq!(model.provider.as_str(), plugin.id),
            (_, Err(e)) => panic!("{} failed: {}", plugin.id, e.reason.message()),
        }
    }
}

#[test]
fn native_api_and_url_less_compatible_fail_typed() {
    let mut route = CatalogRoute::new("custom", "m", Api::Native { url: None });
    route.env = env(&[]);
    assert_eq!(
        registry().resolve(&route).unwrap_err().reason.tag(),
        "NoRoute"
    );
    let mut route = CatalogRoute::aisdk("custom", "m", "@ai-sdk/openai-compatible", None);
    route.env = env(&[]);
    assert_eq!(
        registry().resolve(&route).unwrap_err().reason.tag(),
        "NoRoute"
    );
}

async fn capture(route: CatalogRoute, server: &MockServer) -> wiremock::Request {
    say_hello(registry().resolve(&route).unwrap()).await;
    only_request(server).await
}

#[tokio::test]
async fn nvidia_kilo_zenmux_attribution_headers() {
    for (id, path, extra) in [
        (
            "nvidia",
            "/nv",
            Some(("x-billing-invoke-origin", "OpenCode")),
        ),
        ("kilo", "/kilo", None),
        ("zenmux", "/zen", None),
    ] {
        let server = MockServer::start().await;
        mount(&server, &format!("{path}/chat/completions"), chat_hello()).await;
        let mut route = catalog_for(id);
        // The header rules key on the exact catalog URL; route traffic to wiremock afterwards.
        let request = {
            let mut shaped = route.clone();
            opencode_provider_adapters::providers::loaders::catalog_headers(&mut shaped);
            route.headers = shaped.headers;
            route.api = Api::Aisdk {
                package: "@ai-sdk/openai-compatible".into(),
                url: Some(format!("{}{path}", server.uri())),
            };
            capture(route, &server).await
        };
        assert_eq!(
            header(&request, "http-referer").as_deref(),
            Some("https://opencode.ai/"),
            "{id}"
        );
        assert_eq!(
            header(&request, "x-title").as_deref(),
            Some("opencode"),
            "{id}"
        );
        assert_eq!(
            header(&request, "authorization").as_deref(),
            Some("Bearer key"),
            "{id}"
        );
        if let Some((name, value)) = extra {
            assert_eq!(header(&request, name).as_deref(), Some(value), "{id}");
        }
    }
}

#[test]
fn header_rules_set_vs_default_and_llmgateway_requires_connection() {
    use opencode_provider_adapters::providers::loaders::catalog_headers;
    let mut nvidia = catalog_for("nvidia");
    nvidia
        .headers
        .insert("x-billing-invoke-origin".into(), "Custom".into());
    catalog_headers(&mut nvidia);
    assert_eq!(
        nvidia.headers.get("x-billing-invoke-origin").unwrap(),
        "Custom"
    );
    assert!(!nvidia.headers.contains_key("X-BILLING-INVOKE-ORIGIN"));
    let mut zen = catalog_for("zenmux");
    zen.headers.insert("X-Title".into(), "mine".into());
    catalog_headers(&mut zen);
    assert_eq!(zen.headers.get("X-Title").unwrap(), "mine");
    let mut gw = catalog_for("llmgateway");
    catalog_headers(&mut gw);
    assert!(gw.headers.is_empty());
    gw.connected = true;
    catalog_headers(&mut gw);
    assert_eq!(gw.headers.get("X-Source").unwrap(), "opencode");
    let mut cerebras = catalog_for("cerebras");
    catalog_headers(&mut cerebras);
    assert_eq!(
        cerebras
            .headers
            .get("X-Cerebras-3rd-Party-Integration")
            .unwrap(),
        "opencode"
    );
}

#[tokio::test]
async fn anthropic_registry_adds_beta_header() {
    let server = MockServer::start().await;
    mount(
        &server,
        "/v1/messages",
        cassette_body("anthropic-messages/streams-text"),
    )
    .await;
    let mut route = catalog_for("anthropic");
    route.api = Api::Aisdk {
        package: "@ai-sdk/anthropic".into(),
        url: Some(format!("{}/v1", server.uri())),
    };
    let request = capture(route, &server).await;
    assert_eq!(
        header(&request, "anthropic-beta").as_deref(),
        Some("interleaved-thinking-2025-05-14,fine-grained-tool-streaming-2025-05-14")
    );
    assert_eq!(header(&request, "x-api-key").as_deref(), Some("key"));
}

#[tokio::test]
async fn opencode_without_key_uses_public() {
    let server = MockServer::start().await;
    mount(&server, "/zen/v1/chat/completions", chat_hello()).await;
    let mut route = CatalogRoute::aisdk(
        "opencode",
        "big-pickle",
        "@ai-sdk/openai-compatible",
        Some(&format!("{}/zen/v1", server.uri())),
    );
    route.env = env(&[]);
    let request = capture(route, &server).await;
    assert_eq!(
        header(&request, "authorization").as_deref(),
        Some("Bearer public")
    );
}

#[tokio::test]
async fn azure_registry_resource_url_and_completion_urls() {
    let route = catalog_for("azure");
    let model = registry().resolve(&route).unwrap();
    assert_eq!(model.route.id(), "openai-responses");
    assert!(format!("{:?}", model.route).contains("https://res.openai.azure.com/openai/v1"));
    let mut chat = catalog_for("azure");
    chat.options = json!({ "useCompletionUrls": true })
        .as_object()
        .cloned()
        .unwrap();
    assert_eq!(registry().resolve(&chat).unwrap().route.id(), "openai-chat");
    let mut missing = catalog_for("azure");
    missing.env = env(&[]);
    assert_eq!(
        registry().resolve(&missing).unwrap_err().reason.message(),
        "AZURE_RESOURCE_NAME is missing, set it using env var or reconnecting the azure provider and setting it"
    );
    let server = MockServer::start().await;
    mount(
        &server,
        "/openai/deployments/model-1/chat/completions",
        chat_hello(),
    )
    .await;
    let mut deployments = catalog_for("azure");
    deployments.api = Api::Aisdk {
        package: "@ai-sdk/azure".into(),
        url: Some(format!("{}/openai", server.uri())),
    };
    deployments.options = json!({ "useDeploymentBasedUrls": true, "useCompletionUrls": true, "apiVersion": "2024-10-21" })
        .as_object()
        .cloned()
        .unwrap();
    let request = capture(deployments, &server).await;
    assert_eq!(
        path_and_query(&request),
        "/openai/deployments/model-1/chat/completions?api-version=2024-10-21"
    );
    assert_eq!(header(&request, "api-key").as_deref(), Some("key"));
}

#[test]
fn copilot_registry_endpoint_option_and_default_url() {
    let mut route = catalog_for("github-copilot");
    route.model_id = "gpt-5".into();
    let model = registry().resolve(&route).unwrap();
    assert_eq!(model.route.id(), "openai-responses");
    assert!(format!("{:?}", model.route).contains("https://api.githubcopilot.com"));
    route.options = json!({ "endpoint": "chat" }).as_object().cloned().unwrap();
    assert_eq!(
        registry().resolve(&route).unwrap().route.id(),
        "openai-chat"
    );
}

#[test]
fn cognitive_services_and_deepinfra_urls() {
    let model = registry()
        .resolve(&catalog_for("azure-cognitive-services"))
        .unwrap();
    assert!(
        format!("{:?}", model.route).contains("https://cog.cognitiveservices.azure.com/openai")
    );
    let model = registry().resolve(&catalog_for("deepinfra")).unwrap();
    assert!(format!("{:?}", model.route).contains("https://api.deepinfra.com/v1/openai"));
}
