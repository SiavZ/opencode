//! Purpose: catalog transforms and special loaders that shape a provider before route
//! construction: attribution headers (anthropic, cerebras, kilo, llmgateway, nvidia, openrouter,
//! vercel, zenmux), Azure resource names, Vertex project/location, Cloudflare account/gateway,
//! Snowflake account/token and the opencode public key (packages/core/src/plugin/provider/*.ts,
//! packages/opencode/src/provider/provider.ts:209-1050).
//! Scope: pure functions over `CatalogRoute`; no I/O.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- provider-adapters/src/auth.rs
//! `-- provider-adapters/src/providers/inventory.rs
//! External:
//! `-- str::to_owned
//! Used by:
//! rust/crates/
//! |-- provider-adapters/src/providers/cloud.rs
//! |-- provider-adapters/src/providers/mod.rs
//! |-- provider-adapters/src/providers/registry.rs
//! |-- provider-adapters/src/providers/registry_rules.rs
//! `-- provider-adapters/tests/providers_registry.rs
//! Invariants: `set` rules overwrite, `??=` rules keep any existing value (case-insensitive).
//! Error messages are byte-identical to the baseline loaders.
//! Contract tests: provider-adapters/tests/providers_registry.rs

use opencode_llm_api::LlmError;

use crate::auth::Headers;
use crate::providers::inventory::CatalogRoute;

const REFERER: (&str, &str) = ("HTTP-Referer", "https://opencode.ai/");
const TITLE: (&str, &str) = ("X-Title", "opencode");
pub const ANTHROPIC_BETA: &str =
    "interleaved-thinking-2025-05-14,fine-grained-tool-streaming-2025-05-14";

fn set(headers: &mut Headers, name: &str, value: &str) {
    headers.insert(name.to_owned(), value.to_owned());
}

fn set_default(headers: &mut Headers, name: &str, value: &str) {
    if !headers.keys().any(|k| k.eq_ignore_ascii_case(name)) {
        set(headers, name, value);
    }
}

/// All `catalog.transform` header rules, applied in `ProviderPlugins` order.
pub fn catalog_headers(route: &mut CatalogRoute) {
    let package = route.package().unwrap_or_default().to_owned();
    let url = route.url().unwrap_or_default().to_owned();
    let compat = package == "@ai-sdk/openai-compatible";
    let h = &mut route.headers;
    if package == "@ai-sdk/anthropic" {
        set(h, "anthropic-beta", ANTHROPIC_BETA);
    }
    if package == "@ai-sdk/cerebras" {
        set(h, "X-Cerebras-3rd-Party-Integration", "opencode");
    }
    if compat && url == "https://api.kilo.ai/api/gateway" {
        set(h, REFERER.0, REFERER.1);
        set(h, TITLE.0, TITLE.1);
    }
    if compat && url == "https://api.llmgateway.io/v1" && route.connected {
        set(h, REFERER.0, REFERER.1);
        set(h, TITLE.0, TITLE.1);
        set(h, "X-Source", "opencode");
    }
    if compat && url == "https://integrate.api.nvidia.com/v1" {
        set(h, REFERER.0, REFERER.1);
        set(h, TITLE.0, TITLE.1);
        set_default(h, "X-BILLING-INVOKE-ORIGIN", "OpenCode");
    }
    if package == "@openrouter/ai-sdk-provider" {
        set(h, REFERER.0, REFERER.1);
        set(h, TITLE.0, TITLE.1);
    }
    if package == "@ai-sdk/vercel" {
        set(h, "http-referer", "https://opencode.ai/");
        set(h, "x-title", "opencode");
    }
    if compat && url == "https://zenmux.ai/api/v1" {
        set_default(h, REFERER.0, REFERER.1);
        set_default(h, TITLE.0, TITLE.1);
    }
}

/// OpencodePlugin: without `OPENCODE_API_KEY`, a connection or a configured key the provider
/// uses the literal `public` key (paid models are disabled by the catalog).
pub fn opencode_public_key(route: &mut CatalogRoute) {
    if route.provider_id != "opencode" {
        return;
    }
    let has_key =
        route.env_value("OPENCODE_API_KEY").is_some() || route.connected || route.api_key.is_some();
    if !has_key {
        route.api_key = Some("public".into());
    }
}

