//! Purpose: filesystem identity, read and mutation contracts (canonical targets, BOM-preserving
//! writes, conditional writes under per-target locks, encoding detection results).
//! Scope: traits and values. Native implementation lives in platform-native.
//! Uses:
//! rust/crates/
//! `-- platform-api/src/error.rs
//! External:
//! `-- async_trait::async_trait
//! Used by:
//! rust/crates/
//! `-- platform-api/src/lib.rs
//! Invariants: mutations serialize per canonical target, conditional writes compare and write under
//! the same lock (baseline packages/core/src/file-mutation.ts:69-82).
//! Contract tests: platform-native/tests/mutation.rs

use crate::error::PlatformError;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileType {
    File,
    Directory,
    SymbolicLink,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Metadata {
    pub file_type: FileType,
    pub size: u64,
    pub modified: Option<SystemTime>,
    /// Stable identity (device, inode) where the OS provides one.
    pub identity: Option<(u64, u64)>,
}

/// A canonical mutation target, `packages/core/src/file-mutation.ts` `Target`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// Canonical existing path, or a missing path below a canonical directory.
    pub canonical: PathBuf,
    /// Permission resource: Location-relative for internal paths, canonical for external paths.
    pub resource: String,
    /// Separate approval boundary for explicit absolute external targets.
    pub external_directory: Option<ExternalDirectoryAuthorization>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalDirectoryAuthorization {
    pub directory: PathBuf,
    pub resource: String,
    pub save: String,
}

impl ExternalDirectoryAuthorization {
    pub fn permission(&self) -> (&'static str, Vec<String>, Vec<String>) {
        (
            "external_directory",
            vec![self.resource.clone()],
            vec![self.save.clone()],
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResolveKind {
    #[default]
    File,
    Directory,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolveInput {
    pub path: String,
    pub kind: Option<ResolveKind>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathErrorReason {
    RelativeEscape,
    LocationEscape,
    NonDirectoryAncestor,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PathError {
    #[error("LocationMutation.PathError: {path} ({reason:?})")]
    Invalid {
        path: String,
        reason: PathErrorReason,
    },
    #[error(transparent)]
    Platform(#[from] PlatformError),
}

/// Location-scoped path resolution, separate from serialized file mutations.
#[async_trait]
pub trait LocationMutation: Send + Sync + std::fmt::Debug {
    async fn resolve(&self, input: ResolveInput) -> Result<Target, PathError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MutationOperation {
    Write,
    Remove,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MutationResult {
    pub operation: MutationOperation,
    pub target: PathBuf,
    pub resource: String,
    pub existed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MutationError {
    #[error("stale content: {path}")]
    StaleContent { path: String },
    #[error("target exists: {path}")]
    TargetExists { path: String },
    #[error(transparent)]
    Platform(#[from] PlatformError),
}

/// Detected text encoding of file bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextEncoding {
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
    /// Not text (binary heuristics matched).
    Binary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LineEnding {
    Lf,
    CrLf,
}

/// Raw filesystem operations. Paths are absolute.
#[async_trait]
pub trait FileSystem: Send + Sync + std::fmt::Debug {
    async fn read(&self, path: &Path) -> Result<Vec<u8>, PlatformError>;
    /// Read at most `len` bytes starting at byte `offset`. An empty result means end of file.
    /// Bounded readers (read tool paging, media ingestion caps) rely on this to stop early. The
    /// default reads the whole file, native adapters should override it with a positioned read.
    async fn read_range(
        &self,
        path: &Path,
        offset: u64,
        len: usize,
    ) -> Result<Vec<u8>, PlatformError> {
        let bytes = self.read(path).await?;
        let start = usize::try_from(offset)
            .unwrap_or(usize::MAX)
            .min(bytes.len());
        let end = start.saturating_add(len).min(bytes.len());
        Ok(bytes[start..end].to_vec())
    }
    async fn read_to_string(&self, path: &Path) -> Result<String, PlatformError>;
    async fn metadata(&self, path: &Path) -> Result<Metadata, PlatformError>;
    /// lstat: does not follow the final symlink.
    async fn symlink_metadata(&self, path: &Path) -> Result<Metadata, PlatformError>;
    async fn exists(&self, path: &Path) -> Result<bool, PlatformError>;
    async fn real_path(&self, path: &Path) -> Result<PathBuf, PlatformError>;
    async fn read_dir(&self, path: &Path) -> Result<Vec<(String, FileType)>, PlatformError>;
    async fn create_dir_all(&self, path: &Path) -> Result<(), PlatformError>;
    /// Write, creating parent directories.
    async fn write_with_dirs(&self, path: &Path, content: &[u8]) -> Result<(), PlatformError>;
    async fn remove_file(&self, path: &Path) -> Result<(), PlatformError>;
    async fn remove_dir_all(&self, path: &Path) -> Result<(), PlatformError>;
    async fn rename(&self, from: &Path, to: &Path) -> Result<(), PlatformError>;
}

/// Serialized, identity-aware mutations (`FileMutation.Interface`).
#[async_trait]
pub trait FileMutation: Send + Sync + std::fmt::Debug {
    /// Create without replacing an existing target.
    async fn create(
        &self,
        target: &Target,
        content: &[u8],
    ) -> Result<MutationResult, MutationError>;
    async fn write(&self, target: &Target, content: &[u8])
    -> Result<MutationResult, MutationError>;
    /// Write text while retaining an existing UTF-8 BOM and emitting at most one BOM.
    async fn write_text_preserving_bom(
        &self,
        target: &Target,
        content: &str,
    ) -> Result<MutationResult, MutationError>;
    /// Commit only if the existing target still has the expected bytes.
    async fn write_if_unchanged(
        &self,
        target: &Target,
        content: &[u8],
        expected: &[u8],
    ) -> Result<MutationResult, MutationError>;
    async fn remove(&self, target: &Target) -> Result<MutationResult, MutationError>;
}
