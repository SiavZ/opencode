//! Purpose: permission requests, replies, rules, saved permissions and their event definitions.
//! Scope: pure values from permission.ts and permission-saved.ts.
//! Uses:
//! rust/crates/
//! |-- schema/src/event.rs
//! |-- schema/src/ids.rs
//! `-- schema/src/values.rs
//! External:
//! `-- serde::{Deserialize, Serialize}
//! Used by:
//! rust/crates/
//! |-- schema/src/agent.rs
//! |-- schema/src/lib.rs
//! `-- schema/tests/schema_domains_permission.rs
//! Invariants: declaration order matches baseline JSON, optional fields omit absence and reject null.
//! Contract tests: schema/tests/schema_domains_permission.rs

use crate::event::{Definition, EventData};
use crate::ids::{PermissionID, PermissionSavedID, ProjectID, SessionID};
use crate::values::{JsonObject, optional};
use serde::{Deserialize, Serialize};

/// PermissionV2.Source. The baseline has one tool variant and unbranded string IDs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Source {
    #[serde(rename = "tool")]
    Tool {
        #[serde(rename = "messageID")]
        message_id: String,
        #[serde(rename = "callID")]
        call_id: String,
    },
}

/// PermissionV2.Request, also the payload of permission.v2.asked.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub id: PermissionID,
    #[serde(rename = "sessionID")]
    pub session_id: SessionID,
    pub action: String,
    pub resources: Vec<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub save: Option<Vec<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub metadata: Option<JsonObject>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub source: Option<Source>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Reply {
    Once,
    Always,
    Reject,
}

pub type Asked = Request;
impl EventData for Request {
    const DEFINITION: Definition = Definition {
        r#type: "permission.v2.asked",
        durable: None,
    };
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Replied {
    #[serde(rename = "sessionID")]
    pub session_id: SessionID,
    #[serde(rename = "requestID")]
    pub request_id: PermissionID,
    pub reply: Reply,
}

impl EventData for Replied {
    const DEFINITION: Definition = Definition {
        r#type: "permission.v2.replied",
        durable: None,
    };
}

pub const DEFINITIONS: &[Definition] = &[Asked::DEFINITION, Replied::DEFINITION];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effect {
    Allow,
    Deny,
    Ask,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    pub action: String,
    pub resource: String,
    pub effect: Effect,
}

pub type Ruleset = Vec<Rule>;

/// PermissionSaved.Info.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedInfo {
    pub id: PermissionSavedID,
    #[serde(rename = "projectID")]
    pub project_id: ProjectID,
    pub action: String,
    pub resource: String,
}
