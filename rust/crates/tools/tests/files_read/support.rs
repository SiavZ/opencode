//! Purpose: test fixtures for the read-tool suites: a std::fs-backed `FileSystem` that records
//! content access, a permission recorder, and Location/tool construction over temp dirs.
//! Scope: test support only (labelled fixture adapter, not a production native filesystem).
//! Uses:
//! rust/crates/
//! |-- permissions/src/lib.rs
//! |-- platform-api/src/fs.rs
//! |-- platform-api/src/lib.rs
//! |-- platform-native/src/fs/mod.rs
//! |-- tool-api/src/lib.rs
//! `-- tools/src/files/mod.rs
//! External:
//! |-- async_trait::async_trait
//! |-- base64::Engine
//! |-- base64::engine::general_purpose::STANDARD
//! |-- tempfile::TempDir
//! |-- tempfile::tempdir
//! `-- tokio_util::sync::CancellationToken
//! Used by:
//! rust/crates/
//! |-- tools/tests/files_read.rs
//! |-- tools/tests/files_read/core.rs
//! |-- tools/tests/files_read/filesystem.rs
//! `-- tools/tests/files_read/legacy.rs
//! Invariants: every content byte read goes through `read`/`read_range` and is counted.
//! Contract tests: tools/tests/files_read.rs

use async_trait::async_trait;
use opencode_permissions::{AssertInput, PermissionError};
use opencode_platform_api::fs::{
    FileMutation, FileSystem, FileType, Metadata, MutationError, MutationResult, Target,
};
use opencode_platform_api::{PlatformError, PlatformErrorKind};
use opencode_platform_native::fs::NativeLocationMutation;
use opencode_tool_api::{Authorizer, NoProgress, ToolContext};
use opencode_tools::files::FileServices;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Default)]
pub struct Disk {
    pub content_reads: AtomicUsize,
    pub content_bytes: AtomicUsize,
}

impl Disk {
    pub fn reads(&self) -> usize {
        self.content_reads.load(Ordering::SeqCst)
    }

    pub fn bytes(&self) -> usize {
        self.content_bytes.load(Ordering::SeqCst)
    }

    fn count(&self, bytes: usize) {
        self.content_reads.fetch_add(1, Ordering::SeqCst);
        self.content_bytes.fetch_add(bytes, Ordering::SeqCst);
    }
}

fn error(operation: &str, path: &Path, error: std::io::Error) -> PlatformError {
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
        self.count(bytes.len());
        Ok(bytes)
    }
    async fn read_range(
        &self,
        path: &Path,
        offset: u64,
        len: usize,
    ) -> Result<Vec<u8>, PlatformError> {
        use std::io::{Read, Seek, SeekFrom};
        let mut file = std::fs::File::open(path).map_err(|e| error("open", path, e))?;
        file.seek(SeekFrom::Start(offset))
            .map_err(|e| error("seek", path, e))?;
        let mut buffer = Vec::with_capacity(len);
        file.take(len as u64)
            .read_to_end(&mut buffer)
            .map_err(|e| error("read", path, e))?;
        self.count(buffer.len());
        Ok(buffer)
    }
    async fn read_to_string(&self, path: &Path) -> Result<String, PlatformError> {
        let text = std::fs::read_to_string(path).map_err(|e| error("readFile", path, e))?;
        self.count(text.len());
        Ok(text)
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
                let kind = entry
                    .file_type()
                    .map(|kind| {
                        if kind.is_symlink() {
                            FileType::SymbolicLink
                        } else if kind.is_dir() {
                            FileType::Directory
                        } else if kind.is_file() {
                            FileType::File
                        } else {
                            FileType::Other
                        }
                    })
                    .unwrap_or(FileType::Other);
                (entry.file_name().to_string_lossy().into_owned(), kind)
            })
            .collect())
    }
    async fn create_dir_all(&self, path: &Path) -> Result<(), PlatformError> {
        std::fs::create_dir_all(path).map_err(|e| error("mkdir", path, e))
    }
    async fn write_with_dirs(&self, path: &Path, content: &[u8]) -> Result<(), PlatformError> {
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

/// Read tools never mutate. Any call is a test failure surfaced as a typed error.
#[derive(Debug)]
pub struct NoMutation;

#[async_trait]
impl FileMutation for NoMutation {
    async fn create(&self, _: &Target, _: &[u8]) -> Result<MutationResult, MutationError> {
        Err(PlatformError::unsupported("create", "read tests do not mutate").into())
    }
    async fn write(&self, _: &Target, _: &[u8]) -> Result<MutationResult, MutationError> {
        Err(PlatformError::unsupported("write", "read tests do not mutate").into())
    }
    async fn write_text_preserving_bom(
        &self,
        _: &Target,
        _: &str,
    ) -> Result<MutationResult, MutationError> {
        Err(PlatformError::unsupported("write", "read tests do not mutate").into())
    }
    async fn write_if_unchanged(
        &self,
        _: &Target,
        _: &[u8],
        _: &[u8],
    ) -> Result<MutationResult, MutationError> {
        Err(PlatformError::unsupported("write", "read tests do not mutate").into())
    }
    async fn remove(&self, _: &Target) -> Result<MutationResult, MutationError> {
        Err(PlatformError::unsupported("remove", "read tests do not mutate").into())
    }
}

#[derive(Default)]
pub struct Recorder {
    pub calls: Mutex<Vec<AssertInput>>,
    pub deny: Option<&'static str>,
}

impl Recorder {
    pub fn denying(action: &'static str) -> Arc<Self> {
        Arc::new(Self {
            deny: Some(action),
            ..Self::default()
        })
    }

    pub fn actions(&self) -> Vec<String> {
        let calls = self.calls.lock().unwrap();
        calls.iter().map(|call| call.action.clone()).collect()
    }

    pub fn get(&self, action: &str) -> Option<AssertInput> {
        let calls = self.calls.lock().unwrap();
        calls.iter().find(|call| call.action == action).cloned()
    }
}

#[async_trait]
impl Authorizer for Recorder {
    async fn assert(&self, input: AssertInput) -> Result<(), PermissionError> {
        let denied = self.deny == Some(input.action.as_str());
        self.calls.lock().unwrap().push(input);
        if denied {
            return Err(PermissionError::Declined);
        }
        Ok(())
    }
}

pub fn context(recorder: Arc<Recorder>) -> ToolContext {
    ToolContext {
        session_id: "ses_read".into(),
        agent: "build".into(),
        assistant_message_id: "msg_read".into(),
        tool_call_id: "call_read".into(),
        cancel: CancellationToken::new(),
        authorizer: recorder,
        progress: Arc::new(NoProgress),
    }
}

/// A Location rooted at a fresh temp dir (canonical path, so macOS `/private` does not differ).
pub struct Fixture {
    /// Keeps the temp directory alive for the fixture lifetime.
    pub _temp: tempfile::TempDir,
    pub root: PathBuf,
    pub disk: Arc<Disk>,
    pub services: FileServices,
}

impl Fixture {
    pub async fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        Self::at(temp, root.clone(), root).await
    }

    pub async fn at(temp: tempfile::TempDir, directory: PathBuf, worktree: PathBuf) -> Self {
        let disk = Arc::new(Disk::default());
        let location = NativeLocationMutation::new(&directory).await.unwrap();
        let services = FileServices::new(
            disk.clone(),
            Arc::new(NoMutation),
            Arc::new(location),
            directory.clone(),
            worktree,
        );
        Self {
            _temp: temp,
            root: directory,
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
}

pub const PNG: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";

pub fn png_bytes() -> Vec<u8> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(PNG)
        .unwrap()
}
