//! Purpose: expose public Zen and Go catalog, inference and usage routes.
//! Scope: native legacy inference lifecycle plus migrated inference streaming forwarding.
//! Uses:
//! rust/bins/
//! |-- hosted/src/console/mod.rs
//! `-- hosted/src/console/state.rs
//! External:
//! |-- axum::Router
//! `-- serde_json::Value
//! Used by:
//! `-- hosted/src/console/mod.rs
//! Invariants: provider credentials and full catalog metadata are never returned or logged.
//! Contract tests: hosted/src/console/zen.rs

#[cfg(test)]
use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use opencode_hosted_adapters::console::gateway::MigrationGateway;
use serde_json::{Value, json};

#[cfg(test)]
mod acceptance;
mod catalog;
mod completion;
use catalog::{models_response, options_handler};
mod legacy;
mod locale;
mod metrics;
mod proxy;
use super::ConsoleState;

#[derive(Clone, Default)]
pub struct Config {
    lite_model_ids: Option<Vec<String>>,
    full_model_ids: Option<Vec<String>>,
    catalog: Option<Value>,
    migration: Option<MigrationGateway>,
    limits: Option<Value>,
    tail: Option<super::workers::producer::TailEmitter>,
    redis: Option<opencode_hosted_adapters::console::limits::RedisCounters>,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("catalog_configured", &self.lite_model_ids.is_some())
            .finish()
    }
}

impl Config {
    pub(super) fn workspace_models(&self) -> Option<&serde_json::Map<String, Value>> {
        self.catalog.as_ref()?.get("zenModels")?.as_object()
    }

    #[cfg(test)]
    pub(crate) fn with_billing(
        mut self,
        limits: Value,
        redis: Option<opencode_hosted_adapters::console::limits::RedisCounters>,
    ) -> Self {
        self.limits = Some(limits);
        self.redis = redis;
        self
    }

    pub(crate) fn checkout_counters(
        &self,
    ) -> Option<&opencode_hosted_adapters::console::limits::RedisCounters> {
        self.redis.as_ref()
    }
    pub(crate) fn billing_lite_models(&self) -> Option<&serde_json::Map<String, Value>> {
        self.catalog.as_ref()?.get("liteModels")?.as_object()
    }

    pub(crate) fn billing_limits(&self) -> Option<&Value> {
        self.limits.as_ref()
    }

    pub fn from_env() -> anyhow::Result<Self> {
        let tail = std::env::var("OPENCODE_CONSOLE_TAIL_URL")
            .or_else(|_| std::env::var("OPENCODE_CONSOLE_LOG_INGRESS_URL"))
            .ok()
            .filter(|v| !v.is_empty())
            .map(|endpoint| super::workers::producer::TailEmitter::new(&endpoint))
            .transpose()?;
        let fragments = (1..=30)
            .map(|index| {
                std::env::var(format!("ZEN_MODELS{index}"))
                    .ok()
                    .filter(|value| !value.is_empty())
            })
            .collect::<Vec<_>>();
        let limits = std::env::var("ZEN_LIMITS")
            .ok()
            .filter(|v| !v.is_empty())
            .map(|value| serde_json::from_str::<Value>(&value))
            .transpose()?;
        let redis = std::env::var("UPSTASH_REDIS_REST_URL")
            .ok()
            .filter(|v| !v.is_empty())
            .map(|url| {
                opencode_hosted_adapters::console::limits::RedisCounters::new(
                    &url,
                    &std::env::var("UPSTASH_REDIS_REST_TOKEN").unwrap_or_default(),
                    &std::env::var("OPENCODE_STAGE").unwrap_or_else(|_| "dev".into()),
                )
            })
            .transpose()?;
        let migration = std::env::var("OPENCODE_CONSOLE_INFERENCE_URL")
            .ok()
            .filter(|v| !v.is_empty())
            .map(|url| {
                MigrationGateway::new(
                    &url,
                    &std::env::var("CLOUDFLARE_ACCESS_CLIENT_ID").unwrap_or_default(),
                )
            })
            .transpose()?;
        if fragments.iter().all(Option::is_none) {
            return Ok(Self {
                migration,
                tail,
                limits,
                redis,
                ..Self::default()
            });
        }
        anyhow::ensure!(
            fragments.iter().all(Option::is_some),
            "All ZEN_MODELS1 through ZEN_MODELS30 fragments are required"
        );
        let catalog = fragments.into_iter().flatten().collect::<String>();
        let catalog: Value = serde_json::from_str(&catalog)
            .map_err(|_| anyhow::anyhow!("ZEN_MODELS fragments are not valid JSON"))?;
        let mut config = Self::from_catalog(catalog)?;
        config.migration = migration;
        config.tail = tail;
        config.limits = limits;
        config.redis = redis;
        Ok(config)
    }

