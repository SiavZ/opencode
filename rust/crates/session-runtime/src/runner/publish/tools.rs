//! Purpose: publish-llm-event.ts tool lifecycle (tool input start/end, `tool.called`, result and
//! error settlement), fragment end events, and the `record`/`message` value helpers.
//! Scope: private `impl LlmEventPublisher` half split from runner/publish.rs for size.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/message.rs
//! |-- schema/src/prompt.rs
//! |-- schema/src/session_event.rs
//! |-- schema/src/session_message.rs
//! |-- schema/src/values.rs
//! `-- session-runtime/src/runner/publish.rs
//! External:
//! |-- serde_json::Value
//! `-- serde_json::to_value
//! Used by:
//! rust/crates/
//! `-- session-runtime/src/runner/publish.rs
//! Invariants: a call is published only after its input ended; local results drop the raw
//! `result`, provider-executed results keep it; late error results after settlement are ignored.
//! Contract tests: session-runtime/tests/runner_publish_tools.rs

use opencode_llm_api::message::{ToolContent as LlmToolContent, ToolOutput, ToolResultValue};
use opencode_schema::prompt::{ProviderMetadata, ToolContent};
use opencode_schema::session_event::{
    ReasoningEnded, TextEnded, ToolCalled, ToolFailed, ToolInputEnded, ToolInputStarted,
    ToolProvider, ToolSuccess,
};
use opencode_schema::session_message::UnknownError;
use opencode_schema::values::{DateTimeUtc, JsonObject};
use serde_json::Value;

use super::{LlmEventPublisher, PublishError, ToolEntry, protocol};

