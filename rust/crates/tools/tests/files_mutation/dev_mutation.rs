//! Purpose: dev-only `FileMutation` over the counting `Disk` fixture, mirroring
//! packages/core/src/file-mutation.ts (per-target locks, BOM-preserving and conditional writes).
//! Scope: test support only (labelled dev adapter until p05.native-fs lands the production one).
//! Uses:
//! rust/crates/
//! |-- platform-api/src/fs.rs
//! `-- tools/tests/files_mutation/support.rs
//! External:
//! `-- async_trait::async_trait
//! Used by:
//! rust/crates/
//! `-- tools/tests/files_mutation/support.rs
//! Invariants: compare-and-write holds the target lock, writes go through `Disk::write_with_dirs`.
//! Contract tests: tools/tests/files_edit.rs, tools/tests/files_write.rs

use super::Disk;
use async_trait::async_trait;
use opencode_platform_api::PlatformErrorKind;
use opencode_platform_api::fs::{
    FileMutation, FileSystem, MutationError, MutationOperation, MutationResult, Target,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Dev-only `FileMutation` over `Disk`, mirroring packages/core/src/file-mutation.ts.
#[derive(Debug)]
pub struct DevMutation {
    disk: Arc<Disk>,
    locks: Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>,
}

impl DevMutation {
    pub fn new(disk: Arc<Disk>) -> Self {
        Self {
            disk,
            locks: Mutex::default(),
        }
    }
    fn lock(&self, target: &Target) -> Arc<tokio::sync::Mutex<()>> {
        self.locks
            .lock()
            .unwrap()
            .entry(target.canonical.clone())
            .or_default()
            .clone()
    }
    fn result(target: &Target, existed: bool) -> MutationResult {
        MutationResult {
            operation: MutationOperation::Write,
            target: target.canonical.clone(),
            resource: target.resource.clone(),
            existed,
        }
    }
}

const BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

#[async_trait]
impl FileMutation for DevMutation {
    async fn create(
        &self,
        target: &Target,
        content: &[u8],
    ) -> Result<MutationResult, MutationError> {
        let lock = self.lock(target);
        let _guard = lock.lock().await;
        if target.canonical.exists() {
            return Err(MutationError::TargetExists {
                path: target.canonical.to_string_lossy().into(),
            });
        }
        self.disk
            .write_with_dirs(&target.canonical, content)
            .await?;
        Ok(Self::result(target, false))
    }
    async fn write(
        &self,
        target: &Target,
        content: &[u8],
    ) -> Result<MutationResult, MutationError> {
        let lock = self.lock(target);
        let _guard = lock.lock().await;
        let existed = target.canonical.exists();
        self.disk
            .write_with_dirs(&target.canonical, content)
            .await?;
        Ok(Self::result(target, existed))
    }
    async fn write_text_preserving_bom(
        &self,
        target: &Target,
        content: &str,
    ) -> Result<MutationResult, MutationError> {
        let lock = self.lock(target);
        let _guard = lock.lock().await;
        let stripped = content.trim_start_matches('\u{FEFF}');
        let current = match self.disk.read(&target.canonical).await {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind == PlatformErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        let bom = current
            .as_deref()
            .is_some_and(|bytes| bytes.starts_with(BOM))
            || stripped.len() != content.len();
        let text = if bom {
            format!("\u{FEFF}{stripped}")
        } else {
            stripped.to_string()
        };
        self.disk
            .write_with_dirs(&target.canonical, text.as_bytes())
            .await?;
        Ok(Self::result(target, current.is_some()))
    }
    async fn write_if_unchanged(
        &self,
        target: &Target,
        content: &[u8],
        expected: &[u8],
    ) -> Result<MutationResult, MutationError> {
        let lock = self.lock(target);
        let _guard = lock.lock().await;
        let current = self.disk.read(&target.canonical).await?;
        if current != expected {
            return Err(MutationError::StaleContent {
                path: target.canonical.to_string_lossy().into(),
            });
        }
        self.disk
            .write_with_dirs(&target.canonical, content)
            .await?;
        Ok(Self::result(target, true))
    }
    async fn remove(&self, target: &Target) -> Result<MutationResult, MutationError> {
        let existed = std::fs::remove_file(&target.canonical).is_ok();
        Ok(MutationResult {
            operation: MutationOperation::Remove,
            ..Self::result(target, existed)
        })
    }
}
