//! Purpose: PTY session contract: create/list/update/remove/write and attach with bounded replay,
//! absolute cursor and explicit live activation (baseline packages/core/src/pty.ts).
//! Scope: traits and wire-compatible values. Native backend lives in platform-native.
//! Uses:
//! rust/crates/
//! `-- platform-api/src/error.rs
//! External:
//! `-- async_trait::async_trait
//! Used by:
//! rust/crates/
//! `-- platform-api/src/lib.rs
//! Invariants: cursor units are UTF-16 code units of decoded output, like the baseline JS string
//! `length` (pty.ts:205). Replay buffer is bounded to BUFFER_LIMIT, exited sessions retained up to
//! EXITED_LIMIT. Live delivery starts only after `activate`.
//! Contract tests: platform-native/tests/pty.rs

use crate::error::PlatformError;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Retained output bound, `BUFFER_LIMIT` (2 MiB of code units).
pub const BUFFER_LIMIT: usize = 1024 * 1024 * 2;
/// Exited sessions retained until removed, `EXITED_LIMIT`.
pub const EXITED_LIMIT: usize = 25;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PtyStatus {
    Running,
    Exited,
}

/// `Pty.Info` wire shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PtyInfo {
    pub id: String,
    pub title: String,
    pub command: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub status: PtyStatus,
    pub pid: u32,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub exit_code: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PtyCreateInput {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub args: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub env: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PtySize {
    pub rows: u16,
    pub cols: u16,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PtyUpdateInput {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub size: Option<PtySize>,
}

/// Where replay starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReplayFrom {
    /// Replay the full retained buffer (cursor omitted).
    #[default]
    Start,
    /// Tail from the current end (cursor `-1`).
    End,
    /// Absolute cursor, clamped to the retained window.
    Cursor(u64),
}

/// Output delivered to an attached subscriber.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PtyOutput {
    Data(String),
    /// Session stopped producing output (exit, removal or teardown). Fired once.
    End {
        exit_code: Option<u32>,
    },
}

/// Lifecycle events published by the PTY service (`pty.created`, `pty.updated`, ...).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PtyEvent {
    Created(PtyInfo),
    Updated(PtyInfo),
    Exited { id: String, exit_code: u32 },
    Deleted { id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PtyError {
    #[error("pty not found: {0}")]
    NotFound(String),
    #[error("pty exited: {0}")]
    Exited(String),
    #[error(transparent)]
    Platform(#[from] PlatformError),
}

/// A subscriber handle returned by `attach`.
#[async_trait]
pub trait PtyAttachment: Send + Sync + std::fmt::Debug {
    /// Retained output from the requested cursor to the end at attach time.
    fn replay(&self) -> &str;
    /// Absolute cursor after replay.
    fn cursor(&self) -> u64;
    fn write(&self, data: &str) -> Result<(), PtyError>;
    /// Start live delivery after the caller applied replay and cursor. Output produced between
    /// attach and activate is queued, never dropped or duplicated.
    fn activate(&self);
    /// Next live output, `None` after `End` was delivered or after detach.
    async fn next(&self) -> Option<PtyOutput>;
    fn detach(&self);
}

#[async_trait]
pub trait PtyService: Send + Sync + std::fmt::Debug {
    async fn list(&self) -> Vec<PtyInfo>;
    async fn get(&self, id: &str) -> Result<PtyInfo, PtyError>;
    async fn create(&self, input: PtyCreateInput) -> Result<PtyInfo, PtyError>;
    async fn update(&self, id: &str, input: PtyUpdateInput) -> Result<PtyInfo, PtyError>;
    async fn remove(&self, id: &str) -> Result<(), PtyError>;
    async fn write(&self, id: &str, data: &str) -> Result<(), PtyError>;
    async fn attach(&self, id: &str, from: ReplayFrom) -> Result<Box<dyn PtyAttachment>, PtyError>;
    /// Kill all sessions and join their reader tasks.
    async fn shutdown(&self);
}
