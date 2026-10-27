//! Purpose: expose connected MCP resources through real authorized Location tools.
//! Scope: resource listing, templates and reads, with bounded attachment conversion.
//! Uses:
//! rust/crates/
//! |-- integration-api/src/mcp.rs
//! |-- runtime/src/location_integrations.rs
//! `-- tool-api/src/lib.rs
//! External:
//! |-- async_trait::async_trait
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! `-- runtime/src/location_integrations/tools.rs
//! Invariants: read authorization precedes server access; only supported small blobs become files.
//! Contract tests: runtime/tests/location_integrations.rs

use super::LocationIntegrations;
use async_trait::async_trait;
use opencode_integration_api::mcp::McpService;
use opencode_tool_api::{Tool, ToolContent, ToolContext, ToolFailure, ToolOutput};
use serde_json::{Value, json};
use std::sync::Arc;

pub async fn tools(host: Arc<LocationIntegrations>) -> Vec<(String, Arc<dyn Tool>)> {
    if host.mcp.resource_servers().await.is_empty() {
        return vec![];
    }
    [
        "list_mcp_resources",
        "list_mcp_resource_templates",
        "read_mcp_resource",
    ]
    .into_iter()
    .map(|name| {
        (
            name.into(),
            Arc::new(ResourceTool {
                host: host.clone(),
                name,
            }) as Arc<dyn Tool>,
        )
    })
    .collect()
}
struct ResourceTool {
    host: Arc<LocationIntegrations>,
    name: &'static str,
}
#[async_trait]
impl Tool for ResourceTool {
    fn description(&self) -> &str {
        match self.name {
            "read_mcp_resource" => {
                "Read a resource from a connected MCP server using its server name and exact resource URI."
            }
            "list_mcp_resource_templates" => {
                "List parameterized resource templates from connected MCP servers."
            }
            _ => "List resources available from connected MCP servers.",
        }
    }
    fn input_schema(&self) -> Value {
        if self.name == "read_mcp_resource" {
            return json!({"type":"object","properties":{"server":{"type":"string"},"uri":{"type":"string"}},"required":["server","uri"],"additionalProperties":false});
        }
        json!({"type":"object","properties":{"server":{"type":"string"}},"additionalProperties":false})
    }
    async fn execute(
        &self,
        input: Value,
        context: &ToolContext,
    ) -> Result<ToolOutput, ToolFailure> {
        let server = input
            .get("server")
            .map(|value| {
                value
                    .as_str()
                    .filter(|value| !value.trim().is_empty())
                    .ok_or_else(|| ToolFailure::invalid_input("server must be a nonempty string"))
            })
            .transpose()?;
        let servers = self.host.mcp.resource_servers().await;
        if let Some(server) = server
            && !servers.iter().any(|name| name == server)
        {
            return Err(ToolFailure::new(format!(
                "MCP server \"{server}\" is not connected or does not support resources"
            )));
        }
        if self.name == "read_mcp_resource" {
            let server = server.ok_or_else(|| ToolFailure::invalid_input("server is required"))?;
            let uri = input["uri"]
                .as_str()
                .filter(|uri| !uri.trim().is_empty())
                .ok_or_else(|| ToolFailure::invalid_input("uri is required"))?;
            context
                .authorize(
                    "read",
                    vec![format!("mcp:{server}:{uri}")],
                    Some(vec![format!("mcp:{server}:*")]),
                    Some(serde_json::Map::from_iter([
                        ("server".into(), json!(server)),
                        ("uri".into(), json!(uri)),
                    ])),
                )
                .await?;
            let result = tokio::select! {
                () = context.cancel.cancelled()=>return Err(ToolFailure::new("cancelled")),
                result=self.host.mcp.read_resource(server,uri)=>result.map_err(|error| ToolFailure::new(error.to_string()))?,
            }.ok_or_else(|| ToolFailure::new(format!("Failed to read MCP resource: {server}/{uri}")))?;
            return Ok(format_resource(server, uri, &result));
        }
        let patterns = server
            .map(|server| vec![format!("mcp:{server}:*")])
            .unwrap_or_else(|| {
                servers
                    .iter()
                    .map(|server| format!("mcp:{server}:*"))
                    .collect()
            });
        context
            .authorize(
                "read",
                patterns.clone(),
                Some(patterns),
                Some(serde_json::Map::from_iter([(
                    "server".into(),
                    json!(server),
                )])),
            )
            .await?;
        let values = tokio::select! {
            ()=context.cancel.cancelled()=>return Err(ToolFailure::new("cancelled")),
            result=async {
                if self.name == "list_mcp_resource_templates" { return serde_json::to_value(self.host.mcp.resource_templates(server).await.into_values().collect::<Vec<_>>()); }
                serde_json::to_value(self.host.mcp.resources(server).await.into_values().collect::<Vec<_>>())
            }=>result.map_err(|error| ToolFailure::new(error.to_string()))?,
        };
        let text = serde_json::to_string_pretty(&values)
            .map_err(|error| ToolFailure::new(error.to_string()))?;
        Ok(ToolOutput::new(
            json!({"title":"MCP resources","metadata":{"count":values.as_array().map_or(0,Vec::len),"servers":servers,"server":server},"resources":values}),
            vec![ToolContent::text(text)],
        ))
    }
}
fn format_resource(server: &str, uri: &str, result: &Value) -> ToolOutput {
    let items: Vec<_> = match result.get("contents") {
        Some(Value::Array(items)) => items.iter().collect(),
        Some(item) => vec![item],
        None => vec![],
    };
    let mut text = Vec::new();
    let mut files = Vec::new();
    for item in &items {
        let uri = item["uri"].as_str().unwrap_or(uri);
        let mime = item["mimeType"]
            .as_str()
            .unwrap_or("application/octet-stream");
        if let Some(content) = item["text"].as_str() {
            text.push(format!("Resource: {uri}\nMIME: {mime}\n{content}"));
            continue;
        }
        if let Some(blob) = item["blob"].as_str() {
            let size = (blob
                .bytes()
                .filter(|byte| !byte.is_ascii_whitespace())
                .count()
                .saturating_mul(3)
                / 4)
            .saturating_sub(
                blob.chars()
                    .rev()
                    .take_while(|value| *value == '=')
                    .count()
                    .min(2),
            );
            let supported = matches!(
                mime,
                "application/pdf" | "image/gif" | "image/jpeg" | "image/png" | "image/webp"
            );
            if !supported || size > 10 * 1024 * 1024 {
                text.push(format!(
                    "[Binary MCP resource omitted: {uri} ({mime}, {size} B)]"
                ));
                continue;
            }
            text.push(format!("[Binary MCP resource attached: {uri} ({mime})]"));
            files.push(ToolContent::file_base64(blob, mime, Some(uri.into())));
            continue;
        }
        text.push(format!(
            "[MCP resource content without text or blob: {uri}]"
        ));
    }
    let text = if text.is_empty() {
        format!("MCP resource {uri} from {server} returned no contents.")
    } else {
        text.join("\n\n")
    };
    let metadata =
        json!({"server":server,"uri":uri,"contents":items.len(),"attachments":files.len()});
    let mut content = vec![ToolContent::text(text)];
    content.extend(files);
    ToolOutput::new(
        json!({"title":format!("MCP resource: {uri}"),"metadata":metadata}),
        content,
    )
}