    pub(super) fn from_catalog(catalog: Value) -> anyhow::Result<Self> {
        let models = catalog
            .get("liteModels")
            .and_then(Value::as_object)
            .ok_or_else(|| anyhow::anyhow!("ZEN_MODELS catalog has no liteModels object"))?;
        let full_models = catalog
            .get("zenModels")
            .and_then(Value::as_object)
            .ok_or_else(|| anyhow::anyhow!("ZEN_MODELS catalog has no zenModels object"))?;
        Ok(Self {
            lite_model_ids: Some(
                models
                    .keys()
                    .filter(|id| !id.starts_with("alpha-"))
                    .cloned()
                    .collect(),
            ),
            full_model_ids: Some(
                full_models
                    .keys()
                    .filter(|id| !id.ends_with(":global"))
                    .filter(|id| !id.starts_with("alpha-"))
                    .cloned()
                    .collect(),
            ),
            catalog: Some(catalog),
            migration: None,
            tail: None,
            limits: None,
            redis: None,
        })
    }
}

pub fn routes() -> Router<ConsoleState> {
    Router::new()
        .route(
            "/zen/v1/chat/completions",
            axum::routing::post(proxy::generation).options(options_handler),
        )
        .route(
            "/zen/v1/responses",
            axum::routing::post(proxy::generation).options(options_handler),
        )
        .route(
            "/zen/v1/messages",
            axum::routing::post(proxy::generation).options(options_handler),
        )
        .route(
            "/zen/v1/systemone",
            axum::routing::post(proxy::generation).options(options_handler),
        )
        .route(
            "/zen/v1/models/{model}",
            axum::routing::post(proxy::generation).options(options_handler),
        )
        .route(
            "/zen/go/v1/chat/completions",
            axum::routing::post(proxy::generation).options(options_handler),
        )
        .route(
            "/zen/go/v1/responses",
            axum::routing::post(proxy::generation).options(options_handler),
        )
        .route(
            "/zen/go/v1/messages",
            axum::routing::post(proxy::generation).options(options_handler),
        )
        .route(
            "/zen/go/v1/systemone",
            axum::routing::post(proxy::generation).options(options_handler),
        )
        .route(
            "/zen/go/v1/usage",
            get(proxy::usage).options(options_handler),
        )
        .route("/zen/v1/models", get(full_models).options(options_handler))
        .route(
            "/zen/go/v1/models",
            get(lite_models).options(options_handler),
        )
}

async fn lite_models(
    State(state): State<ConsoleState>,
    request: axum::extract::Request,
) -> Response {
    match proxy::catalog(&state, request).await {
        Ok(Some(response)) => return response,
        Err(response) => return *response,
        Ok(None) => {}
    }
    let Some(ids) = state.config.zen.lite_model_ids.as_ref() else {
        return catalog_unavailable();
    };
    models_response(ids).into_response()
}

async fn full_models(
    State(state): State<ConsoleState>,
    request: axum::extract::Request,
) -> Response {
    let headers = request.headers().clone();
    match proxy::catalog(&state, request).await {
        Ok(Some(response)) => return response,
        Err(response) => return *response,
        Ok(None) => {}
    }
    let Some(ids) = state.config.zen.full_model_ids.as_ref() else {
        return catalog_unavailable();
    };
    let authorization = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let api_key = authorization
        .strip_prefix("Bearer ")
        .filter(|key| !key.is_empty());
    let disabled = match api_key {
        Some(api_key) => {
            match opencode_hosted_adapters::console::tenant::TenantStore::new(state.db.clone())
                .disabled_models_for_key(api_key)
                .await
            {
                Ok(Some(disabled)) => disabled,
                Ok(None) => Vec::new(),
                Err(_) => return catalog_unavailable(),
            }
        }
        None => Vec::new(),
    };
    models_response(
        &ids.iter()
            .filter(|id| !disabled.iter().any(|model| model == *id))
            .cloned()
            .collect::<Vec<_>>(),
    )
    .into_response()
}

