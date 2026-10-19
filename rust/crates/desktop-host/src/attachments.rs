//! Purpose: native attachment picker policy: per-selection tokens scoped to the requesting
//! renderer, the 20 MiB selection budget, bounded file reads, read authorization and release.
//! The native dialog itself is behind [`FileDialog`], implemented by the desktop shell.
//! Scope: process-wide authorization table plus the `open-file-picker`, `read-picked-file`,
//! `release-picked-files`, `open-directory-picker` and `save-file-picker` handlers.
//! Uses:
//! rust/crates/
//! `-- desktop-host/src/attachments/authorization.rs
//! External:
//! |-- parking_lot::Mutex
//! |-- serde::Deserialize
//! |-- serde::Serialize
//! |-- tokio::fs::File::open
//! |-- tokio::fs::metadata
//! |-- tokio::io::AsyncReadExt
//! |-- u64::saturating_add
//! `-- uuid::Uuid::new_v4
//! Used by:
//! rust/crates/
//! |-- desktop-host/src/attachments/authorization.rs
//! |-- desktop-host/src/lib.rs
//! |-- desktop-host/tests/attachments.rs
//! `-- desktop-host/tests/attachments_concurrency.rs
//! Invariants: a path is readable once per token and only by the sender that picked it, reads are
//! charged against the remaining selection budget, a token disappears once its last path is read
//! or it is released by its own sender, oversized files are rejected before allocation, the
//! authorization lock is never held across file io.
//! Contract tests: desktop-host/tests/attachments.rs, desktop-host/tests/attachments_concurrency.rs

mod authorization;

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;

/// Total bytes one picker selection may contain (`attachment-picker.ts:5`).
pub const MAX_ATTACHMENT_BYTES: u64 = 20 * 1024 * 1024;

/// Renderer identity (Electron `webContents.id`). Assigned by the bridge per webview.
pub type SenderId = u64;

/// Localized error messages (`desktop.picker.error.*`). Defaults are the English bundle.
#[derive(Debug, Clone)]
pub struct PickerMessages {
    pub not_selected: String,
    /// Template with a `{{limit}}` placeholder.
    pub size_limit: String,
    pub choose_file: String,
    pub choose_folder: String,
    pub save_file: String,
    pub files_filter: String,
}

impl Default for PickerMessages {
    fn default() -> Self {
        Self {
            not_selected: "File was not selected by the picker".into(),
            size_limit: "Selected attachments exceed the {{limit}} MB limit".into(),
            choose_file: "Choose a file".into(),
            choose_folder: "Choose a folder".into(),
            save_file: "Save file".into(),
            files_filter: "Files".into(),
        }
    }
}

