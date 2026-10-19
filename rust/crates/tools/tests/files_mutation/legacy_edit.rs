//! Purpose: Legacy (V1) edit tool conformance over real temp files, ported from
//! packages/opencode/test/tool/edit.test.ts (creation, replacement, line endings, events,
//! concurrency) plus approval ordering, formatter BOM resync and LSP output.
//! Scope: `LegacyEditTool::execute` through the public tool boundary.
//! Uses:
//! rust/crates/
//! |-- tool-api/src/lib.rs
//! |-- tools/src/files/edit_legacy.rs
//! `-- tools/tests/files_mutation/support.rs
//! External:
//! |-- serde_json::Map::new
//! |-- serde_json::Value
//! |-- serde_json::json
//! `-- tokio::test
//! Used by:
//! rust/crates/
//! `-- tools/tests/files_edit.rs
//! Invariants: nothing is written when approval fails, concurrent edits of one file serialize.
//! Contract tests: this suite

use super::support::{Fixture, Hooks, Progress, Recorder, context, outside, s, with_progress};
use opencode_tool_api::{Tool, ToolOutput};
use opencode_tools::files::edit_legacy::{Event, LegacyEditTool};
use serde_json::{Value, json};
use std::sync::Arc;

fn tool(fixture: &Fixture, hooks: Arc<Hooks>) -> LegacyEditTool {
    LegacyEditTool::new(fixture.services.clone(), hooks)
}

async fn run(fixture: &Fixture, input: Value) -> Result<ToolOutput, String> {
    tool(fixture, Arc::default())
        .execute(input, &context(Arc::default()))
        .await
        .map_err(|failure| failure.message)
}

fn edit(path: &str, old: &str, new: &str) -> Value {
    json!({"filePath": path, "oldString": old, "newString": new})
}

#[tokio::test]
async fn creates_new_and_nested_files_when_old_string_is_empty() {
    let fixture = Fixture::new().await;
    let file = s(&fixture.root.join("newfile.txt"));
    let output = run(&fixture, edit(&file, "", "new content")).await.unwrap();
    assert!(
        output.structured["metadata"]["diff"]
            .as_str()
            .unwrap()
            .contains("new content")
    );
    assert_eq!(fixture.load("newfile.txt"), "new content");
    let nested = s(&fixture.root.join("nested/dir/file.txt"));
    run(&fixture, edit(&nested, "", "nested file"))
        .await
        .unwrap();
    assert_eq!(fixture.load("nested/dir/file.txt"), "nested file");
}

#[tokio::test]
async fn rejects_empty_old_string_on_existing_file() {
    let fixture = Fixture::new().await;
    let original = "\u{FEFF}using System;\n";
    let file = s(&fixture.put("existing.cs", original));
    let error = run(&fixture, edit(&file, "", "using Up;\n"))
        .await
        .unwrap_err();
    assert!(error.contains("oldString cannot be empty"));
    assert_eq!(fixture.load("existing.cs"), original);
}

#[tokio::test]
async fn emits_add_and_change_events() {
    let fixture = Fixture::new().await;
    let hooks = Arc::new(Hooks::default());
    let file = s(&fixture.root.join("new.txt"));
    let recorder = Arc::new(Recorder::default());
    let ctx = context(recorder);
    tool(&fixture, hooks.clone())
        .execute(edit(&file, "", "content"), &ctx)
        .await
        .unwrap();
    tool(&fixture, hooks.clone())
        .execute(edit(&file, "content", "more"), &ctx)
        .await
        .unwrap();
    assert_eq!(
        *hooks.events.lock().unwrap(),
        vec![
            Event::Edited { file: file.clone() },
            Event::Updated {
                file: file.clone(),
                event: "add"
            },
            Event::Edited { file: file.clone() },
            Event::Updated {
                file: file.clone(),
                event: "change"
            },
        ]
    );
    assert_eq!(hooks.touched.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn replaces_text_with_exact_legacy_output_and_metadata() {
    let fixture = Fixture::new().await;
    let file = s(&fixture.put("existing.txt", "old content here"));
    let recorder = Arc::new(Recorder::default());
    let progress = Arc::new(Progress::default());
    let output = tool(&fixture, Arc::default())
        .execute(
            edit(&file, "old content", "new content"),
            &with_progress(recorder.clone(), progress.clone()),
        )
        .await
        .unwrap();
    let diff = format!(
        "Index: {file}\n===================================================================\n--- {file}\n+++ {file}\n@@ -1,1 +1,1 @@\n-old content here\n\\ No newline at end of file\n+new content here\n\\ No newline at end of file\n"
    );
    assert_eq!(
        output.structured,
        json!({
            "title": "existing.txt",
            "metadata": {
                "diagnostics": {},
                "diff": diff,
                "filediff": {"file": file, "patch": diff, "additions": 1, "deletions": 1},
            },
            "output": "Edit applied successfully.",
        })
    );
    assert_eq!(fixture.load("existing.txt"), "new content here");
    let calls = recorder.all();
    assert_eq!(recorder.actions(), vec!["edit"]);
    assert_eq!(calls[0].resources, vec!["existing.txt"]);
    assert_eq!(calls[0].save, Some(vec!["*".to_string()]));
    assert_eq!(calls[0].metadata.as_ref().unwrap()["filepath"], file);
    assert_eq!(calls[0].metadata.as_ref().unwrap()["diff"], diff);
    let updates = progress.updates.lock().unwrap();
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0]["diagnostics"], json!({}));
}

