//! Purpose: `agent.ts` (Agent.ID, Agent.Color, Agent.Info), `command.ts` (Command.Info),
//! and `prompt-input.ts` (PromptInput.FileAttachment, PromptInput.Prompt) pure values.
//! Scope: pure values.
//! Uses:
//! rust/crates/
//! |-- schema/src/ids.rs
//! |-- schema/src/permission.rs
//! |-- schema/src/prompt.rs
//! |-- schema/src/provider.rs
//! `-- schema/src/values.rs
//! External:
//! |-- serde::Deserialize
//! |-- serde::Serialize
//! |-- serde_json::from_value
//! |-- serde_json::json
//! `-- serde_json::to_value
//! Used by:
//! rust/crates/
//! |-- runtime/src/location_agents.rs
//! |-- schema/src/lib.rs
//! |-- schema/tests/schema_domains_agent.rs
//! |-- session-runtime/src/agents.rs
//! |-- session-runtime/tests/runner_agents.rs
//! `-- session-runtime/tests/support/provider.rs
//! Invariants: field order matches the baseline struct, optional keys are omitted when absent and
//! reject explicit `null`, colors are `#rrggbb` or one of the theme literals.
//! Contract tests: schema/tests/schema_domains_agent.rs

pub use crate::ids::AgentID as ID;
use crate::permission::Ruleset;
use crate::prompt::{AgentAttachment, Source};
use crate::provider::{ModelRef, ProviderRequest};
use crate::values::{PositiveInt, optional};
use serde::{Deserialize, Serialize};

const THEME_COLORS: [&str; 7] = [
    "primary",
    "secondary",
    "accent",
    "success",
    "warning",
    "error",
    "info",
];

/// `Agent.Color`: `^#[0-9a-fA-F]{6}$` or a theme literal.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Color(String);

impl Color {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Color {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let hex = value.len() == 7
            && value.starts_with('#')
            && value[1..].chars().all(|c| c.is_ascii_hexdigit());
        if hex || THEME_COLORS.contains(&value.as_str()) {
            return Ok(Self(value));
        }
        Err(format!("Expected Agent.Color, got {value:?}"))
    }
}

impl From<Color> for String {
    fn from(value: Color) -> Self {
        value.0
    }
}

/// `Agent.Info.mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Subagent,
    Primary,
    All,
}

/// `Agent.Info` (`AgentV2.Info`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Info {
    pub id: ID,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub model: Option<ModelRef>,
    pub request: ProviderRequest,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub system: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub description: Option<String>,
    pub mode: Mode,
    pub hidden: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub color: Option<Color>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub steps: Option<PositiveInt>,
    pub permissions: Ruleset,
}

impl Info {
    /// `Agent.Info.empty(id)`: empty request, mode `all`, visible, no permissions.
    pub fn empty(id: ID) -> Self {
        Self {
            id,
            model: None,
            request: ProviderRequest::default(),
            system: None,
            description: None,
            mode: Mode::All,
            hidden: false,
            color: None,
            steps: None,
            permissions: Vec::new(),
        }
    }
}

/// `Command.Info` (`CommandV2.Info`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandInfo {
    pub name: String,
    pub template: String,
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
    pub agent: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub model: Option<ModelRef>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub subtask: Option<bool>,
}

/// `PromptInput.FileAttachment`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptInputFileAttachment {
    pub uri: String,
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

/// `PromptInput.Prompt`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptInputPrompt {
    pub text: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub files: Option<Vec<PromptInputFileAttachment>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub agents: Option<Vec<AgentAttachment>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn empty_matches_baseline_shape() {
        let encoded = serde_json::to_value(Info::empty(ID::new("test"))).unwrap();
        assert_eq!(
            serde_json::to_string(&encoded).unwrap(),
            r#"{"id":"test","request":{"headers":{},"body":{}},"mode":"all","hidden":false,"permissions":[]}"#
        );
    }

    #[test]
    fn color_accepts_hex_and_literals_only() {
        assert!(serde_json::from_value::<Color>(json!("#a0B9fF")).is_ok());
        assert!(serde_json::from_value::<Color>(json!("accent")).is_ok());
        assert!(serde_json::from_value::<Color>(json!("#abc")).is_err());
        assert!(serde_json::from_value::<Color>(json!("purple")).is_err());
    }

    #[test]
    fn optional_fields_reject_null() {
        let value = json!({"id":"a","request":{"headers":{},"body":{}},"mode":"subagent","hidden":true,"permissions":[],"system":null});
        assert!(serde_json::from_value::<Info>(value).is_err());
    }

    #[test]
    fn command_info_omits_absent_and_rejects_null() {
        let value = json!({"name":"n","template":"t","description":null});
        assert!(serde_json::from_value::<CommandInfo>(value).is_err());

        let value = json!({"name":"n","template":"t"});
        let decoded: CommandInfo = serde_json::from_value(value).unwrap();
        assert_eq!(
            serde_json::to_string(&decoded).unwrap(),
            r#"{"name":"n","template":"t"}"#
        );
    }

    #[test]
    fn prompt_input_omits_absent_and_rejects_null() {
        let value = json!({"text":"hi","files":null});
        assert!(serde_json::from_value::<PromptInputPrompt>(value).is_err());

        let value = json!({"text":"hi","files":[{"uri":"u"}]});
        let decoded: PromptInputPrompt = serde_json::from_value(value).unwrap();
        assert_eq!(
            serde_json::to_string(&decoded).unwrap(),
            r#"{"text":"hi","files":[{"uri":"u"}]}"#
        );
    }
}
