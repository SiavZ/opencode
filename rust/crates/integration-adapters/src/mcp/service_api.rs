//! Purpose: `McpService` trait implementation for `McpServiceImpl`, mirroring `MCP.Interface`
//! (baseline index.ts:164-200 and 591-970): status, instructions, tools, prompts, resources,
//! templates, add/connect/disconnect, call_tool, getPrompt/readResource and auth entry points.
//! Scope: thin trait adapter over service.rs and service_auth.rs.
//! Uses:
//! rust/crates/
//! |-- integration-adapters/src/mcp/catalog.rs
//! |-- integration-adapters/src/mcp/client.rs
//! |-- integration-adapters/src/mcp/service.rs
//! |-- integration-api/src/error.rs
//! `-- integration-api/src/mcp.rs
//! External:
//! |-- async_trait::async_trait
//! |-- catalog::prompts
//! |-- catalog::resource_templates
//! |-- catalog::resources
//! |-- catalog::tool_name
//! |-- catalog::translate_call_result
//! |-- serde::de::DeserializeOwned
//! |-- serde_json::Map
//! |-- serde_json::Value
//! |-- serde_json::from_value
//! |-- serde_json::json
//! `-- tracing::warn
//! Used by:
//! rust/crates/
//! `-- integration-adapters/src/mcp/mod.rs
//! Invariants: status lists static config keys then runtime-added keys, missing status reads
//! disabled; getPrompt/readResource swallow client errors as None like the baseline.
//! Contract tests: integration-adapters/tests/mcp_stdio.rs

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use opencode_integration_api::error::IntegrationError;
use opencode_integration_api::mcp::{
    AuthStatus, CallToolResult, GetPromptResult, McpService, McpTool, OAuthSetting, Prompt,
    ReadResourceResult, Resource, ResourceTemplate, ServerConfig, ServerInstructions, StartedAuth,
    Status,
};
use serde_json::{Map, Value, json};

use super::catalog;
use super::client::McpClient;
use super::service::{McpServiceImpl, enable, lock, rpc_to_integration};

fn typed<T: serde::de::DeserializeOwned>(m: BTreeMap<String, Value>) -> BTreeMap<String, T> {
    m.into_iter()
        .filter_map(|(k, v)| serde_json::from_value(v).ok().map(|t| (k, t)))
        .collect()
}

impl McpServiceImpl {
    pub async fn auth_inventory(&self) -> BTreeMap<String, Value> {
        self.0.store.all().await.into_iter().map(|(name, entry)| {
            (name, json!({"tokens":entry.tokens.is_some(),"clientInfo":entry.client_info.is_some()}))
        }).collect()
    }
    pub async fn resource_servers(&self) -> Vec<String> {
        self.ready().await;
        self.connected_clients(None)
            .into_iter()
            .filter_map(|(name, client)| client.has_capability("resources").then_some(name))
            .collect()
    }
}

#[async_trait]
impl McpService for McpServiceImpl {
    async fn status(&self) -> BTreeMap<String, Status> {
        self.ready().await;
        let s = lock(&self.0.state);
        self.0
            .static_config
            .keys()
            .chain(s.config.keys())
            .map(|k| {
                (
                    k.clone(),
                    s.status.get(k).cloned().unwrap_or(Status::Disabled),
                )
            })
            .collect()
    }

    async fn instructions(&self) -> Vec<ServerInstructions> {
        self.ready().await;
        let s = lock(&self.0.state);
        s.instructions
            .iter()
            .filter(|(n, _)| s.status.get(*n) == Some(&Status::Connected))
            .map(|(n, i)| ServerInstructions {
                name: n.clone(),
                instructions: i.clone(),
                tools: s
                    .defs
                    .get(n)
                    .into_iter()
                    .flatten()
                    .map(|d| catalog::tool_name(n, &d.name))
                    .collect(),
            })
            .collect()
    }

    async fn tools(&self) -> BTreeMap<String, McpTool> {
        self.ready().await;
        let clients = self.connected_clients(None);
        let mut out = BTreeMap::new();
        for (name, _) in clients {
            let defs = lock(&self.0.state).defs.get(&name).cloned();
            let Some(defs) = defs else {
                tracing::warn!(client = %name, "missing cached tools for connected server");
                continue;
            };
            let timeout_ms = self.request_timeout(&name);
            for def in defs {
                out.insert(
                    catalog::tool_name(&name, &def.name),
                    McpTool {
                        client: name.clone(),
                        def,
                        timeout_ms,
                    },
                );
            }
        }
        out
    }

    async fn prompts(&self) -> BTreeMap<String, Prompt> {
        typed(
            self.collect(None, "prompts", None, |c, t| async move {
                catalog::prompts(&c, t).await
            })
            .await,
        )
    }