fn catalog_unavailable() -> Response {
    let mut response = (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "error": "Zen model catalog is not configured" })),
    )
        .into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::http::{Request, StatusCode};
    use mysql_async::prelude::Queryable;
    use opencode_hosted_adapters::console::db::TestDatabase;
    use tower::ServiceExt;

    fn app(config: Config) -> Router {
        let config = super::super::state::ConsoleConfig {
            zen: config,
            ..Default::default()
        };
        let state = ConsoleState {
            config: Arc::new(config),
            db: opencode_hosted_adapters::console::db::ConsoleDb::connect(
                "mysql://u:p@127.0.0.1/test",
            )
            .unwrap(),
        };
        routes().with_state(state)
    }

    #[tokio::test]
    async fn go_model_list_returns_only_configured_public_lite_ids() {
        let app = app(Config {
            lite_model_ids: Some(vec!["model-a".into(), "model-b".into()]),
            ..Default::default()
        });
        let response = app
            .oneshot(
                Request::get("/zen/go/v1/models")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(body["object"], "list");
        assert_eq!(body["data"].as_array().unwrap().len(), 2);
        assert_eq!(body["data"][0]["id"], "model-a");
        assert_eq!(body["data"][0]["owned_by"], "opencode");
    }

    #[tokio::test]
    async fn zen_model_list_returns_full_models_without_key_filters_when_anonymous() {
        let response = app(Config {
            full_model_ids: Some(vec!["model-a".into(), "model-b".into()]),
            ..Default::default()
        })
        .oneshot(
            Request::get("/zen/v1/models")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(body["data"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn fixture_zen_model_list_filters_disabled_models_for_the_presented_key() {
        let fixture = TestDatabase::create("zen_go_models").await.unwrap();
        let mut conn = fixture.db.conn().await.unwrap();
        conn.exec_drop(
            "INSERT INTO `workspace` (`id`, `name`) VALUES ('wrk_zen_go', 'Zen Go')",
            (),
        )
        .await
        .unwrap();
        conn.exec_drop(
            "INSERT INTO `key` (`id`, `workspace_id`, `user_id`, `name`, `key`) VALUES ('key_zen_go', 'wrk_zen_go', 'usr_zen_go', 'Zen Go', 'oc_sk_zen_go')",
            (),
        )
        .await
        .unwrap();
        conn.exec_drop(
            "INSERT INTO `model` (`id`, `workspace_id`, `model`) VALUES ('mod_zen_go_disabled', 'wrk_zen_go', 'disabled-model')",
            (),
        )
        .await
        .unwrap();
        drop(conn);

        let config = super::super::state::ConsoleConfig {
            zen: Config {
                full_model_ids: Some(vec!["disabled-model".into(), "allowed-model".into()]),
                ..Default::default()
            },
            ..Default::default()
        };
        let router = routes().with_state(ConsoleState {
            config: Arc::new(config),
            db: fixture.db.clone(),
        });
        let response = router
            .oneshot(
                Request::get("/zen/v1/models")
                    .header(header::AUTHORIZATION, "Bearer oc_sk_zen_go")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        let ids = body["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|model| model["id"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["allowed-model"]);
        fixture.drop_database().await.unwrap();
    }

    #[tokio::test]
    async fn model_list_fails_closed_when_catalog_is_missing() {
        let response = app(Config::default())
            .oneshot(
                Request::get("/zen/v1/models")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }

    #[tokio::test]
    async fn model_options_response_has_openai_compatible_cors_headers() {
        let response = app(Config::default())
            .oneshot(
                Request::options("/zen/v1/models")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN], "*");
        assert_eq!(
            response.headers()[header::ACCESS_CONTROL_ALLOW_HEADERS],
            "Content-Type, Authorization"
        );
    }

    #[test]
    fn debug_output_does_not_expose_catalog_model_ids() {
        let config = Config {
            lite_model_ids: Some(vec!["private-model-name".into()]),
            ..Default::default()
        };
        assert!(!format!("{config:?}").contains("private-model-name"));
    }

    #[test]
    fn catalog_config_keeps_lite_ids_and_omits_alpha_models() {
        let config = Config::from_catalog(json!({
            "liteModels": {
                "allowed-model": { "name": "Allowed", "providers": [] },
                "alpha-hidden": { "name": "Alpha", "providers": [] }
            },
            "zenModels": {
                "full-model": {},
                "alpha-hidden": {},
                "regional:global": {}
            },
            "providers": { "secret-provider": { "apiKey": "must-not-leak" } }
        }))
        .unwrap();
        assert_eq!(config.lite_model_ids.unwrap(), vec!["allowed-model"]);
        assert_eq!(config.full_model_ids.unwrap(), vec!["full-model"]);
    }

    #[test]
    fn catalog_config_rejects_missing_lite_models() {
        assert!(Config::from_catalog(json!({ "providers": {}, "liteModels": {} })).is_err());
    }
}
