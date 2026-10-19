//! Purpose: `SessionRunnerModel` (packages/core/src/session/runner/model.ts): resolve the
//! Session's selected or default catalog model, overlay its variant and credential, and bind
//! it to a native `opencode_llm_api::Route`. Typed errors mirror the baseline tags/messages.
//! Scope: pure resolution logic plus narrow ports (catalog, credentials, route factory).
//! Concrete routes are built by the composition root via `RouteFactory` (provider-adapters
//! `protocols::{openai, anthropic}` routes) because session-runtime may not depend on it.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- schema/src/ids.rs
//! |-- schema/src/provider.rs
//! |-- schema/src/session.rs
//! |-- schema/src/values.rs
//! `-- session-api/src/lib.rs
//! External:
//! |-- async_trait::async_trait
//! |-- fmt::Debug
//! |-- fmt::Formatter
//! |-- fmt::Result
//! |-- indexmap::IndexMap
//! |-- serde::Deserialize
//! |-- serde::Serialize
//! |-- serde_json::Value
//! |-- str::to_owned
//! `-- u64::try_from
//! Used by:
//! rust/crates/
//! |-- runtime/src/location.rs
//! |-- runtime/src/location_models.rs
//! |-- runtime/src/location_routes.rs
//! |-- session-runtime/src/runner/mod.rs
//! |-- session-runtime/src/runner/turn.rs
//! |-- session-runtime/tests/runner_model.rs
//! |-- session-runtime/tests/runner_model_location.rs
//! |-- session-runtime/tests/support/model.rs
//! `-- session-runtime/tests/support/provider.rs
//! Invariants: an explicit Session model never falls back to another model, an explicit
//! unknown variant fails (missing or `default` uses `request.variant`), stored credentials win
//! over configured `apiKey`, `apiKey` never reaches the provider body, only key credential
//! metadata is merged into the body (OAuth metadata is not).
//! Contract tests: session-runtime/tests/runner_model.rs, session-runtime/tests/runner_model_location.rs

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use indexmap::IndexMap;
use opencode_llm_api::{JsonObject, Model, ModelLimits, ProviderId, Route};
use opencode_schema::ids::{IntegrationID, IntegrationMethodID, ModelID, ProviderID, VariantID};
use opencode_schema::provider::{ModelInfo, ProviderApi, ProviderInfo};
use opencode_schema::session::Info as SessionInfo;
use opencode_schema::values::NonNegativeInt;
use opencode_session_api::SessionError;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// `SessionRunnerModel.Error`.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum ModelError {
    #[error("No model is available for session {session_id}")]
    ModelNotSelected {
        session_id: opencode_schema::ids::SessionID,
    },
    #[error("Model unavailable: {provider_id}/{model_id}")]
    ModelUnavailable {
        provider_id: ProviderID,
        model_id: ModelID,
    },
    #[error("Variant unavailable for {provider_id}/{model_id}: {variant}")]
    VariantUnavailable {
        provider_id: ProviderID,
        model_id: ModelID,
        variant: VariantID,
    },
    #[error("Unsupported API for {provider_id}/{model_id}: {api}")]
    UnsupportedApi {
        provider_id: ProviderID,
        model_id: ModelID,
        api: String,
    },
    /// `Integration.AuthorizationError`: resolving the active connection failed.
    #[error("{0}")]
    Authorization(String),
}

impl ModelError {
    /// Baseline `_tag`.
    pub fn tag(&self) -> &'static str {
        match self {
            Self::ModelNotSelected { .. } => "SessionRunnerModel.ModelNotSelectedError",
            Self::ModelUnavailable { .. } => "SessionRunnerModel.ModelUnavailableError",
            Self::VariantUnavailable { .. } => "SessionRunnerModel.VariantUnavailableError",
            Self::UnsupportedApi { .. } => "SessionRunnerModel.UnsupportedApiError",
            Self::Authorization(_) => "Integration.Authorization",
        }
    }
}

impl From<ModelError> for SessionError {
    fn from(error: ModelError) -> Self {
        SessionError::Run {
            message: error.to_string(),
            retryable: false,
        }
    }
}

/// `Credential.Value` (`@opencode-ai/schema/credential`), tagged by `type`.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum CredentialValue {
    #[serde(rename = "oauth")]
    OAuth {
        #[serde(rename = "methodID")]
        method_id: IntegrationMethodID,
        refresh: String,
        access: String,
        expires: NonNegativeInt,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata: Option<JsonObject>,
    },
    #[serde(rename = "key")]
    Key {
        key: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata: Option<JsonObject>,
    },
}

impl fmt::Debug for CredentialValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OAuth { method_id, .. } => f
                .debug_struct("OAuth")
                .field("method_id", method_id)
                .finish_non_exhaustive(),
            Self::Key { .. } => f.debug_struct("Key").finish_non_exhaustive(),
        }
    }
}

