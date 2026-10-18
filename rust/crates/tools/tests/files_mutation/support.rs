//! Purpose: test fixtures for the edit/write suites: a std::fs-backed `FileSystem` that counts
//! content reads and records writes (with an after-read hook for race tests), a dev-only
//! `FileMutation` with per-target locks and conditional writes, permission/progress/hook recorders.
//! Scope: test support only (labelled dev adapters, not the production native filesystem).
//! Uses:
//! rust/crates/
//! |-- platform-api/src/fs.rs
//! |-- platform-native/src/fs/location.rs
//! |-- tool-api/src/context.rs
//! |-- tools/src/files/common.rs
//! |-- tools/src/files/edit_legacy.rs
//! `-- tools/tests/files_mutation/dev_mutation.rs
//! External:
//! |-- async_trait::async_trait
//! `-- tempfile::TempDir
//! Used by:
//! rust/crates/
//! |-- tools/tests/files_edit.rs
//! `-- tools/tests/files_write.rs
//! Invariants: every content byte goes through `Disk::read`, every write through
//! `Disk::write_with_dirs`, mutation compare-and-write holds the target lock (file-mutation.ts).
//! Contract tests: tools/tests/files_edit.rs, tools/tests/files_write.rs

#![allow(dead_code)]

#[path = "dev_mutation.rs"]
mod dev_mutation;
pub use dev_mutation::DevMutation;

use async_trait::async_trait;
use opencode_permissions::{AssertInput, PermissionError};
use opencode_platform_api::fs::{FileSystem, FileType, Metadata};
use opencode_platform_api::{PlatformError, PlatformErrorKind};
use opencode_platform_native::fs::NativeLocationMutation;
use opencode_tool_api::{Authorizer, ToolContext, ToolProgress};
use opencode_tools::files::FileServices;
use opencode_tools::files::edit_legacy::{EditHooks, Event};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

type AfterRead = Box<dyn Fn(&Path, usize) + Send + Sync>;

#[derive(Default)]
pub struct Disk {
    pub reads: Mutex<usize>,
    pub writes: Mutex<Vec<PathBuf>>,
    pub after_read: Mutex<Option<AfterRead>>,
}

impl std::fmt::Debug for Disk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Disk").finish_non_exhaustive()
    }
}

impl Disk {
    pub fn reads(&self) -> usize {
        *self.reads.lock().unwrap()
    }
    pub fn writes(&self) -> Vec<PathBuf> {
        self.writes.lock().unwrap().clone()
    }
    pub fn reset(&self) {
        *self.reads.lock().unwrap() = 0;
        self.writes.lock().unwrap().clear();
        *self.after_read.lock().unwrap() = None;
    }
    pub fn on_read(&self, hook: impl Fn(&Path, usize) + Send + Sync + 'static) {
        *self.after_read.lock().unwrap() = Some(Box::new(hook));
    }
}

pub fn error(operation: &str, path: &Path, error: std::io::Error) -> PlatformError {
    let kind = match error.kind() {
        std::io::ErrorKind::NotFound => PlatformErrorKind::NotFound,
        std::io::ErrorKind::NotADirectory => PlatformErrorKind::NotADirectory,
        std::io::ErrorKind::PermissionDenied => PlatformErrorKind::PermissionDenied,
        _ => PlatformErrorKind::Io,
    };
    PlatformError::new(kind, operation, path.to_string_lossy(), error.to_string())
}

fn metadata(info: std::fs::Metadata) -> Metadata {
    let file_type = if info.is_symlink() {
        FileType::SymbolicLink
    } else if info.is_dir() {
        FileType::Directory
    } else if info.is_file() {
        FileType::File
    } else {
        FileType::Other
    };
    Metadata {
        file_type,
        size: info.len(),
        modified: info.modified().ok(),
        identity: None,
    }
}

#[async_trait]
impl FileSystem for Disk {
    async fn read(&self, path: &Path) -> Result<Vec<u8>, PlatformError> {
        let bytes = std::fs::read(path).map_err(|e| error("readFile", path, e))?;
        let count = {
            let mut reads = self.reads.lock().unwrap();
            *reads += 1;
            *reads
        };
        if let Some(hook) = self.after_read.lock().unwrap().as_ref() {
            hook(path, count);
        }
        Ok(bytes)
    }
    async fn read_to_string(&self, path: &Path) -> Result<String, PlatformError> {
        let bytes = self.read(path).await?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }
    async fn metadata(&self, path: &Path) -> Result<Metadata, PlatformError> {
        std::fs::metadata(path)
            .map(metadata)
            .map_err(|e| error("stat", path, e))
    }
    async fn symlink_metadata(&self, path: &Path) -> Result<Metadata, PlatformError> {
        std::fs::symlink_metadata(path)
            .map(metadata)
            .map_err(|e| error("lstat", path, e))
    }
    async fn exists(&self, path: &Path) -> Result<bool, PlatformError> {
        Ok(path.exists())
    }
    async fn real_path(&self, path: &Path) -> Result<PathBuf, PlatformError> {
        std::fs::canonicalize(path).map_err(|e| error("realPath", path, e))
    }
    async fn read_dir(&self, path: &Path) -> Result<Vec<(String, FileType)>, PlatformError> {
        let entries = std::fs::read_dir(path).map_err(|e| error("readDirectory", path, e))?;
        Ok(entries
            .flatten()
            .map(|entry| {
                (
                    entry.file_name().to_string_lossy().into_owned(),
                    FileType::Other,
                )
            })
            .collect())
    }
    async fn create_dir_all(&self, path: &Path) -> Result<(), PlatformError> {
        std::fs::create_dir_all(path).map_err(|e| error("mkdir", path, e))
    }
    async fn write_with_dirs(&self, path: &Path, content: &[u8]) -> Result<(), PlatformError> {
        self.writes.lock().unwrap().push(path.to_path_buf());
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| error("mkdir", parent, e))?;
        }
        std::fs::write(path, content).map_err(|e| error("writeFile", path, e))
    }
    async fn remove_file(&self, path: &Path) -> Result<(), PlatformError> {
        std::fs::remove_file(path).map_err(|e| error("remove", path, e))
    }
    async fn remove_dir_all(&self, path: &Path) -> Result<(), PlatformError> {
        std::fs::remove_dir_all(path).map_err(|e| error("remove", path, e))
    }
    async fn rename(&self, from: &Path, to: &Path) -> Result<(), PlatformError> {
        std::fs::rename(from, to).map_err(|e| error("rename", from, e))
    }
}

