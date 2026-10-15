//! Purpose: `integration.ts`, `connection.ts`, `credential.ts` integration and credential values.
//! Scope: pure values.
//! Uses:
//! rust/crates/
//! |-- schema/src/ids.rs
//! `-- schema/src/values.rs
//! External:
//! `-- serde::{Deserialize, Serialize}
//! Used by:
//! rust/crates/
//! `-- schema/src/lib.rs
//! Invariants: byte-identical JSON with the baseline module. Tagged unions use strict literals.
//! Contract tests: schema/tests/schema_domains_integration.rs

use crate::ids::{CredentialID, IntegrationID, IntegrationMethodID};
use crate::values::{JsonObject, NonNegativeInt, optional};
use serde::{Deserialize, Serialize};

/// `Integration.When`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct When {
    pub key: String,
    pub op: WhenOp,
    pub value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WhenOp {
    Eq,
    Neq,
}

/// `Integration.Prompt`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Prompt {
    #[serde(rename = "text")]
    Text {
        key: String,
        message: String,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "optional::deserialize"
        )]
        placeholder: Option<String>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "optional::deserialize"
        )]
        when: Option<When>,
    },
    #[serde(rename = "select")]
    Select {
        key: String,
        message: String,
        options: Vec<SelectOption>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "optional::deserialize"
        )]
        when: Option<When>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SelectOption {
    pub label: String,
    pub value: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub hint: Option<String>,
}

/// `Integration.Method`
/// Custom serialization is used to ensure byte-identical field order with the baseline,
/// where `id` precedes `type` in the OAuth variant.
#[derive(Debug, Clone, PartialEq)]
pub enum Method {
    OAuth {
        id: IntegrationMethodID,
        label: String,
        prompts: Option<Vec<Prompt>>,
    },
    Key {
        label: Option<String>,
    },
    Env {
        names: Vec<String>,
    },
}

impl Serialize for Method {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Method::OAuth { id, label, prompts } => {
                #[derive(Serialize)]
                struct Helper<'a> {
                    id: &'a IntegrationMethodID,
                    r#type: &'static str,
                    label: &'a String,
                    #[serde(skip_serializing_if = "Option::is_none")]
                    prompts: &'a Option<Vec<Prompt>>,
                }
                Helper {
                    id,
                    r#type: "oauth",
                    label,
                    prompts,
                }
                .serialize(serializer)
            }
            Method::Key { label } => {
                #[derive(Serialize)]
                struct Helper<'a> {
                    r#type: &'static str,
                    #[serde(skip_serializing_if = "Option::is_none")]
                    label: &'a Option<String>,
                }
                Helper {
                    r#type: "key",
                    label,
                }
                .serialize(serializer)
            }
            Method::Env { names } => {
                #[derive(Serialize)]
                struct Helper<'a> {
                    r#type: &'static str,
                    names: &'a Vec<String>,
                }
                Helper {
                    r#type: "env",
                    names,
                }
                .serialize(serializer)
            }
        }
    }
}

impl<'de> Deserialize<'de> for Method {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(tag = "type")]
        enum MethodHelper {
            #[serde(rename = "oauth")]
            OAuth {
                id: IntegrationMethodID,
                label: String,
                #[serde(
                    default,
                    skip_serializing_if = "Option::is_none",
                    deserialize_with = "optional::deserialize"
                )]
                prompts: Option<Vec<Prompt>>,
            },
            #[serde(rename = "key")]
            Key {
                #[serde(
                    default,
                    skip_serializing_if = "Option::is_none",
                    deserialize_with = "optional::deserialize"
                )]
                label: Option<String>,
            },
            #[serde(rename = "env")]
            Env { names: Vec<String> },
        }
        let helper = MethodHelper::deserialize(deserializer)?;
        Ok(match helper {
            MethodHelper::OAuth { id, label, prompts } => Method::OAuth { id, label, prompts },
            MethodHelper::Key { label } => Method::Key { label },
            MethodHelper::Env { names } => Method::Env { names },
        })
    }
}

/// `Integration.Inputs`
pub type Inputs = std::collections::BTreeMap<String, String>;

/// `Integration.Ref`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ref {
    pub id: IntegrationID,
    pub name: String,
}

/// `Connection.Info`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ConnectionInfo {
    #[serde(rename = "credential")]
    Credential { id: CredentialID, label: String },
    #[serde(rename = "env")]
    Env { name: String },
}

/// `Integration.Info`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Info {
    pub id: IntegrationID,
    pub name: String,
    pub methods: Vec<Method>,
    pub connections: Vec<ConnectionInfo>,
}

/// `Integration.AttemptTime`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttemptTime {
    pub created: i64,
    pub expires: i64,
}

/// `Integration.Attempt`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Attempt {
    #[serde(rename = "attemptID")]
    pub attempt_id: crate::ids::IntegrationAttemptID,
    pub url: String,
    pub instructions: String,
    pub mode: AttemptMode,
    pub time: AttemptTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AttemptMode {
    Auto,
    Code,
}

/// `Integration.AttemptStatus`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status")]
pub enum AttemptStatus {
    #[serde(rename = "pending")]
    Pending { time: AttemptTime },
    #[serde(rename = "complete")]
    Complete { time: AttemptTime },
    #[serde(rename = "failed")]
    Failed { message: String, time: AttemptTime },
    #[serde(rename = "expired")]
    Expired { time: AttemptTime },
}

/// `Credential.Value`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum CredentialValue {
    #[serde(rename = "oauth")]
    OAuth {
        #[serde(rename = "methodID")]
        method_id: IntegrationMethodID,
        refresh: String,
        access: String,
        expires: NonNegativeInt,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "optional::deserialize"
        )]
        metadata: Option<JsonObject>,
    },
    #[serde(rename = "key")]
    Key {
        key: String,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "optional::deserialize"
        )]
        metadata: Option<JsonObject>,
    },
}
