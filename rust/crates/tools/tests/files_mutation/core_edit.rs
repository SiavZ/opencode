//! Purpose: Core (V2) edit tool conformance over real temp files, ported from
//! packages/core/test/tool-edit.test.ts plus Location escape and missing-ancestor cases.
//! Scope: `CoreEditTool::execute` through the public tool boundary.
//! Uses:
//! rust/crates/
//! |-- tool-api/src/lib.rs
//! |-- tools/src/files/edit.rs
//! `-- tools/tests/files_mutation/support.rs
//! External:
//! |-- serde_json::Value
//! |-- serde_json::json
//! `-- tokio::test
//! Used by:
//! rust/crates/
//! `-- tools/tests/files_edit.rs
//! Invariants: denied approvals read zero bytes and write nothing.
//! Contract tests: this suite

use super::support::{Fixture, Recorder, context, outside, s};
use opencode_tool_api::{Tool, ToolContent, ToolOutput};
use opencode_tools::files::edit::CoreEditTool;
use serde_json::{Value, json};
use std::sync::Arc;

async fn exec(
    fixture: &Fixture,
    recorder: Arc<Recorder>,
    input: Value,
) -> Result<ToolOutput, String> {
    CoreEditTool::new(fixture.services.clone())
        .execute(input, &context(recorder))
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
async fn replaces_relative_exact_text_through_file_mutation_once() {
    let fixture = Fixture::new().await;
    let target = fixture.put("hello.txt", "before\nrest\n");
    let recorder = Arc::new(Recorder::default());
    let input = json!({"path": "hello.txt", "oldString": "before", "newString": "after"});
    let output = exec(&fixture, recorder.clone(), input).await.unwrap();
    assert_eq!(
        text(&output),
        "Edited file successfully: hello.txt\nReplacements: 1\n```diff\n-before\n+after\n```"
    );
    assert_eq!(
        output.structured,
        json!({
            "files": [{
                "file": "hello.txt",
                "patch": "Index: hello.txt\n===================================================================\n--- hello.txt\n+++ hello.txt\n@@ -1,2 +1,2 @@\n-before\n+after\n rest\n",
                "additions": 1,
                "deletions": 1,
                "status": "modified",
            }],
            "replacements": 1,
        })
    );
    assert_eq!(fixture.load("hello.txt"), "after\nrest\n");
    let calls = recorder.all();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].action, "edit");
    assert_eq!(calls[0].session_id, "ses_edit_tool_test");
    assert_eq!(calls[0].resources, vec!["hello.txt"]);
    assert_eq!(calls[0].save, Some(vec!["*".to_string()]));
    assert_eq!(fixture.disk.writes(), vec![target]);
}

#[tokio::test]
async fn accepts_absolute_path_inside_location() {
    let fixture = Fixture::new().await;
    let target = fixture.put("absolute.txt", "before");
    let recorder = Arc::new(Recorder::default());
    let input = json!({"path": s(&target), "oldString": "before", "newString": "after"});
    exec(&fixture, recorder.clone(), input).await.unwrap();
    assert_eq!(recorder.actions(), vec!["edit"]);
    assert_eq!(fixture.load("absolute.txt"), "after");
}

#[tokio::test]
async fn approves_external_absolute_path_before_edit() {
    let fixture = Fixture::new().await;
    let (_keep, other) = outside();
    let target = other.join("external.txt");
    std::fs::write(&target, "before").unwrap();
    let recorder = Arc::new(Recorder::default());
    let input = json!({"path": s(&target), "oldString": "before", "newString": "after"});
    let output = exec(&fixture, recorder.clone(), input).await.unwrap();
    assert_eq!(recorder.actions(), vec!["external_directory", "edit"]);
    let calls = recorder.all();
    assert_eq!(calls[0].resources, vec![format!("{}/*", s(&other))]);
    assert_eq!(calls[1].resources, vec![s(&target)]);
    assert_eq!(output.structured["files"][0]["file"], s(&target));
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "after");
    assert_eq!(fixture.disk.writes().len(), 1);
}