/// Native route families the baseline maps catalog AI SDK packages onto.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeRoute {
    /// `@ai-sdk/openai` -> `OpenAIResponses.route`.
    OpenAIResponses,
    /// `@ai-sdk/anthropic` -> `AnthropicMessages.route`.
    AnthropicMessages,
    /// `@ai-sdk/openai-compatible` with a URL -> `OpenAICompatibleChat.route`.
    OpenAICompatibleChat,
}

impl NativeRoute {
    /// Route id of the baseline route value.
    pub fn id(self) -> &'static str {
        match self {
            Self::OpenAIResponses => "openai-responses",
            Self::AnthropicMessages => "anthropic-messages",
            Self::OpenAICompatibleChat => "openai-compatible-chat",
        }
    }
}

/// Route auth chosen by the resolver (`Auth.none`, `Auth.bearer`, `Auth.header`).
#[derive(Clone, PartialEq, Eq)]
pub enum RouteAuth {
    None,
    Bearer(String),
    Header { name: String, value: String },
}

impl fmt::Debug for RouteAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => f.write_str("None"),
            Self::Bearer(_) => f.write_str("Bearer(<redacted>)"),
            Self::Header { name, .. } => write!(f, "Header({name}: <redacted>)"),
        }
    }
}

/// The `route.with({...})` patch the baseline applies before `.model({ id })`.
#[derive(Clone, Debug, PartialEq)]
pub struct RouteSpec {
    pub route: NativeRoute,
    pub provider: ProviderId,
    /// `endpoint.baseURL`, `None` keeps the route's default endpoint.
    pub base_url: Option<String>,
    /// Merged into route default headers.
    pub headers: IndexMap<String, String>,
    /// `http.body` default overlay, never contains `apiKey`.
    pub body: JsonObject,
    pub limits: ModelLimits,
    pub auth: RouteAuth,
}

/// Builds the runnable route for a spec. Implemented in the composition root over
/// provider-adapters' native routes and the shared transport.
pub trait RouteFactory: Send + Sync {
    fn route(&self, spec: RouteSpec) -> Arc<dyn Route>;
}

/// `Catalog.Service` subset used by model resolution.
#[async_trait]
pub trait ModelCatalog: Send + Sync {
    /// `catalog.model.default()`.
    async fn default_model(&self) -> Option<ModelInfo>;
    /// `catalog.model.available()`, in catalog order.
    async fn available_models(&self) -> Vec<ModelInfo>;
    /// `catalog.provider.get(id)`.
    async fn provider(&self, id: &ProviderID) -> Option<ProviderInfo>;
}

/// `Integration.Service.connection.active` followed by `connection.resolve`.
#[async_trait]
pub trait IntegrationCredentials: Send + Sync {
    /// `Ok(None)` when no connection is active or it carries no credential.
    /// `Err` is `Integration.AuthorizationError` (message only).
    async fn active_credential(
        &self,
        id: &IntegrationID,
    ) -> Result<Option<CredentialValue>, String>;
}

/// `SessionRunnerModel.Interface`.
#[async_trait]
pub trait SessionRunnerModel: Send + Sync {
    async fn resolve(&self, session: &SessionInfo) -> Result<Model, ModelError>;
}

/// `SessionRunnerModel.locationLayer`: resolves from the Location catalog.
pub struct LocationModels {
    pub catalog: Arc<dyn ModelCatalog>,
    pub integrations: Arc<dyn IntegrationCredentials>,
    pub routes: Arc<dyn RouteFactory>,
}

#[async_trait]
impl SessionRunnerModel for LocationModels {
    async fn resolve(&self, session: &SessionInfo) -> Result<Model, ModelError> {
        let selected = match &session.model {
            Some(requested) => self
                .catalog
                .available_models()
                .await
                .into_iter()
                .find(|m| m.provider_id == requested.provider_id && m.id == requested.id)
                .ok_or_else(|| ModelError::ModelUnavailable {
                    provider_id: requested.provider_id.clone(),
                    model_id: requested.id.clone(),
                })?,
            None => match self.catalog.default_model().await.filter(supported) {
                Some(model) => model,
                None => self
                    .catalog
                    .available_models()
                    .await
                    .into_iter()
                    .find(supported)
                    .ok_or_else(|| ModelError::ModelNotSelected {
                        session_id: session.id.clone(),
                    })?,
            },
        };
        let integration = self
            .catalog
            .provider(&selected.provider_id)
            .await
            .and_then(|p| p.integration_id)
            .unwrap_or_else(|| IntegrationID::new(selected.provider_id.as_str()));
        let credential = self
            .integrations
            .active_credential(&integration)
            .await
            .map_err(ModelError::Authorization)?;
        resolve(session, selected, credential.as_ref(), self.routes.as_ref())
    }
}

/// `SessionRunnerModel.resolve(session, model, credential)`: variant overlay then catalog mapping.
pub fn resolve(
    session: &SessionInfo,
    model: ModelInfo,
    credential: Option<&CredentialValue>,
    routes: &dyn RouteFactory,
) -> Result<Model, ModelError> {
    let variant = session.model.as_ref().and_then(|m| m.variant.as_ref());
    from_catalog_model(with_variant(model, variant)?, credential, routes)
}

