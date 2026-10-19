//! Purpose: port of packages/core/test/session-runner-model.test.ts (`fromCatalogModel`,
//! `resolve`, `supported`) for `runner::model`.
//! Scope: fixture-based (no network). Routes are captured through a recording `RouteFactory`,
//! so assertions target the `route.with(...)` patch the baseline applies.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- session-runtime/src/runner/model.rs
//! `-- session-runtime/tests/support/model.rs
//! External:
//! |-- serde_json::from_value
//! |-- serde_json::json
//! `-- serde_json::to_string
//! Used by:
//! `-- none, entrypoint
//! Invariants: fixtures are built from baseline-shaped JSON, never from production helpers.
//! Contract tests: this file

#[path = "support/model.rs"]
mod support;

use opencode_llm_api::ModelLimits;
use opencode_session_runtime::runner::model::{
    CredentialValue, ModelError, NativeRoute, RouteAuth, from_catalog_model, resolve, supported,
};
use serde_json::json;
use support::*;

#[test]
fn maps_catalog_openai_models_into_native_responses_routes() {
    let routes = Routes::default();
    let resolved = from_catalog_model(model(openai()), None, &routes).unwrap();
    assert_eq!(resolved.id.as_str(), "api-test-model");
    assert_eq!(resolved.provider.as_str(), "test-provider");
    assert_eq!(resolved.route.id(), "openai-responses");
    let spec = routes.last();
    assert_eq!(spec.route, NativeRoute::OpenAIResponses);
    assert_eq!(spec.provider.as_str(), "test-provider");
    assert_eq!(spec.base_url.as_deref(), Some("https://openai.example/v1"));
    assert_eq!(
        spec.headers.get("x-test").map(String::as_str),
        Some("header")
    );
    assert_eq!(
        spec.limits,
        ModelLimits {
            context: Some(100),
            output: Some(20)
        }
    );
    assert_eq!(
        body(&spec),
        json!({ "custom_extension": { "enabled": true } })
    );
    // Configured body apiKey becomes bearer auth.
    assert_eq!(spec.auth, RouteAuth::Bearer("secret".into()));
}

#[test]
fn keeps_catalog_api_key_credentials_out_of_provider_json() {
    let routes = Routes::default();
    from_catalog_model(model(openai()), None, &routes).unwrap();
    let text = serde_json::to_string(&body(&routes.last())).unwrap();
    assert!(!text.contains("apiKey"));
    assert!(!text.contains("secret"));
    assert!(!format!("{:?}", routes.last()).contains("secret"));
}

#[test]
fn uses_merged_api_settings_for_openai_compatible_auth_and_request_defaults() {
    let routes = Routes::default();
    from_catalog_model(
        model_with(
            json!({
                "type": "aisdk",
                "package": "@ai-sdk/openai-compatible",
                "url": "https://compatible.example/v1",
                "settings": { "apiKey": "settings-secret", "compatibility": "strict" },
            }),
            json!([]),
            json!({ "headers": {}, "body": {} }),
        ),
        None,
        &routes,
    )
    .unwrap();
    let spec = routes.last();
    assert_eq!(spec.route, NativeRoute::OpenAICompatibleChat);
    assert_eq!(spec.auth, RouteAuth::Bearer("settings-secret".into()));
    assert_eq!(body(&spec), json!({}));
}

#[test]
fn overlays_selected_openai_session_variant_bodies() {
    let routes = Routes::default();
    let catalog = model_with(
        openai(),
        json!([{
            "id": "high",
            "headers": { "x-variant": "high" },
            "body": { "store": false, "service_tier": "priority", "temperature": 0.2, "reasoning": { "effort": "high" } },
        }]),
        default_request(),
    );
    resolve(
        &variant_session("ses_model_variant", "high"),
        catalog,
        None,
        &routes,
    )
    .unwrap();
    let spec = routes.last();
    assert_eq!(
        spec.headers.get("x-test").map(String::as_str),
        Some("header")
    );
    assert_eq!(
        spec.headers.get("x-variant").map(String::as_str),
        Some("high")
    );
    assert_eq!(
        body(&spec),
        json!({
            "custom_extension": { "enabled": true },
            "store": false,
            "service_tier": "priority",
            "temperature": 0.2,
            "reasoning": { "effort": "high" },
        })
    );
}

#[test]
fn overlays_selected_openai_compatible_session_variant_bodies() {
    let routes = Routes::default();
    let catalog = model_with(
        json!({ "type": "aisdk", "package": "@ai-sdk/openai-compatible", "url": "https://compatible.example/v1" }),
        json!([{ "id": "high", "headers": {}, "body": { "store": false, "reasoning_effort": "high" } }]),
        default_request(),
    );
    resolve(
        &variant_session("ses_compatible_variant", "high"),
        catalog,
        None,
        &routes,
    )
    .unwrap();
    assert_eq!(
        body(&routes.last()),
        json!({ "custom_extension": { "enabled": true }, "store": false, "reasoning_effort": "high" })
    );
}

#[test]
fn rejects_an_explicit_unavailable_session_variant() {
    let routes = Routes::default();
    let failure = resolve(
        &variant_session("ses_model_variant_unavailable", "unknown"),
        model(openai()),
        None,
        &routes,
    )
    .unwrap_err();
    assert_eq!(failure.tag(), "SessionRunnerModel.VariantUnavailableError");
    let ModelError::VariantUnavailable {
        provider_id,
        model_id,
        variant,
    } = &failure
    else {
        panic!("unexpected {failure:?}");
    };
    assert_eq!(provider_id.as_str(), "test-provider");
    assert_eq!(model_id.as_str(), "test-model");
    assert_eq!(variant.as_str(), "unknown");
    assert_eq!(
        failure.to_string(),
        "Variant unavailable for test-provider/test-model: unknown"
    );
}

