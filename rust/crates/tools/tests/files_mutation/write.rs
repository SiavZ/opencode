//! Purpose: Core and legacy write tool conformance over real temp files, ported from
//! packages/core/test/tool-write.test.ts and packages/opencode/test/tool/write.test.ts.
//! Scope: `CoreWriteTool`/`LegacyWriteTool::execute` through the public tool boundary.
//! Uses:
//! rust/crates/
//! |-- tool-api/src/lib.rs
//! |-- tools/src/files/edit_legacy.rs
//! |-- tools/src/files/write.rs
//! `-- tools/tests/files_mutation/support.rs
//! External:
//! |-- serde_json::Value
//! |-- serde_json::json
//! `-- tokio::test
//! Used by:
//! rust/crates/
//! `-- tools/tests/files_write.rs
//! Invariants: denied approvals write nothing, BOMs are preserved and never doubled.
//! Contract tests: this suite

use super::support::{Fixture, Hooks, Recorder, context, outside, s};
use opencode_tool_api::{Tool, ToolContent, ToolOutput};
use opencode_tools::files::edit_legacy::Event;
use opencode_tools::files::write::{CoreWriteTool, LegacyWriteTool};
use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

async fn core(
    fixture: &Fixture,
    recorder: Arc<Recorder>,
    input: Value,
) -> Result<ToolOutput, String> {
    CoreWriteTool::new(fixture.services.clone())
        .execute(input, &context(recorder))
        .await
        .map_err(|failure| failure.message)
}

async fn legacy(fixture: &Fixture, hooks: Arc<Hooks>, input: Value) -> Result<ToolOutput, String> {
    LegacyWriteTool::new(fixture.services.clone(), hooks)
        .execute(input, &context(Arc::default()))
        .await
        .map_err(|failure| failure.message)
}

fn text(output: &ToolOutput) -> &str {
    match &output.content[0] {
        ToolContent::Text { text } => text,
        other => panic!("expected text, got {other:?}"),
    }
}

#[tokio::test]
async fn core_creates_relative_file_through_file_mutation_once() {
    let fixture = Fixture::new().await;
    let recorder = Arc::new(Recorder::default());
    let output = core(
        &fixture,
        recorder.clone(),
        json!({"path": "src/new.txt", "content": "created"}),
    )
    .await
    .unwrap();
    let target = fixture.root.join("src/new.txt");
    assert_eq!(text(&output), "Created file successfully: src/new.txt");
    assert_eq!(
        output.structured,
        json!({"operation": "write", "target": s(&target), "resource": "src/new.txt", "existed": false})
    );
    assert_eq!(fixture.load("src/new.txt"), "created");
    let calls = recorder.all();
    assert_eq!(recorder.actions(), vec!["edit"]);
    assert_eq!(calls[0].resources, vec!["src/new.txt"]);
    assert_eq!(calls[0].save, Some(vec!["*".to_string()]));
    assert_eq!(fixture.disk.writes(), vec![target]);
}

#[tokio::test]
async fn core_overwrites_and_reports_wrote() {
    let fixture = Fixture::new().await;
    fixture.put("existing.txt", "before");
    let output = core(
        &fixture,
        Arc::default(),
        json!({"path": "existing.txt", "content": "after"}),
    )
    .await
    .unwrap();
    assert_eq!(text(&output), "Wrote file successfully: existing.txt");
    assert_eq!(output.structured["existed"], true);
    assert_eq!(fixture.load("existing.txt"), "after");
    assert_eq!(fixture.disk.writes().len(), 1);
}

#[tokio::test]
async fn core_preserves_exactly_one_bom() {
    let fixture = Fixture::new().await;
    fixture.put("preserved.txt", "\u{FEFF}before");
    fixture.put("deduplicated.txt", "\u{FEFF}before");
    core(
        &fixture,
        Arc::default(),
        json!({"path": "preserved.txt", "content": "after"}),
    )
    .await
    .unwrap();
    core(
        &fixture,
        Arc::default(),
        json!({"path": "deduplicated.txt", "content": "\u{FEFF}after"}),
    )
    .await
    .unwrap();
    assert_eq!(fixture.load("preserved.txt"), "\u{FEFF}after");
    assert_eq!(fixture.load("deduplicated.txt"), "\u{FEFF}after");
}