/// `SessionRunnerModel.fromCatalogModel`.
pub fn from_catalog_model(
    mut model: ModelInfo,
    credential: Option<&CredentialValue>,
    routes: &dyn RouteFactory,
) -> Result<Model, ModelError> {
    if let Some(CredentialValue::Key {
        metadata: Some(metadata),
        ..
    }) = credential
    {
        model
            .request
            .body
            .extend(metadata.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
    let key = api_key(&model, credential);
    let Some(route) = native_route(&model) else {
        return Err(ModelError::UnsupportedApi {
            provider_id: model.provider_id.clone(),
            model_id: model.id.clone(),
            api: api_name(&model.api.api),
        });
    };
    let auth = match (key, route) {
        (None, _) => RouteAuth::None,
        (Some(value), NativeRoute::AnthropicMessages) => RouteAuth::Header {
            name: "x-api-key".into(),
            value,
        },
        (Some(value), _) => RouteAuth::Bearer(value),
    };
    let provider = ProviderId::new(model.provider_id.as_str());
    let mut body = model.request.body;
    body.shift_remove("apiKey");
    let spec = RouteSpec {
        route,
        provider: provider.clone(),
        base_url: api_url(&model.api.api).map(str::to_owned),
        headers: model.request.headers,
        body,
        limits: ModelLimits {
            context: u64::try_from(model.limit.context.get()).ok(),
            output: u64::try_from(model.limit.output.get()).ok(),
        },
        auth,
    };
    Ok(Model::new(
        model.api.id.as_str(),
        provider,
        routes.route(spec),
    ))
}

/// `SessionRunnerModel.supported`: whether the catalog model has a native route.
pub fn supported(model: &ModelInfo) -> bool {
    native_route(model).is_some()
}

fn native_route(model: &ModelInfo) -> Option<NativeRoute> {
    let ProviderApi::Aisdk { package, url, .. } = &model.api.api else {
        return None;
    };
    match package.as_str() {
        "@ai-sdk/openai" => Some(NativeRoute::OpenAIResponses),
        "@ai-sdk/anthropic" => Some(NativeRoute::AnthropicMessages),
        "@ai-sdk/openai-compatible" if url_present(url.as_deref()) => {
            Some(NativeRoute::OpenAICompatibleChat)
        }
        _ => None,
    }
}

/// `resolved.api.url` truthiness: an empty string does not select the compatible route.
fn url_present(url: Option<&str>) -> bool {
    url.is_some_and(|u| !u.is_empty())
}

fn api_url(api: &ProviderApi) -> Option<&str> {
    match api {
        ProviderApi::Aisdk { url, .. } | ProviderApi::Native { url, .. } => url.as_deref(),
    }
}

fn api_name(api: &ProviderApi) -> String {
    match api {
        ProviderApi::Aisdk { package, .. } => format!("aisdk:{package}"),
        ProviderApi::Native { .. } => "native".into(),
    }
}

/// `apiKey`: stored credential, else `request.body.apiKey ?? api.settings?.apiKey` if a string.
fn api_key(model: &ModelInfo, credential: Option<&CredentialValue>) -> Option<String> {
    match credential {
        Some(CredentialValue::Key { key, .. }) => return Some(key.clone()),
        Some(CredentialValue::OAuth { access, .. }) => return Some(access.clone()),
        None => {}
    }
    let settings = match &model.api.api {
        ProviderApi::Aisdk { settings, .. } => settings.as_ref(),
        ProviderApi::Native { settings, .. } => Some(settings),
    };
    // `??` only skips null/undefined, a non-string body value hides settings.
    let value = model
        .request
        .body
        .get("apiKey")
        .filter(|v| !v.is_null())
        .or_else(|| settings.and_then(|s| s.get("apiKey")));
    match value {
        Some(Value::String(s)) => Some(s.clone()),
        _ => None,
    }
}

/// `withVariant`: shallow-assign the selected variant's headers and body.
fn with_variant(
    mut model: ModelInfo,
    requested: Option<&VariantID>,
) -> Result<ModelInfo, ModelError> {
    let explicit = requested.filter(|v| v.as_str() != "default");
    let id = match explicit {
        Some(v) => Some(v.as_str().to_owned()),
        None => model.request.variant.clone(),
    };
    let variant = id
        .as_deref()
        .and_then(|id| model.variants.iter().find(|v| v.id.as_str() == id))
        .cloned();
    let Some(variant) = variant else {
        if let Some(requested) = explicit {
            return Err(ModelError::VariantUnavailable {
                provider_id: model.provider_id.clone(),
                model_id: model.id.clone(),
                variant: requested.clone(),
            });
        }
        return Ok(model);
    };
    model.request.headers.extend(variant.headers);
    model.request.body.extend(variant.body);
    Ok(model)
}
