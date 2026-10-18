//! Purpose: snapshot contract (content-addressed trees of a Location) and the shared file diff value.
//! Scope: trait and values, baseline packages/core/src/snapshot.ts:15-82.
//! Uses:
//! rust/crates/
//! `-- platform-api/src/error.rs
//! External:
//! `-- async_trait::async_trait
//! Used by:
//! rust/crates/
//! `-- platform-api/src/lib.rs
//! Invariants: `capture` is best effort and returns None when disabled/unsupported/failed.
//! Contract tests: platform-native/tests/snapshot.rs

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiffStatus {
    Added,
    Deleted,
    Modified,
}

/// `SnapshotFileDiff` wire shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileDiff {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub patch: Option<String>,
    pub additions: u64,
    pub deletions: u64,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub status: Option<DiffStatus>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SnapshotOperation {
    Capture,
    Files,
    Diff,
    Preview,
    Restore,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("snapshot {operation:?} failed: {message}")]
pub struct SnapshotError {
    pub operation: SnapshotOperation,
    pub message: String,
}

#[async_trait]
pub trait Snapshots: Send + Sync + std::fmt::Debug {
    async fn capture(&self) -> Option<String>;
    async fn files(&self, from: &str, to: &str) -> Result<Vec<String>, SnapshotError>;
    async fn diff(
        &self,
        from: &str,
        to: &str,
        context: Option<u32>,
        paths: Option<&[String]>,
    ) -> Result<Vec<FileDiff>, SnapshotError>;
    /// Keys are project-relative paths, values the tree each path is restored from.
    async fn preview(
        &self,
        files: &BTreeMap<String, String>,
        context: Option<u32>,
    ) -> Result<Vec<FileDiff>, SnapshotError>;
    async fn restore(&self, files: &BTreeMap<String, String>) -> Result<(), SnapshotError>;
    async fn checkout(&self, snapshot: &str) -> Result<(), SnapshotError>;
}
