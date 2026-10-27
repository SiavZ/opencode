//! Purpose: authenticate Honeycomb trigger webhooks and forward incident summaries to Discord.
//! Scope: HTTP webhook handler and bounded single-attempt outbound request.
//! Uses:
//! rust/bins/
//! `-- hosted/src/console/state.rs
//! rust/crates/
//! `-- hosted-adapters/src/console/support.rs
//! External:
//! |-- axum::{Json, body::Bytes, extract::State, http::{HeaderMap, StatusCode}}
//! `-- serde_json::{Value, json}
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/support.rs
//! Invariants: token is timing-safe; resolved alerts do not deliver; logs never include tokens/payloads.
//! Contract tests: hosted/src/console/support/tests.rs

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use opencode_hosted_adapters::console::support::secure_equal;
use serde_json::{Value, json};

use super::super::ConsoleState;
use super::actions::Reply;

pub(super) async fn webhook(
    State(state): State<ConsoleState>,
    headers: HeaderMap,
    body: Bytes,
) -> Reply {
    let reply = |status, message| (status, Json(json!({"message":message})));
    if !state
        .config
        .support
        .honeycomb_secret
        .as_deref()
        .filter(|v| !v.is_empty())
        .is_some_and(|secret| {
            secure_equal(
                headers
                    .get("x-honeycomb-webhook-token")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or(""),
                secret,
            )
        })
    {
        return reply(StatusCode::UNAUTHORIZED, "invalid token");
    }
    let payload: Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => return reply(StatusCode::BAD_REQUEST, "invalid payload"),
    };
    let Some(content) = discord_content(&payload) else {
        return reply(StatusCode::BAD_REQUEST, "invalid payload");
    };
    if payload.get("status").and_then(Value::as_str) != Some("TRIGGERED") {
        return reply(StatusCode::OK, "ignored");
    }
    let Some(url) = state.config.support.discord_webhook_url.as_deref() else {
        return reply(StatusCode::BAD_GATEWAY, "discord webhook failed");
    };
    let client = match reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(30))
        .build()
    {
        Ok(client) => client,
        Err(_) => return reply(StatusCode::BAD_GATEWAY, "discord webhook failed"),
    };
    match client.post(url).json(&json!({"content":content,"allowed_mentions":{"roles":["1520924666359713863"]},"flags":4})).send().await {
        Ok(response) if response.status().is_success() => reply(StatusCode::OK,"sent"),
        _ => reply(StatusCode::BAD_GATEWAY,"discord webhook failed"),
    }
}

pub(super) fn discord_content(payload: &Value) -> Option<String> {
    let kind = payload.get("type")?.as_str()?;
    if ![
        "custom",
        "model_http_errors",
        "model_low_tps",
        "provider_http_errors",
    ]
    .contains(&kind)
    {
        return None;
    }
    for key in ["name", "status"] {
        if payload.get(key).is_some_and(|value| !value.is_string()) {
            return None;
        }
    }
    if payload
        .get("isTest")
        .is_some_and(|value| !value.is_boolean())
    {
        return None;
    }
    let name = payload
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("Honeycomb alert");
    let test = if payload
        .get("isTest")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        "[TEST] "
    } else {
        ""
    };
    let mut lines = vec![format!(
        "[**{test}{name}**]({})",
        payload.get("url")?.as_str()?
    )];
    if kind != "custom" {
        for item in payload.get("groups")?.as_array()? {
            let number = match item.get("result") {
                None | Some(Value::Null) => None,
                Some(Value::Number(value)) => value.as_f64(),
                Some(Value::String(value)) => {
                    if value.trim().is_empty() {
                        Some(0.0)
                    } else {
                        let value = value.trim();
                        [
                            ("0x", 16),
                            ("0X", 16),
                            ("0b", 2),
                            ("0B", 2),
                            ("0o", 8),
                            ("0O", 8),
                        ]
                        .iter()
                        .find_map(|(prefix, base)| {
                            value.strip_prefix(prefix).map(|digits| {
                                u64::from_str_radix(digits, *base).ok().map(|v| v as f64)
                            })
                        })
                        .unwrap_or_else(|| value.parse().ok())
                    }
                }
                _ => return None,
            }
            .filter(|value| value.is_finite());
            for group in item.get("group")?.as_array()? {
                group.get("key")?.as_str()?;
                let value = group.get("value")?.as_str()?;
                let suffix = number
                    .map(|number| {
                        let rounded = if kind == "model_low_tps" {
                            (number + 0.5).floor()
                        } else {
                            (number * 100.0 + 0.5).floor()
                        };
                        let rounded = if rounded == 0.0 { 0.0 } else { rounded };
                        if kind == "model_low_tps" {
                            format!(" ({rounded:.0} TPS)")
                        } else {
                            format!(" ({rounded:.0}% errors)")
                        }
                    })
                    .unwrap_or_default();
                lines.push(format!("- {value}{suffix}"));
            }
        }
    }
    lines.push(String::new());
    lines.push("<@&1520924666359713863>".into());
    Some(lines.join("\n"))
}
