//! Purpose: core and legacy glob/grep tool conformance (ports packages/opencode/test/tool/{glob,grep}
//! .test.ts and the core leaf contracts), descriptions/schemas against the baseline, and the
//! test-backed containment finding for core glob/grep.
//! Scope: live ripgrep in temp dirs with a fixture authorizer.
//! Uses:
//! rust/crates/
//! |-- tool-api/src/lib.rs
//! |-- tools/src/search/core.rs
//! |-- tools/src/search/legacy.rs
//! |-- tools/src/search/mod.rs
//! `-- tools/tests/search_support.rs
//! External:
//! |-- legacy::GLOB_DESCRIPTION
//! |-- legacy::GREP_DESCRIPTION
//! |-- serde_json::Value
//! |-- serde_json::json
//! `-- tokio::test
//! Used by:
//! `-- none, entrypoint
//! Invariants: permission is asked before any search; legacy external paths ask external_directory.
//! Contract tests: this integration suite

#[path = "search_support.rs"]
mod support;

use opencode_tool_api::{NoProgress, Tool};
use opencode_tools::search::{
    CoreGlobTool, CoreGrepTool, LegacyGlobTool, LegacyGrepTool, core, legacy, schemas,
};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
use support::{Recorder, context, services, temp, text, write};

fn repository() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .unwrap()
}

#[test]
fn descriptions_and_schemas_match_baseline() {
    for name in ["glob", "grep"] {
        let source =
            std::fs::read(repository().join(format!("packages/opencode/src/tool/{name}.txt")))
                .unwrap();
        let ours = if name == "glob" {
            legacy::GLOB_DESCRIPTION
        } else {
            legacy::GREP_DESCRIPTION
        };
        assert_eq!(ours.as_bytes(), source, "{name}");
        let core_source =
            std::fs::read_to_string(repository().join(format!("packages/core/src/tool/{name}.ts")))
                .unwrap();
        let literal = if name == "glob" {
            core::GLOB_DESCRIPTION
        } else {
            core::GREP_DESCRIPTION
        };
        assert!(
            core_source.contains(&format!("\"{literal}\"")),
            "{name} core description drifted"
        );
        assert_eq!(schemas::core(name, "input")["required"], json!(["pattern"]));
        assert_eq!(
            schemas::legacy(name)["$schema"],
            "https://json-schema.org/draft/2020-12/schema"
        );
    }
    assert_eq!(
        schemas::core("grep", "output")["items"]["$ref"],
        "#/$defs/FileSystem.Match"
    );
    assert!(schemas::legacy("grep")["properties"]["include"].is_object());
}

#[tokio::test]
async fn core_glob_relative_output_and_limit() {
    let (_g, dir) = temp();
    write(&dir.join("src/a.ts"), "a");
    write(&dir.join("src/b.ts"), "b");
    write(&dir.join("c.txt"), "c");
    let recorder = Arc::new(Recorder::default());
    let ctx = context(recorder.clone(), Arc::new(NoProgress));
    let tool = CoreGlobTool::new(services(&dir, &dir));
    let out = tool
        .execute(json!({"pattern": "*.ts", "path": "src"}), &ctx)
        .await
        .unwrap();
    let mut entries: Vec<String> = out
        .structured
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["path"].as_str().unwrap().to_string())
        .collect();
    entries.sort();
    assert_eq!(entries, ["src/a.ts", "src/b.ts"]);
    assert!(text(&out).contains(&dir.join("src/a.ts").to_string_lossy().to_string()));
    let asked = recorder.find("glob").unwrap();
    assert_eq!(asked.resources, ["*.ts"]);
    assert_eq!(asked.save, Some(vec!["*".into()]));
    assert_eq!(
        asked.metadata.unwrap(),
        json!({"root": "src", "path": "src"})
            .as_object()
            .unwrap()
            .clone()
    );
    let limited = tool
        .execute(json!({"pattern": "**/*", "limit": 1}), &ctx)
        .await
        .unwrap();
    assert_eq!(limited.structured.as_array().unwrap().len(), 1);
    let none = tool
        .execute(json!({"pattern": "*.none"}), &ctx)
        .await
        .unwrap();
    assert_eq!(text(&none), "No files found");
    let invalid = tool
        .execute(json!({"pattern": "*", "limit": 0}), &ctx)
        .await
        .unwrap_err();
    assert_eq!(
        invalid.message,
        "Invalid tool input: Expected a value greater than 0, got 0\n  at [\"limit\"]"
    );
}

