//! Purpose: run the native legacy Zen inference request lifecycle.
//! Scope: model validation, request limits, authentication, provider placement and forwarding.
//! Uses:
//! rust/bins/
//! |-- hosted/src/console/zen/completion.rs
//! `-- hosted/src/console/zen/proxy.rs
//! rust/crates/
//! |-- hosted-adapters/src/console/formats.rs
//! |-- hosted-adapters/src/console/gateway.rs
//! `-- hosted-adapters/src/console/limits.rs
//! External:
//! |-- axum::response::Response
//! `-- serde_json::Value
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/zen/proxy.rs
//! Invariants: billing and limit validation precede provider side effects; credentials never enter responses.
//! Contract tests: hosted/src/console/zen/tests.rs

use super::{
    ConsoleState,
    completion::{Completion, complete},
    proxy::{api_error, usage_error},
};
use axum::{
    http::{StatusCode, request::Parts},
    response::Response,
};
use opencode_hosted_adapters::console::{
    formats::{Format, ProviderHelper},
    gateway::{
        body::{BodyStream, PreparedBody, include_usage},
        catalog,
        native::{self, ProviderRequest},
        usage::UsageService,
    },
    limits::{
        self, LimitError,
        provider::{ProviderBudget, select},
        store::ProviderCounters,
    },
};
use serde_json::Value;
use time::OffsetDateTime;

pub async fn generation(
    state: ConsoleState,
    parts: Parts,
    prepared: Option<PreparedBody>,
    raw: Option<BodyStream>,
    model_id: String,
) -> Response {
    let trace = super::metrics::GatewayTrace::new(&parts, state.config.zen.tail.clone());
    let headers = parts.headers.clone();
    let path = parts.uri.path().to_owned();
    match run(state, parts, prepared, raw, model_id, trace.clone()).await {
        Ok(response) => response,
        Err(response) => {
            let status = response.status().as_u16();
            let (parts, body) = (*response).into_parts();
            let bytes = match axum::body::to_bytes(body, 1024 * 1024).await {
                Ok(bytes) => bytes,
                Err(_) => return internal(),
            };
            let mut value: Value = match serde_json::from_slice(&bytes) {
                Ok(value) => value,
                Err(_) => return Response::from_parts(parts, axum::body::Body::from(bytes)),
            };
            if let Some(message) = value.pointer("/error/message").and_then(Value::as_str) {
                value["error"]["message"] =
                    Value::String(super::locale::translate(message, &headers, &path));
            }
            let mut trace = trace;
            trace.metric["error.type"] = value["error"]["type"].clone();
            trace.metric["error.message"] = value["error"]["message"].clone();
            trace.finish(status, bytes.len(), 0, false).await;
            Response::from_parts(
                parts,
                axum::body::Body::from(serde_json::to_vec(&value).expect("error JSON")),
            )
        }
    }
}