#[derive(Default)]
pub struct Recorder {
    pub calls: Mutex<Vec<AssertInput>>,
    pub deny: Mutex<Option<&'static str>>,
    pub delay: Mutex<Option<std::time::Duration>>,
}

impl Recorder {
    pub fn denying(action: &'static str) -> Arc<Self> {
        let recorder = Self::default();
        *recorder.deny.lock().unwrap() = Some(action);
        Arc::new(recorder)
    }
    pub fn actions(&self) -> Vec<String> {
        let calls = self.calls.lock().unwrap();
        calls.iter().map(|call| call.action.clone()).collect()
    }
    pub fn all(&self) -> Vec<AssertInput> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl Authorizer for Recorder {
    async fn assert(&self, input: AssertInput) -> Result<(), PermissionError> {
        let denied = *self.deny.lock().unwrap() == Some(input.action.as_str());
        let first = {
            let mut calls = self.calls.lock().unwrap();
            calls.push(input);
            calls.len() == 1
        };
        let delay = *self.delay.lock().unwrap();
        if let (true, Some(delay)) = (first, delay) {
            tokio::time::sleep(delay).await;
        }
        if denied {
            return Err(PermissionError::Declined);
        }
        Ok(())
    }
}

#[derive(Default)]
pub struct Progress {
    pub updates: Mutex<Vec<Map<String, Value>>>,
}

impl ToolProgress for Progress {
    fn update(&self, _title: Option<&str>, metadata: &Map<String, Value>) {
        self.updates.lock().unwrap().push(metadata.clone());
    }
}

pub fn context(recorder: Arc<Recorder>) -> ToolContext {
    with_progress(recorder, Arc::new(Progress::default()))
}

pub fn with_progress(recorder: Arc<Recorder>, progress: Arc<Progress>) -> ToolContext {
    ToolContext {
        session_id: "ses_edit_tool_test".into(),
        agent: "build".into(),
        assistant_message_id: "msg_test".into(),
        tool_call_id: "call-edit".into(),
        cancel: CancellationToken::new(),
        authorizer: recorder,
        progress,
    }
}

/// Recording legacy hooks. `strip_bom` simulates a formatter that removes a leading BOM.
#[derive(Debug, Default)]
pub struct Hooks {
    pub events: Mutex<Vec<Event>>,
    pub touched: Mutex<Vec<PathBuf>>,
    pub strip_bom: bool,
    pub diagnostics: Map<String, Value>,
}

#[async_trait]
impl EditHooks for Hooks {
    async fn format(&self, file: &Path) -> bool {
        if !self.strip_bom {
            return false;
        }
        let text = std::fs::read_to_string(file).unwrap();
        std::fs::write(file, text.trim_start_matches('\u{FEFF}')).unwrap();
        true
    }
    fn publish(&self, event: Event) {
        self.events.lock().unwrap().push(event);
    }
    async fn touch(&self, file: &Path) {
        self.touched.lock().unwrap().push(file.to_path_buf());
    }
    async fn diagnostics(&self) -> Map<String, Value> {
        self.diagnostics.clone()
    }
}

pub struct Fixture {
    pub _temp: tempfile::TempDir,
    pub root: PathBuf,
    pub disk: Arc<Disk>,
    pub services: FileServices,
}

impl Fixture {
    pub async fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        let disk = Arc::new(Disk::default());
        let location = NativeLocationMutation::new(&root).await.unwrap();
        let services = FileServices::new(
            disk.clone(),
            Arc::new(DevMutation::new(disk.clone())),
            Arc::new(location),
            root.clone(),
            root.clone(),
        );
        Self {
            _temp: temp,
            root,
            disk,
            services,
        }
    }

    pub fn put(&self, relative: &str, content: impl AsRef<[u8]>) -> PathBuf {
        let path = self.root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, content).unwrap();
        path
    }

    pub fn load(&self, relative: &str) -> String {
        std::fs::read_to_string(self.root.join(relative)).unwrap()
    }
}

/// A second canonical temp dir outside the Location.
pub fn outside() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    (temp, root)
}

pub fn s(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