impl LlmEventPublisher {
    pub(super) async fn publish_tool_call(
        &mut self,
        id: &str,
        name: &str,
        input: &Value,
        provider_executed: Option<bool>,
        provider_metadata: &Option<ProviderMetadata>,
    ) -> Result<(), PublishError> {
        if !self.tools.contains_key(id) {
            self.start_tool_input(id, name).await?;
        }
        if !self.tools[id].input_ended {
            self.end_tool_input(id, name).await?;
        }
        let tool = &mut self.tools[id];
        if tool.name != name {
            return Err(protocol(format!(
                "Tool call name changed for {id}: {} -> {name}",
                tool.name
            )));
        }
        if tool.called {
            return Err(protocol(format!("Duplicate tool call: {id}")));
        }
        tool.called = true;
        tool.provider_executed = provider_executed == Some(true);
        tool.provider_metadata = provider_metadata.clone();
        let event = ToolCalled {
            timestamp: DateTimeUtc::now(),
            session_id: self.input.session_id.clone(),
            assistant_message_id: tool.assistant_message_id.clone(),
            call_id: id.to_owned(),
            tool: name.to_owned(),
            input: record(input.clone()),
            provider: ToolProvider {
                executed: tool.provider_executed,
                metadata: provider_metadata.clone(),
            },
        };
        self.emit(&event).await
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn publish_tool_result(
        &mut self,
        id: &str,
        name: &str,
        result: &ToolResultValue,
        output: Option<&ToolOutput>,
        provider_executed: bool,
        provider_metadata: &Option<ProviderMetadata>,
        output_paths: &[String],
    ) -> Result<(), PublishError> {
        let tool = self.called_tool(id, name, "result")?;
        if tool.settled {
            if matches!(result, ToolResultValue::Error(_)) {
                return Ok(());
            }
            return Err(protocol(format!("Duplicate tool result: {id}")));
        }
        tool.settled = true;
        let provider = ToolProvider {
            executed: provider_executed || tool.provider_executed,
            metadata: provider_metadata.clone(),
        };
        let assistant_message_id = tool.assistant_message_id.clone();
        let raw = serde_json::to_value(result).ok();
        if let ToolResultValue::Error(value) = result {
            let event = ToolFailed {
                timestamp: DateTimeUtc::now(),
                session_id: self.input.session_id.clone(),
                assistant_message_id,
                call_id: id.to_owned(),
                error: UnknownError::new(message(value)),
                result: raw,
                provider,
            };
            return self.emit(&event).await;
        }
        let settled = output
            .cloned()
            .or_else(|| ToolOutput::from_result_value(result))
            .ok_or_else(|| {
                protocol(format!(
                    "Unsupported tool result: {}",
                    raw.as_ref().map(message).unwrap_or_default()
                ))
            })?;
        let event = ToolSuccess {
            timestamp: DateTimeUtc::now(),
            session_id: self.input.session_id.clone(),
            assistant_message_id,
            call_id: id.to_owned(),
            structured: record(settled.structured),
            content: settled.content.into_iter().map(tool_content).collect(),
            output_paths: Some(output_paths.to_vec()),
            result: if provider.executed { raw } else { None },
            provider,
        };
        self.emit(&event).await
    }

    pub(super) async fn start_tool_input(
        &mut self,
        id: &str,
        name: &str,
    ) -> Result<(), PublishError> {
        if self.tools.contains_key(id) {
            return Err(protocol(format!("Duplicate tool input start: {id}")));
        }
        let assistant_message_id = self.start_assistant().await?;
        self.tools.insert(
            id.to_owned(),
            ToolEntry {
                assistant_message_id: assistant_message_id.clone(),
                name: name.to_owned(),
                input_ended: false,
                called: false,
                settled: false,
                provider_executed: false,
                provider_metadata: None,
            },
        );
        self.tool_input.start(id)?;
        let event = ToolInputStarted {
            timestamp: DateTimeUtc::now(),
            session_id: self.input.session_id.clone(),
            assistant_message_id,
            call_id: id.to_owned(),
            name: name.to_owned(),
        };
        self.emit(&event).await
    }

    pub(super) async fn end_tool_input(
        &mut self,
        id: &str,
        name: &str,
    ) -> Result<(), PublishError> {
        let tool = self.known_tool(id, name, "Tool input end before start", "input")?;
        if tool.input_ended {
            return Err(protocol(format!("Duplicate tool input end: {id}")));
        }
        self.end_tool_input_fragment(id).await
    }

    pub(super) async fn end_tool_input_fragment(&mut self, id: &str) -> Result<(), PublishError> {
        let text = self.tool_input.joined(id)?;
        let Some(tool) = self.tools.get(id) else {
            return Err(protocol(format!("Tool input end before start: {id}")));
        };
        let event = ToolInputEnded {
            timestamp: DateTimeUtc::now(),
            session_id: self.input.session_id.clone(),
            assistant_message_id: tool.assistant_message_id.clone(),
            call_id: id.to_owned(),
            text,
        };
        self.emit(&event).await?;
        if let Some(tool) = self.tools.get_mut(id) {
            tool.input_ended = true;
        }
        self.tool_input.chunks.shift_remove(id);
        Ok(())
    }

    pub(super) async fn end_text(&mut self, id: &str) -> Result<(), PublishError> {
        let text = self.text.joined(id)?;
        let event = TextEnded {
            timestamp: DateTimeUtc::now(),
            session_id: self.input.session_id.clone(),
            assistant_message_id: self.current_assistant()?,
            text_id: id.to_owned(),
            text,
        };
        self.emit(&event).await?;
        self.text.chunks.shift_remove(id);
        Ok(())
    }

    pub(super) async fn end_reasoning(
        &mut self,
        id: &str,
        provider_metadata: Option<ProviderMetadata>,
    ) -> Result<(), PublishError> {
        let text = self.reasoning.joined(id)?;
        let event = ReasoningEnded {
            timestamp: DateTimeUtc::now(),
            session_id: self.input.session_id.clone(),
            assistant_message_id: self.current_assistant()?,
            reasoning_id: id.to_owned(),
            text,
            provider_metadata,
        };
        self.emit(&event).await?;
        self.reasoning.chunks.shift_remove(id);
        Ok(())
    }

    /// Tool that has started input and keeps its name (`<missing>: id`, `Tool <kind> name changed`).
    pub(super) fn known_tool(
        &mut self,
        id: &str,
        name: &str,
        missing: &str,
        kind: &str,
    ) -> Result<&mut ToolEntry, PublishError> {
        let Some(tool) = self.tools.get_mut(id) else {
            return Err(protocol(format!("{missing}: {id}")));
        };
        if tool.name != name {
            return Err(protocol(format!(
                "Tool {kind} name changed for {id}: {} -> {name}",
                tool.name
            )));
        }
        Ok(tool)
    }

    /// Tool that was called and keeps its name (`Tool <kind> before call`).
    pub(super) fn called_tool(
        &mut self,
        id: &str,
        name: &str,
        kind: &str,
    ) -> Result<&mut ToolEntry, PublishError> {
        let Some(tool) = self.tools.get_mut(id).filter(|tool| tool.called) else {
            return Err(protocol(format!("Tool {kind} before call: {id}")));
        };
        if tool.name != name {
            return Err(protocol(format!(
                "Tool {kind} name changed for {id}: {} -> {name}",
                tool.name
            )));
        }
        Ok(tool)
    }
}

/// `record(value)`: objects pass through, anything else is wrapped as `{ value }`.
fn record(value: Value) -> JsonObject {
    match value {
        Value::Object(object) => object,
        value => JsonObject::from_iter([("value".to_owned(), value)]),
    }
}

/// `message(value)`: strings verbatim, everything else JSON-encoded.
fn message(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        value => value.to_string(),
    }
}

fn tool_content(content: LlmToolContent) -> ToolContent {
    match content {
        LlmToolContent::Text { text } => ToolContent::Text { text },
        LlmToolContent::File { uri, mime, name } => ToolContent::File { uri, mime, name },
    }
}
