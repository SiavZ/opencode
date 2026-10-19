//! Purpose: shared provider facade configuration (packages/llm/src/route/auth-options.ts,
//! `RouteDefaultsInput`, `mergeRouteDefaults`, `Endpoint.merge`): API key / auth override
//! resolution against an injectable environment, route default merging and transport set.
//! Scope: pure configuration helpers used by every provider facade module.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- llm-api/src/options.rs
//! |-- provider-adapters/src/auth.rs
//! |-- provider-adapters/src/protocols/bedrock/mod.rs
//! |-- provider-adapters/src/route.rs
//! `-- provider-adapters/src/transport/mod.rs
//! External:
//! |-- fmt::Debug
//! |-- fmt::Formatter
//! |-- fmt::Result
//! `-- futures::future::BoxFuture
//! Used by:
//! rust/crates/
//! |-- provider-adapters/src/providers/azure.rs
//! |-- provider-adapters/src/providers/bedrock.rs
//! |-- provider-adapters/src/providers/cloud.rs
//! |-- provider-adapters/src/providers/compatible.rs
//! |-- provider-adapters/src/providers/gateways.rs
//! |-- provider-adapters/src/providers/inventory.rs
//! |-- provider-adapters/src/providers/mod.rs
//! |-- provider-adapters/src/providers/native.rs
//! |-- provider-adapters/src/providers/openai.rs
//! |-- provider-adapters/src/providers/registry.rs
//! |-- provider-adapters/src/providers/registry_rules.rs
//! `-- provider-adapters/tests/providers_support/mod.rs
//! Invariants: env values are read at request time (`Auth.config`), never at configure time.
//! Missing credentials fail with `Missing auth credential: <last source>`.
//! Contract tests: provider-adapters/tests/providers_facades.rs

use std::fmt;
use std::sync::Arc;

use futures::future::BoxFuture;
use opencode_llm_api::options::{
    merge_generation_options, merge_http_options, merge_provider_options,
};
use opencode_llm_api::{HttpOptions, LlmError, RouteDefaults};

use crate::auth::{
    CredentialHeader, CredentialSource, FirstCredential, Headers, Secret, SharedAuth,
    StaticCredential,
};
use crate::route::{ProtocolRoute, Transport};
use crate::transport::{Framing, HttpExecutor, HttpJsonTransport};

/// Environment lookup (`Config` provider in the baseline). Injectable so tests never touch
/// the process environment.
pub type Env = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// The real process environment.
pub fn process_env() -> Env {
    Arc::new(|name| std::env::var(name).ok())
}

/// A fixed environment, for tests and explicit configuration.
pub fn fixed_env(pairs: &[(&str, &str)]) -> Env {
    let map: Headers = pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    Arc::new(move |name| map.get(name).cloned())
}

/// `Auth.config(name)` over an injectable env, read at request time.
pub struct EnvLookup {
    pub name: String,
    pub env: Env,
}

impl fmt::Debug for EnvLookup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "EnvLookup({})", self.name)
    }
}

impl CredentialSource for EnvLookup {
    fn source(&self) -> String {
        self.name.clone()
    }

    fn load(&self) -> BoxFuture<'_, Result<Option<Secret>, LlmError>> {
        Box::pin(async move { Ok((self.env)(&self.name).map(Secret::new)) })
    }
}

/// `ProviderAuthOption<"optional">` plus `RouteDefaultsInput` plus `baseURL`/`queryParams`.
#[derive(Clone)]
pub struct ProviderConfig {
    pub api_key: Option<String>,
    /// Explicit `auth` override; wins over `api_key` and env.
    pub auth: Option<SharedAuth>,
    pub base_url: Option<String>,
    pub query: Headers,
    /// `headers`, `limits`, `generation`, `providerOptions`, `http`.
    pub defaults: RouteDefaults,
    pub env: Env,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            api_key: None,
            auth: None,
            base_url: None,
            query: Headers::new(),
            defaults: RouteDefaults::default(),
            env: process_env(),
        }
    }
}

impl fmt::Debug for ProviderConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderConfig")
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("base_url", &self.base_url)
            .field("query", &self.query)
            .field("defaults", &self.defaults)
            .finish_non_exhaustive()
    }
}

