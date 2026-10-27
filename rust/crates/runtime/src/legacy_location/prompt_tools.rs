//! Purpose: bridge legacy prompt expansion to the real native registry and integrations.
//! Scope: Read, MCP resources, LSP symbols and sequential plugin hooks.
//! Invariants: resource errors propagate; image normalization reports genuine resizer absence.
//! Contract tests: runtime/tests/legacy_model.rs

use std::sync::Arc;

use opencode_integration_api::{language::LanguageService, mcp::McpService};
use opencode_llm_api::{ToolContent, ToolResultValue};
use opencode_session_api::{MessageID, SessionID};
use opencode_session_runtime::legacy::ports::{PortError, PromptTools, ReadContext, ReadOutput};
use opencode_session_runtime::legacy::types::Object;
use opencode_session_runtime::runner::ports::{SettleInput, ToolMaterializer};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::Lease;

pub struct Tools {
    pub lease: Arc<Lease>,
}

#[async_trait::async_trait]
impl PromptTools for Tools {
    async fn read(&self, args: Value, ctx: ReadContext) -> Result<ReadOutput, PortError> {
        let result = self
            .lease
            .legacy_materializer
            .materialize(&[])
            .settle(
                SettleInput {
                    session_id: SessionID::new(ctx.session_id).map_err(failed)?,
                    agent: ctx.agent.unwrap_or_else(|| "build".into()),
                    assistant_message_id: MessageID::new(ctx.message_id).map_err(failed)?,
                    call_id: opencode_session_runtime::legacy::types::part_id(),
                    name: "read".into(),
                    input: args,
                },
                CancellationToken::new(),
            )
            .await
            .map_err(failed)?;
        if let ToolResultValue::Error(error) = result.result {
            return Err(PortError::Failed(
                error
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| error.to_string()),
            ));
        }
        let output = match &result.result {
            ToolResultValue::Text(value) => value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| value.to_string()),
            _ => result
                .output
                .as_ref()
                .map(|output| {
                    output
                        .content
                        .iter()
                        .filter_map(|content| match content {
                            ToolContent::Text { text } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default(),
        };
        let attachments = result
            .output
            .into_iter()
            .flat_map(|output| output.content)
            .filter_map(|content| match content {
                ToolContent::File { uri, mime, name } => {
                    let mut part = Object::new();
                    part.insert("type".into(), json!("file"));
                    part.insert("url".into(), json!(uri));
                    part.insert("mime".into(), json!(mime));
                    if let Some(name) = name {
                        part.insert("filename".into(), json!(name));
                    }
                    Some(part)
                }
                _ => None,
            })
            .collect();
        Ok(ReadOutput {
            output,
            attachments,
        })
    }
    async fn read_resource(&self, client: &str, uri: &str) -> Result<Option<Value>, PortError> {
        self.lease
            .integrations
            .mcp
            .read_resource(client, uri)
            .await
            .map_err(failed)?
            .map(serde_json::to_value)
            .transpose()
            .map_err(failed)
    }
    async fn document_symbols(&self, uri: &str) -> Result<Vec<Value>, PortError> {
        self.lease
            .integrations
            .lsp
            .document_symbol(uri)
            .await
            .map_err(failed)?
            .into_iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()
            .map_err(failed)
    }
    async fn normalize_image(&self, _: &Object) -> Result<Option<Object>, PortError> {
        Ok(None)
    }
    async fn trigger(&self, name: &str, input: Value, output: Value) -> Result<Value, PortError> {
        let name = serde_json::from_value(json!(name)).map_err(failed)?;
        self.lease
            .integrations
            .trigger(name, &input, output)
            .await
            .map_err(failed)
    }
}

fn failed(error: impl std::fmt::Display) -> PortError {
    PortError::Failed(error.to_string())
}
