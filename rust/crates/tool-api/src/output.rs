//! Purpose: tool output values wire-compatible with @opencode-ai/llm (`LLM.ToolContent`,
//! `LLM.ToolOutput`, `ToolResultValue` and `toResultValue`/`fromResultValue`).
//! Scope: pure values.
//! Uses:
//! `-- none
//! External:
//! |-- serde::Deserialize
//! |-- serde::Serialize
//! |-- serde_json::Value
//! `-- serde_json::json
//! Used by:
//! rust/crates/
//! |-- tool-api/src/definition.rs
//! |-- tool-api/src/lib.rs
//! `-- tool-api/src/registry.rs
//! Invariants: packages/llm/src/schema/messages.ts:80-120 conversion rules.
//! Contract tests: tool-api/src/output.rs

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ToolContent {
    Text {
        text: String,
    },
    File {
        uri: String,
        mime: String,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        name: Option<String>,
    },
}

impl ToolContent {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }

    /// Data-URI file content, as `Tool.make` builds from `{type:"file", data, mime, name}`.
    pub fn file_base64(data: &str, mime: &str, name: Option<String>) -> Self {
        Self::File {
            uri: format!("data:{mime};base64,{data}"),
            mime: mime.into(),
            name,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolOutput {
    pub structured: Value,
    pub content: Vec<ToolContent>,
}

impl ToolOutput {
    pub fn new(structured: Value, content: Vec<ToolContent>) -> Self {
        Self {
            structured,
            content,
        }
    }

    /// `ToolOutput.toResultValue`.
    pub fn to_result_value(&self) -> ToolResultValue {
        match self.content.as_slice() {
            [] => ToolResultValue::Json {
                value: self.structured.clone(),
            },
            [ToolContent::Text { text }] => ToolResultValue::Text {
                value: Value::String(text.clone()),
            },
            _ => ToolResultValue::Content {
                value: self.content.clone(),
            },
        }
    }

    /// `ToolOutput.fromResultValue`.
    pub fn from_result_value(result: &ToolResultValue) -> Option<Self> {
        match result {
            ToolResultValue::Json { value } => Some(Self::new(value.clone(), vec![])),
            ToolResultValue::Text { value } => {
                let text = match value {
                    Value::String(text) => text.clone(),
                    other => other.to_string(),
                };
                Some(Self::new(
                    Value::Object(Default::default()),
                    vec![ToolContent::Text { text }],
                ))
            }
            ToolResultValue::Content { value } => {
                Some(Self::new(Value::Object(Default::default()), value.clone()))
            }
            ToolResultValue::Error { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ToolResultValue {
    Json { value: Value },
    Text { value: Value },
    Error { value: Value },
    Content { value: Vec<ToolContent> },
}

impl ToolResultValue {
    pub fn error(message: impl Into<String>) -> Self {
        Self::Error {
            value: Value::String(message.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn result_value_conversion() {
        let out = ToolOutput::new(json!({"a":1}), vec![]);
        assert_eq!(
            out.to_result_value(),
            ToolResultValue::Json {
                value: json!({"a":1})
            }
        );
        let out = ToolOutput::new(json!({}), vec![ToolContent::text("hi")]);
        assert_eq!(
            serde_json::to_string(&out.to_result_value()).unwrap_or_default(),
            r#"{"type":"text","value":"hi"}"#
        );
        let file = ToolContent::file_base64("AA==", "image/png", None);
        assert_eq!(
            serde_json::to_string(&file).unwrap_or_default(),
            r#"{"type":"file","uri":"data:image/png;base64,AA==","mime":"image/png"}"#
        );
    }
}