#[tokio::test]
async fn replaces_first_visible_line_in_bom_files() {
    let fixture = Fixture::new().await;
    let file = s(&fixture.put("existing.cs", "\u{FEFF}using System;\nclass Test {}\n"));
    let output = run(&fixture, edit(&file, "using System;", "using Up;"))
        .await
        .unwrap();
    let diff = output.structured["metadata"]["diff"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(diff.contains("-using System;"));
    assert!(diff.contains("+using Up;"));
    assert!(!diff.contains('\u{FEFF}'));
    assert_eq!(
        fixture.load("existing.cs"),
        "\u{FEFF}using Up;\nclass Test {}\n"
    );
}

#[tokio::test]
async fn reports_missing_identical_unmatched_and_directory_errors() {
    let fixture = Fixture::new().await;
    let missing = s(&fixture.root.join("nonexistent.txt"));
    assert_eq!(
        run(&fixture, edit(&missing, "old", "new"))
            .await
            .unwrap_err(),
        format!("File {missing} not found")
    );
    let file = s(&fixture.put("file.txt", "actual content"));
    assert!(
        run(&fixture, edit(&file, "same", "same"))
            .await
            .unwrap_err()
            .contains("identical")
    );
    assert!(
        run(&fixture, edit(&file, "", ""))
            .await
            .unwrap_err()
            .contains("identical")
    );
    assert_eq!(
        run(&fixture, edit(&file, "not in file", "replacement"))
            .await
            .unwrap_err(),
        "Could not find oldString in the file. It must match exactly, including whitespace, indentation, and line endings."
    );
    std::fs::create_dir(fixture.root.join("adir")).unwrap();
    let dir = s(&fixture.root.join("adir"));
    assert_eq!(
        run(&fixture, edit(&dir, "old", "new")).await.unwrap_err(),
        format!("Path is a directory, not a file: {dir}")
    );
}

#[tokio::test]
async fn rejects_loose_block_anchor_matches() {
    let fixture = Fixture::new().await;
    let old = "function configure() {\n  const enabled = true\n}";
    let new = "function configure() {\n  const enabled = false\n}";
    for original in [
        "function configure() {\n  keepImportantState()\n  removeAllUserData()\n  archiveBackups()\n  auditLog()\n}",
        "function configure() {\n  removeAllUserData()\n}",
    ] {
        let file = s(&fixture.put("file.ts", original));
        assert!(
            run(&fixture, edit(&file, old, new))
                .await
                .unwrap_err()
                .contains("Could not find oldString")
        );
        assert_eq!(fixture.load("file.ts"), original);
    }
}

#[tokio::test]
async fn multiple_fuzzy_matches_are_ambiguous() {
    let fixture = Fixture::new().await;
    let file = s(&fixture.put("multi.txt", "  foo\nbar\n  foo\n"));
    assert_eq!(
        run(&fixture, edit(&file, "foo", "baz")).await.unwrap_err(),
        "Found multiple matches for oldString. Provide more surrounding context to make the match unique."
    );
    assert_eq!(fixture.load("multi.txt"), "  foo\nbar\n  foo\n");
}

#[tokio::test]
async fn replace_all_multiline_and_crlf() {
    let fixture = Fixture::new().await;
    let file = s(&fixture.put("file.txt", "foo bar foo baz foo"));
    let mut input = edit(&file, "foo", "qux");
    input["replaceAll"] = json!(true);
    run(&fixture, input).await.unwrap();
    assert_eq!(fixture.load("file.txt"), "qux bar qux baz qux");
    let file = s(&fixture.put("multi.txt", "line1\nline2\nline3"));
    run(&fixture, edit(&file, "line2", "new line 2\nextra line"))
        .await
        .unwrap();
    assert_eq!(
        fixture.load("multi.txt"),
        "line1\nnew line 2\nextra line\nline3"
    );
    let file = s(&fixture.put("crlf.txt", "line1\r\nold\r\nline3"));
    run(&fixture, edit(&file, "old", "new")).await.unwrap();
    assert_eq!(fixture.load("crlf.txt"), "line1\r\nnew\r\nline3");
}

#[tokio::test]
async fn tracks_file_diff_statistics() {
    let fixture = Fixture::new().await;
    let file = s(&fixture.put("file.txt", "line1\nline2\nline3"));
    let output = run(&fixture, edit(&file, "line2", "new line a\nnew line b"))
        .await
        .unwrap();
    let filediff = &output.structured["metadata"]["filediff"];
    assert_eq!(filediff["file"], file);
    assert_eq!(filediff["additions"], 2);
    assert_eq!(filediff["deletions"], 1);
}

#[tokio::test]
async fn external_paths_ask_external_directory_first_and_denial_writes_nothing() {
    let fixture = Fixture::new().await;
    let (_keep, other) = outside();
    let target = other.join("external.txt");
    std::fs::write(&target, "before").unwrap();
    let recorder = Recorder::denying("external_directory");
    let result = tool(&fixture, Arc::default())
        .execute(
            edit(&s(&target), "before", "after"),
            &context(recorder.clone()),
        )
        .await;
    assert!(result.is_err());
    assert_eq!(recorder.actions(), vec!["external_directory"]);
    assert_eq!(
        recorder.all()[0].resources,
        vec![format!("{}/*", s(&other))]
    );
    assert_eq!(fixture.disk.reads(), 0);
    let recorder = Recorder::denying("edit");
    let result = tool(&fixture, Arc::default())
        .execute(
            edit(&s(&target), "before", "after"),
            &context(recorder.clone()),
        )
        .await;
    assert!(result.is_err());
    assert_eq!(recorder.actions(), vec!["external_directory", "edit"]);
    assert!(fixture.disk.writes().is_empty());
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "before");
}