impl PickerMessages {
    fn size_limit(&self) -> String {
        self.size_limit.replace(
            "{{limit}}",
            &(MAX_ATTACHMENT_BYTES / 1024 / 1024).to_string(),
        )
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AttachmentError {
    /// Unknown token, wrong sender, or path not (or no longer) part of the selection.
    #[error("{0}")]
    NotSelected(String),
    /// Selection total or a single file exceeds the remaining budget.
    #[error("{0}")]
    SizeLimit(String),
    #[error("failed to read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("file dialog failed: {0}")]
    Dialog(String),
}

/// Boxed future returned by the object-safe async traits in this module.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, AttachmentError>> + Send + 'a>>;

/// Bounded reader used by [`PickedFiles`]. Production uses [`read_attachment`].
pub trait AttachmentReader: Send + Sync {
    fn read<'a>(&'a self, path: &'a str, max_bytes: u64) -> BoxFuture<'a, Vec<u8>>;
}

/// Reads from the real filesystem with [`read_attachment`].
pub struct FsReader {
    pub messages: Arc<Mutex<PickerMessages>>,
}

impl AttachmentReader for FsReader {
    fn read<'a>(&'a self, path: &'a str, max_bytes: u64) -> BoxFuture<'a, Vec<u8>> {
        Box::pin(async move {
            let messages = self.messages.lock().clone();
            read_attachment(Path::new(path), max_bytes, &messages).await
        })
    }
}

struct Selection {
    sender: SenderId,
    paths: HashSet<String>,
    remaining: u64,
    in_flight: usize,
}

/// `createPickedFileAuthorizations` (`attachment-picker.ts:7-32`).
pub struct PickedFiles {
    selections: Mutex<HashMap<String, Selection>>,
    reader: Box<dyn AttachmentReader>,
    budget: u64,
    messages: Arc<Mutex<PickerMessages>>,
}

impl PickedFiles {
    /// Real filesystem reader with the 20 MiB budget.
    pub fn new(messages: Arc<Mutex<PickerMessages>>) -> Self {
        Self::with_reader(
            Box::new(FsReader {
                messages: messages.clone(),
            }),
            MAX_ATTACHMENT_BYTES,
            messages,
        )
    }

    pub fn with_reader(
        reader: Box<dyn AttachmentReader>,
        budget: u64,
        messages: Arc<Mutex<PickerMessages>>,
    ) -> Self {
        Self {
            selections: Mutex::new(HashMap::new()),
            reader,
            budget,
            messages,
        }
    }

    /// Registers a selection and returns its random token.
    pub fn add(&self, sender: SenderId, paths: impl IntoIterator<Item = String>) -> String {
        let token = uuid::Uuid::new_v4().to_string();
        self.selections.lock().insert(
            token.clone(),
            Selection {
                sender,
                paths: paths.into_iter().collect(),
                remaining: self.budget,
                in_flight: 0,
            },
        );
        token
    }

    /// Consumes `path` from the selection and reads it within the remaining budget. Like the
    /// baseline, the path stays consumed even when the read fails.
    pub async fn read(
        &self,
        sender: SenderId,
        token: &str,
        path: &str,
    ) -> Result<Vec<u8>, AttachmentError> {
        let remaining = {
            let mut selections = self.selections.lock();
            let selection = selections
                .get_mut(token)
                .filter(|selection| selection.sender == sender)
                .filter(|selection| selection.paths.contains(path));
            let Some(selection) = selection else {
                return Err(AttachmentError::NotSelected(
                    self.messages.lock().not_selected.clone(),
                ));
            };
            selection.paths.remove(path);
            selection.in_flight += 1;
            selection.remaining
        };
        let _lease = authorization::ReadLease {
            picker: self,
            token,
        };
        let result = self.reader.read(path, remaining).await;
        let mut selections = self.selections.lock();
        let Some(selection) = selections.get_mut(token) else {
            return Err(AttachmentError::NotSelected(
                self.messages.lock().not_selected.clone(),
            ));
        };
        // Another read may have spent the budget while this bounded IO was in flight.
        result.and_then(|bytes| {
            if bytes.len() as u64 > selection.remaining {
                return Err(AttachmentError::SizeLimit(
                    self.messages.lock().size_limit(),
                ));
            }
            selection.remaining -= bytes.len() as u64;
            Ok(bytes)
        })
    }

    /// Drops the selection when it belongs to `sender`, otherwise does nothing.
    pub fn release(&self, sender: SenderId, token: &str) {
        let mut selections = self.selections.lock();
        if selections.get(token).is_some_and(|s| s.sender == sender) {
            selections.remove(token);
        }
    }

    /// Drops every selection of a destroyed renderer. Electron leaked these, the native host
    /// calls this when a webview closes.
    pub fn release_sender(&self, sender: SenderId) {
        self.selections.lock().retain(|_, s| s.sender != sender);
    }

    pub fn len(&self) -> usize {
        self.selections.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// `assertAttachmentBudget` (`attachment-picker.ts:34-38`).
pub fn assert_attachment_budget(
    sizes: impl IntoIterator<Item = u64>,
    messages: &PickerMessages,
) -> Result<(), AttachmentError> {
    let total: u64 = sizes.into_iter().fold(0, u64::saturating_add);
    if total <= MAX_ATTACHMENT_BYTES {
        return Ok(());
    }
    Err(AttachmentError::SizeLimit(messages.size_limit()))
}

/// `readAttachment` (`attachment-picker.ts:40-57`): stat first, reject before allocating, then
/// read at most the stat size (a file that shrinks returns what was read).
pub async fn read_attachment(
    path: &Path,
    max_bytes: u64,
    messages: &PickerMessages,
) -> Result<Vec<u8>, AttachmentError> {
    let io = |source| AttachmentError::Io {
        path: path.to_path_buf(),
        source,
    };
    let file = tokio::fs::File::open(path).await.map_err(io)?;
    let size = file.metadata().await.map_err(io)?.len();
    if size > max_bytes {
        return Err(AttachmentError::SizeLimit(messages.size_limit()));
    }
    let mut bytes = Vec::with_capacity(size as usize);
    file.take(size).read_to_end(&mut bytes).await.map_err(io)?;
    Ok(bytes)
}

/// Options passed by the renderer to `openFilePicker` (`preload/types.ts`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilePickerOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multiple: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryPickerOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multiple: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_path: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavePickerOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_path: Option<String>,
}

/// One named extension filter (`ipc.ts:28-31`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogFilter {
    pub name: String,
    pub extensions: Vec<String>,
}

/// Fully resolved native open dialog request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenDialog {
    pub title: String,
    pub default_path: Option<String>,
    pub directory: bool,
    pub multiple: bool,
    /// Directory pickers allow creating folders (`ipc.ts:155`).
    pub create_directory: bool,
    pub filters: Vec<DialogFilter>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveDialog {
    pub title: String,
    pub default_path: Option<String>,
}

/// Native dialogs, provided by the desktop shell. `None` means cancelled.
pub trait FileDialog: Send + Sync {
    fn open<'a>(&'a self, request: OpenDialog) -> BoxFuture<'a, Option<Vec<String>>>;
    fn save<'a>(&'a self, request: SaveDialog) -> BoxFuture<'a, Option<String>>;
}

