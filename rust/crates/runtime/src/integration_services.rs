//! Purpose: serve legacy MCP/LSP and current skill/reference routes from actual Location integrations.
//! Scope: request adapter, acquiring the same Location bundle as Session execution.
//! Uses:
//! rust/crates/
//! |-- integration-api/src/mcp.rs
//! |-- runtime/src/location.rs
//! `-- server/src/services/legacy.rs
//! External:
//! |-- async_trait::async_trait
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! `-- runtime/src/process.rs
//! Invariants: unknown names and unsupported OAuth remain observable typed HTTP errors.
//! Contract tests: runtime/tests/location_integrations.rs

use crate::location::Locations;
use async_trait::async_trait;
use opencode_integration_api::{IntegrationError, language::LanguageService, mcp::McpService};
use opencode_schema::{WorkspaceID, location::Ref, values::AbsolutePath};
use opencode_server::{
    middleware::legacy_routing::RouteContext,
    services::legacy::{LegacyError, LegacySessions},
};
use serde_json::{Map, Value, json};

pub struct IntegrationServices {
    pub locations: Locations,
}

#[async_trait]
impl LegacySessions for IntegrationServices {
    async fn handle(
        &self,
        operation: &str,
        context: &RouteContext,
        params: &Map<String, Value>,
        query: &Map<String, Value>,
        payload: Option<Value>,
    ) -> Result<Value, LegacyError> {
        let location = self
            .locations
            .get(&Ref {
                directory: AbsolutePath::new(context.directory.clone()),
                workspace_id: context
                    .workspace_id
                    .as_deref()
                    .map(WorkspaceID::new)
                    .transpose()
                    .map_err(|error| LegacyError::BadRequest(error.to_string()))?,
            })
            .await
            .map_err(|error| LegacyError::Internal(error.to_string()))?;
        let integrations = &location.integrations;
        let mcp = integrations.mcp.as_ref();
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let payload = payload.unwrap_or(Value::Null);
        let result = match operation {
            "mcp.cli.list" => {
                let statuses = mcp.status().await;
                let mut stored = Map::new();
                let mut auth = Map::new();
                for name in statuses.keys() {
                    stored.insert(name.clone(), json!(mcp.has_stored_tokens(name).await));
                    auth.insert(
                        name.clone(),
                        serde_json::to_value(mcp.auth_status(name).await)
                            .map_err(|error| LegacyError::Internal(error.to_string()))?,
                    );
                }
                return Ok(json!({"statuses":statuses,"stored":stored,"auth":auth}));
            }
            "mcp.auth.list" => serde_json::to_value(mcp.auth_inventory().await),
            "mcp.auth.status" => {
                mcp.status().await;
                return serde_json::to_value(mcp.auth_status(name).await)
                    .map_err(|error| LegacyError::Internal(error.to_string()));
            }
            "mcp.status" => serde_json::to_value(mcp.status().await),
            "mcp.add" => {
                let name = payload["name"]
                    .as_str()
                    .ok_or_else(|| LegacyError::BadRequest("name is required".into()))?;
                let config = serde_json::from_value(payload["config"].clone())
                    .map_err(|error| LegacyError::BadRequest(error.to_string()))?;
                let status = mcp.add(name, config).await.map_err(integration_error)?;
                integrations
                    .refresh_mcp()
                    .await
                    .map_err(integration_error)?;
                serde_json::to_value(status)
            }
            "mcp.connect" => {
                mcp.connect(name).await.map_err(integration_error)?;
                integrations
                    .refresh_mcp()
                    .await
                    .map_err(integration_error)?;
                return Ok(json!(true));
            }
            "mcp.disconnect" => {
                mcp.disconnect(name).await.map_err(integration_error)?;
                integrations
                    .refresh_mcp()
                    .await
                    .map_err(integration_error)?;
                return Ok(json!(true));
            }
            "mcp.auth.remove" => {
                if !mcp.status().await.contains_key(name)
                    && !mcp.auth_inventory().await.contains_key(name)
                {
                    return Err(integration_error(IntegrationError::NotFound {
                        kind: "MCP server",
                        name: name.into(),
                    }));
                }
                mcp.remove_auth(name).await.map_err(integration_error)?;
                return Ok(json!({"success":true}));
            }
            "mcp.auth.start" | "mcp.auth.authenticate" => {
                if !mcp.supports_oauth(name).await.map_err(integration_error)? {
                    return Err(LegacyError::Response {
                        status: 400,
                        body: json!({"error":format!("MCP server {name} does not support OAuth")}),
                    });
                }
                if operation == "mcp.auth.authenticate" {
                    serde_json::to_value(mcp.authenticate(name).await.map_err(integration_error)?)
                } else {
                    let auth = mcp.start_auth(name).await.map_err(integration_error)?;
                    return Ok(
                        json!({"authorizationUrl":auth.authorization_url,"oauthState":auth.oauth_state}),
                    );
                }
            }
            "mcp.auth.callback" => serde_json::to_value(
                mcp.finish_auth(
                    name,
                    payload["code"]
                        .as_str()
                        .ok_or_else(|| LegacyError::BadRequest("code is required".into()))?,
                )
                .await
                .map_err(integration_error)?,
            ),
            "lsp.status" => serde_json::to_value(integrations.lsp.status().await),
            "app.skills" | "v2.skill.list" => {
                if integrations
                    .resources
                    .gaps
                    .iter()
                    .any(|gap| gap.starts_with("skill URL"))
                {
                    return Err(LegacyError::Unavailable(
                        "remote skill discovery is not yet ported".into(),
                    ));
                }
                return Ok(Value::Array(if operation == "app.skills" {
                    integrations.resources.legacy_skills()
                } else {
                    integrations.resources.skills.clone()
                }));
            }
            "v2.reference.list" => {
                if integrations
                    .resources
                    .gaps
                    .iter()
                    .any(|gap| gap.starts_with("reference repository"))
                {
                    return Err(LegacyError::Unavailable(
                        "reference repository materialization is not yet ported".into(),
                    ));
                }
                return Ok(Value::Array(integrations.resources.references.clone()));
            }
            "experimental.resource.list" => serde_json::to_value(
                mcp.resources(query.get("client").and_then(Value::as_str))
                    .await,
            ),
            _ => {
                return Err(LegacyError::Unavailable(format!(
                    "Integration operation not implemented: {operation}"
                )));
            }
        };
        result.map_err(|error| LegacyError::Internal(error.to_string()))
    }
}
fn integration_error(error: IntegrationError) -> LegacyError {
    match error {
        IntegrationError::NotFound { name, .. } => LegacyError::Response {
            status: 404,
            body: json!({"name":"McpServerNotFoundError","data":{"name":name,"message":format!("MCP server not found: {name}")}}),
        },
        IntegrationError::Validation(message) => LegacyError::BadRequest(message),
        other => LegacyError::Internal(other.to_string()),
    }
}