    async fn resources(&self, client: Option<&str>) -> BTreeMap<String, Resource> {
        typed(
            self.collect(client, "resources", Some("uri"), |c, t| async move {
                catalog::resources(&c, t).await
            })
            .await,
        )
    }

    async fn resource_templates(&self, client: Option<&str>) -> BTreeMap<String, ResourceTemplate> {
        let list = |c: Arc<McpClient>, t| async move { catalog::resource_templates(&c, t).await };
        typed(
            self.collect(client, "resource templates", Some("uriTemplate"), list)
                .await,
        )
    }

    async fn add(
        &self,
        name: &str,
        config: ServerConfig,
    ) -> Result<BTreeMap<String, Status>, IntegrationError> {
        self.ready().await;
        lock(&self.0.state)
            .config
            .insert(name.into(), config.clone());
        self.create_and_store(name, config).await;
        Ok(lock(&self.0.state).status.clone())
    }

    async fn connect(&self, name: &str) -> Result<(), IntegrationError> {
        let config = self.require_config(name)?;
        self.ready().await;
        self.create_and_store(name, enable(config)).await;
        Ok(())
    }

    async fn disconnect(&self, name: &str) -> Result<(), IntegrationError> {
        self.require_config(name)?;
        self.ready().await;
        let (client, watcher) = {
            let mut s = lock(&self.0.state);
            s.defs.remove(name);
            s.instructions.remove(name);
            s.status.insert(name.into(), Status::Disabled);
            (s.clients.remove(name), s.watchers.remove(name))
        };
        if let Some(c) = client {
            c.close().await;
        }
        if let Some(w) = watcher {
            w.abort();
            let _ = w.await;
        }
        Ok(())
    }

    async fn call_tool(
        &self,
        client: &str,
        tool: &str,
        arguments: Map<String, Value>,
        timeout_ms: Option<u64>,
    ) -> Result<CallToolResult, IntegrationError> {
        self.ready().await;
        let (c, def) = {
            let s = lock(&self.0.state);
            let def = s
                .defs
                .get(client)
                .and_then(|d| d.iter().find(|d| d.name == tool).cloned());
            (s.clients.get(client).cloned(), def)
        };
        let Some(c) = c else {
            return Err(IntegrationError::NotFound {
                kind: "MCP client",
                name: client.into(),
            });
        };
        let timeout = timeout_ms.or_else(|| self.request_timeout(client));
        let result = c
            .call_tool(tool, Value::Object(arguments), timeout, None)
            .await
            .map_err(|e| match rpc_to_integration(client, e) {
                IntegrationError::Timeout { .. } => IntegrationError::Timeout {
                    ms: timeout.unwrap_or(super::client::SDK_DEFAULT_REQUEST_TIMEOUT_MS),
                },
                other => other,
            })?;
        catalog::translate_call_result(def.as_ref(), result).map_err(|message| {
            IntegrationError::Failed {
                name: client.into(),
                message,
            }
        })
    }

    async fn get_prompt(
        &self,
        client: &str,
        name: &str,
        arguments: Option<BTreeMap<String, String>>,
    ) -> Result<Option<GetPromptResult>, IntegrationError> {
        let mut params = json!({"name": name});
        if let Some(a) = arguments {
            params["arguments"] = json!(a);
        }
        Ok(self.with_client(client, "prompts/get", params).await)
    }

    async fn read_resource(
        &self,
        client: &str,
        uri: &str,
    ) -> Result<Option<ReadResourceResult>, IntegrationError> {
        Ok(self
            .with_client(client, "resources/read", json!({"uri": uri}))
            .await)
    }

    async fn start_auth(&self, name: &str) -> Result<StartedAuth, IntegrationError> {
        self.start_auth_flow(name).await.map(|r| r.started)
    }

    async fn authenticate(&self, name: &str) -> Result<Status, IntegrationError> {
        self.authenticate_flow(name, None).await
    }

    async fn finish_auth(&self, name: &str, code: &str) -> Result<Status, IntegrationError> {
        self.finish_auth_flow(name, code).await
    }

    async fn remove_auth(&self, name: &str) -> Result<(), IntegrationError> {
        self.remove_auth_flow(name).await
    }

    async fn supports_oauth(&self, name: &str) -> Result<bool, IntegrationError> {
        let config = self.require_config(name)?;
        Ok(matches!(config, ServerConfig::Remote(r)
            if !matches!(r.oauth, Some(OAuthSetting::Disabled(_)))))
    }

    async fn has_stored_tokens(&self, name: &str) -> bool {
        self.0
            .store
            .get(name)
            .await
            .is_some_and(|e| e.tokens.is_some())
    }

    async fn auth_status(&self, name: &str) -> AuthStatus {
        self.auth_status_flow(name).await
    }
}