#[tokio::test]
async fn core_absolute_inside_and_external_paths() {
    let fixture = Fixture::new().await;
    let inside = fixture.root.join("absolute.txt");
    let recorder = Arc::new(Recorder::default());
    let output = core(
        &fixture,
        recorder.clone(),
        json!({"path": s(&inside), "content": "inside"}),
    )
    .await
    .unwrap();
    assert_eq!(text(&output), "Created file successfully: absolute.txt");
    assert_eq!(recorder.actions(), vec!["edit"]);
    let (_keep, other) = outside();
    let target = other.join("external.txt");
    let recorder = Arc::new(Recorder::default());
    let output = core(
        &fixture,
        recorder.clone(),
        json!({"path": s(&target), "content": "external"}),
    )
    .await
    .unwrap();
    let calls = recorder.all();
    assert_eq!(recorder.actions(), vec!["external_directory", "edit"]);
    assert_eq!(calls[0].resources, vec![format!("{}/*", s(&other))]);
    assert_eq!(calls[1].resources, vec![s(&target)]);
    assert_eq!(
        output.structured,
        json!({"operation": "write", "target": s(&target), "resource": s(&target), "existed": false})
    );
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "external");
}

#[tokio::test]
async fn core_denials_write_nothing() {
    let fixture = Fixture::new().await;
    let (_keep, other) = outside();
    let external = s(&other.join("denied.txt"));
    let recorder = Recorder::denying("external_directory");
    let result = core(
        &fixture,
        recorder.clone(),
        json!({"path": external, "content": "blocked"}),
    )
    .await;
    assert_eq!(result.unwrap_err(), format!("Unable to write {external}"));
    assert_eq!(recorder.actions(), vec!["external_directory"]);
    let recorder = Recorder::denying("edit");
    let result = core(
        &fixture,
        recorder.clone(),
        json!({"path": "denied.txt", "content": "blocked"}),
    )
    .await;
    assert_eq!(result.unwrap_err(), "Unable to write denied.txt");
    assert_eq!(recorder.actions(), vec!["edit"]);
    assert!(fixture.disk.writes().is_empty());
    assert_eq!(fixture.disk.reads(), 0);
}

#[tokio::test]
async fn core_rejects_escapes_and_non_directory_ancestors() {
    let fixture = Fixture::new().await;
    fixture.put("file.txt", "x");
    let (_keep, other) = outside();
    std::os::unix::fs::symlink(&other, fixture.root.join("link")).unwrap();
    for path in ["../escape.txt", "link/target.txt", "file.txt/child.txt"] {
        let recorder = Arc::new(Recorder::default());
        let result = core(
            &fixture,
            recorder.clone(),
            json!({"path": path, "content": "x"}),
        )
        .await;
        assert_eq!(result.unwrap_err(), format!("Unable to write {path}"));
        assert!(recorder.actions().is_empty());
    }
    assert!(!other.join("target.txt").exists());
    assert_eq!(
        core(&fixture, Arc::default(), json!({"path": "a"}))
            .await
            .unwrap_err(),
        "Invalid tool input: Missing key\n  at [\"content\"]"
    );
}

fn write(path: &str, content: &str) -> Value {
    json!({"filePath": path, "content": content})
}

#[tokio::test]
async fn legacy_creates_new_nested_and_relative_files() {
    let fixture = Fixture::new().await;
    let file = s(&fixture.root.join("newfile.txt"));
    let output = legacy(&fixture, Arc::default(), write(&file, "Hello, World!"))
        .await
        .unwrap();
    assert_eq!(
        output.structured,
        json!({"title": "newfile.txt", "metadata": {"diagnostics": {}, "filepath": file, "exists": false}, "output": "Wrote file successfully."})
    );
    assert_eq!(fixture.load("newfile.txt"), "Hello, World!");
    legacy(
        &fixture,
        Arc::default(),
        write(
            &s(&fixture.root.join("nested/deep/file.txt")),
            "nested content",
        ),
    )
    .await
    .unwrap();
    assert_eq!(fixture.load("nested/deep/file.txt"), "nested content");
    legacy(
        &fixture,
        Arc::default(),
        write("relative.txt", "relative content"),
    )
    .await
    .unwrap();
    assert_eq!(fixture.load("relative.txt"), "relative content");
}

