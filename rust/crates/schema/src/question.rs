//! Purpose: `question.ts` V2 questions and events.
//! Scope: pure values.
//! Uses:
//! rust/crates/
//! |-- schema/src/event.rs
//! |-- schema/src/ids.rs
//! `-- schema/src/values.rs
//! External:
//! |-- serde::Deserialize
//! `-- serde::Serialize
//! Used by:
//! rust/crates/
//! |-- schema/src/lib.rs
//! `-- schema/tests/schema_domains_question.rs
//! Invariants: byte-identical JSON with the baseline module. Field order matches baseline,
//! absent optional keys omit and explicit null fails.
//! Contract tests: schema/tests/schema_domains_question.rs

use crate::event::{Definition, EventData};
use crate::ids::{QuestionID, SessionID};
use crate::values::optional;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionOption {
    pub label: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Info {
    pub question: String,
    pub header: String,
    pub options: Vec<QuestionOption>,
    #[serde(
        default,
        skip_serializing_if = "std::option::Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub multiple: std::option::Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "std::option::Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub custom: std::option::Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Prompt {
    pub question: String,
    pub header: String,
    pub options: Vec<QuestionOption>,
    #[serde(
        default,
        skip_serializing_if = "std::option::Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub multiple: std::option::Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tool {
    #[serde(rename = "messageID")]
    pub message_id: String,
    #[serde(rename = "callID")]
    pub call_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub id: QuestionID,
    #[serde(rename = "sessionID")]
    pub session_id: SessionID,
    pub questions: Vec<Info>,
    #[serde(
        default,
        skip_serializing_if = "std::option::Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub tool: std::option::Option<Tool>,
}

pub type Answer = Vec<String>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reply {
    pub answers: Vec<Answer>,
}

impl EventData for Request {
    const DEFINITION: Definition = Definition {
        r#type: "question.v2.asked",
        durable: None,
    };
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Replied {
    #[serde(rename = "sessionID")]
    pub session_id: SessionID,
    #[serde(rename = "requestID")]
    pub request_id: QuestionID,
    pub answers: Vec<Answer>,
}

impl EventData for Replied {
    const DEFINITION: Definition = Definition {
        r#type: "question.v2.replied",
        durable: None,
    };
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rejected {
    #[serde(rename = "sessionID")]
    pub session_id: SessionID,
    #[serde(rename = "requestID")]
    pub request_id: QuestionID,
}

impl EventData for Rejected {
    const DEFINITION: Definition = Definition {
        r#type: "question.v2.rejected",
        durable: None,
    };
}

pub const DEFINITIONS: &[Definition] = &[
    Request::DEFINITION,
    Replied::DEFINITION,
    Rejected::DEFINITION,
];