#[tokio::test]
async fn denied_external_or_edit_reads_nothing_and_writes_nothing() {
    let fixture = Fixture::new().await;
    let (_keep, other) = outside();
    let external = other.join("denied.txt");
    std::fs::write(&external, "before").unwrap();
    let input = json!({"path": s(&external), "oldString": "before", "newString": "after"});
    for (deny, actions) in [
        ("external_directory", vec!["external_directory"]),
        ("edit", vec!["external_directory", "edit"]),
    ] {
        fixture.disk.reset();
        let recorder = Recorder::denying(deny);
        let result = exec(&fixture, recorder.clone(), input.clone()).await;
        assert_eq!(
            result.unwrap_err(),
            format!("Unable to edit {}", s(&external))
        );
        assert_eq!(recorder.actions(), actions);
        assert_eq!(fixture.disk.reads(), 0);
        assert!(fixture.disk.writes().is_empty());
    }
    assert_eq!(std::fs::read_to_string(&external).unwrap(), "before");
}

/// Mandatory: tool-edit.test.ts "denied edit reads no target content and does not disclose
/// whether oldString matches".
#[tokio::test]
async fn denied_edit_reads_zero_bytes_and_does_not_disclose_match() {
    let fixture = Fixture::new().await;
    fixture.put("secret.txt", "secret content");
    let recorder = Recorder::denying("edit");
    let matching = exec(
        &fixture,
        recorder.clone(),
        json!({"path": "secret.txt", "oldString": "secret content", "newString": "replacement"}),
    )
    .await;
    let missing = exec(
        &fixture,
        recorder.clone(),
        json!({"path": "secret.txt", "oldString": "not present", "newString": "replacement"}),
    )
    .await;
    assert_eq!(matching.clone().unwrap_err(), "Unable to edit secret.txt");
    assert_eq!(missing.unwrap_err(), matching.unwrap_err());
    assert_eq!(recorder.actions(), vec!["edit", "edit"]);
    assert_eq!(fixture.disk.reads(), 0);
    assert!(fixture.disk.writes().is_empty());
}

#[tokio::test]
async fn rejects_noop_empty_missing_and_ambiguous_replacements() {
    let fixture = Fixture::new().await;
    fixture.put("matches.txt", "same same");
    let cases = [
        (
            "same",
            "same",
            "No changes to apply: oldString and newString are identical.",
        ),
        (
            "",
            "after",
            "oldString must not be empty. Use write to create or overwrite a file.",
        ),
        (
            "missing",
            "after",
            "Could not find oldString in the file. It must match exactly, including whitespace and indentation.",
        ),
        (
            "same",
            "after",
            "Found multiple exact matches for oldString. Provide more surrounding context or set replaceAll to true.",
        ),
    ];
    for (old, new, message) in cases {
        let recorder = Arc::new(Recorder::default());
        let input = json!({"path": "matches.txt", "oldString": old, "newString": new});
        assert_eq!(exec(&fixture, recorder, input).await.unwrap_err(), message);
    }
    assert!(fixture.disk.writes().is_empty());
}

#[tokio::test]
async fn noop_and_empty_old_string_fail_before_approval() {
    let fixture = Fixture::new().await;
    let recorder = Arc::new(Recorder::default());
    for (old, new) in [("x", "x"), ("", "y")] {
        let input = json!({"path": "missing.txt", "oldString": old, "newString": new});
        exec(&fixture, recorder.clone(), input).await.unwrap_err();
    }
    assert!(recorder.actions().is_empty());
}

#[tokio::test]
async fn replace_all_replaces_every_exact_occurrence_literally() {
    let fixture = Fixture::new().await;
    fixture.put("all.txt", "same same same");
    let input =
        json!({"path": "all.txt", "oldString": "same", "newString": "after", "replaceAll": true});
    let output = exec(&fixture, Arc::new(Recorder::default()), input)
        .await
        .unwrap();
    assert_eq!(output.structured["replacements"], 3);
    assert_eq!(fixture.load("all.txt"), "after after after");
    assert_eq!(fixture.disk.writes().len(), 1);
    // JS String.replace substitution patterns apply to the replacement string.
    fixture.put("dollar.txt", "a x b");
    let input = json!({"path": "dollar.txt", "oldString": "x", "newString": "[$&|$$|$`]"});
    exec(&fixture, Arc::new(Recorder::default()), input)
        .await
        .unwrap();
    assert_eq!(fixture.load("dollar.txt"), "a [x|$|a ] b");
}