#[tokio::test]
async fn formatter_bom_resync_and_lsp_errors() {
    let fixture = Fixture::new().await;
    let file = s(&fixture.put("formatted.cs", "\u{FEFF}using System;\n"));
    let diagnostic = json!({"severity": 1, "message": "bad", "range": {"start": {"line": 0, "character": 2}, "end": {"line": 0, "character": 3}}});
    let mut diagnostics = serde_json::Map::new();
    diagnostics.insert(file.clone(), json!([diagnostic, {"severity": 2, "message": "warn", "range": {"start": {"line": 1, "character": 0}}}]));
    let hooks = Arc::new(Hooks {
        strip_bom: true,
        diagnostics,
        ..Hooks::default()
    });
    let output = tool(&fixture, hooks)
        .execute(edit(&file, "System", "Up"), &context(Arc::default()))
        .await
        .unwrap();
    assert_eq!(fixture.load("formatted.cs"), "\u{FEFF}using Up;\n");
    assert_eq!(
        output.structured["output"],
        format!(
            "Edit applied successfully.\n\nLSP errors detected in this file, please fix:\n<diagnostics file=\"{file}\">\nERROR [1:3] bad\n</diagnostics>"
        )
    );
}

#[tokio::test]
async fn invalid_arguments_use_legacy_wrapper_message() {
    let fixture = Fixture::new().await;
    assert_eq!(
        run(&fixture, json!({"oldString": "a", "newString": "b"}))
            .await
            .unwrap_err(),
        "The edit tool was called with invalid arguments: SchemaError(Missing key\n  at [\"filePath\"]).\nPlease rewrite the input so it satisfies the expected schema."
    );
    assert_eq!(
        run(&fixture, edit("", "a", "b")).await.unwrap_err(),
        "filePath is required"
    );
}
