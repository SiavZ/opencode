//! Purpose: Core `ReadToolFileSystem` port: inspect, bounded text paging, unpaged UTF-8 reads,
//! media ingestion and contained directory pages over the platform `FileSystem` port.
//! Scope: content reads after approval, paths are already canonical Location targets.
//! Uses:
//! rust/crates/
//! |-- platform-api/src/fs.rs
//! |-- platform-api/src/lib.rs
//! |-- tools/src/files/read_common.rs
//! `-- tools/src/files/read_pager.rs
//! External:
//! |-- base64::Engine
//! |-- base64::engine::general_purpose::STANDARD
//! `-- serde::Serialize
//! Used by:
//! rust/crates/
//! |-- tools/src/files/mod.rs
//! |-- tools/src/files/read_core.rs
//! |-- tools/src/files/read_pager.rs
//! |-- tools/tests/files_read/core.rs
//! `-- tools/tests/files_read/filesystem.rs
//! Invariants: reads stop once the page is complete (bytes past the page are never decoded),
//! media is capped at 20 MiB before encoding, directory entries never escape the listed root.
//! Contract tests: tools/tests/files_read.rs

use super::read_common::{
    CHUNK_BYTES, MAX_MEDIA_INGEST_BYTES, MAX_READ_BYTES, MAX_READ_LINES, Utf8Stream,
    binary_extension, file_url, image_mime, is_binary, locale_compare, mime_type,
};
use super::read_pager::paged_text;
use base64::Engine;
use opencode_platform_api::PlatformError;
use opencode_platform_api::fs::{FileSystem, FileType};
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ReadError {
    #[error(transparent)]
    Platform(#[from] PlatformError),
    #[error("Cannot read binary file: {resource}")]
    Binary { resource: String },
    #[error("Media exceeds {maximum_bytes} byte ingestion limit: {resource}")]
    MediaIngestLimit {
        resource: String,
        maximum_bytes: u64,
    },
    #[error("File is not valid UTF-8: {resource}")]
    MalformedUtf8 { resource: String },
    #[error("Offset {offset} is out of range")]
    OffsetOutOfRange { offset: u64 },
    #[error("Path is not {expected}: {resource}")]
    PathKind {
        resource: String,
        expected: &'static str,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Page {
    pub offset: Option<u64>,
    pub limit: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Encoding {
    Utf8,
    Base64,
}

/// `FileSystem.Content`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Content {
    pub uri: String,
    pub name: String,
    pub content: String,
    pub encoding: Encoding,
    pub mime: String,
}

/// `ReadTool.TextPage`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TextPage {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub content: String,
    pub mime: String,
    pub offset: u64,
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Entry {
    pub path: String,
    #[serde(rename = "type")]
    pub kind: &'static str,
}

/// `ReadTool.ListPage`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ListPage {
    pub entries: Vec<Entry>,
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileRead {
    Content(Content),
    Page(TextPage),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    File,
    Directory,
}

pub async fn inspect(fs: &dyn FileSystem, path: &Path) -> Result<Kind, ReadError> {
    match fs.metadata(path).await?.file_type {
        FileType::File => Ok(Kind::File),
        FileType::Directory => Ok(Kind::Directory),
        _ => Err(ReadError::PathKind {
            resource: path.to_string_lossy().into(),
            expected: "a file or directory",
        }),
    }
}

pub async fn read(
    fs: &dyn FileSystem,
    input: &Path,
    resource: &str,
    page: Page,
) -> Result<FileRead, ReadError> {
    let real = fs.real_path(input).await?;
    let info = fs.metadata(&real).await?;
    if info.file_type != FileType::File {
        return Err(ReadError::PathKind {
            resource: resource.into(),
            expected: "a file",
        });
    }
    let first_size = if info.size == 0 {
        4 * 1024
    } else {
        (info.size as usize).min(CHUNK_BYTES)
    };
    let first = fs.read_range(&real, 0, first_size).await?;
    let url = file_url(&real.to_string_lossy());
    let name = real
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    if let Some(mime) = image_mime(&first) {
        return media(fs, &real, resource, info.size, first, url, name, mime).await;
    }
    if first.starts_with(b"%PDF") || binary_extension(resource) {
        return Err(binary(resource));
    }
    let paged = info.size > MAX_READ_BYTES as u64 || page.offset.is_some() || page.limit.is_some();
    let mime = mime_type(&real.to_string_lossy());
    if !paged {
        return unpaged(fs, &real, resource, first, url, name, mime).await;
    }
    paged_text(fs, &real, resource, page, first, mime).await
}

#[allow(clippy::too_many_arguments)]
async fn media(
    fs: &dyn FileSystem,
    real: &Path,
    resource: &str,
    size: u64,
    first: Vec<u8>,
    uri: String,
    name: String,
    mime: &str,
) -> Result<FileRead, ReadError> {
    let limit = || ReadError::MediaIngestLimit {
        resource: resource.into(),
        maximum_bytes: MAX_MEDIA_INGEST_BYTES,
    };
    if size > MAX_MEDIA_INGEST_BYTES {
        return Err(limit());
    }
    let mut bytes = first;
    while bytes.len() as u64 <= MAX_MEDIA_INGEST_BYTES {
        let want = CHUNK_BYTES.min((MAX_MEDIA_INGEST_BYTES + 1) as usize - bytes.len());
        let chunk = fs.read_range(real, bytes.len() as u64, want).await?;
        if chunk.is_empty() {
            break;
        }
        bytes.extend_from_slice(&chunk);
    }
    if bytes.len() as u64 > MAX_MEDIA_INGEST_BYTES {
        return Err(limit());
    }
    Ok(FileRead::Content(Content {
        uri,
        name,
        content: base64::engine::general_purpose::STANDARD.encode(&bytes),
        encoding: Encoding::Base64,
        mime: mime.into(),
    }))
}

async fn unpaged(
    fs: &dyn FileSystem,
    real: &Path,
    resource: &str,
    first: Vec<u8>,
    uri: String,
    name: String,
    mime: String,
) -> Result<FileRead, ReadError> {
    if is_binary(resource, &first) {
        return Err(binary(resource));
    }
    let malformed = |_| ReadError::MalformedUtf8 {
        resource: resource.into(),
    };
    let mut decoder = Utf8Stream::new(true);
    let mut text = decoder.push(&first).map_err(malformed)?;
    let mut position = first.len() as u64;
    loop {
        let chunk = fs.read_range(real, position, CHUNK_BYTES).await?;
        if chunk.is_empty() {
            break;
        }
        if chunk.contains(&0) {
            return Err(binary(resource));
        }
        position += chunk.len() as u64;
        text.push_str(&decoder.push(&chunk).map_err(malformed)?);
    }
    text.push_str(&decoder.finish().map_err(malformed)?);
    Ok(FileRead::Content(Content {
        uri,
        name,
        content: text,
        encoding: Encoding::Utf8,
        mime,
    }))
}

pub async fn list(fs: &dyn FileSystem, input: &Path, page: Page) -> Result<ListPage, ReadError> {
    let real = fs.real_path(input).await?;
    let items = fs.read_dir(&real).await?;
    let mut visible = Vec::new();
    for (name, _) in items {
        let Ok(target) = fs.real_path(&real.join(&name)).await else {
            continue;
        };
        if !target.starts_with(&real) {
            continue;
        }
        let kind = match fs.metadata(&target).await.map(|info| info.file_type) {
            Ok(FileType::Directory) => "directory",
            Ok(FileType::File) => "file",
            _ => continue,
        };
        let suffix = if kind == "directory" {
            std::path::MAIN_SEPARATOR_STR
        } else {
            ""
        };
        visible.push(Entry {
            path: format!("{name}{suffix}"),
            kind,
        });
    }
    visible.sort_by(|a, b| match (a.kind == b.kind, a.kind) {
        (true, _) => locale_compare(&a.path, &b.path),
        (false, "directory") => std::cmp::Ordering::Less,
        (false, _) => std::cmp::Ordering::Greater,
    });
    let offset = page.offset.unwrap_or(1);
    let limit = page
        .limit
        .map_or(MAX_READ_LINES, |limit| (limit as usize).min(MAX_READ_LINES));
    let start = (offset as usize - 1).min(visible.len());
    let total = visible.len();
    let selected: Vec<Entry> = visible.drain(start..).take(limit).collect();
    let truncated = start + selected.len() < total;
    let next = truncated.then(|| offset + selected.len() as u64);
    Ok(ListPage {
        entries: selected,
        truncated,
        next,
    })
}

fn binary(resource: &str) -> ReadError {
    ReadError::Binary {
        resource: resource.into(),
    }
}
