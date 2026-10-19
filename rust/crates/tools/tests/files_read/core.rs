//! Purpose: Core read tool and `ReadToolFileSystem` conformance over real temp files, ported from
//! packages/core/test/tool-read.test.ts.
//! Scope: live temp-directory files through `CoreReadTool::execute`.
//! Uses:
//! rust/crates/
//! |-- tool-api/src/lib.rs
//! |-- tools/src/files/read_core.rs
//! |-- tools/src/files/read_files.rs
//! `-- tools/tests/files_read/support.rs
//! External:
//! |-- async_trait::async_trait
//! |-- serde_json::Value
//! |-- serde_json::from_value
//! |-- serde_json::json
//! |-- tempfile::tempdir
//! `-- tokio::test
//! Used by:
//! rust/crates/
//! `-- tools/tests/files_read.rs
//! Invariants: denied or failed approvals read no content bytes.
//! Contract tests: this suite

use super::support::{Fixture, PNG, Recorder, context, png_bytes};
use async_trait::async_trait;
use opencode_tool_api::{Tool, ToolContent};
use opencode_tools::files::read_core::{CoreReadTool, ImageError, ImageNormalizer, NoResizer};
use opencode_tools::files::read_files::Content;
use serde_json::{Value, json};
use std::sync::Arc;

fn tool(fixture: &Fixture) -> CoreReadTool {
    CoreReadTool::new(fixture.services.clone(), Arc::new(NoResizer))
}

async fn run(fixture: &Fixture, input: Value) -> (Result<Value, String>, Arc<Recorder>) {
    let recorder = Arc::new(Recorder::default());
    let result = tool(fixture)
        .execute(input, &context(recorder.clone()))
        .await
        .map(|output| {
            if output.content.is_empty() {
                output.structured
            } else {
                json!({"structured": output.structured, "content": output.content})
            }
        })
        .map_err(|failure| failure.message);
    (result, recorder)
}

#[tokio::test]
async fn reads_small_text_as_utf8_content_and_authorizes_relative_resource() {
    let fixture = Fixture::new().await;
    let path = fixture.put("README.md", "hello");
    let (result, recorder) = run(&fixture, json!({"path": "README.md"})).await;
    assert_eq!(
        result.unwrap(),
        json!({
            "uri": format!("file://{}", path.to_string_lossy()),
            "name": "README.md",
            "content": "hello",
            "encoding": "utf8",
            "mime": "text/markdown",
        })
    );
    assert_eq!(recorder.actions(), ["read"]);
    let read = recorder.get("read").unwrap();
    assert_eq!(read.resources, ["README.md"]);
    assert_eq!(read.save, Some(vec!["*".to_string()]));
    assert!(fixture.services.read_stamp("ses_read", &path).is_some());
}

#[tokio::test]
async fn external_absolute_path_asks_external_directory_before_read() {
    let fixture = Fixture::new().await;
    let outside = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(outside.path()).unwrap();
    std::fs::write(root.join("notes.txt"), "outside").unwrap();
    let path = root.join("notes.txt").to_string_lossy().into_owned();
    let (result, recorder) = run(&fixture, json!({"path": path})).await;
    assert_eq!(result.unwrap()["content"], "outside");
    assert_eq!(recorder.actions(), ["external_directory", "read"]);
    let external = recorder.get("external_directory").unwrap();
    assert_eq!(
        external.resources,
        [format!("{}/*", root.to_string_lossy())]
    );
    assert_eq!(recorder.get("read").unwrap().resources, [path]);
}

#[tokio::test]
async fn denied_external_directory_touches_no_metadata_or_content() {
    let fixture = Fixture::new().await;
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.txt"), "secret").unwrap();
    let recorder = Recorder::denying("external_directory");
    let path = outside.path().join("secret.txt");
    let result = tool(&fixture)
        .execute(json!({"path": path}), &context(recorder.clone()))
        .await;
    assert_eq!(
        result.unwrap_err().message,
        format!("Unable to read {}", path.to_string_lossy())
    );
    assert_eq!(recorder.actions(), ["external_directory"]);
    assert_eq!(fixture.disk.reads(), 0);
}