#[test]
fn default_variant_and_missing_variant_use_request_variant() {
    let routes = Routes::default();
    let mut request = default_request();
    request["variant"] = json!("low");
    let catalog = model_with(
        openai(),
        json!([{ "id": "low", "headers": {}, "body": { "effort": "low" } }]),
        request,
    );
    resolve(
        &variant_session("ses_default", "default"),
        catalog.clone(),
        None,
        &routes,
    )
    .unwrap();
    assert_eq!(body(&routes.last())["effort"], json!("low"));
    resolve(&session("ses_none", None), catalog, None, &routes).unwrap();
    assert_eq!(body(&routes.last())["effort"], json!("low"));
}

#[test]
fn overlays_selected_anthropic_session_variant_bodies() {
    let routes = Routes::default();
    let catalog = model_with(
        json!({ "type": "aisdk", "package": "@ai-sdk/anthropic", "url": "https://anthropic.example/v1" }),
        json!([{ "id": "high", "headers": {}, "body": { "thinking": { "type": "enabled", "budget_tokens": 12000 } } }]),
        default_request(),
    );
    resolve(
        &variant_session("ses_anthropic_variant", "high"),
        catalog,
        None,
        &routes,
    )
    .unwrap();
    assert_eq!(
        body(&routes.last()),
        json!({
            "custom_extension": { "enabled": true },
            "thinking": { "type": "enabled", "budget_tokens": 12000 },
        })
    );
}

#[test]
fn maps_catalog_anthropic_models_into_native_routes_with_x_api_key() {
    let routes = Routes::default();
    let resolved = from_catalog_model(
        model(json!({ "type": "aisdk", "package": "@ai-sdk/anthropic", "url": "https://anthropic.example/v1" })),
        None,
        &routes,
    )
    .unwrap();
    assert_eq!(resolved.route.id(), "anthropic-messages");
    let spec = routes.last();
    assert_eq!(
        spec.base_url.as_deref(),
        Some("https://anthropic.example/v1")
    );
    assert_eq!(
        spec.auth,
        RouteAuth::Header {
            name: "x-api-key".into(),
            value: "secret".into()
        }
    );
}

#[test]
fn uses_resolved_credentials_for_bearer_auth() {
    let routes = Routes::default();
    let credential = CredentialValue::Key {
        key: "secret".into(),
        metadata: None,
    };
    from_catalog_model(
        model_with(openai(), json!([]), json!({ "headers": {}, "body": {} })),
        Some(&credential),
        &routes,
    )
    .unwrap();
    assert_eq!(routes.last().auth, RouteAuth::Bearer("secret".into()));
}

#[test]
fn prefers_stored_credentials_over_configured_auth() {
    let routes = Routes::default();
    let credential: CredentialValue = serde_json::from_value(
        json!({ "type": "key", "key": "stored-secret", "metadata": { "tenant": "work" } }),
    )
    .unwrap();
    from_catalog_model(
        model_with(
            openai(),
            json!([]),
            json!({ "headers": {}, "body": { "apiKey": "configured-secret" } }),
        ),
        Some(&credential),
        &routes,
    )
    .unwrap();
    let spec = routes.last();
    assert_eq!(spec.auth, RouteAuth::Bearer("stored-secret".into()));
    assert_eq!(body(&spec), json!({ "tenant": "work" }));
}

#[test]
fn does_not_project_oauth_account_metadata_into_the_request_body() {
    let routes = Routes::default();
    let credential: CredentialValue = serde_json::from_value(json!({
        "type": "oauth",
        "methodID": "device",
        "access": "secret",
        "refresh": "refresh",
        "expires": 1_000_000,
        "metadata": { "server": "https://console.example", "orgID": "org_123" },
    }))
    .unwrap();
    from_catalog_model(
        model_with(openai(), json!([]), json!({ "headers": {}, "body": {} })),
        Some(&credential),
        &routes,
    )
    .unwrap();
    let spec = routes.last();
    assert_eq!(body(&spec), json!({}));
    assert_eq!(spec.auth, RouteAuth::Bearer("secret".into()));
    assert!(!format!("{credential:?}").contains("secret"));
}

#[test]
fn rejects_catalog_apis_without_a_native_route() {
    let routes = Routes::default();
    let failure = from_catalog_model(
        model(json!({ "type": "aisdk", "package": "@ai-sdk/google", "url": "https://google.example/v1" })),
        None,
        &routes,
    )
    .unwrap_err();
    assert_eq!(failure.tag(), "SessionRunnerModel.UnsupportedApiError");
    assert_eq!(
        failure.to_string(),
        "Unsupported API for test-provider/test-model: aisdk:@ai-sdk/google"
    );
    let native = from_catalog_model(
        model(json!({ "type": "native", "settings": {} })),
        None,
        &routes,
    )
    .unwrap_err();
    assert_eq!(
        native.to_string(),
        "Unsupported API for test-provider/test-model: native"
    );
}

#[test]
fn reports_whether_a_catalog_model_has_a_supported_native_route() {
    assert!(supported(&model(openai())));
    assert!(!supported(&model(
        json!({ "type": "aisdk", "package": "@ai-sdk/google", "url": "https://google.example/v1" })
    )));
    assert!(!supported(&model(
        json!({ "type": "native", "settings": {} })
    )));
    assert!(!supported(&model(
        json!({ "type": "aisdk", "package": "@ai-sdk/openai-compatible" })
    )));
}