/// AzurePlugin `resourceName`: non-blank option, credential metadata, then `AZURE_RESOURCE_NAME`.
pub fn azure_resource_name(route: &CatalogRoute) -> Option<String> {
    [route.option("resourceName"), route.metadata("resourceName")]
        .into_iter()
        .flatten()
        .find(|v| !v.trim().is_empty())
        .map(str::to_owned)
        .or_else(|| route.env_value("AZURE_RESOURCE_NAME"))
}

pub const AZURE_RESOURCE_MISSING: &str = "AZURE_RESOURCE_NAME is missing, set it using env var or reconnecting the azure provider and setting it";

/// `AzureCognitiveServicesPlugin`: `https://<name>.cognitiveservices.azure.com/openai`.
pub fn azure_cognitive_base_url(route: &CatalogRoute) -> Option<String> {
    if !route.provider_id.contains("azure-cognitive-services") {
        return None;
    }
    let name = route.env_value("AZURE_COGNITIVE_SERVICES_RESOURCE_NAME")?;
    Some(format!("https://{name}.cognitiveservices.azure.com/openai"))
}

/// GoogleVertexPlugin `resolveProject`.
pub fn vertex_project(route: &CatalogRoute) -> Option<String> {
    route.option("project").map(str::to_owned).or_else(|| {
        route.first_env(&[
            "GOOGLE_VERTEX_PROJECT",
            "GOOGLE_CLOUD_PROJECT",
            "GCP_PROJECT",
            "GCLOUD_PROJECT",
        ])
    })
}

/// GoogleVertexPlugin `resolveLocation`.
pub fn vertex_location(route: &CatalogRoute) -> String {
    route
        .option("location")
        .map(str::to_owned)
        .or_else(|| {
            route.first_env(&[
                "GOOGLE_VERTEX_LOCATION",
                "GOOGLE_CLOUD_LOCATION",
                "VERTEX_LOCATION",
            ])
        })
        .unwrap_or_else(|| "us-central1".into())
}

/// Vertex API host for a location (`getHost` in @ai-sdk/google-vertex).
pub fn vertex_host(location: &str) -> String {
    match location {
        "global" => "aiplatform.googleapis.com".into(),
        "eu" | "us" => format!("aiplatform.{location}.rep.googleapis.com"),
        _ => format!("{location}-aiplatform.googleapis.com"),
    }
}

/// `replaceVertexVars`: expand `${GOOGLE_VERTEX_*}` templates in catalog URLs. The core
/// plugin's `vertexEndpoint` has no `eu`/`us` REP branch; the SDK and legacy loader do.
pub fn replace_vertex_vars(value: &str, project: Option<&str>, location: &str) -> String {
    let endpoint = match location {
        "global" => "aiplatform.googleapis.com".to_owned(),
        _ => format!("{location}-aiplatform.googleapis.com"),
    };
    value
        .replace(
            "${GOOGLE_VERTEX_PROJECT}",
            project.unwrap_or("${GOOGLE_VERTEX_PROJECT}"),
        )
        .replace("${GOOGLE_VERTEX_LOCATION}", location)
        .replace("${GOOGLE_VERTEX_ENDPOINT}", &endpoint)
}

/// GoogleVertexAnthropicPlugin project (`GOOGLE_VERTEX_PROJECT` is the SDK's own fallback).
pub fn vertex_anthropic_project(route: &CatalogRoute) -> Option<String> {
    route.option("project").map(str::to_owned).or_else(|| {
        route.first_env(&[
            "GOOGLE_CLOUD_PROJECT",
            "GCP_PROJECT",
            "GCLOUD_PROJECT",
            "GOOGLE_VERTEX_PROJECT",
        ])
    })
}

/// GoogleVertexAnthropicPlugin location (default `global`).
pub fn vertex_anthropic_location(route: &CatalogRoute) -> String {
    route
        .option("location")
        .map(str::to_owned)
        .or_else(|| route.first_env(&["GOOGLE_CLOUD_LOCATION", "VERTEX_LOCATION"]))
        .unwrap_or_else(|| "global".into())
}

/// Cloudflare account id: `CLOUDFLARE_ACCOUNT_ID`, option, then credential metadata.
pub fn cloudflare_account(route: &CatalogRoute) -> Option<String> {
    route
        .env_value("CLOUDFLARE_ACCOUNT_ID")
        .or_else(|| route.option("accountId").map(str::to_owned))
        .or_else(|| route.metadata("accountId").map(str::to_owned))
}