#[tokio::test]
async fn legacy_overwrites_preserves_bom_and_restores_after_formatter() {
    let fixture = Fixture::new().await;
    let file = s(&fixture.put("existing.txt", "old content"));
    let output = legacy(&fixture, Arc::default(), write(&file, "new content"))
        .await
        .unwrap();
    assert_eq!(output.structured["metadata"]["exists"], true);
    assert_eq!(output.structured["metadata"]["filepath"], file);
    assert_eq!(fixture.load("existing.txt"), "new content");
    let file = s(&fixture.put("existing.cs", "\u{FEFF}using System;\n"));
    legacy(&fixture, Arc::default(), write(&file, "using Up;\n"))
        .await
        .unwrap();
    assert_eq!(fixture.load("existing.cs"), "\u{FEFF}using Up;\n");
    let file = s(&fixture.put("formatted.cs", "\u{FEFF}using System;\n"));
    let hooks = Arc::new(Hooks {
        strip_bom: true,
        ..Hooks::default()
    });
    legacy(&fixture, hooks.clone(), write(&file, "using Up;\n"))
        .await
        .unwrap();
    assert_eq!(fixture.load("formatted.cs"), "\u{FEFF}using Up;\n");
    assert_eq!(
        *hooks.events.lock().unwrap(),
        vec![
            Event::Edited { file: file.clone() },
            Event::Updated {
                file,
                event: "change"
            }
        ]
    );
}

#[tokio::test]
async fn legacy_content_types_and_mode() {
    let fixture = Fixture::new().await;
    let cases = [
        ("data.json", "{\n  \"key\": \"value\"\n}"),
        ("binary.bin", "Hello\u{0}World\u{1}\u{2}\u{3}"),
        ("empty.txt", ""),
        ("multiline.txt", "Line 1\nLine 2\nLine 3\n"),
        ("crlf.txt", "Line 1\r\nLine 2\r\nLine 3"),
    ];
    for (name, content) in cases {
        legacy(
            &fixture,
            Arc::default(),
            write(&s(&fixture.root.join(name)), content),
        )
        .await
        .unwrap();
        assert_eq!(fixture.load(name), content);
    }
    let mode = std::fs::metadata(fixture.root.join("data.json"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o644);
}

#[tokio::test]
async fn legacy_permission_metadata_title_and_os_denial() {
    let fixture = Fixture::new().await;
    std::fs::create_dir_all(fixture.root.join("src/components")).unwrap();
    let file = s(&fixture.root.join("src/components/Button.tsx"));
    let recorder = Arc::new(Recorder::default());
    let output = LegacyWriteTool::new(fixture.services.clone(), Arc::new(Hooks::default()))
        .execute(
            write(&file, "export const Button = () => {}"),
            &context(recorder.clone()),
        )
        .await
        .unwrap();
    assert_eq!(output.structured["title"], "src/components/Button.tsx");
    let call = &recorder.all()[0];
    assert_eq!(call.resources, vec!["src/components/Button.tsx"]);
    let diff = format!(
        "Index: {file}\n===================================================================\n--- {file}\n+++ {file}\n@@ -0,0 +1,1 @@\n+export const Button = () => {{}}\n\\ No newline at end of file\n"
    );
    assert_eq!(call.metadata.as_ref().unwrap()["diff"], diff);
    let readonly = fixture.put("readonly.txt", "test");
    std::fs::set_permissions(&readonly, std::fs::Permissions::from_mode(0o444)).unwrap();
    assert!(
        legacy(
            &fixture,
            Arc::default(),
            write(&s(&readonly), "new content")
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn legacy_external_directory_denial_writes_nothing() {
    let fixture = Fixture::new().await;
    let (_keep, other) = outside();
    let target = other.join("x.txt");
    let recorder = Recorder::denying("external_directory");
    let result = LegacyWriteTool::new(fixture.services.clone(), Arc::new(Hooks::default()))
        .execute(write(&s(&target), "x"), &context(recorder.clone()))
        .await;
    assert!(result.is_err());
    assert_eq!(recorder.actions(), vec!["external_directory"]);
    assert!(!target.exists());
    assert!(fixture.disk.writes().is_empty());
}
