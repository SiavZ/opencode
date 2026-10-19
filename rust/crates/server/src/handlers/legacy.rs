//! Purpose: route HTTP operations to concrete domain services and retain API-specific envelopes.
//! Scope: request boundary.
//! Uses:
//! rust/crates/
//! |-- protocol/src/operation.rs
//! |-- server/src/handlers/mod.rs
//! |-- server/src/middleware/cors.rs
//! |-- server/src/middleware/errors.rs
//! |-- server/src/middleware/legacy_routing.rs
//! |-- server/src/services/legacy.rs
//! `-- server/src/services/streams.rs
//! External:
//! |-- axum::http
//! |-- axum::response::Response
//! |-- reqwest::Url
//! `-- serde_json::json
//! Used by:
//! rust/crates/
//! `-- server/src/handlers/mod.rs
//! Invariants: absent composition is never successful; domain errors and pagination headers survive unchanged.
//! Contract tests: server/tests/http_legacy.rs, server/tests/http_domains.rs

use crate::handlers::Ctx;
use crate::middleware::cors::{CorsConfig, is_allowed_request_origin};
use crate::middleware::errors::HttpError;
use crate::middleware::legacy_routing::RouteContext;
use crate::services::legacy::{LegacyError, LegacyResponse};
use crate::services::streams::TicketScope;
use axum::http::{HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use opencode_protocol::operation::{Middleware, SuccessEncoding};
use serde_json::json;

pub async fn handle(mut ctx: Ctx) -> Result<Response, HttpError> {
    for field in ["roots", "archived"] {
        if let Some(value) = ctx.query.get(field).and_then(serde_json::Value::as_str) {
            let value = value == "true";
            ctx.query.insert(field.into(), value.into());
        }
    }
    let name = ctx.endpoint.name.as_str();
    let services = &ctx.state.services;
    let service = match name {
        name if ctx.current && name.starts_with("integration.") => &services.legacy_providers,
        name if name.starts_with("session.permission.")
            || name.starts_with("session.question.") =>
        {
            &services.legacy_native
        }
        name if name.starts_with("session.")
            || name.starts_with("part.")
            || name.starts_with("experimental.session.") =>
        {
            &services.legacy_sessions
        }
        name if name.starts_with("permission.")
            || name.starts_with("question.")
            || name.starts_with("pty.")
            || name.starts_with("tool.") =>
        {
            &services.legacy_native
        }
        name if name.starts_with("mcp.")
            || name.starts_with("lsp.")
            || name.starts_with("integration.")
            || name.starts_with("reference.")
            || name.starts_with("skill.")
            || matches!(
                name,
                "app.skills" | "formatter.status" | "experimental.resource.list"
            ) =>
        {
            &services.legacy_integrations
        }
        name if name.starts_with("provider.")
            || name.starts_with("model.")
            || name.starts_with("credential.")
            || name.starts_with("auth.")
            || name == "config.providers" =>
        {
            &services.legacy_providers
        }
        _ => &services.legacy_global,
    }
    .as_ref()
    .ok_or_else(|| HttpError::not_yet_ported(name))?;
    let context = context(&ctx)?;
    if name == "permission.respond" {
        let sessions = services
            .legacy_sessions
            .as_ref()
            .ok_or_else(|| HttpError::not_yet_ported("session.get"))?;
        if let Err(error) = sessions
            .handle("session.get", &context, &ctx.params, &ctx.query, None)
            .await
        {
            return domain_error(error);
        }
    }
    if name == "pty.connectToken" {
        return token(&ctx, &context, service.as_ref()).await;
    }
    let operation = if ctx.current {
        &ctx.endpoint.operation_id
    } else {
        name
    };
    if ctx
        .endpoint
        .successes
        .iter()
        .any(|success| success.encoding == SuccessEncoding::Bytes)
    {
        return match service
            .handle_bytes(
                operation,
                &context,
                &ctx.params,
                &ctx.query,
                ctx.payload.clone(),
            )
            .await
        {
            Ok(value) => {
                let status = StatusCode::from_u16(value.status)
                    .map_err(|error| HttpError::Defect(error.to_string()))?;
                let mut response = (status, value.body).into_response();
                for (name, value) in value.headers {
                    response.headers_mut().insert(
                        HeaderName::try_from(name)
                            .map_err(|error| HttpError::Defect(error.to_string()))?,
                        HeaderValue::try_from(value)
                            .map_err(|error| HttpError::Defect(error.to_string()))?,
                    );
                }
                Ok(response)
            }
            Err(error) => domain_error(error),
        };
    }
    match service
        .handle_response(
            operation,
            &context,
            &ctx.params,
            &ctx.query,
            ctx.payload.clone(),
        )
        .await
    {
        Ok(mut value) => {
            if ctx
                .endpoint
                .successes
                .iter()
                .any(|success| success.encoding == SuccessEncoding::Void)
            {
                value.status = 204;
            } else if ctx.current {
                if ctx.endpoint.middleware.contains(&Middleware::Location) {
                    value.body = json!({"location":ctx.location,"data":value.body});
                } else {
                    value.body = json!({"data":value.body});
                }
            }
            if name == "session.messages" && !ctx.current {
                pagination(&ctx, &mut value)?;
            }
            response(value)
        }
        Err(error) => domain_error(error),
    }
}

fn context(ctx: &Ctx) -> Result<RouteContext, HttpError> {
    if let Some(context) = &ctx.legacy {
        return Ok(context.clone());
    }
    if let Some(location) = &ctx.location {
        return Ok(RouteContext {
            directory: location.directory.to_string(),
            workspace_id: location.workspace_id.as_ref().map(ToString::to_string),
        });
    }
    Ok(RouteContext {
        directory: std::env::current_dir()
            .map_err(|error| HttpError::Defect(error.to_string()))?
            .to_string_lossy()
            .into_owned(),
        workspace_id: None,
    })
}

fn domain_error(error: LegacyError) -> Result<Response, HttpError> {
    match error {
        LegacyError::NotFound(message) => Ok(crate::middleware::errors::json(
            StatusCode::NOT_FOUND,
            &json!({"name":"NotFoundError","data":{"message":message}}),
        )),
        LegacyError::BadRequest(_) => Ok(crate::middleware::errors::json(
            StatusCode::BAD_REQUEST,
            &json!({"_tag":"BadRequest"}),
        )),
        LegacyError::Response { status, body } => Ok(crate::middleware::errors::json(
            StatusCode::from_u16(status).map_err(|error| HttpError::Defect(error.to_string()))?,
            &body,
        )),
        LegacyError::Unavailable(operation) => Err(HttpError::not_yet_ported(&operation)),
        LegacyError::Internal(message) => Err(HttpError::Defect(message)),
    }
}

async fn token(
    ctx: &Ctx,
    context: &RouteContext,
    service: &dyn crate::services::legacy::LegacySessions,
) -> Result<Response, HttpError> {
    let header = |name| {
        ctx.request_headers
            .get(name)
            .and_then(|value| value.to_str().ok())
    };
    let defaults = CorsConfig::default();
    if header("x-opencode-ticket") != Some("1")
        || !is_allowed_request_origin(
            header("origin"),
            header("host"),
            ctx.state.config.cors.as_ref().unwrap_or(&defaults),
        )
    {
        let body = if ctx.current {
            json!({"_tag":"ForbiddenError","message":"Invalid PTY connect token request"})
        } else {
            json!({"name":"ForbiddenError","data":{"message":"Invalid PTY connect token request"}})
        };
        return Ok(crate::middleware::errors::json(
            StatusCode::FORBIDDEN,
            &body,
        ));
    }
    let operation = if ctx.current { "v2.pty.get" } else { "pty.get" };
    if let Err(error) = service
        .handle(operation, context, &ctx.params, &ctx.query, None)
        .await
    {
        return domain_error(error);
    }
    let body = serde_json::to_value(ctx.state.services.streams.tickets.issue(TicketScope {
        pty_id: ctx.param("ptyID"),
        directory: Some(context.directory.clone()),
        workspace_id: context.workspace_id.clone(),
    }))
    .map_err(|error| HttpError::Defect(error.to_string()))?;
    response(if ctx.current {
        json!({"location":ctx.location,"data":body}).into()
    } else {
        body.into()
    })
}

fn pagination(ctx: &Ctx, response: &mut LegacyResponse) -> Result<(), HttpError> {
    let Some(cursor) = response
        .headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("x-next-cursor"))
        .map(|(_, value)| value.clone())
    else {
        return Ok(());
    };
    let host = ctx
        .request_headers
        .get("host")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("localhost");
    let scheme = ctx
        .request_headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("http");
    let mut url = reqwest::Url::parse(&format!("{scheme}://{host}{}", ctx.request_uri))
        .map_err(|error| HttpError::Defect(error.to_string()))?;
    let pairs: Vec<(String, String)> = url
        .query_pairs()
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect();
    let limit = ctx
        .query_num("limit")
        .map(|value| value.to_string())
        .unwrap_or_default();
    let first_limit = pairs.iter().position(|(name, _)| name == "limit");
    let first_before = pairs.iter().position(|(name, _)| name == "before");
    url.set_query(None);
    url.query_pairs_mut()
        .extend_pairs(
            pairs
                .iter()
                .enumerate()
                .filter_map(|(index, (name, value))| match name.as_str() {
                    "limit" => {
                        (Some(index) == first_limit).then_some((name.as_str(), limit.as_str()))
                    }
                    "before" => {
                        (Some(index) == first_before).then_some((name.as_str(), cursor.as_str()))
                    }
                    _ => Some((name.as_str(), value.as_str())),
                }),
        );
    if first_limit.is_none() {
        url.query_pairs_mut().append_pair("limit", &limit);
    }
    if first_before.is_none() {
        url.query_pairs_mut().append_pair("before", &cursor);
    }
    response
        .headers
        .push(("Link".into(), format!("<{url}>; rel=\"next\"")));
    response
        .headers
        .retain(|(name, _)| !name.eq_ignore_ascii_case("access-control-expose-headers"));
    response.headers.push((
        "Access-Control-Expose-Headers".into(),
        "Link, X-Next-Cursor".into(),
    ));
    Ok(())
}

fn response(value: LegacyResponse) -> Result<Response, HttpError> {
    let status =
        StatusCode::from_u16(value.status).map_err(|error| HttpError::Defect(error.to_string()))?;
    let raw = value
        .headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
        .is_some_and(|(_, value)| !value.starts_with("application/json"));
    let mut response = if status == StatusCode::NO_CONTENT {
        status.into_response()
    } else if raw {
        (status, value.body.as_str().unwrap_or_default().to_string()).into_response()
    } else {
        crate::middleware::errors::json(status, &value.body)
    };
    for (name, value) in value.headers {
        response.headers_mut().insert(
            HeaderName::try_from(name).map_err(|error| HttpError::Defect(error.to_string()))?,
            HeaderValue::try_from(value).map_err(|error| HttpError::Defect(error.to_string()))?,
        );
    }
    Ok(response)
}