/// Cloudflare gateway id: `CLOUDFLARE_GATEWAY_ID`, `gatewayId`, `gateway`, metadata.
pub fn cloudflare_gateway(route: &CatalogRoute) -> Option<String> {
    route
        .env_value("CLOUDFLARE_GATEWAY_ID")
        .or_else(|| route.option("gatewayId").map(str::to_owned))
        .or_else(|| route.option("gateway").map(str::to_owned))
        .or_else(|| route.metadata("gatewayId").map(str::to_owned))
}

/// Legacy `cloudflare-ai-gateway` loader error for missing ids.
pub fn cloudflare_gateway_missing(account: bool, gateway: bool) -> LlmError {
    let missing: Vec<&str> = [
        (!account).then_some("CLOUDFLARE_ACCOUNT_ID"),
        (!gateway).then_some("CLOUDFLARE_GATEWAY_ID"),
    ]
    .into_iter()
    .flatten()
    .collect();
    let exports: Vec<String> = missing
        .iter()
        .map(|x| format!("export {x}=<value>"))
        .collect();
    LlmError::invalid_request(format!(
        "{} missing. Set with: {}",
        missing.join(" and "),
        exports.join(" && ")
    ))
}

pub const CLOUDFLARE_TOKEN_MISSING: &str = "CLOUDFLARE_API_TOKEN (or CF_AIG_TOKEN) is required for Cloudflare AI Gateway. Set it via environment variable or run `opencode auth cloudflare-ai-gateway`.";
pub const CLOUDFLARE_ACCOUNT_MISSING: &str =
    "CLOUDFLARE_ACCOUNT_ID is missing. Set it with: export CLOUDFLARE_ACCOUNT_ID=<your-account-id>";

/// `expandAccountId(baseURL)` for Workers AI catalog URLs.
pub fn expand_account_id(url: &str, route: &CatalogRoute) -> String {
    match route.env_value("CLOUDFLARE_ACCOUNT_ID") {
        Some(account) => url.replace("${CLOUDFLARE_ACCOUNT_ID}", &account),
        None => url.to_owned(),
    }
}

/// Snowflake base URL and bearer token (legacy loader precedence). A catalog URL stands in
/// for the account.
pub fn snowflake(route: &CatalogRoute) -> Result<(String, String), LlmError> {
    let account = route
        .env_value("SNOWFLAKE_ACCOUNT")
        .or_else(|| route.metadata("account").map(str::to_owned))
        .or_else(|| route.option("account").map(str::to_owned));
    let token = route
        .first_env(&["SNOWFLAKE_CORTEX_TOKEN", "SNOWFLAKE_CORTEX_PAT"])
        .or_else(|| route.api_key.clone())
        .or_else(|| route.option("token").map(str::to_owned))
        .or_else(|| route.option("apiKey").map(str::to_owned));
    let has_url = route.url().is_some();
    let base = route
        .url()
        .map(str::to_owned)
        .or_else(|| account.as_deref().map(snowflake_base_url));
    match (base, token) {
        (Some(base), Some(token)) => Ok((base, token)),
        (_, token) => {
            let missing: Vec<&str> = [
                (account.is_none() && !has_url).then_some("SNOWFLAKE_ACCOUNT"),
                token.is_none().then_some("SNOWFLAKE_CORTEX_TOKEN"),
            ]
            .into_iter()
            .flatten()
            .collect();
            Err(LlmError::invalid_request(format!(
                "Snowflake Cortex: missing credentials ({}). Provide a bearer token (OAuth, JWT, or PAT) via env var, opencode auth, or provider options.",
                missing.join(", ")
            )))
        }
    }
}

/// `https://<account>.snowflakecomputing.com/api/v2/cortex/v1`.
pub fn snowflake_base_url(account: &str) -> String {
    format!("https://{account}.snowflakecomputing.com/api/v2/cortex/v1")
}

/// Node-style platform user agent suffix: `(<platform> <release>; <arch>)`.
pub fn platform_suffix(release: &str) -> String {
    let platform = match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        "x86" => "ia32",
        other => other,
    };
    format!("({platform} {release}; {arch})")
}
