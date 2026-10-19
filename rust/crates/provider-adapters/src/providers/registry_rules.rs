//! Purpose: per-package registry rules that need provider-specific URL and auth assembly:
//! OpenAI-compatible (Workers AI, Vertex OpenAI endpoints, Azure Cognitive Services), Cloudflare
//! AI Gateway, Azure OpenAI and GitHub Copilot (packages/core/src/plugin/provider/{azure,
//! cloudflare-*,github-copilot,google-vertex}.ts, @ai-sdk/azure URL rules).
//! Scope: second `impl ProviderRegistry` block; route construction only.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- provider-adapters/src/auth.rs
//! |-- provider-adapters/src/protocols/openai/mod.rs
//! |-- provider-adapters/src/providers/azure.rs
//! |-- provider-adapters/src/providers/config.rs
//! |-- provider-adapters/src/providers/gateways.rs
//! |-- provider-adapters/src/providers/inventory.rs
//! |-- provider-adapters/src/providers/loaders.rs
//! |-- provider-adapters/src/providers/registry.rs
//! |-- provider-adapters/src/route.rs
//! |-- provider-adapters/src/shared.rs
//! `-- provider-adapters/src/unsupported.rs
//! External:
//! |-- gateways::AI_GATEWAY_ENV
//! |-- gateways::WORKERS_AI_ID
//! |-- gateways::ai_gateway
//! |-- gateways::is_workers_ai_model
//! |-- gateways::workers_ai_base_url
//! |-- loaders::AZURE_RESOURCE_MISSING
//! |-- loaders::CLOUDFLARE_ACCOUNT_MISSING
//! |-- loaders::CLOUDFLARE_TOKEN_MISSING
//! |-- loaders::azure_cognitive_base_url
//! |-- loaders::azure_resource_name
//! |-- loaders::cloudflare_account
//! |-- loaders::cloudflare_gateway
//! |-- loaders::cloudflare_gateway_missing
//! |-- loaders::expand_account_id
//! |-- loaders::replace_vertex_vars
//! |-- loaders::vertex_location
//! |-- loaders::vertex_project
//! |-- openai::chat_route
//! |-- openai::responses_route
//! |-- str::to_owned
//! `-- url::Url::parse
//! Used by:
//! rust/crates/
//! `-- provider-adapters/src/providers/mod.rs
//! Invariants: Azure adds `api-version` only for `*.openai.azure.com` or deployment URLs;
//! Cloudflare upstream bearer is sent only for Workers AI model ids.
//! Contract tests: provider-adapters/tests/providers_registry.rs

use std::sync::Arc;

use opencode_llm_api::{LlmError, ModelId, ProviderId};

use crate::auth::{AndThen, CredentialHeader, Headers, StaticHeaders};
use crate::protocols::openai;
use crate::providers::azure::{CopilotEndpoint, should_use_responses_api};
use crate::providers::config::{ProviderConfig, credential};
use crate::providers::gateways::{self, CloudflareConfig};
use crate::providers::inventory::CatalogRoute;
use crate::providers::loaders;
use crate::providers::registry::ProviderRegistry;
use crate::route::ProtocolRoute;
use crate::unsupported::not_yet_ported;

impl ProviderRegistry {
    /// `@ai-sdk/openai-compatible`, incl. Workers AI, Vertex OpenAI and Cognitive Services.
    pub(super) fn compatible(&self, route: &CatalogRoute) -> Result<ProtocolRoute, LlmError> {
        if route.provider_id == gateways::WORKERS_AI_ID {
            return self.workers_ai(route);
        }
        if let Some(base) = loaders::azure_cognitive_base_url(route) {
            return Ok(self.chat(route, &base, &[]));
        }
        let Some(url) = route.url() else {
            return Err(not_yet_ported(
                &ProviderId::new(route.provider_id.as_str()),
                &ModelId::new(route.model_id.as_str()),
                "openai-compatible without api url",
            ));
        };
        if route.provider_id == "google-vertex" {
            let project = loaders::vertex_project(route);
            let location = loaders::vertex_location(route);
            let base = loaders::replace_vertex_vars(url, project.as_deref(), &location);
            let mut r = self.chat(route, &base, &[]);
            r.auth = route.auth.clone().ok_or_else(|| {
                not_yet_ported(
                    &ProviderId::new(route.provider_id.as_str()),
                    &ModelId::new(route.model_id.as_str()),
                    "google-vertex ADC access token",
                )
            })?;
            return Ok(r);
        }
        Ok(self.chat(route, url, &[]))
    }

    fn workers_ai(&self, route: &CatalogRoute) -> Result<ProtocolRoute, LlmError> {
        let account = loaders::cloudflare_account(route);
        let base = match (route.url(), account.as_deref()) {
            (Some(url), _) => loaders::expand_account_id(url, route),
            (None, Some(account)) => gateways::workers_ai_base_url(None, Some(account))?,
            (None, None) => {
                return Err(LlmError::invalid_request(
                    loaders::CLOUDFLARE_ACCOUNT_MISSING,
                ));
            }
        };
        let mut r = self.chat(route, &base, &[]);
        let key = route
            .env_value("CLOUDFLARE_API_KEY")
            .or_else(|| route.api_key.clone());
        r.auth = Arc::new(CredentialHeader::bearer(credential(
            key.as_deref(),
            &["CLOUDFLARE_WORKERS_AI_TOKEN"],
            &route.env,
        )));
        r.headers = Some(self.static_headers("User-Agent", &self.ua("cloudflare-workers-ai")));
        Ok(r)
    }

