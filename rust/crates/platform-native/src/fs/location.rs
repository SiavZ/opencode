//! Purpose: canonical Location-scoped mutation path resolution without reading file contents.
//! Scope: native path metadata only.
//! Uses:
//! rust/crates/
//! `-- platform-api/src/fs.rs
//! External:
//! |-- async_trait::async_trait
//! `-- tokio::fs
//! Used by:
//! rust/crates/
//! `-- platform-native/src/fs/mod.rs
//! Invariants: lexical relative escapes and internal symlink escapes are rejected before approval.
//! Contract tests: tools/tests/files_common.rs

use async_trait::async_trait;
use opencode_platform_api::fs::{
    ExternalDirectoryAuthorization, LocationMutation, PathError, PathErrorReason, ResolveInput,
    ResolveKind, Target,
};
use opencode_platform_api::{PlatformError, PlatformErrorKind};
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone)]
pub struct NativeLocationMutation {
    directory: PathBuf,
    root: PathBuf,
}

impl NativeLocationMutation {
    pub async fn new(directory: impl Into<PathBuf>) -> Result<Self, PathError> {
        let directory = directory.into();
        let absolute = if directory.is_absolute() {
            directory
        } else {
            std::env::current_dir()
                .map_err(|error| io_error("cwd", Path::new("."), error))?
                .join(directory)
        };
        let directory = normalize(&absolute);
        let root = tokio::fs::canonicalize(&directory)
            .await
            .map_err(|error| io_error("realPath", &directory, error))?;
        Ok(Self { directory, root })
    }
}

#[async_trait]
impl LocationMutation for NativeLocationMutation {
    async fn resolve(&self, input: ResolveInput) -> Result<Target, PathError> {
        let path = Path::new(&input.path);
        let relative = !path.is_absolute();
        let absolute = normalize(&self.directory.join(path));
        let internal = absolute.starts_with(&self.directory);
        if relative && !internal {
            return Err(PathError::Invalid {
                path: input.path,
                reason: PathErrorReason::RelativeEscape,
            });
        }
        let (canonical, directory, is_directory) = resolve_path(&absolute).await?;
        if internal && !canonical.starts_with(&self.root) {
            return Err(PathError::Invalid {
                path: input.path,
                reason: PathErrorReason::LocationEscape,
            });
        }
        let resource = if internal {
            let value = canonical.strip_prefix(&self.root).unwrap_or(&canonical);
            if value.as_os_str().is_empty() {
                ".".into()
            } else {
                slash(value)
            }
        } else {
            slash(&canonical)
        };
        let external_directory = if internal {
            None
        } else {
            let directory = if input.kind == Some(ResolveKind::Directory) && is_directory {
                canonical.clone()
            } else {
                directory
            };
            let resource = slash(&directory.join("*"));
            Some(ExternalDirectoryAuthorization {
                directory,
                save: resource.clone(),
                resource,
            })
        };
        Ok(Target {
            canonical,
            resource,
            external_directory,
        })
    }
}

async fn resolve_path(absolute: &Path) -> Result<(PathBuf, PathBuf, bool), PathError> {
    match tokio::fs::canonicalize(absolute).await {
        Ok(canonical) => {
            let info = tokio::fs::metadata(&canonical)
                .await
                .map_err(|error| io_error("stat", &canonical, error))?;
            let directory = if info.is_dir() {
                canonical.clone()
            } else {
                canonical.parent().unwrap_or(&canonical).to_path_buf()
            };
            return Ok((canonical, directory, info.is_dir()));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(io_error("realPath", absolute, error).into()),
    }
    let mut anchor = absolute.parent().unwrap_or(absolute);
    loop {
        match tokio::fs::canonicalize(anchor).await {
            Ok(canonical) => {
                let info = tokio::fs::metadata(&canonical)
                    .await
                    .map_err(|error| io_error("stat", &canonical, error))?;
                if !info.is_dir() {
                    return Err(ancestor_error(absolute));
                }
                let suffix = absolute
                    .strip_prefix(anchor)
                    .map_err(|_| ancestor_error(absolute))?;
                return Ok((canonical.join(suffix), canonical, false));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotADirectory => {
                return Err(ancestor_error(absolute));
            }
            Err(error) => return Err(io_error("realPath", anchor, error).into()),
        }
        let parent = anchor.parent().ok_or_else(|| ancestor_error(absolute))?;
        anchor = parent;
    }
}

fn ancestor_error(path: &Path) -> PathError {
    PathError::Invalid {
        path: path.to_string_lossy().into_owned(),
        reason: PathErrorReason::NonDirectoryAncestor,
    }
}

fn normalize(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            _ => result.push(part.as_os_str()),
        }
    }
    result
}

fn slash(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn io_error(operation: &str, path: &Path, error: std::io::Error) -> PlatformError {
    let kind = match error.kind() {
        std::io::ErrorKind::NotFound => PlatformErrorKind::NotFound,
        std::io::ErrorKind::NotADirectory => PlatformErrorKind::NotADirectory,
        std::io::ErrorKind::PermissionDenied => PlatformErrorKind::PermissionDenied,
        _ => PlatformErrorKind::Io,
    };
    PlatformError::new(kind, operation, path.to_string_lossy(), error.to_string())
}
