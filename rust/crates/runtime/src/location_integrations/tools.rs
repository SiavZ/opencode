//! Purpose: register real MCP, plugin and skill tools into the shared Location tool registry.
//! Scope: adapters retaining the integration host for each materialized call.
//! Uses:
//! rust/crates/
//! |-- runtime/src/location_integrations.rs
//! `-- tool-api/src/lib.rs
//! External:
//! |-- async_trait::async_trait
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! `-- runtime/src/location_integrations.rs
//! Invariants: authorization precedes foreign execution, cancellation reaches plugin abort callbacks.
//! Contract tests: runtime/tests/location_integrations.rs

use super::LocationIntegrations;
use async_trait::async_trait;
use opencode_integration_api::{
    IntegrationError,
    mcp::{McpService, McpTool},
    plugin::CustomTool,
    plugin_abi,
};
use opencode_tool_api::{
    Registration, Tool, ToolContent, ToolContext, ToolFailure, ToolOutput, ToolRegistry,
};
use serde_json::{Value, json};
use std::sync::Arc;

pub async fn register(
    host: Arc<LocationIntegrations>,
    registry: &ToolRegistry,
) -> Result<Registration, IntegrationError> {
    let mut tools: Vec<(String, Arc<dyn Tool>)> = Vec::new();
    for plugin in &host.plugins {
        for tool in &plugin.tools {
            tools.push((
                tool.id.clone(),
                Arc::new(Plugin {
                    host: host.clone(),
                    plugin: plugin.clone(),
                    tool: tool.clone(),
                }),
            ));
        }
    }
    tools.push(("skill".into(), Arc::new(Skill(host))));
    registry
        .register(tools)
        .map_err(|error| IntegrationError::Validation(error.to_string()))
}

pub async fn mcp_tools(host: Arc<LocationIntegrations>) -> Vec<(String, Arc<dyn Tool>)> {
    let mut tools: Vec<_> = host
        .mcp
        .tools()
        .await
        .into_iter()
        .map(|(name, tool)| {
            (
                name.clone(),
                Arc::new(Mcp {
                    host: host.clone(),
                    name,
                    tool,
                }) as Arc<dyn Tool>,
            )
        })
        .collect();
    tools.extend(super::mcp_resources::tools(host).await);
    tools
}

pub fn signature(tools: &[(String, Arc<dyn Tool>)]) -> Value {
    json!(tools.iter().map(|(name,tool)| json!({"name":name,"description":tool.description(),"input":tool.input_schema(),"output":tool.output_schema()})).collect::<Vec<_>>())
}

struct Mcp {
    host: Arc<LocationIntegrations>,
    name: String,
    tool: McpTool,
}
#[async_trait]
impl Tool for Mcp {
    fn description(&self) -> &str {
        self.tool.def.description.as_deref().unwrap_or("")
    }
    fn input_schema(&self) -> Value {
        self.tool.def.input_schema.clone()
    }
    fn output_schema(&self) -> Option<Value> {
        self.tool.def.output_schema.clone()
    }
    async fn execute(
        &self,
        input: Value,
        context: &ToolContext,
    ) -> Result<ToolOutput, ToolFailure> {
        let arguments = input
            .as_object()
            .cloned()
            .ok_or_else(|| ToolFailure::invalid_input("expected an object"))?;
        context
            .authorize(&self.name, vec!["*".into()], Some(vec!["*".into()]), None)
            .await?;
        let result = tokio::select! {
            () = context.cancel.cancelled() => return Err(ToolFailure::new("cancelled")),
            result = self.host.mcp.call_tool(&self.tool.client, &self.tool.def.name, arguments, self.tool.timeout_ms) => result.map_err(failure)?,
        };
        let content = result["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|content| match content["type"].as_str() {
                Some("text") => Some(ToolContent::text(
                    content["text"].as_str().unwrap_or_default(),
                )),
                Some("image" | "audio") => Some(ToolContent::file_base64(
                    content["data"].as_str().unwrap_or_default(),
                    content["mimeType"]
                        .as_str()
                        .unwrap_or("application/octet-stream"),
                    None,
                )),
                Some("resource") => content["resource"]["text"].as_str().map(ToolContent::text),
                _ => None,
            })
            .collect();
        if result["isError"] == true {
            return Err(ToolFailure::new(result["content"].to_string()));
        }
        Ok(ToolOutput::new(
            result
                .get("structuredContent")
                .cloned()
                .unwrap_or(json!({})),
            content,
        ))
    }
}

struct Plugin {
    host: Arc<LocationIntegrations>,
    plugin: Arc<opencode_integration_adapters::plugins::Instance>,
    tool: CustomTool,
}
#[async_trait]
impl Tool for Plugin {
    fn description(&self) -> &str {
        &self.tool.description
    }
    fn input_schema(&self) -> Value {
        self.tool.parameters.clone()
    }
    async fn execute(
        &self,
        input: Value,
        context: &ToolContext,
    ) -> Result<ToolOutput, ToolFailure> {
        context
            .authorize(&self.tool.id, vec!["*".into()], None, None)
            .await?;
        let call_id = format!("{}:{}", self.plugin.handle, context.tool_call_id);
        self.host.callbacks.enter(call_id.clone(), context.clone());
        let result = tokio::select! {
            () = context.cancel.cancelled() => {
                let _ = self.plugin.host.notify(plugin_abi::TOOL_ABORT, json!({"callId":call_id})).await;
                Err(ToolFailure::new("cancelled"))
            }
            result = self.plugin.host.request(plugin_abi::TOOL_EXECUTE, json!({
                "handle":self.plugin.handle,"tool":self.tool.id,"args":input,"callId":call_id,
                "context":{"sessionID":context.session_id,"messageID":context.assistant_message_id,"agent":context.agent,"directory":self.host.directory,"worktree":self.host.project}
            })) => result.map_err(failure),
        };
        self.host.callbacks.leave(&call_id);
        let result = result?;
        let text = result
            .as_str()
            .or_else(|| result["output"].as_str())
            .ok_or_else(|| {
                ToolFailure::new("plugin tool result must be text or structured output")
            })?;
        Ok(ToolOutput::new(
            result.clone(),
            vec![ToolContent::text(text)],
        ))
    }
}

struct Skill(Arc<LocationIntegrations>);
#[async_trait]
impl Tool for Skill {
    fn description(&self) -> &str {
        "Load a skill to get detailed instructions for a specific task."
    }
    fn input_schema(&self) -> Value {
        json!({"type":"object","properties":{"name":{"type":"string"}},"required":["name"],"additionalProperties":false})
    }
    async fn execute(
        &self,
        input: Value,
        context: &ToolContext,
    ) -> Result<ToolOutput, ToolFailure> {
        let name = input["name"]
            .as_str()
            .ok_or_else(|| ToolFailure::invalid_input("name is required"))?;
        context
            .authorize("skill", vec![name.into()], Some(vec![name.into()]), None)
            .await?;
        let skill = self
            .0
            .resources
            .skills
            .iter()
            .find(|skill| skill["name"] == name)
            .ok_or_else(|| ToolFailure::new(format!("Skill not found: {name}")))?;
        let text = format!(
            "<skill_content name=\"{name}\">\n{}\n\nBase directory for this skill: {}\n</skill_content>",
            skill["content"].as_str().unwrap_or_default(),
            std::path::Path::new(skill["location"].as_str().unwrap_or_default())
                .parent()
                .unwrap_or(std::path::Path::new("/"))
                .display()
        );
        Ok(ToolOutput::new(
            skill.clone(),
            vec![ToolContent::text(text)],
        ))
    }
}
fn failure(error: IntegrationError) -> ToolFailure {
    ToolFailure::new(error.to_string())
}
