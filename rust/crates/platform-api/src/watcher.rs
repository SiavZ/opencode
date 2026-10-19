//! Purpose: filesystem watcher contract with coalescing and explicit overflow signalling.
//! Scope: values and trait. Native implementation (notify) lives in platform-native.
//! Uses:
//! rust/crates/
//! `-- platform-api/src/error.rs
//! External:
//! |-- async_trait::async_trait
//! |-- serde::Deserialize
//! `-- serde::Serialize
//! Used by:
//! rust/crates/
//! |-- platform-api/src/lib.rs
//! |-- platform-native/src/fs/watcher.rs
//! `-- platform-native/tests/fs_watcher.rs
//! Invariants: bounded queue; when it fills, events are dropped and one `Overflow` is delivered so
//! consumers rescan instead of trusting a partial stream.
//! Contract tests: platform-native/tests/watcher.rs

use crate::error::PlatformError;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

/// `file.watcher.updated` event kinds from the baseline watcher.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WatchKind {
    Add,
    Change,
    Unlink,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchEvent {
    Changed {
        path: PathBuf,
        kind: WatchKind,
    },
    /// Events were dropped. Consumers must rescan.
    Overflow,
}

#[derive(Debug, Clone)]
pub struct WatchOptions {
    pub recursive: bool,
    /// Glob-style ignore patterns relative to the root.
    pub ignore: Vec<String>,
    /// Window over which repeated events for one path are coalesced.
    pub coalesce: Duration,
    /// Queue capacity before overflow.
    pub capacity: usize,
}

impl Default for WatchOptions {
    fn default() -> Self {
        Self {
            recursive: true,
            ignore: Vec::new(),
            coalesce: Duration::from_millis(50),
            capacity: 1024,
        }
    }
}

#[async_trait]
pub trait WatchHandle: Send + Sync + std::fmt::Debug {
    async fn next(&self) -> Option<WatchEvent>;
    /// Stop watching and join the background task.
    async fn close(&self);
}

#[async_trait]
pub trait Watcher: Send + Sync + std::fmt::Debug {
    async fn watch(
        &self,
        root: PathBuf,
        options: WatchOptions,
    ) -> Result<Box<dyn WatchHandle>, PlatformError>;
}