impl ProviderConfig {
    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = Some(url.into());
        self
    }

    pub fn with_env(mut self, env: Env) -> Self {
        self.env = env;
        self
    }

    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.defaults
            .headers
            .get_or_insert_with(Default::default)
            .insert(name.to_owned(), value.to_owned());
        self
    }

    /// `Auth.optional(apiKey, "apiKey").orElse(Auth.config(env0)).orElse(...)` credential.
    pub fn credential(&self, env_vars: &[&str]) -> Arc<dyn CredentialSource> {
        credential(self.api_key.as_deref(), env_vars, &self.env)
    }

    /// `AuthOptions.bearer(options, envVars)`.
    pub fn bearer(&self, env_vars: &[&str]) -> SharedAuth {
        if let Some(auth) = &self.auth {
            return auth.clone();
        }
        Arc::new(CredentialHeader::bearer(self.credential(env_vars)))
    }

    /// `Auth.optional(apiKey).orElse(Auth.config(env)).pipe(Auth.header(name))`.
    pub fn header_auth(&self, name: &str, env_vars: &[&str]) -> SharedAuth {
        if let Some(auth) = &self.auth {
            return auth.clone();
        }
        Arc::new(CredentialHeader::header(name, self.credential(env_vars)))
    }
}

/// Credential chain: optional literal, then env names in order. The missing-credential
/// source is the last link (`orElse` reports the final failure).
pub fn credential(value: Option<&str>, env_vars: &[&str], env: &Env) -> Arc<dyn CredentialSource> {
    let mut sources: Vec<Arc<dyn CredentialSource>> = vec![Arc::new(StaticCredential {
        source: "apiKey".into(),
        value: value.map(Secret::new),
    })];
    sources.extend(env_vars.iter().map(|name| {
        Arc::new(EnvLookup {
            name: (*name).to_owned(),
            env: env.clone(),
        }) as Arc<dyn CredentialSource>
    }));
    Arc::new(FirstCredential(sources))
}

/// Static literal secret credential under `source`.
pub fn literal(source: &str, value: &str) -> Arc<dyn CredentialSource> {
    Arc::new(StaticCredential {
        source: source.into(),
        value: Some(Secret::new(value)),
    })
}

/// `mergeRouteDefaults(route.defaults, patch)`: headers also flow into `http.headers`.
pub fn apply_defaults(route: &mut ProtocolRoute, patch: &RouteDefaults) {
    let base = &route.defaults;
    let headers = merge_headers(base.headers.as_ref(), patch.headers.as_ref());
    let header_http = headers.as_ref().map(|h| HttpOptions {
        headers: Some(h.clone()),
        ..HttpOptions::default()
    });
    route.defaults = RouteDefaults {
        limits: patch.limits.clone().or_else(|| base.limits.clone()),
        generation: merge_generation_options([base.generation.as_ref(), patch.generation.as_ref()]),
        provider_options: merge_provider_options([
            base.provider_options.as_ref(),
            patch.provider_options.as_ref(),
        ]),
        http: merge_http_options([
            base.http.as_ref(),
            patch.http.as_ref(),
            header_http.as_ref(),
        ]),
        headers,
    };
}

fn merge_headers(a: Option<&Headers>, b: Option<&Headers>) -> Option<Headers> {
    let merged: Headers = a
        .into_iter()
        .chain(b)
        .flat_map(|h| h.iter().map(|(k, v)| (k.clone(), v.clone())))
        .collect();
    (!merged.is_empty()).then_some(merged)
}

/// `Endpoint.merge`: `baseURL` patch wins when set, query maps merge.
pub fn patch_endpoint(route: &mut ProtocolRoute, base_url: Option<&str>, query: &Headers) {
    if let Some(url) = base_url {
        route.endpoint.base_url = Some(url.to_owned());
    }
    for (k, v) in query {
        route.endpoint.query.insert(k.clone(), v.clone());
    }
}

/// `Route.model` precondition: the endpoint needs a base URL.
pub fn require_base_url(route: &ProtocolRoute) -> Result<(), LlmError> {
    if route.endpoint.base_url.is_some() {
        return Ok(());
    }
    Err(LlmError::invalid_request(format!(
        "Route.model({}) requires an endpoint baseURL — configure it on the route first",
        route.id
    )))
}

/// The transports provider routes run over (`HttpTransport.sseJson`, AWS event-stream).
#[derive(Clone, Debug)]
pub struct Transports {
    pub sse: Arc<dyn Transport>,
    pub event_stream: Arc<dyn Transport>,
}

impl Transports {
    pub fn new(executor: HttpExecutor) -> Self {
        Self {
            sse: Arc::new(HttpJsonTransport::sse(executor.clone())),
            event_stream: Arc::new(HttpJsonTransport::new(
                executor,
                Framing::aws_event_stream(crate::protocols::bedrock::ID),
            )),
        }
    }
}
