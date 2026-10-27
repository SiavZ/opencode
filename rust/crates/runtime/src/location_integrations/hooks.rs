//! Purpose: run ordered plugin tool hooks around actual registered native tool execution.
//! Scope: runner materialization wrapper, preserving captured registration identity and permissions.
//! Uses:
//! rust/crates/
//! |-- integration-api/src/plugin.rs
//! |-- runtime/src/location_integrations.rs
//! `-- session-runtime/src/runner/ports.rs
//! External:
//! |-- async_trait::async_trait
//! `-- tokio_util::sync::CancellationToken
//! Used by:
//! rust/crates/
//! `-- runtime/src/location.rs
//! Invariants: hook failures stop execution; cancellation interrupts a held hook; no tool is rematerialized.
//! Contract tests: runtime/tests/location_integrations.rs

use super::LocationIntegrations;
use async_trait::async_trait;
use opencode_integration_api::plugin::TriggerName;
use opencode_llm_api::{ToolContent, ToolDefinition};
use opencode_schema::permission::Rule;
use opencode_session_runtime::runner::ports::{
    MaterializedTools, RegistryMaterializer, SettleError, SettleInput, Settlement, ToolMaterializer,
};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub struct IntegrationMaterializer {
    pub inner: Arc<dyn ToolMaterializer>,
    pub integrations: Arc<LocationIntegrations>,
    pub source: Arc<RegistryMaterializer>,
}
impl ToolMaterializer for IntegrationMaterializer {
    fn materialize(&self, permissions: &[Rule]) -> Arc<dyn MaterializedTools> {
        Arc::new(Hooked {
            inner: self.inner.materialize(permissions),
            integrations: self.integrations.clone(),
            source: self.source.clone(),
        })
    }
}
struct Hooked {
    inner: Arc<dyn MaterializedTools>,
    integrations: Arc<LocationIntegrations>,
    source: Arc<RegistryMaterializer>,
}
#[async_trait]
impl MaterializedTools for Hooked {
    fn definitions(&self) -> Vec<ToolDefinition> {
        self.inner.definitions()
    }
    async fn settle(
        &self,
        mut input: SettleInput,
        cancel: CancellationToken,
    ) -> Result<Settlement, SettleError> {
        let hook_input =
            json!({"tool":input.name,"sessionID":input.session_id,"callID":input.call_id});
        let before = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(SettleError::Failed("tool hook interrupted".into())),
            result = self.integrations.trigger(TriggerName::ToolExecuteBefore,&hook_input,json!({"args":input.input})) => result.map_err(|error| SettleError::Failed(error.to_string()))?,
        };
        input.input = before
            .get("args")
            .cloned()
            .ok_or_else(|| SettleError::Failed("plugin before hook removed tool args".into()))?;
        let mut after_input = hook_input;
        after_input["args"] = input.input.clone();
        let context = opencode_tool_api::ToolContext {
            session_id: input.session_id.as_str().into(),
            agent: input.agent.clone(),
            assistant_message_id: input.assistant_message_id.as_str().into(),
            tool_call_id: input.call_id.clone(),
            cancel: cancel.clone(),
            authorizer: self.source.authorizer.clone(),
            progress: Arc::new(opencode_tool_api::NoProgress),
        };
        let mut settlement = self.inner.settle(input, cancel.clone()).await?;
        let Some(output) = settlement.output.as_mut() else {
            return Ok(settlement);
        };
        let original = output.clone();
        let original_text = output
            .content
            .iter()
            .filter_map(|content| match content {
                ToolContent::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        let mut envelope = if output.structured.is_object() {
            output.structured.clone()
        } else {
            json!({"value":output.structured})
        };
        envelope["output"] = json!(original_text);
        let after = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(SettleError::Failed("tool hook interrupted".into())),
            result = self.integrations.trigger(TriggerName::ToolExecuteAfter,&after_input,envelope) => result.map_err(|error| SettleError::Failed(error.to_string()))?,
        };
        if let Some(text) = after
            .get("output")
            .and_then(Value::as_str)
            .filter(|text| *text != original_text)
        {
            output
                .content
                .retain(|content| !matches!(content, ToolContent::Text { .. }));
            output
                .content
                .insert(0, ToolContent::Text { text: text.into() });
        }
        if let Some(map) = after.as_object() {
            for (key, value) in map.iter().filter(|(key, _)| key.as_str() != "output") {
                if let Some(structured) = output.structured.as_object_mut() {
                    structured.insert(key.clone(), value.clone());
                }
            }
        }
        if *output != original {
            let output_value = serde_json::to_value(&*output)
                .and_then(serde_json::from_value)
                .map_err(|error| SettleError::Failed(error.to_string()))?;
            let bounded = self
                .source
                .registry
                .bound(output_value, &context)
                .await
                .map_err(SettleError::Failed)?;
            *output = serde_json::to_value(bounded.output)
                .and_then(serde_json::from_value)
                .map_err(|error| SettleError::Failed(error.to_string()))?;
            settlement.output_paths.extend(bounded.output_paths);
            settlement.output_paths.sort();
            settlement.output_paths.dedup();
        }
        settlement.result = output.to_result_value();
        Ok(settlement)
    }
}
