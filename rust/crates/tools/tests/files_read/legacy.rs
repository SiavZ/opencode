//! Purpose: Legacy read tool conformance over real temp files, ported from
//! packages/opencode/test/tool/read.test.ts (permissions, truncation, attachments, binary).
//! Scope: live temp-directory files through `LegacyReadTool`.
//! Uses:
//! rust/crates/
//! |-- tools/src/files/read_legacy.rs
//! `-- tools/tests/files_read/support.rs
//! External:
//! `-- serde_json::json
//! Used by:
//! rust/crates/
//! `-- tools/tests/files_read.rs
//! Invariants: denied approvals read no content bytes.
//! Contract tests: this suite

use super::support::{Fixture, Recorder, context, png_bytes};
use async_trait::async_trait;
use opencode_tool_api::{Tool, ToolContext, ToolOutput};
use opencode_tools::files::read_legacy::{
    LegacyReadHooks, LegacyReadTool, LoadedInstruction, NoHooks,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

fn tool(fixture: &Fixture) -> LegacyReadTool {
    LegacyReadTool::new(fixture.services.clone(), Arc::new(NoHooks))
}

async fn exec(fixture: &Fixture, input: Value) -> (Result<ToolOutput, String>, Arc<Recorder>) {
    let recorder = Arc::new(Recorder::default());
    let result = tool(fixture)
        .execute(input, &context(recorder.clone()))
        .await
        .map_err(|failure| failure.message);
    (result, recorder)
}

async fn output(fixture: &Fixture, input: Value) -> Value {
    exec(fixture, input).await.0.unwrap().structured
}

fn s(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[tokio::test]
async fn reads_absolute_paths_inside_project_without_external_directory() {
    let fixture = Fixture::new().await;
    let file = fixture.put("subdir/test.txt", "nested content");
    let (result, recorder) = exec(&fixture, json!({"filePath": s(&file)})).await;
    let out = result.unwrap();
    assert_eq!(
        out.structured["output"],
        format!(
            "<path>{}</path>\n<type>file</type>\n<content>\n1: nested content\n\n(End of file - total 1 lines)\n</content>",
            s(&file)
        )
    );
    assert_eq!(out.structured["title"], "subdir/test.txt");
    assert_eq!(recorder.actions(), ["read"]);
    let read = recorder.get("read").unwrap();
    assert_eq!(read.resources, ["subdir/test.txt"]);
    assert_eq!(read.save, Some(vec!["*".to_string()]));
    assert!(fixture.services.read_stamp("ses_read", &file).is_some());
}

#[tokio::test]
async fn external_file_and_directory_ask_scoped_external_directory() {
    let fixture = Fixture::new().await;
    let outer = tempfile::tempdir().unwrap();
    let outer_root = std::fs::canonicalize(outer.path()).unwrap();
    std::fs::create_dir_all(outer_root.join("external")).unwrap();
    std::fs::write(outer_root.join("secret.txt"), "secret data").unwrap();
    std::fs::write(outer_root.join("external/a.txt"), "a").unwrap();
    let (_, recorder) = exec(
        &fixture,
        json!({"filePath": s(&outer_root.join("secret.txt"))}),
    )
    .await;
    let external = recorder.get("external_directory").unwrap();
    assert_eq!(external.resources, [format!("{}/*", s(&outer_root))]);
    assert_eq!(external.save, Some(external.resources.clone()));
    assert_eq!(
        external.metadata.unwrap(),
        json!({"filepath": s(&outer_root.join("secret.txt")), "parentDir": s(&outer_root)})
            .as_object()
            .unwrap()
            .clone()
    );
    assert_eq!(recorder.actions(), ["external_directory", "read"]);
    let (_, recorder) = exec(
        &fixture,
        json!({"filePath": s(&outer_root.join("external"))}),
    )
    .await;
    assert_eq!(
        recorder.get("external_directory").unwrap().resources,
        [format!("{}/external/*", s(&outer_root))]
    );
}

#[tokio::test]
async fn relative_outside_path_asks_external_directory_then_reports_missing() {
    let fixture = Fixture::new().await;
    let (result, recorder) = exec(&fixture, json!({"filePath": "../outside.txt"})).await;
    let target = fixture.root.parent().unwrap().join("outside.txt");
    assert!(
        result
            .unwrap_err()
            .starts_with(&format!("File not found: {}", s(&target)))
    );
    assert_eq!(recorder.actions(), ["external_directory", "read"]);
}

#[tokio::test]
async fn denied_permissions_read_no_content() {
    let fixture = Fixture::new().await;
    let outer = tempfile::tempdir().unwrap();
    std::fs::write(outer.path().join("secret.txt"), "secret").unwrap();
    fixture.put("inside.txt", "inside");
    let cases = [
        ("external_directory", s(&outer.path().join("secret.txt"))),
        ("read", s(&fixture.root.join("inside.txt"))),
    ];
    for (action, path) in cases {
        let recorder = Recorder::denying(action);
        let result = tool(&fixture)
            .execute(json!({"filePath": path}), &context(recorder))
            .await;
        assert_eq!(result.unwrap_err().message, "permission declined");
    }
    assert_eq!(fixture.disk.reads(), 0);
}

#[tokio::test]
async fn bypass_and_non_git_worktree_semantics() {
    let fixture = Fixture::new().await;
    let outer = tempfile::tempdir().unwrap();
    std::fs::write(outer.path().join("a.txt"), "a").unwrap();
    let recorder = Arc::new(Recorder::default());
    let path = s(&outer.path().join("a.txt"));
    tool(&fixture)
        .read(json!({"filePath": path}), &context(recorder.clone()), true)
        .await
        .unwrap();
    assert_eq!(recorder.actions(), ["read"]);
    // Non-git projects use worktree "/" which must not suppress external approval.
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    let rooted = Fixture::at(temp, root, PathBuf::from("/")).await;
    let (_, recorder) = exec(&rooted, json!({"filePath": path})).await;
    assert_eq!(recorder.actions(), ["external_directory", "read"]);
}

#[tokio::test]
async fn truncates_by_bytes_and_stops_streaming() {
    let fixture = Fixture::new().await;
    let content = format!("{}\n", "x".repeat(80)).repeat(50_000);
    let file = fixture.put("huge.txt", &content);
    let out = output(&fixture, json!({"filePath": s(&file)})).await;
    assert_eq!(out["metadata"]["truncated"], true);
    let text = out["output"].as_str().unwrap();
    assert!(text.ends_with(
        "\n\n(Output capped at 50 KB. Showing lines 1-632. Use offset=633 to continue.)\n</content>"
    ));
    assert!(fixture.disk.bytes() < content.len() / 2);
}

#[tokio::test]
async fn truncates_by_line_count_and_respects_offset() {
    let fixture = Fixture::new().await;
    let lines: Vec<String> = (0..100).map(|i| format!("line{i}")).collect();
    let file = fixture.put("many.txt", lines.join("\n"));
    let out = output(&fixture, json!({"filePath": s(&file), "limit": 10})).await;
    let text = out["output"].as_str().unwrap();
    assert!(text.contains("(Showing lines 1-10 of 100. Use offset=11 to continue.)"));
    assert!(text.contains("10: line9") && !text.contains("line10"));
    assert_eq!(out["metadata"]["truncated"], true);
    let out = output(
        &fixture,
        json!({"filePath": s(&file), "offset": 10, "limit": 5}),
    )
    .await;
    let text = out["output"].as_str().unwrap();
    assert!(text.contains("10: line9\n11: line10") && text.contains("14: line13"));
    assert!(!text.contains("15: line14"));
    assert_eq!(out["metadata"]["display"]["lineStart"], 10);
    assert_eq!(out["metadata"]["display"]["lineEnd"], 14);
    assert_eq!(out["metadata"]["display"]["totalLines"], 100);
    // offset 0 behaves like 1 (`params.offset || 1`).
    let out = output(
        &fixture,
        json!({"filePath": s(&file), "offset": 0, "limit": 1}),
    )
    .await;
    assert!(out["output"].as_str().unwrap().contains("1: line0"));
}

#[tokio::test]
async fn small_and_empty_files_and_offset_errors() {
    let fixture = Fixture::new().await;
    let small = fixture.put("small.txt", "hello world");
    let out = output(&fixture, json!({"filePath": s(&small)})).await;
    assert_eq!(
        out["metadata"],
        json!({
            "preview": "hello world",
            "truncated": false,
            "loaded": [],
            "display": {"type": "file", "path": s(&small), "text": "hello world", "lineStart": 1, "lineEnd": 1, "totalLines": 1, "truncated": false},
        })
    );
    let empty = fixture.put("empty.txt", "");
    let out = output(&fixture, json!({"filePath": s(&empty)})).await;
    assert!(
        out["output"]
            .as_str()
            .unwrap()
            .contains("(End of file - total 0 lines)")
    );
    assert_eq!(out["metadata"]["display"]["lineEnd"], 0);
    let (result, _) = exec(&fixture, json!({"filePath": s(&empty), "offset": 2})).await;
    assert_eq!(
        result.unwrap_err(),
        "Offset 2 is out of range for this file (0 lines)"
    );
    let short = fixture.put("short.txt", "line1\nline2\nline3");
    let (result, _) = exec(
        &fixture,
        json!({"filePath": s(&short), "offset": 4, "limit": 5}),
    )
    .await;
    assert_eq!(
        result.unwrap_err(),
        "Offset 4 is out of range for this file (3 lines)"
    );
}

#[tokio::test]
async fn crlf_lone_cr_bom_and_trailing_newline_split_like_effect() {
    let fixture = Fixture::new().await;
    let file = fixture.put("crlf.txt", "\u{feff}a\r\nb\rc\n\nd\n");
    let out = output(&fixture, json!({"filePath": s(&file)})).await;
    assert_eq!(out["metadata"]["display"]["text"], "a\nb\nc\n\nd");
    assert_eq!(out["metadata"]["display"]["totalLines"], 5);
    let file = fixture.put("bad.txt", [b'o', b'k', 0xff, b'\n', b'x', 0xe2, 0x82]);
    let out = output(&fixture, json!({"filePath": s(&file)})).await;
    assert_eq!(out["metadata"]["display"]["text"], "ok\u{fffd}\nx");
}

#[tokio::test]
async fn directory_pages_with_final_page_not_truncated() {
    let fixture = Fixture::new().await;
    for i in 1..=10 {
        fixture.put(&format!("dir/file-{i}.txt"), format!("line{i}"));
    }
    let dir = fixture.root.join("dir");
    let out = output(
        &fixture,
        json!({"filePath": s(&dir), "offset": 6, "limit": 5}),
    )
    .await;
    assert_eq!(
        out["metadata"]["display"],
        json!({
            "type": "directory",
            "path": s(&dir),
            "entries": ["file-5.txt", "file-6.txt", "file-7.txt", "file-8.txt", "file-9.txt"],
            "offset": 6,
            "totalEntries": 10,
            "truncated": false,
        })
    );
    std::fs::create_dir(fixture.root.join("dir/sub")).unwrap();
    let out = output(&fixture, json!({"filePath": s(&dir), "limit": 2})).await;
    assert_eq!(
        out["output"],
        format!(
            "<path>{}</path>\n<type>directory</type>\n<entries>\nfile-1.txt\nfile-10.txt\n\n(Showing 2 of 11 entries. Use 'offset' parameter to read beyond entry 3)\n</entries>",
            s(&dir)
        )
    );
    let out = output(&fixture, json!({"filePath": s(&dir), "offset": 11})).await;
    assert_eq!(out["metadata"]["display"]["entries"], json!(["sub/"]));
    assert!(out["output"].as_str().unwrap().contains("\n(11 entries)\n"));
}

#[tokio::test]
async fn long_lines_are_truncated() {
    let fixture = Fixture::new().await;
    let file = fixture.put("long-line.txt", "x".repeat(3000));
    let out = output(&fixture, json!({"filePath": s(&file)})).await;
    let text = out["output"].as_str().unwrap();
    assert!(text.contains("(line truncated to 2000 chars)"));
    assert!(text.len() < 3000);
}

#[tokio::test]
async fn images_and_pdfs_become_attachments_detected_by_content() {
    let fixture = Fixture::new().await;
    let png = fixture.put("image.png", png_bytes());
    let (result, _) = exec(&fixture, json!({"filePath": s(&png)})).await;
    let out = result.unwrap();
    assert_eq!(out.structured["output"], "Image read successfully");
    assert_eq!(
        out.structured["metadata"],
        json!({"preview": "Image read successfully", "truncated": false, "loaded": []})
    );
    let attachment = &out.structured["attachments"][0];
    assert_eq!(attachment["type"], "file");
    assert!(attachment.get("id").is_none());
    let jpeg = fixture.put(
        "image.bin",
        [
            0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, 0x4a, 0x46, 0x49, 0x46, 0x00, 0x01,
        ],
    );
    let out = output(&fixture, json!({"filePath": s(&jpeg)})).await;
    assert_eq!(out["attachments"][0]["mime"], "image/jpeg");
    assert!(
        out["attachments"][0]["url"]
            .as_str()
            .unwrap()
            .starts_with("data:image/jpeg;base64,")
    );
    let pdf = fixture.put("doc.pdf", b"%PDF-1.4 body");
    let out = output(&fixture, json!({"filePath": s(&pdf)})).await;
    assert_eq!(out["output"], "PDF read successfully");
    assert_eq!(out["attachments"][0]["mime"], "application/pdf");
}

#[tokio::test]
async fn large_fixture_image_is_attached() {
    let fixture = Fixture::new().await;
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../packages/opencode/test/tool/fixtures/large-image.png");
    let file = fixture.put("large-image.png", std::fs::read(source).unwrap());
    let out = output(&fixture, json!({"filePath": s(&file)})).await;
    assert_eq!(out["metadata"]["truncated"], false);
    assert_eq!(out["attachments"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn text_lookalikes_stay_text() {
    let fixture = Fixture::new().await;
    let fbs = fixture.put("schema.fbs", "namespace MyGame;\n\ntable Monster {}");
    let out = output(&fixture, json!({"filePath": s(&fbs)})).await;
    assert!(out.get("attachments").is_none());
    assert!(out["output"].as_str().unwrap().contains("namespace MyGame"));
    for (name, text) in [
        ("photo.tiff", "II text content"),
        ("photo.avif", "avif text content"),
    ] {
        let file = fixture.put(name, text);
        let out = output(&fixture, json!({"filePath": s(&file)})).await;
        assert!(out.get("attachments").is_none());
        assert!(out["output"].as_str().unwrap().contains(text));
    }
}

#[tokio::test]
async fn binary_detection_rejects_nul_and_known_extensions() {
    let fixture = Fixture::new().await;
    let nul = fixture.put("null-byte.txt", b"hello\0world");
    let (result, _) = exec(&fixture, json!({"filePath": s(&nul)})).await;
    assert_eq!(
        result.unwrap_err(),
        format!("Cannot read binary file: {}", s(&nul))
    );
    let wasm = fixture.put("module.wasm", "not really wasm");
    let (result, _) = exec(&fixture, json!({"filePath": s(&wasm)})).await;
    assert!(result.unwrap_err().contains("Cannot read binary file"));
}

#[tokio::test]
async fn missing_file_suggests_similar_names() {
    let fixture = Fixture::new().await;
    fixture.put("config.json", "{}");
    let missing = fixture.root.join("config");
    let (result, _) = exec(&fixture, json!({"filePath": s(&missing)})).await;
    assert_eq!(
        result.unwrap_err(),
        format!(
            "File not found: {}\n\nDid you mean one of these?\n{}",
            s(&missing),
            s(&fixture.root.join("config.json"))
        )
    );
}

#[tokio::test]
async fn invalid_arguments_use_legacy_wrapper_message() {
    let fixture = Fixture::new().await;
    let (result, _) = exec(&fixture, json!({"filePath": "a", "offset": -1})).await;
    assert_eq!(
        result.unwrap_err(),
        "The read tool was called with invalid arguments: SchemaError(Expected a value greater than or equal to 0, got -1\n  at [\"offset\"]).\nPlease rewrite the input so it satisfies the expected schema."
    );
}

#[derive(Debug, Default)]
struct Hooks {
    warmed: Mutex<Vec<PathBuf>>,
}

#[async_trait]
impl LegacyReadHooks for Hooks {
    async fn instructions(
        &self,
        filepath: &Path,
        _: &ToolContext,
    ) -> Result<Vec<LoadedInstruction>, String> {
        let agents = filepath
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("AGENTS.md");
        let content = std::fs::read_to_string(&agents).map_err(|e| e.to_string())?;
        Ok(vec![LoadedInstruction {
            filepath: s(&agents),
            content: format!("Instructions from: {}\n{content}", s(&agents)),
        }])
    }
    fn warm(&self, filepath: &Path) {
        self.warmed.lock().unwrap().push(filepath.into());
    }
}

#[tokio::test]
async fn loaded_instructions_are_appended_and_lsp_is_warmed() {
    let fixture = Fixture::new().await;
    let agents = fixture.put(
        "subdir/AGENTS.md",
        "# Test Instructions\nDo something special.",
    );
    let file = fixture.put("subdir/nested/test.txt", "test content");
    let hooks = Arc::new(Hooks::default());
    let tool = LegacyReadTool::new(fixture.services.clone(), hooks.clone());
    let out = tool
        .execute(json!({"filePath": s(&file)}), &context(Arc::default()))
        .await
        .unwrap()
        .structured;
    let text = out["output"].as_str().unwrap();
    assert!(text.ends_with(&format!(
        "</content>\n\n<system-reminder>\nInstructions from: {}\n# Test Instructions\nDo something special.\n</system-reminder>",
        s(&agents)
    )));
    assert_eq!(out["metadata"]["loaded"], json!([s(&agents)]));
    assert_eq!(*hooks.warmed.lock().unwrap(), [file]);
}
