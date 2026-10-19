//! Purpose: port of attachment-picker.test.ts plus picker dialog flow tests.
//! Scope: integration test through `opencode_desktop_host::attachments`. The dialog is a
//! scripted test double (fixture), file reads use real temp files.
//! Uses:
//! rust/crates/
//! `-- desktop-host/src/attachments.rs
//! External:
//! |-- parking_lot::Mutex
//! |-- serde_json::to_value
//! |-- tempfile::tempdir
//! `-- tokio::test
//! Used by:
//! `-- none, entrypoint
//! Invariants: never touches real user data.
//! Contract tests: desktop-host/tests/attachments.rs

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use opencode_desktop_host::attachments::*;
use parking_lot::Mutex;

fn messages() -> Arc<Mutex<PickerMessages>> {
    Arc::new(Mutex::new(PickerMessages::default()))
}

struct EchoReader;
impl AttachmentReader for EchoReader {
    fn read<'a>(
        &'a self,
        path: &'a str,
        _max: u64,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, AttachmentError>> + Send + 'a>> {
        Box::pin(async move { Ok(path.as_bytes().to_vec()) })
    }
}

struct SixBytes;
impl AttachmentReader for SixBytes {
    fn read<'a>(
        &'a self,
        _path: &'a str,
        max: u64,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, AttachmentError>> + Send + 'a>> {
        Box::pin(async move {
            if 6 > max {
                return Err(AttachmentError::SizeLimit("budget exceeded".into()));
            }
            Ok(vec![0; 6])
        })
    }
}

fn echo() -> PickedFiles {
    PickedFiles::with_reader(Box::new(EchoReader), MAX_ATTACHMENT_BYTES, messages())
}

#[test]
fn accepts_selections_within_the_media_ingest_limit() {
    assert!(
        assert_attachment_budget(
            [MAX_ATTACHMENT_BYTES / 2, MAX_ATTACHMENT_BYTES / 2],
            &PickerMessages::default()
        )
        .is_ok()
    );
}

#[test]
fn rejects_the_selection_when_its_total_exceeds_the_limit() {
    let err = assert_attachment_budget([MAX_ATTACHMENT_BYTES, 1], &PickerMessages::default())
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "Selected attachments exceed the 20 MB limit"
    );
}

#[tokio::test]
async fn reads_an_approved_file_through_a_bounded_buffer() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("example.txt");
    std::fs::write(&file, "lorem ipsum").unwrap();
    let bytes = read_attachment(&file, MAX_ATTACHMENT_BYTES, &PickerMessages::default())
        .await
        .unwrap();
    assert_eq!(bytes, b"lorem ipsum");
}

#[tokio::test]
async fn rejects_an_oversized_file_before_allocating_its_contents() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("oversized.txt");
    std::fs::File::create(&file)
        .unwrap()
        .set_len(MAX_ATTACHMENT_BYTES + 1)
        .unwrap();
    let err = read_attachment(&file, MAX_ATTACHMENT_BYTES, &PickerMessages::default())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("20 MB limit"));
}

#[tokio::test]
async fn keeps_concurrent_picker_selections_isolated() {
    let auth = echo();
    let first = auth.add(1, ["a.txt".into(), "b.txt".into()]);
    let second = auth.add(1, ["c.txt".into()]);
    assert_eq!(auth.read(1, &first, "a.txt").await.unwrap(), b"a.txt");
    assert_eq!(auth.read(1, &second, "c.txt").await.unwrap(), b"c.txt");
    assert_eq!(auth.read(1, &first, "b.txt").await.unwrap(), b"b.txt");
    assert!(auth.is_empty());
}

#[tokio::test]
async fn releases_unread_files_for_one_picker_without_affecting_another() {
    let auth = echo();
    let first = auth.add(1, ["a.txt".into()]);
    let second = auth.add(1, ["b.txt".into()]);
    auth.release(1, &first);
    let err = auth.read(1, &first, "a.txt").await.unwrap_err();
    assert!(err.to_string().contains("not selected"));
    assert_eq!(auth.read(1, &second, "b.txt").await.unwrap(), b"b.txt");
}

#[tokio::test]
async fn keeps_picker_tokens_scoped_to_their_renderer() {
    let auth = echo();
    let token = auth.add(1, ["a.txt".into()]);
    assert!(matches!(
        auth.read(2, &token, "a.txt").await,
        Err(AttachmentError::NotSelected(_))
    ));
    // Another sender cannot release it either, and a failed read did not consume the path.
    auth.release(2, &token);
    assert_eq!(auth.read(1, &token, "a.txt").await.unwrap(), b"a.txt");
    // Each path can be read once.
    let again = auth.add(1, ["x".into(), "y".into()]);
    auth.read(1, &again, "x").await.unwrap();
    assert!(matches!(
        auth.read(1, &again, "x").await,
        Err(AttachmentError::NotSelected(_))
    ));
    auth.release_sender(1);
    assert!(auth.is_empty());
}