/// One picked file as returned to the renderer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PickedFile {
    pub path: String,
    pub name: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PickedSelection {
    pub token: String,
    pub files: Vec<PickedFile>,
}

/// Single or multiple directory result (`ipc.ts:160`): a string unless `multiple`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DirectorySelection {
    One(String),
    Many(Vec<String>),
}

/// Picker capability composed from a dialog, the authorization table and messages.
pub struct AttachmentPicker {
    pub dialog: Arc<dyn FileDialog>,
    pub picked: PickedFiles,
    pub messages: Arc<Mutex<PickerMessages>>,
}

impl AttachmentPicker {
    pub fn new(dialog: Arc<dyn FileDialog>, messages: Arc<Mutex<PickerMessages>>) -> Self {
        Self {
            dialog,
            picked: PickedFiles::new(messages.clone()),
            messages,
        }
    }

    /// `open-file-picker` (`ipc.ts:165-189`).
    pub async fn open_file_picker(
        &self,
        sender: SenderId,
        options: FilePickerOptions,
    ) -> Result<Option<PickedSelection>, AttachmentError> {
        let messages = self.messages.lock().clone();
        let filters = match options.extensions {
            Some(ext) if !ext.is_empty() => vec![DialogFilter {
                name: messages.files_filter.clone(),
                extensions: ext,
            }],
            _ => Vec::new(),
        };
        let request = OpenDialog {
            title: options.title.unwrap_or(messages.choose_file.clone()),
            default_path: options.default_path,
            directory: false,
            multiple: options.multiple.unwrap_or(false),
            create_directory: false,
            filters,
        };
        let Some(paths) = self.dialog.open(request).await? else {
            return Ok(None);
        };
        let mut files = Vec::with_capacity(paths.len());
        for path in &paths {
            let size = tokio::fs::metadata(path)
                .await
                .map_err(|source| AttachmentError::Io {
                    path: path.into(),
                    source,
                })?
                .len();
            let name = Path::new(path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            files.push(PickedFile {
                path: path.clone(),
                name,
                size,
            });
        }
        assert_attachment_budget(files.iter().map(|f| f.size), &messages)?;
        let token = self.picked.add(sender, paths);
        Ok(Some(PickedSelection { token, files }))
    }

    /// `open-directory-picker` (`ipc.ts:151-162`).
    pub async fn open_directory_picker(
        &self,
        options: DirectoryPickerOptions,
    ) -> Result<Option<DirectorySelection>, AttachmentError> {
        let multiple = options.multiple.unwrap_or(false);
        let request = OpenDialog {
            title: options
                .title
                .unwrap_or(self.messages.lock().choose_folder.clone()),
            default_path: options.default_path,
            directory: true,
            multiple,
            create_directory: true,
            filters: Vec::new(),
        };
        let Some(mut paths) = self.dialog.open(request).await? else {
            return Ok(None);
        };
        if multiple {
            return Ok(Some(DirectorySelection::Many(paths)));
        }
        // `result.filePaths[0]` is undefined (serialized as null) when the list is empty.
        Ok((!paths.is_empty()).then(|| DirectorySelection::One(paths.swap_remove(0))))
    }

    /// `save-file-picker` (`ipc.ts:199-209`).
    pub async fn save_file_picker(
        &self,
        options: SavePickerOptions,
    ) -> Result<Option<String>, AttachmentError> {
        let request = SaveDialog {
            title: options
                .title
                .unwrap_or(self.messages.lock().save_file.clone()),
            default_path: options.default_path,
        };
        self.dialog.save(request).await
    }

    /// `read-picked-file`.
    pub async fn read_picked_file(
        &self,
        sender: SenderId,
        token: &str,
        path: &str,
    ) -> Result<Vec<u8>, AttachmentError> {
        self.picked.read(sender, token, path).await
    }

    /// `release-picked-files`.
    pub fn release_picked_files(&self, sender: SenderId, token: &str) {
        self.picked.release(sender, token);
    }
}
