//! Purpose: project metadata, copy inputs and project event definitions.
//! Scope: pure values from project.ts, project-copy.ts and project-directories.ts.
//! Uses:
//! rust/crates/
//! |-- schema/src/event.rs
//! |-- schema/src/ids.rs
//! |-- schema/src/lib.rs
//! `-- schema/src/values.rs
//! External:
//! |-- serde::Deserialize
//! `-- serde::Serialize
//! Used by:
//! rust/crates/
//! |-- locations/src/project_store.rs
//! |-- schema/src/lib.rs
//! `-- schema/tests/schema_domains_project.rs
//! Invariants: field order matches baseline, absent optional keys omit and explicit null fails.
//! Contract tests: schema/tests/schema_domains_project.rs

use crate::event::{Definition, EventData};
use crate::ids::{ProjectCopyStrategyID, ProjectID};
use crate::values::{AbsolutePath, NonNegativeInt, optional};
use serde::{Deserialize, Serialize};

crate::literal!(Vcs = "git");

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Icon {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub url: Option<String>,
    #[serde(
        rename = "override",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub override_url: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub color: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Commands {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub start: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Time {
    pub created: NonNegativeInt,
    pub updated: NonNegativeInt,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub initialized: Option<NonNegativeInt>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Info {
    pub id: ProjectID,
    pub worktree: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub vcs: Option<Vcs>,
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
    pub icon: Option<Icon>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub commands: Option<Commands>,
    pub time: Time,
    pub sandboxes: Vec<String>,
}

pub type Updated = Info;
impl EventData for Info {
    const DEFINITION: Definition = Definition {
        r#type: "project.updated",
        durable: None,
    };
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectoriesUpdated {
    #[serde(rename = "projectID")]
    pub project_id: ProjectID,
}
impl EventData for DirectoriesUpdated {
    const DEFINITION: Definition = Definition {
        r#type: "project.directories.updated",
        durable: None,
    };
}

pub const PROJECT_DEFINITIONS: &[Definition] = &[Updated::DEFINITION];
pub const DIRECTORIES_DEFINITIONS: &[Definition] = &[DirectoriesUpdated::DEFINITION];
pub const DEFINITIONS: &[Definition] = &[Updated::DEFINITION, DirectoriesUpdated::DEFINITION];

/// ProjectCopy.StrategyID, centralized with the other branded identifiers.
pub type StrategyID = ProjectCopyStrategyID;
pub type StrategyError = crate::ids::ProjectCopyStrategyError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateInput {
    #[serde(rename = "projectID")]
    pub project_id: ProjectID,
    pub strategy: ProjectCopyStrategyID,
    #[serde(rename = "sourceDirectory")]
    pub source_directory: AbsolutePath,
    pub directory: AbsolutePath,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoveInput {
    #[serde(rename = "projectID")]
    pub project_id: ProjectID,
    pub directory: AbsolutePath,
    pub force: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Copy {
    pub directory: AbsolutePath,
}