#[tokio::test]
async fn denied_read_permission_reads_no_content_for_files_or_directories() {
    let fixture = Fixture::new().await;
    fixture.put("README.md", "hello");
    fixture.put("src/a.ts", "a");
    for path in ["README.md", "src"] {
        let recorder = Recorder::denying("read");
        let result = tool(&fixture)
            .execute(json!({"path": path}), &context(recorder))
            .await;
        assert_eq!(
            result.unwrap_err().message,
            format!("Unable to read {path}")
        );
    }
    assert_eq!(fixture.disk.reads(), 0);
    assert!(
        fixture
            .services
            .read_stamp("ses_read", &fixture.root.join("README.md"))
            .is_none()
    );
}

#[tokio::test]
async fn missing_paths_and_relative_escapes_fail_before_approval() {
    let fixture = Fixture::new().await;
    for path in ["missing.txt", "../outside.txt"] {
        let (result, recorder) = run(&fixture, json!({"path": path})).await;
        assert_eq!(result.unwrap_err(), format!("Unable to read {path}"));
        // Prospective paths resolve, then inspect fails before read approval.
        assert!(recorder.actions().is_empty());
    }
    assert_eq!(fixture.disk.reads(), 0);
}

#[cfg(unix)]
#[tokio::test]
async fn symlink_escape_is_rejected_without_reading() {
    let fixture = Fixture::new().await;
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.txt"), "secret").unwrap();
    std::os::unix::fs::symlink(outside.path(), fixture.root.join("escape")).unwrap();
    let (result, recorder) = run(&fixture, json!({"path": "escape/secret.txt"})).await;
    assert_eq!(result.unwrap_err(), "Unable to read escape/secret.txt");
    assert!(recorder.actions().is_empty());
    assert_eq!(fixture.disk.reads(), 0);
}

#[tokio::test]
async fn small_png_returns_native_media_content() {
    let fixture = Fixture::new().await;
    fixture.put("pixel.png", png_bytes());
    let (result, _) = run(&fixture, json!({"path": "pixel.png"})).await;
    let value = result.unwrap();
    assert_eq!(value["structured"]["encoding"], "base64");
    assert_eq!(value["structured"]["mime"], "image/png");
    assert_eq!(value["structured"]["content"], PNG);
    let content: Vec<ToolContent> = serde_json::from_value(value["content"].clone()).unwrap();
    assert_eq!(
        content,
        [
            ToolContent::text("Image read successfully"),
            ToolContent::file_base64(PNG, "image/png", Some("pixel.png".into())),
        ]
    );
}

#[tokio::test]
async fn image_contents_win_over_misleading_binary_extension() {
    let fixture = Fixture::new().await;
    fixture.put("pixel.bin", png_bytes());
    let (result, _) = run(&fixture, json!({"path": "pixel.bin"})).await;
    let value = result.unwrap();
    assert_eq!(value["content"][1]["mime"], "image/png");
    assert_eq!(value["content"][1]["name"], "pixel.bin");
}

#[derive(Debug)]
struct Failing(ImageError);

#[async_trait]
impl ImageNormalizer for Failing {
    async fn normalize(&self, _: &str, _: Content) -> Result<Content, ImageError> {
        Err(self.0.clone())
    }
}

#[tokio::test]
async fn image_normalizer_errors_are_model_visible() {
    let fixture = Fixture::new().await;
    fixture.put("truncated.png", png_bytes());
    let error = ImageError::Decode {
        resource: "truncated.png".into(),
    };
    let tool = CoreReadTool::new(fixture.services.clone(), Arc::new(Failing(error)));
    let result = tool
        .execute(json!({"path": "truncated.png"}), &context(Arc::default()))
        .await;
    assert_eq!(
        result.unwrap_err().message,
        "Image could not be decoded: truncated.png"
    );
    let size = ImageError::Size {
        resource: "wide.png".into(),
        width: 16,
        height: 1,
        bytes: 100,
        max_width: 4,
        max_height: 2000,
        max_bytes: 5242880,
    };
    assert!(
        size.to_string()
            .contains("exceeding configured limits 4x2000/5242880 bytes")
    );
}

#[tokio::test]
async fn binary_extension_and_pdf_and_nul_are_rejected_with_resource() {
    let fixture = Fixture::new().await;
    fixture.put("archive.dat", [0u8, 1, 2, 3]);
    fixture.put("doc.pdf", b"%PDF-1.4\n...");
    fixture.put("null.txt", b"hello\0world");
    for (path, resource) in [
        ("archive.dat", "archive.dat"),
        ("doc.pdf", "doc.pdf"),
        ("null.txt", "null.txt"),
    ] {
        let (result, _) = run(&fixture, json!({"path": path, "offset": 1})).await;
        assert_eq!(
            result.unwrap_err(),
            format!("Cannot read binary file: {resource}"),
            "{path}"
        );
    }
}

