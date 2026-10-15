//! Purpose: `pty.ts` and `pty-ticket.ts` terminal session values and events.
//! Scope: pure values.
//! Uses:
//! rust/crates/
//! |-- schema/src/event.rs
//! |-- schema/src/ids.rs
//! `-- schema/src/values.rs
//! External:
//! `-- serde::{Deserialize, Serialize}
//! Used by:
//! rust/crates/
//! `-- schema/src/lib.rs
//! Invariants: byte-identical JSON with the baseline module.
//! Contract tests: schema/tests/schema_domains_pty.rs

use crate::event::{Definition, EventData};
pub use crate::ids::PtyID as ID;
use crate::values::{JsonObject, NonNegativeInt, PositiveInt, optional};
use serde::{Deserialize, Serialize};

/// `Pty.Info.status`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Running,
    Exited,
}

/// `Pty.Info`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Info {
    pub id: ID,
    pub title: String,
    pub command: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub status: Status,
    pub pid: NonNegativeInt,
    #[serde(
        rename = "exitCode",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub exit_code: Option<NonNegativeInt>,
}

/// `pty.created` event data
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Created {
    pub info: Info,
}

impl EventData for Created {
    const DEFINITION: Definition = Definition {
        r#type: "pty.created",
        durable: None,
    };
}

/// `pty.updated` event data
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Updated {
    pub info: Info,
}

impl EventData for Updated {
    const DEFINITION: Definition = Definition {
        r#type: "pty.updated",
        durable: None,
    };
}

/// `pty.exited` event data
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Exited {
    pub id: ID,
    #[serde(rename = "exitCode")]
    pub exit_code: NonNegativeInt,
}

impl EventData for Exited {
    const DEFINITION: Definition = Definition {
        r#type: "pty.exited",
        durable: None,
    };
}

/// `pty.deleted` event data
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Deleted {
    pub id: ID,
}

impl EventData for Deleted {
    const DEFINITION: Definition = Definition {
        r#type: "pty.deleted",
        durable: None,
    };
}

/// `Pty.Event.Definitions` in inventory order.
pub const DEFINITIONS: &[Definition] = &[
    Created::DEFINITION,
    Updated::DEFINITION,
    Exited::DEFINITION,
    Deleted::DEFINITION,
];

/// `Pty.CreateInput`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateInput {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub command: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub args: Option<Vec<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub cwd: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub title: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub env: Option<JsonObject>,
}

/// `Pty.UpdateInput.size`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Size {
    pub rows: PositiveInt,
    pub cols: PositiveInt,
}

/// `Pty.UpdateInput`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateInput {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub title: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub size: Option<Size>,
}

/// `PtyTicket.ConnectToken`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConnectToken {
    pub ticket: String,
    pub expires_in: PositiveInt,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Event;
    use serde_json::json;

    #[test]
    fn fixture_captured_from_bun_at_baseline() {
        let info_json = r#"{"id":"pty_1","title":"t","command":"c","args":["a"],"cwd":"/","status":"running","pid":123,"exitCode":0}"#;
        let info: Info = serde_json::from_str(info_json).unwrap();
        assert_eq!(serde_json::to_string(&info).unwrap(), info_json);

        let info_no_exit = r#"{"id":"pty_2","title":"t","command":"c","args":[],"cwd":"/","status":"exited","pid":456}"#;
        let info2: Info = serde_json::from_str(info_no_exit).unwrap();
        assert_eq!(serde_json::to_string(&info2).unwrap(), info_no_exit);

        let created_json = r#"{"id":"evt_1","type":"pty.created","data":{"info":{"id":"pty_1","title":"t","command":"c","args":["a"],"cwd":"/","status":"running","pid":123}}}"#;
        let created: Event<Created> = serde_json::from_str(created_json).unwrap();
        assert_eq!(serde_json::to_string(&created).unwrap(), created_json);

        let exited_json =
            r#"{"id":"evt_2","type":"pty.exited","data":{"id":"pty_1","exitCode":1}}"#;
        let exited: Event<Exited> = serde_json::from_str(exited_json).unwrap();
        assert_eq!(serde_json::to_string(&exited).unwrap(), exited_json);
    }

    #[test]
    fn optional_fields_reject_null() {
        let mut info: serde_json::Value = serde_json::from_str(r#"{"id":"pty_1","title":"t","command":"c","args":[],"cwd":"/","status":"running","pid":123}"#).unwrap();
        info["exitCode"] = json!(null);
        assert!(serde_json::from_value::<Info>(info).is_err());

        let mut create: serde_json::Value = json!({"command": "c"});
        create["env"] = json!(null);
        assert!(serde_json::from_value::<CreateInput>(create).is_err());
    }

    #[test]
    fn inventory_matches_baseline() {
        assert_eq!(
            DEFINITIONS.iter().map(|d| d.r#type).collect::<Vec<_>>(),
            ["pty.created", "pty.updated", "pty.exited", "pty.deleted"]
        );
    }
}
