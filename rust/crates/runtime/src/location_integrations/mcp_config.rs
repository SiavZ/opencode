//! Purpose: lower ordered canonical MCP config into native transport options without losing deadlines.
//! Scope: current {servers,timeout} and legacy flat compatibility at the runtime boundary.
//! Uses:
//! rust/crates/
//! |-- config/src/current.rs
//! `-- integration-adapters/src/mcp/service.rs
//! External:
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! `-- runtime/src/location_integrations.rs
//! Invariants: disabled is inverted exactly once; OAuth keys retain their values; startup and requests stay distinct.
//! Contract tests: runtime/tests/location_integrations.rs

use opencode_config::CurrentConfig;
use opencode_integration_adapters::mcp::McpOptions;
use opencode_integration_api::IntegrationError;
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};

pub fn options(
    config: &CurrentConfig,
    directory: &Path,
    data: &Path,
) -> Result<McpOptions, IntegrationError> {
    let mut entries = BTreeMap::new();
    let mut startup = None;
    let mut request = None;
    for document in config.documents() {
        let Some(mcp) = document.info.get("mcp").and_then(Value::as_object) else {
            continue;
        };
        if let Some(timeout) = mcp.get("timeout") {
            startup = timeout.get("startup").and_then(Value::as_u64).or(startup);
            request = timeout.get("request").and_then(Value::as_u64).or(request);
        }
        let servers = mcp.get("servers").and_then(Value::as_object).unwrap_or(mcp);
        for (name, server) in servers
            .iter()
            .filter(|(_, server)| server.get("type").is_some())
        {
            entries.insert(name.clone(), server.clone());
        }
    }
    let mut starts = BTreeMap::new();
    let servers = entries
        .into_iter()
        .map(|(name, mut value)| {
            if let Some(timeout) = value.get("timeout").cloned().filter(Value::is_object) {
                starts.insert(
                    name.clone(),
                    timeout
                        .get("startup")
                        .and_then(Value::as_u64)
                        .or(startup)
                        .unwrap_or(30_000),
                );
                value["timeout"] = timeout.get("request").cloned().unwrap_or(Value::Null);
                if value["timeout"].is_null() {
                    value
                        .as_object_mut()
                        .expect("server object")
                        .remove("timeout");
                }
            } else {
                starts.insert(name.clone(), startup.unwrap_or(30_000));
            }
            if value.get("timeout").is_none()
                && let Some(request) = request
            {
                value["timeout"] = json!(request);
            }
            if let Some(disabled) = value.get("disabled").and_then(Value::as_bool) {
                value["enabled"] = json!(!disabled);
            }
            if let Some(oauth) = value.get_mut("oauth").and_then(Value::as_object_mut) {
                for (from, to) in [
                    ("client_id", "clientId"),
                    ("client_secret", "clientSecret"),
                    ("callback_port", "callbackPort"),
                    ("redirect_uri", "redirectUri"),
                ] {
                    if let Some(value) = oauth.remove(from) {
                        oauth.insert(to.into(), value);
                    }
                }
            }
            serde_json::from_value(value)
                .map(|value| (name, value))
                .map_err(|error| IntegrationError::Validation(error.to_string()))
        })
        .collect::<Result<_, _>>()?;
    let mut options = McpOptions::new(directory.into(), data.into(), servers);
    options.default_timeout_ms = request.or_else(|| {
        config
            .latest("experimental")
            .and_then(|value| value.get("mcp_timeout"))
            .and_then(Value::as_u64)
    });
    options.startup_timeouts = starts;
    Ok(options)
}
