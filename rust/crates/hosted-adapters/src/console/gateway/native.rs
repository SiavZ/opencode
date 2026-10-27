//! Purpose: forward native legacy provider requests using the baseline format helpers.
//! Scope: upstream URL, credential headers, metadata placeholders and streamed request transport.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/formats.rs
//! `-- hosted-adapters/src/console/gateway.rs
//! External:
//! |-- http::{HeaderMap, HeaderName, HeaderValue}
//! `-- serde_json::Value
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/zen/legacy.rs
//! Invariants: external providers never receive internal session metadata or caller authorization.
//! Contract tests: hosted-adapters/tests/console_gateway_stream.rs

use super::GatewayError;
use crate::console::formats::ProviderHelper;
use http::{HeaderMap, HeaderName, HeaderValue};
use serde_json::Value;

pub struct ProviderRequest<'a> {
    pub provider: &'a Value,
    pub headers: &'a HeaderMap,
    pub helper: &'a ProviderHelper,
    pub sticky_id: &'a str,
    pub model: &'a str,
    pub ip: &'a str,
    pub workspace: Option<&'a str>,
    pub billing_source: &'a str,
    pub lite: bool,
    pub google_stream: bool,
    pub body: reqwest::Body,
}

pub async fn forward(input: ProviderRequest<'_>) -> Result<reqwest::Response, GatewayError> {
    let mut headers = input.headers.clone();
    input
        .helper
        .modify_headers(
            &mut headers,
            input.provider["apiKey"].as_str().unwrap_or_default(),
            input.sticky_id,
        )
        .map_err(|_| GatewayError::Configuration)?;
    let session = input
        .headers
        .get("x-opencode-session")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let org = input.workspace.map(|id| id.replace("wrk_", "org_"));
    if let Some(modifiers) = input.provider["headerModifier"].as_object() {
        for (name, value) in modifiers {
            let Some(value) = value.as_str() else {
                return Err(GatewayError::Configuration);
            };
            let value = match value {
                "$ip" => Some(input.ip),
                "$caller" => Some(input.sticky_id),
                "$session" => Some(session),
                "$model" => Some(input.model),
                "$workspace" => input.workspace,
                "$org" => org.as_deref(),
                "$request" => Some(
                    input
                        .headers
                        .get("x-opencode-request")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or_default(),
                ),
                "$client" => Some(
                    input
                        .headers
                        .get("x-opencode-client")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or_default(),
                ),
                "$project" => Some(
                    input
                        .headers
                        .get("x-opencode-project")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or_default(),
                ),
                _ => Some(value),
            };
            if let Some(value) = value {
                headers.insert(
                    HeaderName::from_bytes(name.as_bytes())
                        .map_err(|_| GatewayError::Configuration)?,
                    HeaderValue::from_str(value).map_err(|_| GatewayError::Configuration)?,
                );
            }
        }
    }
    let id = input.provider["id"].as_str().unwrap_or_default();
    let internal = ["console.", "console-go.", "inf.", "inf-go."]
        .iter()
        .any(|prefix| id.starts_with(prefix));
    if internal {
        headers.insert(
            "x-zen-model",
            HeaderValue::from_str(input.model).map_err(|_| GatewayError::Configuration)?,
        );
        if input.lite {
            headers.insert(
                "x-zen-billing-source",
                HeaderValue::from_static(if input.billing_source == "lite" {
                    "go"
                } else {
                    "credit"
                }),
            );
        }
    }
    for name in ["host", "content-length"] {
        headers.remove(name);
    }
    if !internal {
        for name in [
            "x-opencode-session",
            "x-opencode-project",
            "x-opencode-client",
            "x-opencode-request",
            "x-zen-model",
            "x-zen-billing-source",
        ] {
            headers.remove(name);
        }
    }
    let api = input.provider["api"]
        .as_str()
        .ok_or(GatewayError::Configuration)?;
    let url = input.helper.modify_url(api, input.google_stream);
    let client = reqwest::Client::builder().build()?;
    Ok(client
        .post(url)
        .headers(headers)
        .body(input.body)
        .send()
        .await?)
}