#[tokio::test]
async fn preserves_bom_and_crlf_line_endings() {
    let fixture = Fixture::new().await;
    fixture.put("windows.txt", "\u{FEFF}before\r\nrest\r\n");
    let input =
        json!({"path": "windows.txt", "oldString": "before\nrest", "newString": "after\nrest"});
    exec(&fixture, Arc::new(Recorder::default()), input)
        .await
        .unwrap();
    assert_eq!(fixture.load("windows.txt"), "\u{FEFF}after\r\nrest\r\n");
}

#[tokio::test]
async fn rejects_in_place_change_between_match_and_conditional_commit() {
    let fixture = Fixture::new().await;
    let target = fixture.put("concurrent.txt", "before\n");
    let racing = target.clone();
    fixture.disk.on_read(move |_, count| {
        if count == 1 {
            std::fs::write(&racing, "newer\n").unwrap();
        }
    });
    let input = json!({"path": "concurrent.txt", "oldString": "before", "newString": "after"});
    let result = exec(&fixture, Arc::new(Recorder::default()), input).await;
    assert_eq!(
        result.unwrap_err(),
        "File changed after permission approval. Read it again before editing."
    );
    assert_eq!(fixture.load("concurrent.txt"), "newer\n");
    assert!(fixture.disk.writes().is_empty());
}

#[tokio::test]
async fn missing_file_relative_escape_and_symlink_escape_are_unable_to_edit() {
    let fixture = Fixture::new().await;
    let (_keep, other) = outside();
    std::fs::write(other.join("target.txt"), "before").unwrap();
    std::os::unix::fs::symlink(&other, fixture.root.join("link")).unwrap();
    for path in [
        "missing.txt",
        "../escape.txt",
        "link/target.txt",
        "nested/deeper/x.txt",
    ] {
        let recorder = Arc::new(Recorder::default());
        let input = json!({"path": path, "oldString": "before", "newString": "after"});
        let result = exec(&fixture, recorder.clone(), input).await;
        assert_eq!(result.unwrap_err(), format!("Unable to edit {path}"));
    }
    assert_eq!(
        std::fs::read_to_string(other.join("target.txt")).unwrap(),
        "before"
    );
    assert!(fixture.disk.writes().is_empty());
}

#[tokio::test]
async fn invalid_input_uses_core_schema_messages() {
    let fixture = Fixture::new().await;
    let cases = [
        (json!(5), "Invalid tool input: Expected object, got 5"),
        (
            json!({"oldString": "x"}),
            "Invalid tool input: Missing key\n  at [\"path\"]",
        ),
        (
            json!({"path": "a", "oldString": 1, "newString": "y"}),
            "Invalid tool input: Expected string, got 1\n  at [\"oldString\"]",
        ),
        (
            json!({"path": "a", "oldString": "x", "newString": "y", "replaceAll": null}),
            "Invalid tool input: Expected boolean | undefined, got null\n  at [\"replaceAll\"]",
        ),
    ];
    for (input, message) in cases {
        let result = exec(&fixture, Arc::new(Recorder::default()), input).await;
        assert_eq!(result.unwrap_err(), message);
    }
}

#[tokio::test]
async fn model_output_preview_is_bounded() {
    let fixture = Fixture::new().await;
    let long = "x".repeat(250);
    let old = format!("{long}\n2\n3\n4\n5\n6\n7");
    fixture.put("big.txt", &old);
    let input = json!({"path": "big.txt", "oldString": old, "newString": "y"});
    let output = exec(&fixture, Arc::new(Recorder::default()), input)
        .await
        .unwrap();
    let expected = format!(
        "Edited file successfully: big.txt\nReplacements: 1\n```diff\n-{}...\n-2\n-3\n-4\n-5\n-6\n-...\n+y\n```",
        "x".repeat(240)
    );
    assert_eq!(text(&output), expected);
    assert_eq!(output.structured["files"][0]["deletions"], 7);
    assert_eq!(output.structured["files"][0]["additions"], 1);
}

#[tokio::test]
async fn schema_and_description_match_baseline() {
    let tool = CoreEditTool::new(Fixture::new().await.services.clone());
    let schema = tool.input_schema();
    let mut keys: Vec<_> = schema["properties"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    keys.sort();
    assert_eq!(keys, ["newString", "oldString", "path", "replaceAll"]);
    assert!(
        tool.description()
            .starts_with("Replace exact text in one file.")
    );
    assert!(tool.output_schema().is_some());
    assert_eq!(tool.permission(), Some("edit"));
}
