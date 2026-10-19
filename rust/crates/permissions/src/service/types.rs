//! Purpose: permission request, reply, grant, event and error wire contracts.
//! Scope: data definitions and the AssertInput constructor, not permission decisions or pending state.
//! Uses:
//! rust/crates/
//! `-- permissions/src/rule.rs
//! External:
//! |-- serde::Deserialize
//! |-- serde::Serialize
//! |-- serde_json::Map
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! `-- permissions/src/service.rs
//! Invariants: serde field names, optional omission, enum tags and public service paths remain unchanged.
//! Contract tests: permissions/tests/service.rs

use crate::rule::{Effect, Ruleset};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// `PermissionV2.Source`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Source {
    Tool {
        #[serde(rename = "messageID")]
        message_id: String,
        #[serde(rename = "callID")]
        call_id: String,
    },
}

/// `PermissionV2.Request` wire shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PermissionRequest {
    pub id: String,
    #[serde(rename = "sessionID")]
    pub session_id: String,
    pub action: String,
    pub resources: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub save: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub metadata: Option<Map<String, Value>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub source: Option<Source>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Reply {
    Once,
    Always,
    Reject,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AssertInput {
    pub id: Option<String>,
    pub session_id: String,
    pub action: String,
    pub resources: Vec<String>,
    pub save: Option<Vec<String>>,
    pub metadata: Option<Map<String, Value>>,
    pub source: Option<Source>,
    pub agent: Option<String>,
}

impl AssertInput {
    pub fn new(
        session_id: impl Into<String>,
        action: impl Into<String>,
        resources: Vec<String>,
    ) -> Self {
        Self {
            id: None,
            session_id: session_id.into(),
            action: action.into(),
            resources,
            save: None,
            metadata: None,
            source: None,
            agent: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplyInput {
    #[serde(rename = "requestID")]
    pub request_id: String,
    pub reply: Reply,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskResult {
    pub id: String,
    pub effect: Effect,
}

/// `PermissionSaved.Info`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedGrant {
    pub id: String,
    #[serde(rename = "projectID")]
    pub project_id: String,
    pub action: String,
    pub resource: String,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum PermissionError {
    /// `PermissionV2.BlockedError`: denied by policy, carries the relevant rules.
    #[error("permission blocked by rules")]
    Blocked { rules: Ruleset },
    /// `PermissionV2.CorrectedError`: rejected with feedback.
    #[error("{feedback}")]
    Corrected { feedback: String },
    /// `PermissionV2.DeclinedError`: rejected without feedback, cascaded reject, or teardown.
    #[error("permission declined")]
    Declined,
    #[error("permission request not found: {request_id}")]
    NotFound { request_id: String },
    #[error("session not found: {session_id}")]
    SessionNotFound { session_id: String },
    #[error("duplicate pending permission ID: {id}")]
    Duplicate { id: String },
    #[error("permission port failed: {0}")]
    Port(String),
}

/// Events published by the service.
#[derive(Debug, Clone, PartialEq)]
pub enum PermissionEvent {
    /// `permission.v2.asked`.
    Asked(PermissionRequest),
    /// `permission.v2.replied`.
    Replied {
        session_id: String,
        request_id: String,
        reply: Reply,
    },
}
