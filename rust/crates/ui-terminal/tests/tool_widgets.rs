//! Purpose: verify specialized tool bodies use real metadata shapes and lifecycle gates.
//! Scope: controlled public renderer fixtures.
//! Uses:
//! rust/crates/
//! `-- ui-terminal/src/timeline/tool.rs
//! External:
//! |-- ratatui::style::Color
//! `-- serde_json::json
//! Used by:
//! `-- none, entrypoint
//! Invariants: pending metadata is hidden; diagnostic warnings are not errors.
//! Contract tests: this file
use opencode_ui_terminal::timeline::tool::{Status, Tool, render};
use ratatui::style::Color;
use serde_json::{Value, json};
fn model(name: &str, input: Value, metadata: Value) -> Tool {
    Tool {
        name: name.into(),
        status: Status::Completed,
        title: None,
        input,
        structured: metadata,
        output: String::new(),
        error: None,
        expanded: false,
    }
}
fn visible(model: &Tool) -> String {
    render(model)
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}
#[test]
fn read_shows_only_valid_loaded_paths_after_completion() {
    let mut tool = model(
        "read",
        json!({"filePath":"a.rs"}),
        json!({"loaded":["AGENTS.md", 2]}),
    );
    assert!(visible(&tool).contains("↳ Loaded AGENTS.md"));
    tool.status = Status::Running;
    assert!(!visible(&tool).contains("Loaded"));
}
#[test]
fn edit_patch_and_diagnostics_preserve_hunks_and_error_coordinates() {
    let tool = model(
        "edit",
        json!({"filePath":"a.rs"}),
        json!({"diff":"--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-old\n+new", "diagnostics":{"a.rs":[{"severity":1,"range":{"start":{"line":2,"character":4}},"message":"bad type"},{"severity":2,"range":{"start":{"line":0,"character":0}},"message":"warning"}]}}),
    );
    let rows = render(&tool);
    assert!(visible(&tool).contains("@@ -1 +1 @@"));
    assert!(
        rows.iter()
            .any(|line| line.to_string() == "+new" && line.style.fg == Some(Color::Green))
    );
    assert!(visible(&tool).contains("Error [3:5] bad type"));
    assert!(!visible(&tool).contains("warning"));
}
#[test]
fn patch_files_keep_create_delete_move_labels() {
    let tool = model(
        "apply_patch",
        json!({}),
        json!({"files":[
            {"type":"delete","relativePath":"gone.rs","filePath":"gone.rs","patch":"","deletions":1},
            {"type":"add","relativePath":"new.rs","filePath":"new.rs","patch":"+hello","deletions":0},
            {"type":"move","relativePath":"new-name.rs","filePath":"old-name.rs","patch":" hello","deletions":0},
            {"type":"delete","relativePath":"malformed"}
        ]}),
    );
    let output = visible(&tool);
    for expected in [
        "# Deleted gone.rs",
        "-1 line",
        "# Created new.rs",
        "+hello",
        "# Moved old-name.rs → new-name.rs",
    ] {
        assert!(output.contains(expected));
    }
    assert!(!output.contains("malformed"));
}
#[test]
fn todos_questions_and_execute_retain_status_and_answers() {
    let todos = model(
        "todowrite",
        json!({"todos":[{"status":"completed","content":"done"},{"status":"in_progress","content":"working"}]}),
        json!({"todos":[{}]}),
    );
    assert!(visible(&todos).contains("[x] done"));
    assert!(visible(&todos).contains("[•] working"));
    let questions = model(
        "question",
        json!({"questions":[{"question":"Which?"},{"question":"Empty?"}]}),
        json!({"answers":[["one","two"],[]]}),
    );
    assert!(visible(&questions).contains("Which?\none, two\nEmpty?\n(no answer)"));
    let execute = model(
        "execute",
        json!({}),
        json!({"toolCalls":[{"tool":"read","status":"completed"},{"tool":"bash","status":"error"},{"tool":"hidden","status":"bogus"}]}),
    );
    assert!(visible(&execute).contains("↳ bash (failed)"));
    assert!(!visible(&execute).contains("hidden"));
}
#[test]
fn pending_specialized_metadata_stays_hidden() {
    let mut tool = model(
        "edit",
        json!({"filePath":"a.rs"}),
        json!({"diff":"-secret\n+secret"}),
    );
    tool.status = Status::Pending;
    assert!(!visible(&tool).contains("secret"));
}
