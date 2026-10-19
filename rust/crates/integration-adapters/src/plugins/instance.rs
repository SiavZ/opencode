//! Purpose: implement plugin hook and custom tool calls over the native bridge.
//! Scope: one loaded plugin handle.
//! Uses:
//! rust/crates/
//! |-- integration-adapters/src/plugins/mod.rs
//! `-- integration-api/src/plugin.rs
//! External:
//! |-- async_trait::async_trait
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! `-- integration-adapters/src/plugins/mod.rs
//! Invariants: hook output is threaded by Rust and tool schemas preserve bridge failures.
//! Contract tests: integration-adapters/tests/plugin_host.rs

use super::Host;
use async_trait::async_trait;
use opencode_integration_api::IntegrationError;
use opencode_integration_api::plugin::{
    CustomTool, HookCapabilities, PluginId, PluginInstance, ToolContext, ToolResult, TriggerName,
};
use opencode_integration_api::plugin_abi::{self, ArgsShape, InitResult};
use serde_json::{Value, json};
use std::sync::Arc;

pub struct Instance {
    pub host: Arc<Host>,
    pub handle: u64,
    pub tools: Vec<CustomTool>,
    id: PluginId,
    capabilities: HookCapabilities,
}

impl Instance {
    pub(super) fn new(
        host: Arc<Host>,
        id: PluginId,
        result: InitResult,
    ) -> Result<Self, IntegrationError> {
        let tools = result
            .hooks
            .tools
            .into_iter()
            .map(|tool| {
                let parameters = match tool.args {
                    ArgsShape::Zod { schema } => schema,
                    ArgsShape::Legacy { entries } => json!({"type":"object","properties":entries}),
                    ArgsShape::Error { message } => {
                        return Err(IntegrationError::Validation(message));
                    }
                };
                Ok(CustomTool {
                    id: tool.id,
                    description: tool.description,
                    parameters,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let capabilities = HookCapabilities {
            triggers: result
                .hooks
                .triggers
                .iter()
                .filter_map(|name| TriggerName::parse(name))
                .collect(),
            event: result.hooks.event,
            config: result.hooks.config,
            dispose: result.hooks.dispose,
            tools: tools.iter().map(|tool| tool.id.clone()).collect(),
            auth_provider: result.hooks.auth_provider,
            provider: result.hooks.provider,
        };
        Ok(Self {
            host,
            handle: result.handle,
            tools,
            id,
            capabilities,
        })
    }
}

#[async_trait]
impl PluginInstance for Instance {
    fn id(&self) -> &PluginId {
        &self.id
    }
    fn capabilities(&self) -> &HookCapabilities {
        &self.capabilities
    }
    async fn trigger(
        &self,
        name: TriggerName,
        input: &Value,
        output: Value,
    ) -> Result<Value, IntegrationError> {
        let result = self
            .host
            .request(
                plugin_abi::HOOK_TRIGGER,
                json!({"handle":self.handle,"name":name.as_str(),"input":input,"output":output}),
            )
            .await?;
        Ok(result["output"].clone())
    }
    async fn event(&self, event: &Value) -> Result<(), IntegrationError> {
        self.host
            .notify(
                plugin_abi::HOOK_EVENT,
                json!({"handle":self.handle,"event":event}),
            )
            .await
    }
    async fn config(&self, config: &Value) -> Result<(), IntegrationError> {
        self.host
            .request(
                plugin_abi::HOOK_CONFIG,
                json!({"handle":self.handle,"config":config}),
            )
            .await
            .map(|_| ())
    }
    async fn execute_tool(
        &self,
        tool: &str,
        args: Value,
        context: ToolContext,
    ) -> Result<ToolResult, IntegrationError> {
        let value = self.host.request(plugin_abi::TOOL_EXECUTE, json!({"handle":self.handle,"tool":tool,"args":args,"callId":format!("{}:{}",context.message_id,tool),"context":context})).await?;
        serde_json::from_value(value)
            .map_err(|error| IntegrationError::Validation(error.to_string()))
    }
    async fn dispose(&self) -> Result<(), IntegrationError> {
        self.host
            .request(plugin_abi::HOOK_DISPOSE, json!({"handle":self.handle}))
            .await
            .map(|_| ())
    }
}
