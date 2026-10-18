//! Purpose: Legacy (V1) edit line-ending and concurrency conformance over real temp files, ported
//! from the "line endings" and "concurrent editing" blocks of packages/opencode/test/tool/edit.test.ts.
//! Scope: `LegacyEditTool::execute` through the public tool boundary.
//! Uses:
//! rust/crates/
//! |-- tools/src/files/edit_legacy.rs
//! `-- tools/tests/files_mutation/support.rs
//! External:
//! `-- serde_json::json
//! Used by:
//! rust/crates/
//! `-- tools/tests/files_edit.rs
//! Invariants: output line endings follow the file, concurrent edits of one file serialize.
//! Contract tests: this suite

use super::support::{Fixture, Hooks, Recorder, context, s};
use opencode_tool_api::{Tool, ToolOutput};
use opencode_tools::files::edit_legacy::LegacyEditTool;
use serde_json::{Value, json};
use std::sync::Arc;

fn tool(fixture: &Fixture) -> LegacyEditTool {
    LegacyEditTool::new(fixture.services.clone(), Arc::new(Hooks::default()))
}

async fn run(fixture: &Fixture, input: Value) -> Result<ToolOutput, String> {
    tool(fixture)
        .execute(input, &context(Arc::default()))
        .await
        .map_err(|failure| failure.message)
}

fn edit(path: &str, old: &str, new: &str) -> Value {
    json!({"filePath": path, "oldString": old, "newString": new})
}

fn normalize(text: &str, crlf: bool) -> String {
    let lf = text.replace("\r\n", "\n");
    if crlf { lf.replace('\n', "\r\n") } else { lf }
}

#[tokio::test]
async fn line_endings_follow_the_file() {
    let fixture = Fixture::new().await;
    let (old, next, alt) = (
        "alpha\nbeta\ngamma",
        "alpha\nbeta-updated\ngamma",
        "alpha\nbeta\nomega",
    );
    let cases: Vec<(bool, String, String, String)> = [false, true]
        .into_iter()
        .flat_map(|crlf| {
            [
                (normalize(old, crlf), normalize(next, crlf), next),
                (normalize(old, !crlf), normalize(next, !crlf), next),
                (normalize(old, crlf), normalize(next, !crlf), next),
                (
                    if crlf {
                        "alpha\r\nbeta\ngamma"
                    } else {
                        "alpha\nbeta\r\ngamma"
                    }
                    .to_string(),
                    if crlf {
                        "alpha\nbeta\r\nomega"
                    } else {
                        "alpha\r\nbeta\nomega"
                    }
                    .to_string(),
                    alt,
                ),
            ]
            .into_iter()
            .map(move |(o, n, expected)| (crlf, o, n, normalize(&format!("{expected}\n"), crlf)))
        })
        .collect();
    for (crlf, old_string, new_string, expected) in cases {
        let file = s(&fixture.put("test.txt", normalize(&format!("{old}\n"), crlf)));
        run(&fixture, edit(&file, &old_string, &new_string))
            .await
            .unwrap();
        assert_eq!(
            fixture.load("test.txt"),
            expected,
            "crlf={crlf} old={old_string:?}"
        );
    }
    for crlf in [false, true] {
        let block = normalize("alpha\nbeta\nalpha\nbeta\n", crlf);
        let file = s(&fixture.put("test.txt", &block));
        let mut input = edit(
            &file,
            &normalize("alpha\nbeta", crlf),
            &normalize("alpha\nbeta-updated", crlf),
        );
        input["replaceAll"] = json!(true);
        run(&fixture, input).await.unwrap();
        assert_eq!(
            fixture.load("test.txt"),
            normalize("alpha\nbeta-updated\nalpha\nbeta-updated\n", crlf)
        );
    }
}

#[tokio::test]
async fn concurrent_edits_to_one_file_serialize() {
    let fixture = Fixture::new().await;
    let file = s(&fixture.put("file.txt", "top = 0\nmiddle = keep\nbottom = 0\n"));
    let recorder = Arc::new(Recorder::default());
    *recorder.delay.lock().unwrap() = Some(std::time::Duration::from_millis(50));
    let ctx = context(recorder.clone());
    let first_tool = tool(&fixture);
    let second_tool = tool(&fixture);
    let (first, second) = tokio::join!(
        first_tool.execute(edit(&file, "top = 0", "top = 1"), &ctx),
        async {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            second_tool
                .execute(edit(&file, "bottom = 0", "bottom = 2"), &ctx)
                .await
        }
    );
    first.unwrap();
    second.unwrap();
    assert_eq!(
        fixture.load("file.txt"),
        "top = 1\nmiddle = keep\nbottom = 2\n"
    );
}
