//! Purpose: native `FileSystem` adapter plus the `FSUtil` helpers config and tools need
//! (`readFileStringSafe`, `existsSafe`, `isDir`, `isFile`, `ensureDir`, `findUp`, `up`, `globUp`).
//! Scope: tokio/std filesystem I/O, no Location policy.
//! Uses:
//! rust/crates/
//! |-- platform-api/src/fs.rs
//! |-- platform-api/src/lib.rs
//! |-- platform-native/src/fs/error.rs
//! `-- platform-native/src/fs/glob.rs
//! External:
//! |-- async_trait::async_trait
//! |-- tokio::fs::File::open
//! |-- tokio::fs::canonicalize
//! |-- tokio::fs::create_dir_all
//! |-- tokio::fs::metadata
//! |-- tokio::fs::read
//! |-- tokio::fs::read_dir
//! |-- tokio::fs::remove_dir_all
//! |-- tokio::fs::remove_file
//! |-- tokio::fs::rename
//! |-- tokio::fs::symlink_metadata
//! |-- tokio::fs::write
//! |-- tokio::io::AsyncReadExt
//! |-- tokio::io::AsyncSeekExt
//! `-- tokio::task::spawn_blocking
//! Used by:
//! rust/crates/
//! |-- platform-native/src/fs/mod.rs
//! `-- platform-native/src/fs/mutation.rs
//! Invariants: `read_to_string` decodes like `TextDecoder("utf-8")` (leading BOM dropped, invalid
//! sequences replaced); `exists` is false only on NotFound; `write_with_dirs` creates the parent
//! only after a NotFound write failure; walks stop at `stop` or the filesystem root
//! (`packages/core/src/fs-util.ts:153-200`).
//! Contract tests: platform-native/tests/fs_util.rs

use super::error::{io_error, join_error};
use super::glob::{GlobOptions, scan};
use async_trait::async_trait;
use opencode_platform_api::PlatformError;
use opencode_platform_api::fs::{FileSystem, FileType, Metadata};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default)]
pub struct NativeFileSystem;

impl NativeFileSystem {
    pub fn new() -> Self {
        Self
    }

