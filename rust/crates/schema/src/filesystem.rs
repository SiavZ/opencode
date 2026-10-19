//! Purpose: filesystem search values, input DTOs and file/watcher event definitions.
//! Scope: pure values from filesystem.ts and filesystem-watcher.ts.
//! Uses:
//! rust/crates/
//! |-- schema/src/event.rs
//! `-- schema/src/values.rs
//! External:
//! |-- serde::Deserialize
//! `-- serde::Serialize
//! Used by:
//! rust/crates/
//! |-- schema/src/lib.rs
//! |-- schema/tests/schema_domains_filesystem.rs
//! |-- tools/src/search/core.rs
//! |-- tools/src/search/ripgrep.rs
//! `-- tools/tests/search_ripgrep.rs
//! Invariants: baseline field order, safe integer bounds and optional null rejection are retained.
//! Contract tests: schema/tests/schema_domains_filesystem.rs

use crate::event::{Definition, EventData};
use crate::values::{NonNegativeInt, PositiveInt, RelativePath, optional};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryType {
    File,
    Directory,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub path: RelativePath,
    pub r#type: EntryType,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Submatch {
    pub text: String,
    pub start: NonNegativeInt,
    pub end: NonNegativeInt,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Match {
    pub entry: Entry,
    pub line: PositiveInt,
    pub offset: NonNegativeInt,
    pub text: String,
    pub submatches: Vec<Submatch>,
}

/// FileSystem.FindInput's Schema.Class has no additional defaults or methods.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindInput {
    pub query: String,
    #[serde(
        rename = "type",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub entry_type: Option<EntryType>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub limit: Option<PositiveInt>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edited {
    pub file: String,
}
impl EventData for Edited {
    const DEFINITION: Definition = Definition {
        r#type: "file.edited",
        durable: None,
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WatcherEvent {
    Add,
    Change,
    Unlink,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatcherUpdated {
    pub file: String,
    pub event: WatcherEvent,
}
impl EventData for WatcherUpdated {
    const DEFINITION: Definition = Definition {
        r#type: "file.watcher.updated",
        durable: None,
    };
}

pub const FILE_DEFINITIONS: &[Definition] = &[Edited::DEFINITION];
pub const WATCHER_DEFINITIONS: &[Definition] = &[WatcherUpdated::DEFINITION];
pub const DEFINITIONS: &[Definition] = &[Edited::DEFINITION, WatcherUpdated::DEFINITION];
