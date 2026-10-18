//! Purpose: serialized, identity-aware file mutations (`packages/core/src/file-mutation.ts`):
//! create without replace, write, BOM-preserving text write, conditional write, remove.
//! Scope: per-canonical-target async keyed lock around native writes.
//! Uses:
//! rust/crates/
//! |-- platform-api/src/fs.rs
//! |-- platform-native/src/fs/error.rs
//! `-- platform-native/src/fs/native.rs
//! External:
//! |-- async_trait::async_trait
//! `-- tokio::sync::Mutex
//! Used by:
//! rust/crates/
//! |-- platform-native/src/fs/mod.rs
//! `-- platform-native/tests/fs_mutation.rs
//! Invariants: every operation holds the lock for `target.canonical` for its whole body; the
//! conditional write compares and writes under that same lock (file-mutation.ts:69-82); each body
//! runs in its own task so caller cancellation cannot interrupt a started write (baseline
//! `Effect.uninterruptible`); lock entries are dropped once no holder or waiter remains.
//! Contract tests: platform-native/tests/fs_mutation.rs

use super::error::{io_error, join_error};
use super::native::NativeFileSystem;
use async_trait::async_trait;
use opencode_platform_api::PlatformErrorKind;
use opencode_platform_api::fs::{
    FileMutation, FileSystem, MutationError, MutationOperation, MutationResult, Target,
};
use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const BOM: char = '\u{feff}';

#[derive(Debug, Clone, Default)]
pub struct NativeFileMutation {
    fs: NativeFileSystem,
    locks: Arc<Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>>,
}

impl NativeFileMutation {
    pub fn new() -> Self {
        Self::default()
    }

    /// Run `body` under the canonical target lock in a detached-but-joined task.
    async fn locked<T, F, Fut>(&self, target: &Target, body: F) -> Result<T, MutationError>
    where
        T: Send + 'static,
        F: FnOnce(NativeFileSystem) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, MutationError>> + Send + 'static,
    {
        let key = target.canonical.clone();
        let entry = {
            let mut locks = self
                .locks
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            locks.entry(key.clone()).or_default().clone()
        };
        let locks = self.locks.clone();
        let fs = self.fs.clone();
        let task = tokio::spawn(async move {
            let result = {
                let _guard = entry.lock().await;
                body(fs).await
            };
            let mut map = locks.lock().unwrap_or_else(|poison| poison.into_inner());
            // Two references remain when idle: the map's and this task's `entry`.
            if map
                .get(&key)
                .is_some_and(|held| Arc::strong_count(held) <= 2)
            {
                map.remove(&key);
            }
            result
        });
        task.await
            .map_err(|error| join_error("mutation", &target.canonical, error))?
    }

    /// Number of live lock entries. Exposed for leak tests.
    pub fn lock_entries(&self) -> usize {
        self.locks.lock().map(|map| map.len()).unwrap_or(0)
    }
}

fn write_result(target: &Target, existed: bool) -> MutationResult {
    MutationResult {
        operation: MutationOperation::Write,
        target: target.canonical.clone(),
        resource: target.resource.clone(),
        existed,
    }
}

/// `splitBom`: strip every leading U+FEFF.
pub fn split_bom(text: &str) -> (bool, &str) {
    let stripped = text.trim_start_matches(BOM);
    (stripped.len() != text.len(), stripped)
}

/// `joinBom`: at most one BOM.
pub fn join_bom(text: &str, bom: bool) -> String {
    let (_, stripped) = split_bom(text);
    if bom {
        return format!("{BOM}{stripped}");
    }
    stripped.to_string()
}

pub fn has_utf8_bom(content: &[u8]) -> bool {
    content.starts_with(&[0xef, 0xbb, 0xbf])
}

async fn create_new(path: &Path, content: &[u8]) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt;
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .await?;
    file.write_all(content).await?;
    file.flush().await
}

#[async_trait]
impl FileMutation for NativeFileMutation {
    async fn create(
        &self,
        target: &Target,
        content: &[u8],
    ) -> Result<MutationResult, MutationError> {
        let path = target.canonical.clone();
        let content = content.to_vec();
        self.locked(target, move |fs| async move {
            let attempt = match create_new(&path, &content).await {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    let parent = path.parent().unwrap_or(&path).to_path_buf();
                    fs.ensure_dir(&parent).await?;
                    create_new(&path, &content).await
                }
                other => other,
            };
            match attempt {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    Err(MutationError::TargetExists {
                        path: path.to_string_lossy().into_owned(),
                    })
                }
                Err(error) => Err(io_error("writeFile", &path, &error).into()),
            }
        })
        .await?;
        Ok(write_result(target, false))
    }

    async fn write(
        &self,
        target: &Target,
        content: &[u8],
    ) -> Result<MutationResult, MutationError> {
        let path = target.canonical.clone();
        let content = content.to_vec();
        let existed = self
            .locked(target, move |fs| async move {
                let existed = fs.exists(&path).await?;
                fs.write_with_dirs(&path, &content).await?;
                Ok(existed)
            })
            .await?;
        Ok(write_result(target, existed))
    }

    async fn write_text_preserving_bom(
        &self,
        target: &Target,
        content: &str,
    ) -> Result<MutationResult, MutationError> {
        let path = target.canonical.clone();
        let content = content.to_string();
        let existed = self
            .locked(target, move |fs| async move {
                let (bom, text) = split_bom(&content);
                let current = match fs.read(&path).await {
                    Ok(bytes) => Some(bytes),
                    Err(error) if error.kind == PlatformErrorKind::NotFound => None,
                    Err(error) => return Err(error.into()),
                };
                let keep = current.as_deref().is_some_and(has_utf8_bom) || bom;
                fs.write_with_dirs(&path, join_bom(text, keep).as_bytes())
                    .await?;
                Ok(current.is_some())
            })
            .await?;
        Ok(write_result(target, existed))
    }

    async fn write_if_unchanged(
        &self,
        target: &Target,
        content: &[u8],
        expected: &[u8],
    ) -> Result<MutationResult, MutationError> {
        let path = target.canonical.clone();
        let content = content.to_vec();
        let expected = expected.to_vec();
        self.locked(target, move |fs| async move {
            let current = fs.read(&path).await?;
            if current != expected {
                return Err(MutationError::StaleContent {
                    path: path.to_string_lossy().into_owned(),
                });
            }
            tokio::fs::write(&path, &content)
                .await
                .map_err(|error| io_error("writeFile", &path, &error))?;
            Ok(())
        })
        .await?;
        Ok(write_result(target, true))
    }

    async fn remove(&self, target: &Target) -> Result<MutationResult, MutationError> {
        let path = target.canonical.clone();
        let existed = self
            .locked(target, move |_| async move {
                match remove_any(&path).await {
                    Ok(()) => Ok(true),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
                    Err(error) => Err(io_error("remove", &path, &error).into()),
                }
            })
            .await?;
        Ok(MutationResult {
            operation: MutationOperation::Remove,
            target: target.canonical.clone(),
            resource: target.resource.clone(),
            existed,
        })
    }
}

/// effect `fs.remove(path)` is node `fs.rm` without `recursive`: files and symlinks unlink, any
/// directory (even empty) fails with EISDIR.
async fn remove_any(path: &Path) -> std::io::Result<()> {
    let info = tokio::fs::symlink_metadata(path).await?;
    if info.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::IsADirectory,
            "Path is a directory",
        ));
    }
    tokio::fs::remove_file(path).await
}
