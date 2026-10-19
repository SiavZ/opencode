//! Purpose: `revert.ts` (Revert.State, File.Diff) and `file-diff.ts` (SnapshotFileDiff).
//! Scope: pure values.
//! Uses:
//! rust/crates/
//! |-- schema/src/ids.rs
//! `-- schema/src/values.rs
//! External:
//! |-- serde::Deserialize
//! |-- serde::Serialize
//! `-- serde_json::from_str
//! Used by:
//! rust/crates/
//! |-- schema/src/lib.rs
//! |-- schema/src/session.rs
//! |-- schema/src/session_event/stream.rs
//! |-- session-api/src/info.rs
//! `-- session-runtime/src/revert.rs
//! Invariants: File.Diff counts are non-negative safe integers, SnapshotFileDiff counts are any
//! finite number; status literal sets differ in order only (`added|modified|deleted`).
//! Contract tests: schema/src/revert.rs (unit tests), schema/tests/schema_session.rs

use crate::ids::MessageID;
use crate::values::{Finite, NonNegativeInt, RelativePath, optional};
use serde::{Deserialize, Serialize};

/// File change status shared by `File.Diff` and `SnapshotFileDiff`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileStatus {
    Added,
    Modified,
    Deleted,
}

/// `File.Diff` (`revert.ts` `FileDiff`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileDiff {
    pub path: RelativePath,
    pub status: FileStatus,
    pub additions: NonNegativeInt,
    pub deletions: NonNegativeInt,
    pub patch: String,
}

/// `Revert.State`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct State {
    #[serde(rename = "messageID")]
    pub message_id: MessageID,
    #[serde(
        rename = "partID",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub part_id: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub snapshot: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub diff: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub files: Option<Vec<FileDiff>>,
}

/// `SnapshotFileDiff` (`file-diff.ts` `FileDiff.Info`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SnapshotFileDiff {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub file: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub patch: Option<String>,
    pub additions: Finite,
    pub deletions: Finite,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub status: Option<FileStatus>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_order_and_validation() {
        let state: State =
            serde_json::from_str(r#"{"files":[{"patch":"p","deletions":0,"additions":2,"status":"added","path":"a"}],"messageID":"msg_1"}"#)
                .unwrap();
        assert_eq!(
            serde_json::to_string(&state).unwrap(),
            r#"{"messageID":"msg_1","files":[{"path":"a","status":"added","additions":2,"deletions":0,"patch":"p"}]}"#
        );
        assert!(serde_json::from_str::<State>(r#"{"messageID":"msg_1","diff":null}"#).is_err());
        assert!(
            serde_json::from_str::<FileDiff>(
                r#"{"path":"a","status":"added","additions":-1,"deletions":0,"patch":""}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<SnapshotFileDiff>(
                r#"{"additions":1.5,"deletions":0,"status":"renamed"}"#
            )
            .is_err()
        );
        let snap: SnapshotFileDiff =
            serde_json::from_str(r#"{"additions":1.5,"deletions":0}"#).unwrap();
        assert_eq!(
            serde_json::to_string(&snap).unwrap(),
            r#"{"additions":1.5,"deletions":0}"#
        );
    }
}