#[tokio::test]
async fn core_grep_output_text_file_path_and_denial() {
    let (_g, dir) = temp();
    write(&dir.join("a.txt"), "alpha\nneedle one\n");
    write(&dir.join("sub/b.txt"), "needle two\n");
    let recorder = Arc::new(Recorder::default());
    let ctx = context(recorder.clone(), Arc::new(NoProgress));
    let tool = CoreGrepTool::new(services(&dir, &dir));
    let out = tool
        .execute(json!({"pattern": "needle", "path": "sub/b.txt"}), &ctx)
        .await
        .unwrap();
    assert_eq!(out.structured[0]["entry"]["path"], "sub/b.txt");
    assert_eq!(
        text(&out),
        format!(
            "Found 1 matches\n{}:\n  Line 1: needle two\n",
            dir.join("sub/b.txt").display()
        )
    );
    assert_eq!(
        recorder.find("grep").unwrap().metadata.unwrap()["root"],
        "."
    );
    let denied = context(Recorder::deny("grep"), Arc::new(NoProgress));
    let error = tool
        .execute(json!({"pattern": "needle"}), &denied)
        .await
        .unwrap_err();
    assert_eq!(error.message, "Unable to grep for needle");
    let invalid = tool
        .execute(json!({"pattern": "("}), &ctx)
        .await
        .unwrap_err();
    assert_eq!(invalid.message, "Unable to grep for (");
}

/// FINDING (preserved, not fixed): core glob/grep authorize only the pattern, then resolve the
/// unvalidated `path` against the Location without containment. A `../` path searches outside the
/// Location with no external_directory request. Baseline: packages/core/src/tool/glob.ts:62-78,
/// grep.ts:77-97.
#[tokio::test]
async fn finding_core_search_escapes_location_without_external_approval() {
    let (_g, root) = temp();
    let location = root.join("project");
    write(&location.join("inside.txt"), "x");
    write(&root.join("secret/key.txt"), "TOKEN=abc\n");
    let recorder = Arc::new(Recorder::default());
    let ctx = context(recorder.clone(), Arc::new(NoProgress));
    let services = services(&location, &location);
    let globbed = CoreGlobTool::new(services.clone())
        .execute(json!({"pattern": "*.txt", "path": "../secret"}), &ctx)
        .await
        .unwrap();
    assert_eq!(globbed.structured[0]["path"], "../secret/key.txt");
    let grepped = CoreGrepTool::new(services)
        .execute(
            json!({"pattern": "TOKEN", "path": root.join("secret").to_string_lossy()}),
            &ctx,
        )
        .await
        .unwrap();
    assert_eq!(grepped.structured[0]["text"], "TOKEN=abc\n");
    assert_eq!(
        recorder.actions(),
        ["glob", "grep"],
        "no external_directory request was made"
    );
}

