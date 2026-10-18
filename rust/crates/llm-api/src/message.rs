//! Purpose: canonical request values: system parts, messages with tagged content parts,
//! tool definitions/choices/results, response format and `LlmRequest` itself.
//! Scope: pure values with ergonomic constructors.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/ids.rs
//! |-- llm-api/src/options.rs
//! `-- llm-api/src/route.rs
//! External:
//! |-- serde::{Deserialize, Serialize}
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! |-- llm-api/src/event.rs
//! |-- llm-api/src/lib.rs
//! |-- llm-api/src/response.rs
//! `-- llm-api/src/route.rs
//! Invariants: tagged unions use `type` with baseline spellings (`text`, `media`,
//! `tool-call`, `tool-result`, `reasoning`); optional keys are omitted when absent.
//! Contract tests: llm-api/tests/contract.rs

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ids::{JsonObject, MessageRole, ProviderMetadata};
use crate::options::{CacheHint, CachePolicy, GenerationOptions, HttpOptions, ProviderOptions};
use crate::route::Model;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename = "text")]
pub struct SystemPart {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache: Option<CacheHint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<JsonObject>,
}

impl SystemPart {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            cache: None,
            metadata: None,
        }
    }
}

/// Media payload: base64/data-URL/URL string, or raw bytes (`Uint8Array`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MediaData {
    Text(String),
    Bytes(Vec<u8>),
}

/// `@opencode-ai/schema/llm` `ToolContent`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ToolContent {
    Text {
        text: String,
    },
    File {
        uri: String,
        mime: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
}

/// `LLM.ToolResult`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "lowercase")]
pub enum ToolResultValue {
    Json(Value),
    Text(Value),
    Error(Value),
    Content(Vec<ToolContent>),
}

/// `LLM.ToolOutput`: structured value plus model-visible content.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolOutput {
    pub structured: Value,
    pub content: Vec<ToolContent>,
}

impl ToolOutput {
    /// `ToolOutput.toResultValue`.
    pub fn to_result_value(&self) -> ToolResultValue {
        match self.content.as_slice() {
            [] => ToolResultValue::Json(self.structured.clone()),
            [ToolContent::Text { text }] => ToolResultValue::Text(Value::String(text.clone())),
            _ => ToolResultValue::Content(self.content.clone()),
        }
    }

    /// `ToolOutput.fromResultValue` (`None` for `error`).
    pub fn from_result_value(result: &ToolResultValue) -> Option<Self> {
        match result {
            ToolResultValue::Json(v) => Some(Self {
                structured: v.clone(),
                content: vec![],
            }),
            ToolResultValue::Text(v) => Some(Self {
                structured: Value::Object(Default::default()),
                content: vec![ToolContent::Text {
                    text: tool_result_text(v),
                }],
            }),
            ToolResultValue::Content(c) => Some(Self {
                structured: Value::Object(Default::default()),
                content: c.clone(),
            }),
            ToolResultValue::Error(_) => None,
        }
    }
}

fn tool_result_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallPart {
    pub id: String,
    pub name: String,
    pub input: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_executed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<JsonObject>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_metadata: Option<ProviderMetadata>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResultPart {
    pub id: String,
    pub name: String,
    pub result: ToolResultValue,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_executed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache: Option<CacheHint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<JsonObject>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_metadata: Option<ProviderMetadata>,
}

/// `LLM.ContentPart` tagged union.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ContentPart {
    #[serde(rename_all = "camelCase")]
    Text {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache: Option<CacheHint>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata: Option<JsonObject>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    #[serde(rename_all = "camelCase")]
    Media {
        media_type: String,
        data: MediaData,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filename: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata: Option<JsonObject>,
    },
    ToolCall(ToolCallPart),
    ToolResult(ToolResultPart),
    #[serde(rename_all = "camelCase")]
    Reasoning {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        encrypted: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata: Option<JsonObject>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
}

impl ContentPart {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text {
            text: text.into(),
            cache: None,
            metadata: None,
            provider_metadata: None,
        }
    }

    pub fn reasoning(text: impl Into<String>) -> Self {
        Self::Reasoning {
            text: text.into(),
            encrypted: None,
            metadata: None,
            provider_metadata: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Message {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub role: MessageRole,
    pub content: Vec<ContentPart>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<JsonObject>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native: Option<JsonObject>,
}

impl Message {
    pub fn new(role: MessageRole, content: Vec<ContentPart>) -> Self {
        Self {
            id: None,
            role,
            content,
            metadata: None,
            native: None,
        }
    }

    pub fn user(text: impl Into<String>) -> Self {
        Self::new(MessageRole::User, vec![ContentPart::text(text)])
    }

    pub fn assistant(content: Vec<ContentPart>) -> Self {
        Self::new(MessageRole::Assistant, content)
    }

    /// Operator-authored instruction at this chronological point (not the initial system prompt).
    pub fn system(text: impl Into<String>) -> Self {
        Self::new(MessageRole::System, vec![ContentPart::text(text)])
    }

    pub fn tool(result: ToolResultPart) -> Self {
        Self::new(MessageRole::Tool, vec![ContentPart::ToolResult(result)])
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: JsonObject,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<JsonObject>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache: Option<CacheHint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<JsonObject>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native: Option<JsonObject>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolChoiceType {
    Auto,
    None,
    Required,
    Tool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolChoice {
    #[serde(rename = "type")]
    pub kind: ToolChoiceType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl ToolChoice {
    /// `ToolChoice.make(string)`: mode names map to modes, anything else names a tool.
    pub fn parse(value: &str) -> Self {
        let kind = match value {
            "auto" => ToolChoiceType::Auto,
            "none" => ToolChoiceType::None,
            "required" => ToolChoiceType::Required,
            _ => return Self::named(value),
        };
        Self { kind, name: None }
    }

    pub fn named(name: impl Into<String>) -> Self {
        Self {
            kind: ToolChoiceType::Tool,
            name: Some(name.into()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ResponseFormat {
    Text,
    Json { schema: JsonObject },
    Tool { tool: Box<ToolDefinition> },
}

/// `LLM.Request`. `model` carries the resolved route; it is not part of any wire body.
#[derive(Clone, Debug)]
pub struct LlmRequest {
    pub id: Option<String>,
    pub model: Model,
    pub system: Vec<SystemPart>,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolDefinition>,
    pub tool_choice: Option<ToolChoice>,
    pub generation: Option<GenerationOptions>,
    pub provider_options: Option<ProviderOptions>,
    pub http: Option<HttpOptions>,
    pub response_format: Option<ResponseFormat>,
    pub cache: Option<CachePolicy>,
    pub metadata: Option<JsonObject>,
}

impl LlmRequest {
    pub fn new(model: Model) -> Self {
        Self {
            id: None,
            model,
            system: vec![],
            messages: vec![],
            tools: vec![],
            tool_choice: None,
            generation: None,
            provider_options: None,
            http: None,
            response_format: None,
            cache: None,
            metadata: None,
        }
    }
}
