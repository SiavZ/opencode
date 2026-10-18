//! Purpose: `prompt.ts` (Prompt, Source, FileAttachment, AgentAttachment) and
//! `llm.ts` (ProviderMetadata, ToolContent) shared prompt and tool content values.
//! Scope: pure values.
//! Uses:
//! rust/crates/
//! `-- schema/src/values.rs
//! External:
//! `-- serde::{Deserialize, Serialize}
//! Used by:
//! rust/crates/
//! `-- schema/src/lib.rs
//! Invariants: key order follows the baseline Struct order; optional keys are omitted;
//! ToolContent is tagged by `type` with the tag first.
//! Contract tests: schema/src/prompt.rs (unit tests), schema/tests/baseline_fixtures.rs

use crate::values::{Finite, JsonObject, optional};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

/// `Prompt.Source`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Source {
    pub start: Finite,
    pub end: Finite,
    pub text: String,
}

/// `Prompt.FileAttachment`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileAttachment {
    pub uri: String,
    pub mime: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub name: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub description: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub source: Option<Source>,
}

/// `Prompt.AgentAttachment`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentAttachment {
    pub name: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub source: Option<Source>,
}

/// `Prompt`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Prompt {
    pub text: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub files: Option<Vec<FileAttachment>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub agents: Option<Vec<AgentAttachment>>,
}

impl Prompt {
    /// Text-only prompt.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            files: None,
            agents: None,
        }
    }
}

/// `LLM.ProviderMetadata`: `Record<string, Record<string, unknown>>`.
pub type ProviderMetadata = IndexMap<String, JsonObject>;

/// `Tool.TextContent` / `Tool.FileContent` union (`LLM.ToolContent`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ToolContent {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "file")]
    File {
        uri: String,
        mime: String,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "optional::deserialize"
        )]
        name: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_omits_absent_lists() {
        let prompt: Prompt =
            serde_json::from_str(r#"{"agents":[{"name":"a"}],"text":"hi"}"#).unwrap();
        assert_eq!(
            serde_json::to_string(&prompt).unwrap(),
            r#"{"text":"hi","agents":[{"name":"a"}]}"#
        );
    }

    #[test]
    fn tool_content_tag_first() {
        let content: ToolContent =
            serde_json::from_str(r#"{"mime":"m","uri":"u","type":"file"}"#).unwrap();
        assert_eq!(
            serde_json::to_string(&content).unwrap(),
            r#"{"type":"file","uri":"u","mime":"m"}"#
        );
        assert!(serde_json::from_str::<ToolContent>(r#"{"type":"image"}"#).is_err());
    }
}
