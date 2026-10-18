//! Purpose: enterprise share HTTP API (`/api/share*`, `/api/support/actions/remove-share`, `/api/doc`)
//! ported from packages/enterprise/src/routes/api/[...path].ts:11-155 onto axum.
//! Scope: request decoding, status/header/body mapping. Share rules live in hosted-adapters.
//! Uses:
//! rust/bins/
//! `-- hosted/src/sharing/validate.rs
//! rust/crates/
//! |-- hosted-adapters/src/sharing/objects.rs
//! `-- hosted-adapters/src/sharing/service.rs
//! External:
//! `-- axum::{Router, routing}
//! Used by:
//! rust/bins/
//! |-- hosted/src/sharing/mod.rs
//! `-- hosted/src/sharing/tests.rs
//! Invariants: every response carries `access-control-allow-origin: *` (hono cors default),
//! thrown share errors are a plain-text 500 `Internal Server Error`, the data route always sets
//! `public, max-age=30, s-maxage=300, stale-while-revalidate=86400`, `/api/doc` is a 500 because
//! the baseline cannot render `z.custom` schemas, the support token compares in constant time.
//! Contract tests: bins/hosted/src/sharing/tests.rs

use std::sync::Arc;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use opencode_hosted_adapters::sharing::objects::ObjectStore;
use opencode_hosted_adapters::sharing::service::ShareService;
use opencode_hosted_domain::sharing::share::ShareData;
use serde_json::{Value, json};

use crate::sharing::validate;

pub const DATA_CACHE_CONTROL: &str =
    "public, max-age=30, s-maxage=300, stale-while-revalidate=86400";

#[derive(Clone)]
pub struct EnterpriseState {
    pub store: Arc<dyn ObjectStore>,
    pub node_env_test: bool,
    /// `Resource.SUPPORT_API_KEY.value`.
    pub support_api_key: String,
}

impl EnterpriseState {
    fn service(&self) -> ShareService<'_> {
        ShareService {
            store: self.store.as_ref(),
            node_env_test: self.node_env_test,
        }
    }
}

pub fn routes() -> Router<EnterpriseState> {
    Router::new()
        .route("/api/doc", get(doc))
        .route("/api/share", post(create))
        .route("/api/share/{share_id}/sync", post(sync))
        .route("/api/share/{share_id}/data", get(data))
        .route("/api/share/{share_id}", delete(remove))
        .route("/api/support/actions/remove-share", delete(support_remove))
}

fn json_response(status: StatusCode, body: &Value) -> Response {
    let text = serde_json::to_string(body).unwrap_or_default();
    (status, [(header::CONTENT_TYPE, "application/json")], text).into_response()
}

fn internal_error() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        [(header::CONTENT_TYPE, "text/plain; charset=UTF-8")],
        "Internal Server Error",
    )
        .into_response()
}

/// hono `validator("json")`: non-JSON bodies are a bare 400 text, schema failures a JSON 400.
fn decode_json(headers: &HeaderMap, body: &Bytes) -> Result<Value, Box<Response>> {
    let is_json = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.to_ascii_lowercase().starts_with("application/json"));
    if !is_json {
        // Without a JSON content type hono validates an empty object.
        return Ok(json!({}));
    }
    serde_json::from_slice(body).map_err(|_| {
        // hono answers with a bare body and no content-type header.
        let mut response = Response::new(axum::body::Body::from("Malformed JSON in request body"));
        *response.status_mut() = StatusCode::BAD_REQUEST;
        Box::new(response)
    })
}

fn validated(
    headers: &HeaderMap,
    body: &Bytes,
    check: impl Fn(&Value) -> Result<(), Vec<Value>>,
) -> Result<Value, Box<Response>> {
    let value = decode_json(headers, body)?;
    check(&value).map_err(|issues| {
        Box::new(json_response(
            StatusCode::BAD_REQUEST,
            &validate::failure(&value, issues),
        ))
    })?;
    Ok(value)
}

async fn doc() -> Response {
    internal_error()
}

async fn create(State(state): State<EnterpriseState>, headers: HeaderMap, body: Bytes) -> Response {
    let value = match validated(&headers, &body, |v| validate::strings(v, &["sessionID"])) {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let session_id = value["sessionID"].as_str().unwrap_or_default();
    let Ok(share) = state.service().create(session_id).await else {
        return internal_error();
    };
    let header_str = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    let protocol = header_str("x-forwarded-proto")
        .or_else(|| header_str("x-forwarded-protocol"))
        .unwrap_or("https".into());
    let host = header_str("x-forwarded-host")
        .or_else(|| header_str("host"))
        .unwrap_or("undefined".into());
    json_response(
        StatusCode::OK,
        &json!({ "id": share.id, "secret": share.secret, "url": format!("{protocol}://{host}/share/{}", share.id) }),
    )
}

async fn sync(
    State(state): State<EnterpriseState>,
    Path(share_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let value = match validated(&headers, &body, validate::sync_body) {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let Ok(items) = serde_json::from_value::<Vec<ShareData>>(value["data"].clone()) else {
        return internal_error();
    };
    match state
        .service()
        .sync(
            &share_id,
            value["secret"].as_str().unwrap_or_default(),
            &items,
        )
        .await
    {
        Ok(()) => json_response(StatusCode::OK, &json!({})),
        Err(_) => internal_error(),
    }
}

async fn data(State(state): State<EnterpriseState>, Path(share_id): Path<String>) -> Response {
    match state.service().data(&share_id).await {
        Ok(items) => {
            let mut response = json_response(
                StatusCode::OK,
                &serde_json::to_value(items).unwrap_or_default(),
            );
            response.headers_mut().insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static(DATA_CACHE_CONTROL),
            );
            response
        }
        Err(_) => internal_error(),
    }
}

async fn remove(
    State(state): State<EnterpriseState>,
    Path(share_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let value = match validated(&headers, &body, |v| validate::strings(v, &["secret"])) {
        Ok(value) => value,
        Err(response) => return *response,
    };
    match state
        .service()
        .remove(&share_id, value["secret"].as_str().unwrap_or_default())
        .await
    {
        Ok(()) => json_response(StatusCode::OK, &json!({})),
        Err(_) => internal_error(),
    }
}

async fn support_remove(
    State(state): State<EnterpriseState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let actual = headers
        .get(header::AUTHORIZATION)
        .map(|v| v.as_bytes().to_vec())
        .unwrap_or_default();
    let expected = format!("Bearer {}", state.support_api_key).into_bytes();
    // `timingSafeEqual` after a length check (api route lines 145-148).
    let same = actual.len() == expected.len()
        && actual
            .iter()
            .zip(&expected)
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0;
    if !same {
        return json_response(
            StatusCode::UNAUTHORIZED,
            &json!({ "error": "Unauthorized" }),
        );
    }
    // `c.req.json().catch(() => undefined)` ignores the content type.
    let parsed: Option<Value> = serde_json::from_slice(&body).ok();
    let share_id = match validate::support_body(parsed.as_ref()) {
        Ok(id) => id,
        Err(issues) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                &json!({ "error": "Invalid request", "issues": issues }),
            );
        }
    };
    match state.service().remove_admin(&share_id).await {
        Ok(()) => json_response(
            StatusCode::OK,
            &json!({ "success": true, "message": "Share removed" }),
        ),
        Err(error) => json_response(
            StatusCode::BAD_REQUEST,
            &json!({ "error": error.to_string() }),
        ),
    }
}
