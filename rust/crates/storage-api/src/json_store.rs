//! Purpose: contract for the surviving legacy JSON key/value store (`packages/opencode/src/storage/storage.ts:53-59`).
//! Scope: async trait and key helpers. Files are `<dir>/<key...>.json`, pretty-printed with two spaces.
//! Uses:
//! rust/crates/
//! `-- storage-api/src/error.rs
//! External:
//! |-- async_trait::async_trait
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! `-- storage-api/src/lib.rs
//! Invariants: reads take a per-file read lock, update/write/remove a per-file write lock. Missing files
//! read as `NotFound("<absolute path>")`, missing removes succeed, list of a missing prefix is empty and
//! sorted by joined key.
//! Contract tests: storage-sqlite/tests/legacy_json.rs

use async_trait::async_trait;
use serde_json::Value;

use crate::error::Result;

/// Mutation applied by [`JsonStore::update`] while the write lock is held.
pub type Update = Box<dyn FnOnce(&mut Value) + Send + 'static>;

/// Legacy JSON store. Implemented by `opencode-storage-sqlite` (`legacy_files`).
#[async_trait]
pub trait JsonStore: Send + Sync + 'static {
    async fn read(&self, key: &[String]) -> Result<Value>;
    async fn write(&self, key: &[String], content: &Value) -> Result<()>;
    async fn update(&self, key: &[String], update: Update) -> Result<Value>;
    async fn remove(&self, key: &[String]) -> Result<()>;
    async fn list(&self, prefix: &[String]) -> Result<Vec<Vec<String>>>;
}

/// Build a key from string parts.
pub fn key<I, S>(parts: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    parts.into_iter().map(Into::into).collect()
}