#[tokio::test]
async fn legacy_glob_matches_rejects_files_and_truncates() {
    let (_g, dir) = temp();
    write(&dir.join("a.ts"), "export const a = 1\n");
    write(&dir.join("b.txt"), "hello\n");
    let recorder = Arc::new(Recorder::default());
    let ctx = context(recorder.clone(), Arc::new(NoProgress));
    let tool = LegacyGlobTool::new(services(&dir, &dir));
    let out = tool
        .execute(
            json!({"pattern": "*.ts", "path": dir.to_string_lossy()}),
            &ctx,
        )
        .await
        .unwrap();
    assert_eq!(
        out.structured["metadata"],
        json!({"count": 1, "truncated": false})
    );
    assert_eq!(
        out.structured["output"],
        Value::from(dir.join("a.ts").to_string_lossy().to_string())
    );
    let error = tool
        .execute(
            json!({"pattern": "*.ts", "path": dir.join("a.ts").to_string_lossy()}),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(error.message.contains("glob path must be a directory"));
    for index in 0..120 {
        write(&dir.join(format!("many/f{index}.md")), "");
    }
    let many = tool
        .execute(json!({"pattern": "*.md", "path": "many"}), &ctx)
        .await
        .unwrap();
    assert_eq!(
        many.structured["metadata"],
        json!({"count": 100, "truncated": true})
    );
    assert!(many.structured["output"].as_str().unwrap().ends_with("\n\n(Results are truncated: showing first 100 results. Consider using a more specific path or pattern.)"));
    assert_eq!(many.structured["title"], "many");
    assert!(
        !recorder
            .actions()
            .contains(&"external_directory".to_string())
    );
}

#[tokio::test]
async fn legacy_grep_no_match_exact_file_truncation_and_external() {
    let (_g, root) = temp();
    let dir = root.join("project");
    write(&dir.join("test.txt"), "line1\nline2\nline3");
    let recorder = Arc::new(Recorder::default());
    let ctx = context(recorder.clone(), Arc::new(NoProgress));
    let tool = LegacyGrepTool::new(services(&dir, &dir));
    let none = tool
        .execute(
            json!({"pattern": "xyznonexistentpatternxyz123", "path": dir.to_string_lossy()}),
            &ctx,
        )
        .await
        .unwrap();
    assert_eq!(
        none.structured,
        json!({"title": "xyznonexistentpatternxyz123", "metadata": {"matches": 0, "truncated": false}, "output": "No files found"})
    );
    let exact = tool
        .execute(
            json!({"pattern": "line2", "path": dir.join("test.txt").to_string_lossy()}),
            &ctx,
        )
        .await
        .unwrap();
    assert_eq!(exact.structured["metadata"]["matches"], 1);
    let output = exact.structured["output"].as_str().unwrap();
    assert!(output.contains(&dir.join("test.txt").to_string_lossy().to_string()));
    assert!(output.contains("Line 2: line2"));
    for index in 0..101 {
        write(&dir.join(format!("m/match-{index}.txt")), "needle");
    }
    let many = tool
        .execute(
            json!({"pattern": "needle", "path": "m", "include": "*.txt"}),
            &ctx,
        )
        .await
        .unwrap();
    let output = many.structured["output"].as_str().unwrap();
    assert!(output.starts_with("Found 100 matches (more matches available)\n"));
    assert!(
        output
            .ends_with("\n\n(Results truncated. Consider using a more specific path or pattern.)")
    );
    write(&root.join("outside/o.txt"), "needle");
    tool.execute(
        json!({"pattern": "needle", "path": root.join("outside").to_string_lossy()}),
        &ctx,
    )
    .await
    .unwrap();
    let external = recorder.find("external_directory").unwrap();
    assert_eq!(
        external.resources,
        [format!("{}/*", root.join("outside").display())]
    );
    let empty = tool
        .execute(json!({"pattern": ""}), &ctx)
        .await
        .unwrap_err();
    assert_eq!(empty.message, "pattern is required");
    let invalid = tool.execute(json!({}), &ctx).await.unwrap_err();
    assert!(
        invalid.message.starts_with(
            "The grep tool was called with invalid arguments: SchemaError(Missing key"
        )
    );
}

#[cfg(unix)]
#[tokio::test]
async fn legacy_grep_reports_alias_paths_not_real_paths() {
    let (_g, root) = temp();
    let dir = root.join("project");
    std::fs::create_dir_all(&dir).unwrap();
    write(&root.join("real/test.txt"), "needle");
    std::os::unix::fs::symlink(root.join("real"), root.join("alias")).unwrap();
    let recorder = Arc::new(Recorder::default());
    let ctx = context(recorder.clone(), Arc::new(NoProgress));
    let tool = LegacyGrepTool::new(services(&dir, &dir));
    let out = tool.execute(json!({"pattern": "needle", "path": root.join("alias").to_string_lossy(), "include": "*.txt"}), &ctx).await.unwrap();
    let output = out.structured["output"].as_str().unwrap();
    assert!(output.contains(&root.join("alias/test.txt").to_string_lossy().to_string()));
    assert!(!output.contains(&root.join("real/test.txt").to_string_lossy().to_string()));
    assert_eq!(
        recorder.find("external_directory").unwrap().resources,
        [format!("{}/*", root.join("alias").display())]
    );
}
