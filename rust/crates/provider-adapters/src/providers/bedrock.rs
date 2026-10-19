//! Purpose: Amazon Bedrock facade (packages/llm/src/providers/amazon-bedrock.ts) plus the
//! cross-region inference prefix rules from packages/core/src/plugin/provider/amazon-bedrock.ts
//! `resolveModelID` and the legacy loader (packages/opencode/src/provider/provider.ts:336-498).
//! Scope: route configuration (SigV4 or bearer auth, region base URL) and model id rewriting.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- provider-adapters/src/auth.rs
//! |-- provider-adapters/src/protocols/bedrock/mod.rs
//! |-- provider-adapters/src/providers/config.rs
//! `-- provider-adapters/src/route.rs
//! External:
//! |-- bedrock::base_url
//! `-- bedrock::route
//! Used by:
//! rust/crates/
//! |-- provider-adapters/src/providers/cloud.rs
//! |-- provider-adapters/src/providers/mod.rs
//! `-- provider-adapters/tests/providers_facades.rs
//! Invariants: an `apiKey` selects bearer auth, otherwise SigV4 over `credentials`. Region is
//! `region ?? credentials.region ?? "us-east-1"`. Prefixing never double-prefixes or touches ARNs.
//! Contract tests: provider-adapters/tests/providers_facades.rs, provider-adapters/tests/providers_registry.rs

use std::sync::Arc;

use opencode_llm_api::{Model, ProviderId};

use crate::auth::bearer_value;
use crate::protocols::bedrock::{self, BedrockCredentials, SigV4Auth};
use crate::providers::config::{ProviderConfig, Transports, apply_defaults};
use crate::route::ProtocolRoute;

pub const ID: &str = "amazon-bedrock";
pub const DEFAULT_REGION: &str = "us-east-1";
const CROSS_REGION_PREFIXES: &[&str] = &["global.", "us.", "eu.", "jp.", "apac.", "au."];

/// `AmazonBedrock.Config` beyond the shared provider config.
#[derive(Clone, Debug, Default)]
pub struct BedrockConfig {
    pub credentials: Option<BedrockCredentials>,
    pub region: Option<String>,
}

#[derive(Clone, Debug)]
pub struct BedrockProvider {
    pub route: ProtocolRoute,
    pub region: String,
}

/// `AmazonBedrock.configure(input)`.
pub fn configure(
    config: &ProviderConfig,
    bedrock: &BedrockConfig,
    transports: &Transports,
) -> BedrockProvider {
    let region = bedrock
        .region
        .clone()
        .or_else(|| bedrock.credentials.as_ref().map(|c| c.region.clone()))
        .unwrap_or_else(|| DEFAULT_REGION.to_owned());
    let mut route = bedrock::route(transports.event_stream.clone(), bedrock.credentials.clone());
    route.provider = Some(ProviderId::new(ID));
    route.endpoint.base_url = Some(
        config
            .base_url
            .clone()
            .unwrap_or_else(|| bedrock::base_url(&region)),
    );
    route.auth = match (&config.auth, &config.api_key) {
        (Some(auth), _) => auth.clone(),
        (None, Some(key)) => bearer_value(key.clone()),
        (None, None) => Arc::new(SigV4Auth::new(bedrock.credentials.clone())),
    };
    apply_defaults(&mut route, &config.defaults);
    BedrockProvider { route, region }
}

impl BedrockProvider {
    pub fn model(&self, id: &str) -> Model {
        Model::new(id, ID, Arc::new(self.route.clone()))
    }

    /// Catalog model selection: applies the cross-region prefix for this provider's region.
    pub fn catalog_model(&self, id: &str) -> Model {
        self.model(&resolve_model_id(id, Some(&self.region)))
    }
}

/// `resolveModelID(modelID, region)`: cross-region inference profile prefixes.
pub fn resolve_model_id(model_id: &str, region: Option<&str>) -> String {
    if model_id.starts_with("arn:") {
        return model_id.to_owned();
    }
    if CROSS_REGION_PREFIXES
        .iter()
        .any(|p| model_id.starts_with(p))
    {
        return model_id.to_owned();
    }
    let region = region.unwrap_or(DEFAULT_REGION);
    let prefix = region.split('-').next().unwrap_or_default();
    let has = |items: &[&str]| items.iter().any(|item| model_id.contains(item));
    match prefix {
        "us" => {
            let requires = has(&[
                "nova-micro",
                "nova-lite",
                "nova-pro",
                "nova-premier",
                "nova-2",
                "claude",
                "deepseek.r1",
            ]);
            if requires && !region.starts_with("us-gov") {
                return format!("us.{model_id}");
            }
            model_id.to_owned()
        }
        "eu" => {
            let region_requires = [
                "eu-west-1",
                "eu-west-2",
                "eu-west-3",
                "eu-north-1",
                "eu-central-1",
                "eu-south-1",
                "eu-south-2",
            ]
            .iter()
            .any(|r| region.contains(r));
            let model_requires = has(&["claude", "nova-lite", "nova-micro", "llama3", "pixtral"]);
            if region_requires && model_requires {
                return format!("eu.{model_id}");
            }
            model_id.to_owned()
        }
        "ap" => {
            let australia = ["ap-southeast-2", "ap-southeast-4"].contains(&region);
            if australia && has(&["anthropic.claude-sonnet-4-5", "anthropic.claude-haiku"]) {
                return format!("au.{model_id}");
            }
            let prefix = if region == "ap-northeast-1" {
                "jp"
            } else {
                "apac"
            };
            if has(&["claude", "nova-lite", "nova-micro", "nova-pro"]) {
                return format!("{prefix}.{model_id}");
            }
            model_id.to_owned()
        }
        _ => model_id.to_owned(),
    }
}

/// `selectMantleModel`: the two safeguard models use Chat, everything else Responses.
pub fn mantle_uses_chat(model_id: &str) -> bool {
    model_id == "openai.gpt-oss-safeguard-20b" || model_id == "openai.gpt-oss-safeguard-120b"
}

/// `https://bedrock-mantle.<region>.api.aws/v1` (`@ai-sdk/amazon-bedrock/mantle`).
pub fn mantle_base_url(region: &str) -> String {
    format!("https://bedrock-mantle.{region}.api.aws/v1")
}
