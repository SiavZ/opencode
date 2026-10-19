//! Purpose: `SessionRunnerModel.locationLayer` selection for `runner::model::LocationModels`:
//! explicit model without fallback (location-layer.test.ts), default/available fallback,
//! provider integration credential lookup and authorization failure.
//! Scope: fixture-based (no network). Catalog and integrations are in-memory port fixtures.
//! Uses:
//! rust/crates/
//! |-- schema/src/ids.rs
//! |-- schema/src/provider.rs
//! |-- session-runtime/src/runner/model.rs
//! `-- session-runtime/tests/support/model.rs
//! External:
//! |-- async_trait::async_trait
//! |-- serde_json::Value
//! |-- serde_json::from_value
//! |-- serde_json::json
//! `-- tokio::test
//! Used by:
//! `-- none, entrypoint
//! Invariants: catalog order is preserved, integration id falls back to provider id.
//! Contract tests: this file

#[path = "support/model.rs"]
mod support;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use opencode_schema::ids::{IntegrationID, ProviderID};
use opencode_schema::provider::{ModelInfo, ProviderInfo};
use opencode_session_runtime::runner::model::{
    CredentialValue, IntegrationCredentials, LocationModels, ModelCatalog, NativeRoute, RouteAuth,
    SessionRunnerModel,
};
use serde_json::{Value, json};
use support::*;

struct Catalog {
    default: Option<ModelInfo>,
    available: Vec<ModelInfo>,
    providers: Vec<ProviderInfo>,
}

#[async_trait]
impl ModelCatalog for Catalog {
    async fn default_model(&self) -> Option<ModelInfo> {
        self.default.clone()
    }
    async fn available_models(&self) -> Vec<ModelInfo> {
        self.available.clone()
    }
    async fn provider(&self, id: &ProviderID) -> Option<ProviderInfo> {
        self.providers.iter().find(|p| &p.id == id).cloned()
    }
}

#[derive(Default)]
struct Integrations {
    seen: Mutex<Vec<IntegrationID>>,
    result: Option<Result<Option<CredentialValue>, String>>,
}

#[async_trait]
impl IntegrationCredentials for Integrations {
    async fn active_credential(
        &self,
        id: &IntegrationID,
    ) -> Result<Option<CredentialValue>, String> {
        self.seen.lock().unwrap().push(id.clone());
        self.result.clone().unwrap_or(Ok(None))
    }
}

fn named(provider: &str, id: &str, api: Value) -> ModelInfo {
    let mut m = model(api);
    m.provider_id = ProviderID::new(provider);
    m.id = serde_json::from_value(json!(id)).unwrap();
    m
}

fn location(catalog: Catalog, integrations: Arc<Integrations>) -> (LocationModels, Arc<Routes>) {
    let routes = Arc::new(Routes::default());
    (
        LocationModels {
            catalog: Arc::new(catalog),
            integrations,
            routes: routes.clone(),
        },
        routes,
    )
}

#[tokio::test]
async fn explicit_unavailable_model_fails_without_fallback() {
    let (models, _) = location(
        Catalog {
            default: Some(named("test-provider", "test-model", openai())),
            available: vec![named("test-provider", "test-model", openai())],
            providers: vec![],
        },
        Arc::default(),
    );
    let failure = models
        .resolve(&session(
            "ses_unavailable_model",
            Some(json!({ "id": "chat", "providerID": "unavailable" })),
        ))
        .await
        .unwrap_err();
    assert_eq!(failure.tag(), "SessionRunnerModel.ModelUnavailableError");
    assert_eq!(failure.to_string(), "Model unavailable: unavailable/chat");
}

#[tokio::test]
async fn no_supported_model_reports_not_selected() {
    let google = json!({ "type": "aisdk", "package": "@ai-sdk/google" });
    let (models, _) = location(
        Catalog {
            default: Some(named("g", "m", google.clone())),
            available: vec![named("g", "m", google)],
            providers: vec![],
        },
        Arc::default(),
    );
    let failure = models
        .resolve(&session("ses_none", None))
        .await
        .unwrap_err();
    assert_eq!(failure.tag(), "SessionRunnerModel.ModelNotSelectedError");
    assert_eq!(
        failure.to_string(),
        "No model is available for session ses_none"
    );
}

#[tokio::test]
async fn unsupported_default_falls_back_to_first_supported_available_model() {
    let integrations = Arc::new(Integrations::default());
    let (models, routes) = location(
        Catalog {
            default: Some(named(
                "g",
                "gemini",
                json!({ "type": "aisdk", "package": "@ai-sdk/google" }),
            )),
            available: vec![
                named(
                    "g",
                    "gemini",
                    json!({ "type": "aisdk", "package": "@ai-sdk/google" }),
                ),
                named(
                    "anth",
                    "claude",
                    json!({ "type": "aisdk", "package": "@ai-sdk/anthropic" }),
                ),
                named("oai", "gpt", openai()),
            ],
            providers: vec![],
        },
        integrations.clone(),
    );
    let resolved = models.resolve(&session("ses_default", None)).await.unwrap();
    assert_eq!(resolved.provider.as_str(), "anth");
    assert_eq!(routes.last().route, NativeRoute::AnthropicMessages);
    // No provider record: the integration id falls back to the provider id.
    assert_eq!(
        integrations.seen.lock().unwrap().clone(),
        vec![IntegrationID::new("anth")]
    );
}

#[tokio::test]
async fn provider_integration_credential_is_used_and_authorization_errors_surface() {
    let mut provider: ProviderInfo = serde_json::from_value(json!({
        "id": "test-provider",
        "integrationID": "shared-integration",
        "name": "Test",
        "api": { "type": "native", "settings": {} },
        "request": { "headers": {}, "body": {} },
    }))
    .unwrap();
    provider.disabled = None;
    let catalog = || Catalog {
        default: Some(model(openai())),
        available: vec![model(openai())],
        providers: vec![provider.clone()],
    };
    let integrations = Arc::new(Integrations {
        seen: Mutex::default(),
        result: Some(Ok(Some(CredentialValue::Key {
            key: "stored".into(),
            metadata: None,
        }))),
    });
    let (models, routes) = location(catalog(), integrations.clone());
    models.resolve(&session("ses_cred", None)).await.unwrap();
    assert_eq!(routes.last().auth, RouteAuth::Bearer("stored".into()));
    assert_eq!(
        integrations.seen.lock().unwrap().clone(),
        vec![IntegrationID::new("shared-integration")]
    );

    let denied = Arc::new(Integrations {
        seen: Mutex::default(),
        result: Some(Err("refresh failed".into())),
    });
    let (models, _) = location(catalog(), denied);
    let failure = models
        .resolve(&session("ses_denied", None))
        .await
        .unwrap_err();
    assert_eq!(failure.tag(), "Integration.Authorization");
}