async fn run(
    state: ConsoleState,
    parts: Parts,
    prepared: Option<PreparedBody>,
    raw: Option<BodyStream>,
    model_id: String,
    mut trace: super::metrics::GatewayTrace,
) -> Result<Response, Box<Response>> {
    let lite = parts.uri.path().starts_with("/zen/go/");
    let format: Format =
        serde_json::from_value(Value::String(super::proxy::format(parts.uri.path()).into()))
            .map_err(|_| internal())?;
    let config = &state.config.zen;
    let catalog = config.catalog.as_ref().ok_or_else(internal)?;
    let requested = catalog
        .get(if lite { "liteModels" } else { "zenModels" })
        .and_then(|models| models.get(&model_id));
    if requested.and_then(Value::as_array).is_some_and(|entries| {
        !entries
            .iter()
            .any(|entry| entry["formatFilter"] == super::proxy::format(parts.uri.path()))
    }) {
        return Err(Box::new(api_error(
            StatusCode::UNAUTHORIZED,
            "ModelError",
            &format!(
                "Model {model_id} is not supported for format {}",
                super::proxy::format(parts.uri.path())
            ),
        )));
    }
    let (model, providers) = catalog::normalize(
        catalog,
        lite,
        &model_id,
        super::proxy::format(parts.uri.path()),
    )
    .ok_or_else(|| {
        api_error(
            StatusCode::UNAUTHORIZED,
            "ModelError",
            &format!("Model {model_id} is not supported"),
        )
    })?;
    if model["trialEnded"] == true {
        return Err(Box::new(api_error(
            StatusCode::UNAUTHORIZED,
            "ModelError",
            &format!(
                "Free promotion has ended for {}. You can continue using the model by subscribing to OpenCode Go - https://opencode.ai/go",
                model["name"].as_str().unwrap_or_default()
            ),
        )));
    }
    country_policy(&model_id, &parts)?;
    let limits = config.limits.as_ref().ok_or_else(internal)?;
    let now = OffsetDateTime::now_utc();
    let ip = limits::normalized_ip(header(&parts, "x-real-ip"));
    let header_name = opencode_hosted_domain::console::inference::api_key_header(parts.uri.path());
    let key = if header_name == "authorization" {
        header(&parts, header_name).split(' ').nth(1)
    } else {
        parts.headers.get(header_name).and_then(|v| v.to_str().ok())
    }
    .filter(|key| *key != "public");
    let anonymous = model["allowAnonymous"] == true;
    if !anonymous && key.is_none() {
        return Err(Box::new(api_error(
            StatusCode::UNAUTHORIZED,
            "AuthError",
            "Missing API key.",
        )));
    }
    let redis = config.redis.as_ref().ok_or_else(internal)?;
    let request_limit = if anonymous || key.is_some() {
        Some(
            redis
                .request(
                    &model_id,
                    model["rateLimit"].as_i64(),
                    anonymous,
                    if anonymous {
                        &ip
                    } else {
                        key.unwrap_or_default()
                    },
                    limits["free"]["dailyRequests"]
                        .as_i64()
                        .ok_or_else(internal)?,
                    now,
                )
                .await
                .map_err(limit_error)?,
        )
    } else {
        None
    };
    let service = usage_service(&state, limits.clone())?;
    let mut context = service
        .authenticate_identity(key, &model_id, &model, lite)
        .await
        .map_err(usage_error)?;
    if lite
        && ["muse-spark-1.3-contributor", "muse-spark-1.2-contributor"].contains(&model_id.as_str())
        && context.workspace_id.is_some()
        && !context.allow_training
    {
        return Err(Box::new(api_error(
            StatusCode::FORBIDDEN,
            "DataPolicyError",
            &format!(
                "This model collects data used to improve its quality and requires explicit opt in: https://opencode.ai/workspace/{}/go",
                context.workspace_id.as_deref().unwrap_or_default()
            ),
        )));
    }
    service
        .default_region(
            &mut context,
            parts
                .headers
                .get("cf-ipcountry")
                .and_then(|value| value.to_str().ok()),
        )
        .await
        .map_err(usage_error)?;
    if lite
        && [
            "deepseek-v4.1-flash",
            "deepseek-flash",
            "deepseek-v4-flash",
            "deepseek-v4-pro",
        ]
        .contains(&model_id.as_str())
        && context.workspace_id.is_some()
        && !context
            .region
            .as_array()
            .is_some_and(|regions| regions.iter().any(|r| r == "cn"))
    {
        return Err(Box::new(api_error(
            StatusCode::FORBIDDEN,
            "RegionError",
            &format!(
                "The latest version of this model is only available hosted in China and requires explicit opt in: https://opencode.ai/workspace/{}/go",
                context.workspace_id.as_deref().unwrap_or_default()
            ),
        )));
    }
    let session = header(&parts, "x-opencode-session").to_owned();
    let sticky_id = if !session.is_empty() {
        session.clone()
    } else {
        context.workspace_id.clone().unwrap_or(ip.clone())
    };
    let counters = ProviderCounters::new(state.db.clone(), now);
    let snapshot = counters
        .snapshot(
            &model,
            &model_id,
            &sticky_id,
            &ip,
            limits["free"]["promoTokens"]
                .as_i64()
                .ok_or_else(internal)?,
        )
        .await
        .map_err(|_| internal())?;
    service
        .validate_context(&mut context, &model, lite)
        .map_err(usage_error)?;
    let merged = model["providers"]
        .as_array()
        .ok_or_else(internal)?
        .iter()
        .map(|provider| {
            let mut entry = provider.clone();
            if let Some(props) = provider["id"]
                .as_str()
                .and_then(|id| providers.get(id))
                .and_then(Value::as_object)
            {
                entry
                    .as_object_mut()
                    .expect("provider object")
                    .extend(props.clone());
            }
            entry
        })
        .collect::<Vec<_>>();
    let budget = ProviderBudget::new(redis.clone(), &merged, now);
    let budget_snapshot = match &budget {
        Some(budget) => Some(budget.snapshot().await.map_err(limit_error)?),
        None => None,
    };
    let mut provider = select(
        &model,
        &sticky_id,
        context
            .provider_credentials
            .as_ref()
            .is_some_and(|v| !v.is_empty()),
        &snapshot,
        budget_snapshot.as_ref(),
    )
    .cloned()
    .ok_or_else(|| {
        api_error(
            StatusCode::UNAUTHORIZED,
            "ModelError",
            "No provider available",
        )
    })?;
    let provider_id = provider["id"].as_str().ok_or_else(internal)?.to_owned();
    provider.as_object_mut().ok_or_else(internal)?.extend(
        providers
            .get(&provider_id)
            .and_then(Value::as_object)
            .ok_or_else(|| {
                api_error(
                    StatusCode::UNAUTHORIZED,
                    "ModelError",
                    &format!("Provider {provider_id} not supported"),
                )
            })?
            .clone(),
    );
    service
        .validate_model_settings(&context, &model)
        .map_err(usage_error)?;
    if let Some(credentials) = &context.provider_credentials {
        provider["apiKey"] = Value::String(credentials.clone());
    }
    let provider_format: Format = serde_json::from_value(
        provider
            .get("format")
            .cloned()
            .unwrap_or(Value::String("oa-compat".into())),
    )
    .map_err(|_| internal())?;
    if provider_format != format {
        return Err(Box::new(internal()));
    }
    let native = provider["model"].as_str().ok_or_else(internal)?.to_owned();
    if provider_format == Format::Anthropic
        && (native.starts_with("arn:aws:bedrock:")
            || native.starts_with("global.anthropic.")
            || native.starts_with("databricks-claude-"))
    {
        return Err(Box::new(internal()));
    }
    trace.identity(&context, &model_id, &provider_id, &native);
    let helper = ProviderHelper::new(
        provider_format,
        &model_id,
        &native,
        provider["adjustCacheUsage"] == true,
    );
    let stream = if let Some(prepared) = prepared {
        let stream = prepared.stream(&native).map_err(|_| internal())?;
        if format == Format::OaCompat {
            include_usage(stream)
        } else {
            stream
        }
    } else {
        raw.ok_or_else(internal)?
    };
    let response = native::forward(ProviderRequest {
        provider: &provider,
        headers: &parts.headers,
        helper: &helper,
        sticky_id: &sticky_id,
        model: &model_id,
        ip: &ip,
        workspace: context.workspace_id.as_deref(),
        billing_source: context.source.as_str(),
        lite,
        google_stream: parts.uri.path().ends_with(":streamGenerateContent"),
        body: reqwest::Body::wrap_stream(stream),
    })
    .await
    .map_err(|_| internal())?;
    if ["console.", "console-go.", "inf.", "inf-go."]
        .iter()
        .any(|prefix| provider_id.starts_with(prefix))
        && let (Some(endpoint), Some(native_model)) = (
            response
                .headers()
                .get("x-opencode-endpoint-id")
                .and_then(|value| value.to_str().ok()),
            response
                .headers()
                .get("x-opencode-upstream-model-id")
                .and_then(|value| value.to_str().ok()),
        )
    {
        trace.metric["provider"] = serde_json::json!(endpoint);
        trace.metric["provider.model"] = serde_json::json!(native_model);
    }
    if response.status() == StatusCode::OK
        && model.get("stickyProvider").is_some()
        && !sticky_id.is_empty()
    {
        counters
            .sticky(&model_id, &sticky_id, &provider_id)
            .await
            .map_err(|_| internal())?;
    }
    complete(
        response,
        Completion {
            service,
            trace,
            context,
            model,
            model_id,
            provider,
            provider_id,
            session,
            helper,
            format,
            request_limit,
            counters,
            trial: snapshot.trial,
            ip,
            budget,
        },
    )
    .await
}

