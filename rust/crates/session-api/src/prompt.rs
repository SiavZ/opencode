//! Purpose: the resolved durable `Prompt` (re-exported from schema) plus the unresolved
//! `PromptInput` accepted by `SessionV2.prompt` before mime resolution.
//! Scope: pure values.
//! Uses:
//! rust/crates/
//! `-- schema/src/prompt.rs
//! External:
//! |-- serde::Deserialize
//! `-- serde::Serialize
//! Used by:
//! rust/crates/
//! |-- session-api/src/admission.rs
//! |-- session-api/src/lib.rs
//! |-- session-runtime/src/admission.rs
//! |-- session-runtime/src/runner/to_llm_message.rs
//! `-- session-runtime/tests/admission.rs
//! Invariants: `PromptInput` matches `packages/schema/src/prompt-input.ts` key order and omission.
//! Contract tests: session-api/tests/wire.rs

use serde::{Deserialize, Serialize};

pub use opencode_schema::prompt::{AgentAttachment, FileAttachment, Prompt, Source};

/// `PromptInput.FileAttachment`, before mime resolution.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InputFileAttachment {
    pub uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,
}

/// `PromptInput`, the caller-supplied prompt.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PromptInput {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<InputFileAttachment>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agents: Option<Vec<AgentAttachment>>,
}

impl PromptInput {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            files: None,
            agents: None,
        }
    }
}
