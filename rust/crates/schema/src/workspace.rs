//! Purpose: workspace connection values and workspace/worktree event payloads.
//! Scope: pure values from workspace.ts, workspace-event.ts and worktree-event.ts.
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
//! |-- locations/src/workspace/mod.rs
//! |-- locations/src/workspace/sync.rs
//! |-- locations/tests/workspace_lifecycle.rs
//! |-- schema/src/lib.rs
//! `-- schema/tests/schema_domains_workspace.rs
//! Invariants: field order, closed status literals and inventory order match baseline.
//! Contract tests: schema/tests/schema_domains_workspace.rs

use crate::event::{Definition, EventData};
use crate::ids::WorkspaceID;
use crate::values::optional;
use serde::{Deserialize, Serialize};

pub type ID = WorkspaceID;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionState {
    Connected,
    Connecting,
    Disconnected,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionStatus {
    #[serde(rename = "workspaceID")]
    pub workspace_id: WorkspaceID,
    pub status: ConnectionState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ready {
    pub name: String,
}
impl EventData for Ready {
    const DEFINITION: Definition = Definition {
        r#type: "workspace.ready",
        durable: None,
    };
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Failed {
    pub message: String,
}
impl EventData for Failed {
    const DEFINITION: Definition = Definition {
        r#type: "workspace.failed",
        durable: None,
    };
}

pub type Status = ConnectionStatus;
impl EventData for ConnectionStatus {
    const DEFINITION: Definition = Definition {
        r#type: "workspace.status",
        durable: None,
    };
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeReady {
    pub name: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub branch: Option<String>,
}
impl EventData for WorktreeReady {
    const DEFINITION: Definition = Definition {
        r#type: "worktree.ready",
        durable: None,
    };
}

// A distinct payload type is required even though both Failed schemas have the same fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeFailed {
    pub message: String,
}
impl EventData for WorktreeFailed {
    const DEFINITION: Definition = Definition {
        r#type: "worktree.failed",
        durable: None,
    };
}

pub const WORKSPACE_DEFINITIONS: &[Definition] =
    &[Ready::DEFINITION, Failed::DEFINITION, Status::DEFINITION];
pub const WORKTREE_DEFINITIONS: &[Definition] =
    &[WorktreeReady::DEFINITION, WorktreeFailed::DEFINITION];
pub const DEFINITIONS: &[Definition] = &[
    Ready::DEFINITION,
    Failed::DEFINITION,
    Status::DEFINITION,
    WorktreeReady::DEFINITION,
    WorktreeFailed::DEFINITION,
];