    /// `readFileStringSafe`: NotFound and PermissionDenied read as `None`, other errors fail.
    pub async fn read_to_string_safe(&self, path: &Path) -> Result<Option<String>, PlatformError> {
        match self.read_to_string(path).await {
            Ok(text) => Ok(Some(text)),
            Err(error)
                if matches!(
                    error.kind,
                    opencode_platform_api::PlatformErrorKind::NotFound
                        | opencode_platform_api::PlatformErrorKind::PermissionDenied
                ) =>
            {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    /// `existsSafe`: any failure reads as absent.
    pub async fn exists_safe(&self, path: &Path) -> bool {
        self.exists(path).await.unwrap_or(false)
    }

    pub async fn is_dir(&self, path: &Path) -> bool {
        tokio::fs::metadata(path)
            .await
            .is_ok_and(|info| info.is_dir())
    }

    pub async fn is_file(&self, path: &Path) -> bool {
        tokio::fs::metadata(path)
            .await
            .is_ok_and(|info| info.is_file())
    }

    /// `ensureDir`: recursive mkdir, an existing directory is success.
    pub async fn ensure_dir(&self, path: &Path) -> Result<(), PlatformError> {
        match tokio::fs::create_dir_all(path).await {
            Ok(()) => Ok(()),
            Err(error)
                if error.kind() == std::io::ErrorKind::AlreadyExists && self.is_dir(path).await =>
            {
                Ok(())
            }
            Err(error) => Err(io_error("makeDirectory", path, &error)),
        }
    }

    /// `FSUtil.findUp(target, start, stop)`.
    pub async fn find_up(
        &self,
        target: &str,
        start: &Path,
        stop: Option<&Path>,
    ) -> Result<Vec<PathBuf>, PlatformError> {
        self.up(&[target], start, stop).await
    }

    /// `FSUtil.up({ targets, start, stop })`: every existing `dir/target`, nearest directory first,
    /// targets in input order within one directory.
    pub async fn up(
        &self,
        targets: &[&str],
        start: &Path,
        stop: Option<&Path>,
    ) -> Result<Vec<PathBuf>, PlatformError> {
        let mut result = Vec::new();
        for current in ancestors(start, stop) {
            for target in targets {
                let search = current.join(target);
                if self.exists(&search).await? {
                    result.push(search);
                }
            }
        }
        Ok(result)
    }

    /// `FSUtil.globUp(pattern, start, stop)`: absolute file matches with dot files, per ancestor;
    /// scan failures in one directory contribute nothing.
    pub async fn glob_up(&self, pattern: &str, start: &Path, stop: Option<&Path>) -> Vec<PathBuf> {
        let mut result = Vec::new();
        for current in ancestors(start, stop) {
            let options = GlobOptions {
                cwd: current,
                absolute: true,
                include_dirs: false,
                dot: true,
                follow_symlinks: false,
            };
            if let Ok(matches) = self.glob(pattern, options).await {
                result.extend(matches.into_iter().map(PathBuf::from));
            }
        }
        result
    }

    /// `FSUtil.glob(pattern, options)` (node-glob semantics subset, see `glob.rs`).
    pub async fn glob(
        &self,
        pattern: &str,
        options: GlobOptions,
    ) -> Result<Vec<String>, PlatformError> {
        let pattern = pattern.to_string();
        let cwd = options.cwd.clone();
        tokio::task::spawn_blocking(move || scan(&pattern, &options))
            .await
            .map_err(|error| join_error("glob", &cwd, error))?
    }
}

/// Directories from `start` upward, ending at `stop` (inclusive) or the root.
fn ancestors(start: &Path, stop: Option<&Path>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut current = start.to_path_buf();
    loop {
        out.push(current.clone());
        if stop.is_some_and(|stop| stop == current) {
            break;
        }
        match current.parent() {
            Some(parent) if parent != current => current = parent.to_path_buf(),
            _ => break,
        }
    }
    out
}

fn file_type(kind: std::fs::FileType) -> FileType {
    if kind.is_symlink() {
        return FileType::SymbolicLink;
    }
    if kind.is_dir() {
        return FileType::Directory;
    }
    if kind.is_file() {
        return FileType::File;
    }
    FileType::Other
}

fn metadata(info: std::fs::Metadata) -> Metadata {
    #[cfg(unix)]
    let identity = {
        use std::os::unix::fs::MetadataExt;
        Some((info.dev(), info.ino()))
    };
    #[cfg(not(unix))]
    let identity = None;
    Metadata {
        file_type: file_type(info.file_type()),
        size: info.len(),
        modified: info.modified().ok(),
        identity,
    }
}

/// `TextDecoder("utf-8").decode`: strips one leading BOM, replaces malformed sequences.
pub(crate) fn decode_utf8(bytes: &[u8]) -> String {
    let body = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    String::from_utf8_lossy(body).into_owned()
}

#[async_trait]
impl FileSystem for NativeFileSystem {
    async fn read(&self, path: &Path) -> Result<Vec<u8>, PlatformError> {
        tokio::fs::read(path)
            .await
            .map_err(|error| io_error("readFile", path, &error))
    }

    async fn read_range(
        &self,
        path: &Path,
        offset: u64,
        len: usize,
    ) -> Result<Vec<u8>, PlatformError> {
        use tokio::io::{AsyncReadExt, AsyncSeekExt};
        let mut file = tokio::fs::File::open(path)
            .await
            .map_err(|error| io_error("open", path, &error))?;
        file.seek(std::io::SeekFrom::Start(offset))
            .await
            .map_err(|error| io_error("seek", path, &error))?;
        let mut buffer = Vec::with_capacity(len.min(1 << 20));
        file.take(len as u64)
            .read_to_end(&mut buffer)
            .await
            .map_err(|error| io_error("read", path, &error))?;
        Ok(buffer)
    }

    async fn read_to_string(&self, path: &Path) -> Result<String, PlatformError> {
        Ok(decode_utf8(&self.read(path).await?))
    }

    async fn metadata(&self, path: &Path) -> Result<Metadata, PlatformError> {
        tokio::fs::metadata(path)
            .await
            .map(metadata)
            .map_err(|error| io_error("stat", path, &error))
    }

    async fn symlink_metadata(&self, path: &Path) -> Result<Metadata, PlatformError> {
        tokio::fs::symlink_metadata(path)
            .await
            .map(metadata)
            .map_err(|error| io_error("lstat", path, &error))
    }

    async fn exists(&self, path: &Path) -> Result<bool, PlatformError> {
        match tokio::fs::metadata(path).await {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(io_error("access", path, &error)),
        }
    }

    async fn real_path(&self, path: &Path) -> Result<PathBuf, PlatformError> {
        tokio::fs::canonicalize(path)
            .await
            .map_err(|error| io_error("realPath", path, &error))
    }

    async fn read_dir(&self, path: &Path) -> Result<Vec<(String, FileType)>, PlatformError> {
        let mut entries = tokio::fs::read_dir(path)
            .await
            .map_err(|error| io_error("readDirectory", path, &error))?;
        let mut out = Vec::new();
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|error| io_error("readDirectory", path, &error))?
        {
            let kind = entry
                .file_type()
                .await
                .map(file_type)
                .unwrap_or(FileType::Other);
            out.push((entry.file_name().to_string_lossy().into_owned(), kind));
        }
        Ok(out)
    }

    async fn create_dir_all(&self, path: &Path) -> Result<(), PlatformError> {
        self.ensure_dir(path).await
    }

    async fn write_with_dirs(&self, path: &Path, content: &[u8]) -> Result<(), PlatformError> {
        match tokio::fs::write(path, content).await {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error("writeFile", path, &error)),
        }
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|error| io_error("makeDirectory", parent, &error))?;
        }
        tokio::fs::write(path, content)
            .await
            .map_err(|error| io_error("writeFile", path, &error))
    }

    async fn remove_file(&self, path: &Path) -> Result<(), PlatformError> {
        tokio::fs::remove_file(path)
            .await
            .map_err(|error| io_error("remove", path, &error))
    }

    async fn remove_dir_all(&self, path: &Path) -> Result<(), PlatformError> {
        tokio::fs::remove_dir_all(path)
            .await
            .map_err(|error| io_error("remove", path, &error))
    }

    async fn rename(&self, from: &Path, to: &Path) -> Result<(), PlatformError> {
        tokio::fs::rename(from, to)
            .await
            .map_err(|error| io_error("rename", from, &error))
    }
}
