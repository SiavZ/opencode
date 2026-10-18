//! Purpose: Core `ReadToolFileSystem` conformance over real temp files, ported from
//! packages/core/test/tool-read-filesystem.test.ts plus live-baseline differential values.
//! Scope: live temp-directory files through `read_files::{read, list}`.
//! Uses:
//! rust/crates/
//! |-- tools/src/files/read_files.rs
//! `-- tools/tests/files_read/support.rs
//! External:
//! `-- serde_json::json
//! Used by:
//! rust/crates/
//! `-- tools/tests/files_read.rs
//! Invariants: paged reads stop early, typed errors keep baseline messages.
//! Contract tests: this suite

use super::support::{Fixture, png_bytes};
use opencode_tools::files::read_files::{self, FileRead, Page, ReadError};
use serde_json::{Value, json};

// ReadToolFileSystem (packages/core/test/tool-read-filesystem.test.ts)

async fn read(fixture: &Fixture, name: &str, page: Page) -> Result<FileRead, ReadError> {
    let path = fixture.root.join(name);
    read_files::read(fixture.disk.as_ref(), &path, name, page).await
}

fn page(offset: Option<u64>, limit: Option<u64>) -> Page {
    Page { offset, limit }
}

fn text_page(result: Result<FileRead, ReadError>) -> Value {
    match result.unwrap() {
        FileRead::Page(page) => serde_json::to_value(page).unwrap(),
        FileRead::Content(content) => panic!("expected page, got {content:?}"),
    }
}

#[tokio::test]
async fn missing_file_and_wrong_kind_are_typed() {
    let fixture = Fixture::new().await;
    assert!(matches!(
        read(&fixture, "missing.txt", Page::default()).await,
        Err(ReadError::Platform(_))
    ));
    std::fs::create_dir(fixture.root.join("folder")).unwrap();
    assert!(matches!(
        read(&fixture, "folder", Page::default()).await,
        Err(ReadError::PathKind {
            expected: "a file",
            ..
        })
    ));
    let file = fixture.put("file.txt", "hello");
    assert!(matches!(
        read_files::list(fixture.disk.as_ref(), &file, Page::default()).await,
        Err(ReadError::Platform(_))
    ));
}

#[tokio::test]
async fn binary_and_malformed_utf8_are_typed() {
    let fixture = Fixture::new().await;
    fixture.put("archive.dat", [0u8, 1, 2, 3]);
    let mut malformed = vec![b'a'; 64 * 1024 + 1];
    malformed[64 * 1024] = 0x80;
    fixture.put("malformed.txt", &malformed);
    let error = read(&fixture, "archive.dat", Page::default())
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "Cannot read binary file: archive.dat");
    assert!(matches!(
        read(&fixture, "malformed.txt", Page::default()).await,
        Err(ReadError::MalformedUtf8 { .. })
    ));
    fixture.put("small-bad.txt", [b'o', b'k', 0xc3]);
    assert_eq!(
        read(&fixture, "small-bad.txt", Page::default())
            .await
            .unwrap_err()
            .to_string(),
        "File is not valid UTF-8: small-bad.txt"
    );
}

#[tokio::test]
async fn out_of_range_offset_is_typed() {
    let fixture = Fixture::new().await;
    fixture.put("short.txt", "one\n");
    let error = read(&fixture, "short.txt", page(Some(2), None))
        .await
        .unwrap_err();
    assert_eq!(error, ReadError::OffsetOutOfRange { offset: 2 });
    assert_eq!(error.to_string(), "Offset 2 is out of range");
    fixture.put("empty.txt", "");
    assert_eq!(
        text_page(read(&fixture, "empty.txt", page(Some(1), None)).await),
        json!({"type": "text-page", "content": "", "mime": "text/plain", "offset": 1, "truncated": false})
    );
}

#[tokio::test]
async fn stops_reading_after_requested_page() {
    let fixture = Fixture::new().await;
    for (name, trailing) in [("malformed.txt", 0x80u8), ("nul.txt", 0)] {
        fixture.put(name, [b'o', b'n', b'e', b'\n', trailing]);
        assert_eq!(
            text_page(read(&fixture, name, page(None, Some(1))).await),
            json!({"type": "text-page", "content": "one", "mime": "text/plain", "offset": 1, "truncated": true, "next": 2})
        );
    }
}

