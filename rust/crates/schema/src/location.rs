//! Purpose: `location.ts` Location.Ref and Location.Info, plus the `response` wrapper shape.
//! Scope: pure values.
//! Uses:
//! rust/crates/
//! |-- schema/src/ids.rs
//! `-- schema/src/values.rs
//! External:
//! |-- serde::Deserialize
//! `-- serde::Serialize
//! Used by:
//! rust/crates/
//! |-- locations/src/lib.rs
//! |-- locations/src/ports.rs
//! |-- locations/src/registry.rs
//! |-- runtime/src/adapters.rs
//! |-- runtime/src/location.rs
//! |-- runtime/src/process.rs
//! |-- schema/src/event.rs
//! |-- schema/src/lib.rs
//! |-- schema/src/session.rs
//! |-- schema/src/session_event.rs
//! |-- server/src/services/ports.rs
//! |-- session-api/src/ids.rs
//! |-- session-runtime/src/service.rs
//! |-- session-runtime/tests/admission.rs
//! |-- session-runtime/tests/admission_historical.rs
//! |-- session-runtime/tests/admission_inbox.rs
//! |-- session-runtime/tests/admission_projection.rs
//! `-- session-runtime/tests/support/provider.rs
//! Invariants: field order `directory`, `workspaceID`, `project`; `workspaceID` omitted when absent.
//! Contract tests: schema/tests/baseline_fixtures.rs

use crate::ids::{ProjectID, WorkspaceID};
use crate::values::{AbsolutePath, optional};
use serde::{Deserialize, Serialize};

/// `Location.Ref`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ref {
    pub directory: AbsolutePath,
    #[serde(
        rename = "workspaceID",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub workspace_id: Option<WorkspaceID>,
}

/// `Location.Info` project summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InfoProject {
    pub id: ProjectID,
    pub directory: AbsolutePath,
}

/// `Location.Info`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Info {
    pub directory: AbsolutePath,
    #[serde(
        rename = "workspaceID",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub workspace_id: Option<WorkspaceID>,
    pub project: InfoProject,
}

/// `Location.response(data)`: `{ location, data }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response<T> {
    pub location: Info,
    pub data: T,
}