    fn static_headers(&self, name: &str, value: &str) -> crate::route::HeadersFn {
        let headers = Headers::from([(name.to_owned(), value.to_owned())]);
        Arc::new(move |_| headers.clone())
    }

    pub(super) fn cloudflare_gateway(
        &self,
        route: &CatalogRoute,
    ) -> Result<ProtocolRoute, LlmError> {
        let account = loaders::cloudflare_account(route);
        let gateway = loaders::cloudflare_gateway(route);
        if route.url().is_none() && (account.is_none() || gateway.is_none()) {
            return Err(loaders::cloudflare_gateway_missing(
                account.is_some(),
                gateway.is_some(),
            ));
        }
        let token = route
            .first_env(gateways::AI_GATEWAY_ENV)
            .or_else(|| route.api_key.clone())
            .ok_or_else(|| LlmError::invalid_request(loaders::CLOUDFLARE_TOKEN_MISSING))?;
        let cf = CloudflareConfig {
            account_id: account,
            gateway_id: gateway,
            gateway_api_key: Some(token.clone()),
        };
        let mut r = gateways::ai_gateway(
            &ProviderConfig {
                base_url: route.url().map(str::to_owned),
                env: route.env.clone(),
                ..ProviderConfig::default()
            },
            &cf,
            &self.transports,
        )?
        .route;
        if gateways::is_workers_ai_model(&route.model_id) {
            r.auth = Arc::new(AndThen(r.auth, crate::auth::bearer_value(token)));
        }
        let mut headers = Headers::new();
        for (option, header) in [
            ("cacheTtl", "cf-aig-cache-ttl"),
            ("cacheKey", "cf-aig-cache-key"),
            ("collectLog", "cf-aig-collect-log"),
        ] {
            if let Some(value) = route.options.get(option).filter(|v| !v.is_null()) {
                headers.insert(header.into(), crate::shared::js_string(value));
            }
        }
        if route.options.get("skipCache").and_then(|v| v.as_bool()) == Some(true) {
            headers.insert("cf-aig-skip-cache".into(), "true".into());
        }
        if let Some(metadata) = route.options.get("metadata").filter(|v| !v.is_null()) {
            headers.insert("cf-aig-metadata".into(), metadata.to_string());
        }
        headers.insert("User-Agent".into(), self.ua("cloudflare-ai-gateway"));
        r.auth = Arc::new(AndThen(Arc::new(StaticHeaders(headers)), r.auth));
        Ok(r)
    }

    pub(super) fn azure(&self, route: &CatalogRoute) -> Result<ProtocolRoute, LlmError> {
        let resource = loaders::azure_resource_name(route);
        let deployments = route
            .options
            .get("useDeploymentBasedUrls")
            .and_then(|v| v.as_bool())
            == Some(true);
        let base = match (route.url().or(route.option("baseURL")), resource) {
            (Some(url), _) => url.trim_end_matches('/').to_owned(),
            (None, Some(resource)) => format!("https://{resource}.openai.azure.com/openai"),
            (None, None) => return Err(LlmError::invalid_request(loaders::AZURE_RESOURCE_MISSING)),
        };
        let azure_host = url::Url::parse(&base)
            .ok()
            .and_then(|u| u.host_str().map(|h| h.ends_with(".openai.azure.com")))
            .unwrap_or(false);
        let use_chat = route
            .options
            .get("useCompletionUrls")
            .and_then(|v| v.as_bool())
            == Some(true);
        let mut r = if use_chat {
            openai::chat_route(self.transports.sse.clone())
        } else {
            openai::responses_route(self.transports.sse.clone())
        };
        r.endpoint.base_url = Some(match (deployments, azure_host) {
            (true, _) => format!("{base}/deployments/{}", route.model_id),
            (false, true) => format!("{base}/v1"),
            (false, false) => base,
        });
        if deployments || azure_host {
            let version = route.option("apiVersion").unwrap_or("v1");
            r.endpoint
                .query
                .insert("api-version".into(), version.to_owned());
        }
        let config = self.config(route);
        r.auth = match &config.auth {
            Some(auth) => auth.clone(),
            None => Arc::new(CredentialHeader::header(
                "api-key",
                config.credential(&Self::env_names(route, &["AZURE_API_KEY"])),
            )),
        };
        Ok(r)
    }

    pub(super) fn copilot(&self, route: &CatalogRoute) -> ProtocolRoute {
        let endpoint = match route.option("endpoint") {
            Some("responses") => Some(CopilotEndpoint::Responses),
            Some("chat") => Some(CopilotEndpoint::Chat),
            _ => None,
        };
        let mut r = if should_use_responses_api(&route.model_id, endpoint) {
            openai::responses_route(self.transports.sse.clone())
        } else {
            openai::chat_route(self.transports.sse.clone())
        };
        r.endpoint.base_url = Some(
            route
                .url()
                .unwrap_or("https://api.githubcopilot.com")
                .to_owned(),
        );
        r.auth = self.bearer(route, &[]);
        r
    }
}