#[tokio::test]
async fn media_ingestion_limit_message() {
    let fixture = Fixture::new().await;
    let path = fixture.put("oversized.png", &png_bytes()[..8]);
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(20 * 1024 * 1024 + 1)
        .unwrap();
    let error = read(&fixture, "oversized.png", Page::default())
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Media exceeds 20971520 byte ingestion limit: oversized.png"
    );
    assert!(fixture.disk.bytes() <= 64 * 1024);
}

#[tokio::test]
async fn paging_offsets_crlf_bom_and_continuation() {
    let fixture = Fixture::new().await;
    fixture.put("crlf.txt", "\u{feff}a\r\nb\r\nc\r\nd");
    assert_eq!(
        text_page(read(&fixture, "crlf.txt", page(Some(2), Some(2))).await),
        json!({"type": "text-page", "content": "b\nc", "mime": "text/plain", "offset": 2, "truncated": true, "next": 4})
    );
    assert_eq!(
        text_page(read(&fixture, "crlf.txt", page(Some(1), None)).await)["content"],
        "a\nb\nc\nd"
    );
    // Unpaged small reads keep CRLF but strip the BOM like TextDecoder.
    match read(&fixture, "crlf.txt", Page::default()).await.unwrap() {
        FileRead::Content(content) => assert_eq!(content.content, "a\r\nb\r\nc\r\nd"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn large_files_page_by_bytes_and_truncate_long_lines() {
    let fixture = Fixture::new().await;
    let line = "x".repeat(80);
    let content = format!("{line}\n").repeat(50_000);
    fixture.put("huge.txt", &content);
    let value = text_page(read(&fixture, "huge.txt", Page::default()).await);
    assert_eq!(value["truncated"], true);
    // 80 + 81 * 631 bytes fit in 50 KiB, so line 633 continues.
    assert_eq!(value["next"], 633);
    assert!(fixture.disk.bytes() < content.len() / 2);
    fixture.put("long.txt", format!("{}\nnext", "y".repeat(70_000)));
    let value = text_page(read(&fixture, "long.txt", page(Some(1), None)).await);
    let expected = format!(
        "{}... (line truncated to 2000 chars)\nnext",
        "y".repeat(2000)
    );
    assert_eq!(value["content"], expected);
    assert_eq!(
        value["content"].as_str().unwrap().encode_utf16().count(),
        2039
    );
    let huge = text_page(read(&fixture, "huge.txt", Page::default()).await);
    assert_eq!(huge["content"].as_str().unwrap().len(), 51191);
    // Values below were observed from the live baseline ReadToolFileSystem.read on 2026-10-04.
    fixture.put("emoji.txt", format!("{}\nz", "😀".repeat(1500)));
    let value = text_page(read(&fixture, "emoji.txt", page(Some(1), None)).await);
    assert_eq!(
        value["content"].as_str().unwrap().encode_utf16().count(),
        2036
    );
    fixture.put("cr.txt", "a\rb\n");
    assert_eq!(
        text_page(read(&fixture, "cr.txt", page(Some(1), None)).await)["content"],
        "a\rb"
    );
    fixture.put("trail.txt", "a\nb\n");
    assert_eq!(
        read(&fixture, "trail.txt", page(Some(3), None))
            .await
            .unwrap_err(),
        ReadError::OffsetOutOfRange { offset: 3 }
    );
    assert_eq!(
        text_page(read(&fixture, "trail.txt", page(Some(2), None)).await)["content"],
        "b"
    );
    let numbered: Vec<String> = (0..3000).map(|i| format!("l{i}")).collect();
    fixture.put("big-lines.txt", numbered.join("\n"));
    assert_eq!(
        text_page(read(&fixture, "big-lines.txt", page(Some(2999), None)).await),
        json!({"type": "text-page", "content": "l2998\nl2999", "mime": "text/plain", "offset": 2999, "truncated": false})
    );
    let wide = "€".repeat(20_000);
    fixture.put("wide.txt", format!("{wide}\n{wide}\n{wide}"));
    let value = text_page(read(&fixture, "wide.txt", page(Some(1), None)).await);
    assert_eq!(value["truncated"], false);
    assert_eq!(value["content"].as_str().unwrap().lines().count(), 3);
}