fn usage_service(state: &ConsoleState, limits: Value) -> Result<UsageService, Box<Response>> {
    let service = UsageService::new(state.db.clone(), limits)
        .with_production(state.config.stage == "production");
    let service = if let Some(redis) = state.config.zen.redis.as_ref() {
        service.with_usage_batcher(redis.clone())
    } else {
        service
    };
    let config = &state.config.billing;
    if let Some(secret) = config.secret_key.as_deref() {
        let stripe = opencode_hosted_adapters::console::stripe::StripeClient::new(
            secret,
            config
                .api_base
                .as_deref()
                .unwrap_or(opencode_hosted_adapters::console::stripe::DEFAULT_API_BASE),
        )
        .map_err(|_| internal())?;
        return Ok(service.with_stripe(stripe));
    }
    Ok(service)
}

pub fn limit_error(error: LimitError) -> Response {
    use axum::response::IntoResponse;
    if let LimitError::Exceeded { kind, retry_after } = error {
        return (StatusCode::TOO_MANY_REQUESTS, [("retry-after", retry_after.to_string())], axum::Json(serde_json::json!({"type":"error","error":{"type":kind,"message":"Rate limit exceeded. Please try again later."},"metadata":{}}))).into_response();
    }
    internal()
}

fn header<'a>(parts: &'a Parts, name: &str) -> &'a str {
    parts
        .headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
}
pub fn internal() -> Response {
    api_error(
        StatusCode::INTERNAL_SERVER_ERROR,
        "error",
        "Internal server error",
    )
}

fn country_policy(model: &str, parts: &Parts) -> Result<(), Box<Response>> {
    if ![
        "muse-spark-1.3-contributor",
        "muse-spark-1.3-contributor-free",
        "muse-spark-1.2-contributor",
        "muse-spark-1.2-contributor-free",
    ]
    .contains(&model)
    {
        return Ok(());
    }
    let country = header(parts, "cf-ipcountry").to_uppercase();
    if [
        "AF", "BY", "CN", "CU", "EH", "ER", "ET", "HK", "HT", "IQ", "IR", "KH", "KP", "LY", "MM",
        "MO", "NI", "PK", "RU", "SO", "SY", "VE",
    ]
    .contains(&country.as_str())
    {
        return Err(Box::new(api_error(
            StatusCode::FORBIDDEN,
            "RegionError",
            "This model is not available in your country.",
        )));
    }
    Ok(())
}