#[tokio::test]
async fn charges_actual_reads_against_the_selection_budget() {
    let auth = PickedFiles::with_reader(Box::new(SixBytes), 10, messages());
    let token = auth.add(1, ["a.txt".into(), "b.txt".into()]);
    auth.read(1, &token, "a.txt").await.unwrap();
    let err = auth.read(1, &token, "b.txt").await.unwrap_err();
    assert_eq!(err.to_string(), "budget exceeded");
}

#[derive(Default)]
struct ScriptedDialog {
    answer: Mutex<Option<Vec<String>>>,
    seen: Mutex<Vec<OpenDialog>>,
    saves: Mutex<Vec<SaveDialog>>,
}

impl FileDialog for ScriptedDialog {
    fn open<'a>(
        &'a self,
        request: OpenDialog,
    ) -> Pin<Box<dyn Future<Output = Result<Option<Vec<String>>, AttachmentError>> + Send + 'a>>
    {
        Box::pin(async move {
            self.seen.lock().push(request);
            Ok(self.answer.lock().clone())
        })
    }
    fn save<'a>(
        &'a self,
        request: SaveDialog,
    ) -> Pin<Box<dyn Future<Output = Result<Option<String>, AttachmentError>> + Send + 'a>> {
        Box::pin(async move {
            self.saves.lock().push(request);
            Ok(Some("/tmp/out.txt".into()))
        })
    }
}

#[tokio::test]
async fn open_file_picker_registers_token_and_reports_files() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.png");
    std::fs::write(&a, "12345").unwrap();
    let dialog = Arc::new(ScriptedDialog::default());
    *dialog.answer.lock() = Some(vec![a.to_string_lossy().into()]);
    let picker = AttachmentPicker::new(dialog.clone(), messages());
    let selection = picker
        .open_file_picker(
            7,
            FilePickerOptions {
                extensions: Some(vec!["png".into()]),
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        selection.files,
        [PickedFile {
            path: a.to_string_lossy().into(),
            name: "a.png".into(),
            size: 5
        }]
    );
    let seen = dialog.seen.lock()[0].clone();
    assert_eq!(seen.title, "Choose a file");
    assert_eq!(
        seen.filters,
        [DialogFilter {
            name: "Files".into(),
            extensions: vec!["png".into()]
        }]
    );
    assert!(!seen.multiple && !seen.directory);
    let path = a.to_string_lossy();
    assert!(
        picker
            .read_picked_file(8, &selection.token, &path)
            .await
            .is_err()
    );
    assert_eq!(
        picker
            .read_picked_file(7, &selection.token, &path)
            .await
            .unwrap(),
        b"12345"
    );
    let json = serde_json::to_value(&selection).unwrap();
    assert_eq!(json["files"][0]["size"], 5);

    *dialog.answer.lock() = None;
    assert!(
        picker
            .open_file_picker(7, FilePickerOptions::default())
            .await
            .unwrap()
            .is_none()
    );
    assert!(dialog.seen.lock()[1].filters.is_empty());
}

#[tokio::test]
async fn open_file_picker_rejects_over_budget_selection_without_token() {
    let dir = tempfile::tempdir().unwrap();
    let big = dir.path().join("big.bin");
    std::fs::File::create(&big)
        .unwrap()
        .set_len(MAX_ATTACHMENT_BYTES + 1)
        .unwrap();
    let dialog = Arc::new(ScriptedDialog::default());
    *dialog.answer.lock() = Some(vec![big.to_string_lossy().into()]);
    let picker = AttachmentPicker::new(dialog, messages());
    assert!(matches!(
        picker
            .open_file_picker(1, FilePickerOptions::default())
            .await,
        Err(AttachmentError::SizeLimit(_))
    ));
    assert!(picker.picked.is_empty());
}

#[tokio::test]
async fn directory_and_save_pickers() {
    let dialog = Arc::new(ScriptedDialog::default());
    *dialog.answer.lock() = Some(vec!["/a".into(), "/b".into()]);
    let picker = AttachmentPicker::new(dialog.clone(), messages());
    let one = picker
        .open_directory_picker(DirectoryPickerOptions::default())
        .await
        .unwrap();
    assert_eq!(serde_json::to_value(one).unwrap(), serde_json::json!("/a"));
    let many = picker
        .open_directory_picker(DirectoryPickerOptions {
            multiple: Some(true),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(many).unwrap(),
        serde_json::json!(["/a", "/b"])
    );
    let seen = dialog.seen.lock()[0].clone();
    assert!(seen.directory && seen.create_directory);
    assert_eq!(seen.title, "Choose a folder");
    assert_eq!(
        picker
            .save_file_picker(SavePickerOptions::default())
            .await
            .unwrap()
            .as_deref(),
        Some("/tmp/out.txt")
    );
    assert_eq!(dialog.saves.lock()[0].title, "Save file");
}

#[tokio::test]
async fn localized_messages_are_used() {
    let msgs = messages();
    msgs.lock().not_selected = "Nicht ausgewählt".into();
    let auth = PickedFiles::with_reader(Box::new(EchoReader), 10, msgs);
    assert_eq!(
        auth.read(1, "nope", "a").await.unwrap_err().to_string(),
        "Nicht ausgewählt"
    );
}
