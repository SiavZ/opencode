//! Purpose: serve Zen generation and catalog migration forwarding routes.
//! Scope: request replay, opaque streaming responses and explicit legacy fallback.
//! Uses:
//! rust/bins/
//! `-- hosted/src/console/state.rs
//! rust/crates/
//! `-- hosted-adapters/src/console/gateway.rs
//! External:
//! |-- axum::{body::Body, extract::Request, response::Response}
//! `-- futures::StreamExt
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/zen.rs
//! Invariants: no background forwarding task survives downstream body drop.
//! Contract tests: hosted/src/console/zen.rs

use axum::{
    Json,
    body::Body,
    extract::{Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use futures::StreamExt;
use opencode_hosted_adapters::console::gateway::body::{BodyStream, PreparedBody};
use serde_json::{Value, json};

use super::ConsoleState;

pub async fn generation(State(state): State<ConsoleState>, request: Request) -> Response {
    let cancellation = request
        .extensions()
        .get::<tokio_util::sync::CancellationToken>()
        .cloned()
        .unwrap_or_default();
    tokio::select! {
        biased;
        () = cancellation.cancelled() => StatusCode::from_u16(499).expect("client closed status").into_response(),
        response = generation_inner(state, request) => response,
    }
}

async fn generation_inner(state: ConsoleState, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let stream: BodyStream = Box::pin(
        body.into_data_stream()
            .map(|chunk| chunk.map_err(std::io::Error::other)),
    );
    let google = parts.uri.path().starts_with("/zen/v1/models/");
    let mut raw = Some(stream);
    let prepared = if google {
        None
    } else {
        match PreparedBody::read(raw.take().expect("request stream")).await {
            Ok(prepared) => Some(prepared),
            Err(_) => {
                return api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "error",
                    "Internal server error",
                );
            }
        }
    };
    // Google model IDs come from the path; its request body is never parsed by the baseline.
    let model = prepared
        .as_ref()
        .map(|p| p.model.clone())
        .unwrap_or_else(|| {
            parts
                .uri
                .path()
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .split(':')
                .next()
                .unwrap_or_default()
                .into()
        });
    let format = format(parts.uri.path());
    let mapping = if !parts.uri.path().starts_with("/zen/go/") {
        model_entry(&state, &model, format, false).and_then(|entry| {
            let provider = entry.get("byokProvider")?.as_str()?;
            let native = entry
                .get("providers")
                .and_then(Value::as_array)
                .and_then(|providers| {
                    providers
                        .iter()
                        .find(|p| p.get("id").and_then(Value::as_str) == Some(provider))
                })
                .and_then(|provider| provider.get("model"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            Some((provider.to_owned(), native.to_owned()))
        })
    } else {
        None
    };
    if let Some(gateway) = state
        .config
        .zen
        .migration
        .as_ref()
        .filter(|_| !model.is_empty())
    {
        match gateway
            .target(
                &state.db,
                &parts.method,
                parts.uri.path(),
                parts.uri.query(),
                &parts.headers,
                mapping.as_ref().map(|(p, m)| (p.as_str(), m.as_str())),
            )
            .await
        {
            Ok(Some(target)) => {
                let native = target.model.as_deref().unwrap_or(&model).to_owned();
                let stream = match prepared {
                    Some(body) => match body.stream(&native) {
                        Ok(stream) => stream,
                        Err(_) => return unavailable(),
                    },
                    None => raw.expect("Google request stream"),
                };
                return match gateway
                    .forward(parts.method, target, reqwest::Body::wrap_stream(stream))
                    .await
                {
                    Ok(response) => upstream(response),
                    Err(_) => unavailable(),
                };
            }
            Err(_) => return unavailable(),
            Ok(None) => {}
        }
    }
    super::legacy::generation(state, parts, prepared, raw, model).await
}

pub async fn catalog(
    state: &ConsoleState,
    request: Request,
) -> Result<Option<Response>, Box<Response>> {
    let Some(gateway) = state.config.zen.migration.as_ref() else {
        return Ok(None);
    };
    match gateway
        .target(
            &state.db,
            request.method(),
            request.uri().path(),
            request.uri().query(),
            request.headers(),
            None,
        )
        .await
    {
        Ok(Some(target)) => gateway
            .forward(
                request.method().clone(),
                target,
                reqwest::Body::from(Vec::new()),
            )
            .await
            .map(|response| Some(upstream(response)))
            .map_err(|_| Box::new(unavailable())),
        Ok(None) => Ok(None),
        Err(_) => Err(Box::new(unavailable())),
    }
}

pub async fn usage(State(state): State<ConsoleState>, request: Request) -> Response {
    let authorization = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    match catalog(&state, request).await {
        Ok(Some(response)) => response,
        Err(response) => *response,
        Ok(None) => {
            let Some(limits) = state.config.zen.limits.clone() else {
                return super::legacy::internal();
            };
            let key = authorization
                .strip_prefix("Bearer ")
                .filter(|value| !value.is_empty() && !value.chars().any(char::is_whitespace));
            let service = opencode_hosted_adapters::console::gateway::usage::UsageService::new(
                state.db.clone(),
                limits,
            );
            match service.go_usage(key).await {
                Ok(value) => Json(value).into_response(),
                Err(error) => usage_error(error),
            }
        }
    }
}

fn upstream(response: reqwest::Response) -> Response {
    let status = response.status();
    let headers = response.headers().clone();
    let mut output = Response::new(Body::from_stream(response.bytes_stream()));
    *output.status_mut() = status;
    *output.headers_mut() = headers;
    output
}

pub(super) fn format(path: &str) -> &'static str {
    if path.ends_with("/messages") {
        return "anthropic";
    }
    if path.ends_with("/responses") {
        return "openai";
    }
    if path.ends_with("/systemone") {
        return "systemone";
    }
    if path.starts_with("/zen/v1/models/") {
        return "google";
    }
    "oa-compat"
}

fn model_entry<'a>(
    state: &'a ConsoleState,
    model: &str,
    format: &str,
    lite: bool,
) -> Option<&'a Value> {
    let entry = state
        .config
        .zen
        .catalog
        .as_ref()?
        .get(if lite { "liteModels" } else { "zenModels" })?
        .get(model)?;
    if let Some(entries) = entry.as_array() {
        return entries
            .iter()
            .find(|entry| entry.get("formatFilter").and_then(Value::as_str) == Some(format));
    }
    Some(entry)
}

fn unavailable() -> Response {
    (StatusCode::SERVICE_UNAVAILABLE, [("cache-control", "no-store")], Json(json!({"error":{"type":"api_error","message":"Inference routing is unavailable. Please retry later."}}))).into_response()
}

pub(super) fn usage_error(
    error: opencode_hosted_adapters::console::gateway::usage::UsageError,
) -> Response {
    let mut response = (
        StatusCode::from_u16(error.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        Json(error.response_json()),
    )
        .into_response();
    if let Some(seconds) = error.retry_after
        && let Ok(value) = axum::http::HeaderValue::from_str(&seconds.to_string())
    {
        response.headers_mut().insert("retry-after", value);
    }
    response
}

pub(super) fn api_error(status: StatusCode, kind: &str, message: &str) -> Response {
    (
        status,
        Json(json!({"type":"error","error":{"type":kind,"message":message}})),
    )
        .into_response()
}
