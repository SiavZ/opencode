//! Purpose: Azure OpenAI and GitHub Copilot facades (packages/llm/src/providers/azure.ts,
//! github-copilot.ts): Azure resource URL + `api-version` query + `api-key` header auth, Copilot
//! Responses-vs-Chat routing (`shouldUseResponsesApi`), both with OpenAI option defaults.
//! Scope: route configuration and model construction. No I/O.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- provider-adapters/src/auth.rs
//! |-- provider-adapters/src/protocols/openai/mod.rs
//! |-- provider-adapters/src/providers/config.rs
//! |-- provider-adapters/src/providers/openai.rs
//! `-- provider-adapters/src/route.rs
//! External:
//! |-- char::is_ascii_digit
//! |-- openai::chat_route
//! `-- openai::responses_route
//! Used by:
//! rust/crates/
//! |-- provider-adapters/src/providers/mod.rs
//! |-- provider-adapters/src/providers/registry_rules.rs
//! `-- provider-adapters/tests/providers_facades.rs
//! Invariants: Azure removes `authorization` before setting `api-key`; `baseURL` wins over
//! `resourceName`. Copilot GPT >= 5 (except gpt-5-mini) uses Responses unless `endpoint` is set.
//! Contract tests: provider-adapters/tests/providers_facades.rs

use std::sync::Arc;

use opencode_llm_api::{LlmError, Model, ProviderId, RouteDefaults};

use crate::auth::{AndThen, CredentialHeader, RemoveHeader, SharedAuth};
use crate::protocols::openai;
use crate::providers::config::{ProviderConfig, Transports, patch_endpoint};
use crate::providers::openai::model_with_options;
use crate::route::ProtocolRoute;

pub const AZURE_ID: &str = "azure";
pub const AZURE_ENV: &[&str] = &["AZURE_OPENAI_API_KEY"];
pub const COPILOT_ID: &str = "github-copilot";

/// `https://<resourceName>.openai.azure.com/openai/v1`.
pub fn azure_resource_base_url(resource: &str) -> String {
    format!("https://{}.openai.azure.com/openai/v1", resource.trim())
}

/// Azure-specific options (`resourceName`, `apiVersion`, `useCompletionUrls`).
#[derive(Clone, Debug, Default)]
pub struct AzureConfig {
    pub resource_name: Option<String>,
    pub api_version: Option<String>,
    pub use_completion_urls: bool,
}

#[derive(Clone, Debug)]
pub struct AzureProvider {
    pub responses_route: ProtocolRoute,
    pub chat_route: ProtocolRoute,
    pub defaults: RouteDefaults,
    pub use_completion_urls: bool,
}

/// `Azure.configure(input)`. Fails when neither `baseURL` nor `resourceName` is set (the TS
/// type `AtLeastOne` makes that a compile error).
pub fn azure(
    config: &ProviderConfig,
    azure: &AzureConfig,
    transports: &Transports,
) -> Result<AzureProvider, LlmError> {
    let base_url = match (&config.base_url, &azure.resource_name) {
        (Some(url), _) => url.clone(),
        (None, Some(resource)) => azure_resource_base_url(resource),
        (None, None) => {
            return Err(LlmError::invalid_request(
                "Azure.configure requires resourceName or baseURL",
            ));
        }
    };
    let auth: SharedAuth = match &config.auth {
        Some(auth) => auth.clone(),
        None => Arc::new(AndThen(
            Arc::new(RemoveHeader("authorization".into())),
            Arc::new(CredentialHeader::header(
                "api-key",
                config.credential(AZURE_ENV),
            )),
        )),
    };
    let mut query = crate::auth::Headers::new();
    if let Some(version) = azure.api_version.as_deref().filter(|v| !v.is_empty()) {
        query.insert("api-version".into(), version.to_owned());
    }
    query.extend(config.query.clone());
    let configured = |mut route: ProtocolRoute, id: &str| {
        route.id = id.to_owned();
        route.provider = Some(ProviderId::new(AZURE_ID));
        route
            .endpoint
            .query
            .insert("api-version".into(), "v1".into());
        route.auth = auth.clone();
        patch_endpoint(&mut route, Some(&base_url), &query);
        route
    };
    Ok(AzureProvider {
        responses_route: configured(
            openai::responses_route(transports.sse.clone()),
            "azure-openai-responses",
        ),
        chat_route: configured(
            openai::chat_route(transports.sse.clone()),
            "azure-openai-chat",
        ),
        defaults: config.defaults.clone(),
        use_completion_urls: azure.use_completion_urls,
    })
}

impl AzureProvider {
    pub fn model(&self, id: &str) -> Model {
        if self.use_completion_urls {
            return self.chat(id);
        }
        self.responses(id)
    }

    pub fn responses(&self, id: &str) -> Model {
        model_with_options(&self.responses_route, &self.defaults, AZURE_ID, id, false)
    }

    pub fn chat(&self, id: &str) -> Model {
        model_with_options(&self.chat_route, &self.defaults, AZURE_ID, id, false)
    }
}

/// Copilot endpoint override (`model.api.endpoint` / `options.endpoint`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopilotEndpoint {
    Chat,
    Responses,
}

/// `shouldUseResponsesApi(modelID, endpoint)`.
pub fn should_use_responses_api(model_id: &str, endpoint: Option<CopilotEndpoint>) -> bool {
    if let Some(endpoint) = endpoint {
        return endpoint == CopilotEndpoint::Responses;
    }
    let Some(rest) = model_id.strip_prefix("gpt-") else {
        return false;
    };
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    let Ok(major) = digits.parse::<u64>() else {
        return false;
    };
    major >= 5 && !model_id.starts_with("gpt-5-mini")
}

#[derive(Clone, Debug)]
pub struct CopilotProvider {
    pub responses_route: ProtocolRoute,
    pub chat_route: ProtocolRoute,
    pub defaults: RouteDefaults,
    pub endpoint: Option<CopilotEndpoint>,
}

/// `GitHubCopilot.configure(options)`: `baseURL` is required (no canonical public URL).
pub fn copilot(
    config: &ProviderConfig,
    endpoint: Option<CopilotEndpoint>,
    transports: &Transports,
) -> Result<CopilotProvider, LlmError> {
    let Some(base_url) = config.base_url.as_deref() else {
        return Err(LlmError::invalid_request(
            "GitHubCopilot.configure requires baseURL",
        ));
    };
    let auth = config.bearer(&[]);
    let configured = |mut route: ProtocolRoute| {
        route.provider = Some(ProviderId::new(COPILOT_ID));
        route.auth = auth.clone();
        patch_endpoint(&mut route, Some(base_url), &config.query);
        route
    };
    Ok(CopilotProvider {
        responses_route: configured(openai::responses_route(transports.sse.clone())),
        chat_route: configured(openai::chat_route(transports.sse.clone())),
        defaults: config.defaults.clone(),
        endpoint,
    })
}

impl CopilotProvider {
    pub fn model(&self, id: &str) -> Model {
        if should_use_responses_api(id, self.endpoint) {
            return self.responses(id);
        }
        self.chat(id)
    }

    pub fn responses(&self, id: &str) -> Model {
        model_with_options(&self.responses_route, &self.defaults, COPILOT_ID, id, false)
    }

    pub fn chat(&self, id: &str) -> Model {
        model_with_options(&self.chat_route, &self.defaults, COPILOT_ID, id, false)
    }
}