#[tokio::test]
async fn directories_list_bounded_pages_directories_first() {
    let fixture = Fixture::new().await;
    for name in ["b.txt", "a.txt", "C.txt", "_x.txt"] {
        fixture.put(&format!("dir/{name}"), "x");
    }
    std::fs::create_dir_all(fixture.root.join("dir/zeta")).unwrap();
    let (result, recorder) = run(&fixture, json!({"path": "dir", "offset": 2, "limit": 2})).await;
    assert_eq!(
        result.unwrap(),
        json!({
            "entries": [{"path": "_x.txt", "type": "file"}, {"path": "a.txt", "type": "file"}],
            "truncated": true,
            "next": 4,
        })
    );
    assert_eq!(recorder.get("read").unwrap().resources, ["dir"]);
    let (result, _) = run(&fixture, json!({"path": "dir", "offset": 5})).await;
    assert_eq!(
        result.unwrap(),
        json!({"entries": [{"path": "C.txt", "type": "file"}], "truncated": false})
    );
    let (result, _) = run(&fixture, json!({"path": "dir", "limit": 1})).await;
    assert_eq!(
        result.unwrap()["entries"],
        json!([{"path": "zeta/", "type": "directory"}])
    );
    // Baseline localeCompare order observed live: _x, 10, 9, a, b, Bb, C.
    for name in ["Bb.txt", "10.txt", "9.txt"] {
        fixture.put(&format!("dir/{name}"), "x");
    }
    let (result, _) = run(&fixture, json!({"path": "dir"})).await;
    let names: Vec<String> = result.unwrap()["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["path"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        names,
        [
            "zeta/", "_x.txt", "10.txt", "9.txt", "a.txt", "b.txt", "Bb.txt", "C.txt"
        ]
    );
}

#[cfg(unix)]
#[tokio::test]
async fn directory_listing_hides_escaping_and_dangling_symlinks() {
    let fixture = Fixture::new().await;
    let outside = tempfile::tempdir().unwrap();
    fixture.put("dir/inside.txt", "x");
    std::os::unix::fs::symlink(outside.path(), fixture.root.join("dir/escape")).unwrap();
    std::os::unix::fs::symlink(
        fixture.root.join("missing"),
        fixture.root.join("dir/dangle"),
    )
    .unwrap();
    std::os::unix::fs::symlink(
        fixture.root.join("dir/inside.txt"),
        fixture.root.join("dir/link"),
    )
    .unwrap();
    let (result, _) = run(&fixture, json!({"path": "dir"})).await;
    assert_eq!(
        result.unwrap()["entries"],
        json!([{"path": "inside.txt", "type": "file"}, {"path": "link", "type": "file"}])
    );
}

#[tokio::test]
async fn invalid_input_reports_schema_issue() {
    let fixture = Fixture::new().await;
    for (input, message) in [
        (
            json!({"path": "a", "offset": 0}),
            "Expected a value greater than 0, got 0\n  at [\"offset\"]",
        ),
        (
            json!({"path": "a", "limit": 2001}),
            "Expected a value less than or equal to 2000, got 2001\n  at [\"limit\"]",
        ),
        (
            json!({"path": "a", "offset": 1.5}),
            "Expected an integer, got 1.5\n  at [\"offset\"]",
        ),
        (
            json!({"path": "a", "offset": null}),
            "Expected number | undefined, got null\n  at [\"offset\"]",
        ),
        (
            json!({"path": "a", "offset": "2"}),
            "Expected number | undefined, got \"2\"\n  at [\"offset\"]",
        ),
        (json!({}), "Missing key\n  at [\"path\"]"),
        (
            json!({"path": 1}),
            "Expected string, got 1\n  at [\"path\"]",
        ),
        (json!("x"), "Expected object, got \"x\""),
        (
            json!({"path": "a", "offset": 9007199254740992u64}),
            "Expected an integer, got 9007199254740992\n  at [\"offset\"]",
        ),
    ] {
        let (result, recorder) = run(&fixture, input).await;
        assert_eq!(
            result.unwrap_err(),
            format!("Invalid tool input: {message}")
        );
        assert!(recorder.actions().is_empty());
    }
}
