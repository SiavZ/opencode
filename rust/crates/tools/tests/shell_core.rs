//! Purpose: core V2 bash tool conformance with real processes (port of packages/core/test/
//! tool-bash.test.ts): output shape, workdir resolution, approval ordering and denial, advisory
//! warnings, non-zero exit, capture truncation, timeout with descendant kill, cancellation.
//! Scope: live /bin/sh in temp dirs, native Location resolution, fixture authorizer.
//! Uses:
//! rust/crates/
//! |-- platform-native/src/fs/location.rs
//! |-- platform-native/src/process/mod.rs
//! |-- tools/src/shell/core.rs
//! `-- tools/tests/search_support.rs
//! External:
//! `-- serde_json::json
//! Used by:
//! `-- none, entrypoint
//! Invariants: no process spawns after a denial; timeouts leave no descendant alive.
//! Contract tests: this integration suite
#![cfg(unix)]

#[path = "search_support.rs"]
mod support;

use opencode_platform_native::fs::NativeLocationMutation;
use opencode_platform_native::process::NativeProcessRunner;
use opencode_tool_api::{NoProgress, Tool, ToolContent};
use opencode_tools::shell::CoreBashTool;
use opencode_tools::shell::core::{MAX_CAPTURE_BYTES, description};
use serde_json::json;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use support::{Recorder, context, temp};

async fn tool(dir: &Path) -> CoreBashTool {
    let location = NativeLocationMutation::new(dir).await.unwrap();
    CoreBashTool::new(Arc::new(NativeProcessRunner), Arc::new(location), None)
}

fn texts(output: &opencode_tool_api::ToolOutput) -> Vec<String> {
    output
        .content
        .iter()
        .map(|item| match item {
            ToolContent::Text { text } => text.clone(),
            _ => String::new(),
        })
        .collect()
}

#[test]
fn description_matches_baseline_template() {
    let source = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(3)
            .unwrap()
            .join("packages/core/src/tool/bash.ts"),
    )
    .unwrap();
    let template = description()
        .replace("120000", "${DEFAULT_TIMEOUT_MS}")
        .replace("600000", "${MAX_TIMEOUT_MS}");
    assert!(source.contains(&format!("`{template}`")));
}

#[tokio::test]
async fn real_command_output_and_approval() {
    let (_g, dir) = temp();
    let recorder = Arc::new(Recorder::default());
    let ctx = context(recorder.clone(), Arc::new(NoProgress));
    let out = tool(&dir)
        .await
        .execute(json!({"command": "printf core-bash"}), &ctx)
        .await
        .unwrap();
    assert_eq!(texts(&out), ["core-bash", "Command exited with code 0."]);
    assert_eq!(out.structured, json!({"exit": 0, "truncated": false}));
    let asked = recorder.find("bash").unwrap();
    assert_eq!(asked.resources, ["printf core-bash"]);
    assert_eq!(asked.save, Some(vec!["printf core-bash".into()]));
    assert_eq!(recorder.actions(), ["bash"]);
    let empty = tool(&dir)
        .await
        .execute(json!({"command": "true"}), &ctx)
        .await
        .unwrap();
    assert_eq!(texts(&empty)[0], "(no output)");
}

#[tokio::test]
async fn relative_and_external_workdir() {
    let (_g, dir) = temp();
    let (_o, outside) = temp();
    std::fs::create_dir(dir.join("src")).unwrap();
    let recorder = Arc::new(Recorder::default());
    let ctx = context(recorder.clone(), Arc::new(NoProgress));
    let out = tool(&dir)
        .await
        .execute(json!({"command": "pwd", "workdir": "src"}), &ctx)
        .await
        .unwrap();
    assert_eq!(texts(&out)[0].trim(), dir.join("src").to_string_lossy());
    let out = tool(&dir)
        .await
        .execute(
            json!({"command": "pwd", "workdir": outside.to_string_lossy()}),
            &ctx,
        )
        .await
        .unwrap();
    assert_eq!(texts(&out)[0].trim(), outside.to_string_lossy());
    assert_eq!(recorder.actions(), ["bash", "external_directory", "bash"]);
    assert_eq!(
        recorder.find("external_directory").unwrap().resources,
        [format!("{}/*", outside.display())]
    );
}

#[tokio::test]
async fn denial_prevents_execution() {
    let (_g, dir) = temp();
    let (_o, outside) = temp();
    let marker = dir.join("ran");
    let command = format!("touch {}", marker.display());
    let denied = Recorder::deny("external_directory");
    let ctx = context(denied.clone(), Arc::new(NoProgress));
    let error = tool(&dir)
        .await
        .execute(
            json!({"command": command, "workdir": outside.to_string_lossy()}),
            &ctx,
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.message,
        format!("Unable to execute command: {command}")
    );
    assert_eq!(denied.actions(), ["external_directory"]);
    let denied = Recorder::deny("bash");
    let ctx = context(denied.clone(), Arc::new(NoProgress));
    tool(&dir)
        .await
        .execute(json!({"command": command}), &ctx)
        .await
        .unwrap_err();
    assert_eq!(denied.actions(), ["bash"]);
    assert!(!marker.exists());
}

#[tokio::test]
async fn workdir_that_is_a_file_is_rejected_after_approval() {
    let (_g, dir) = temp();
    std::fs::write(dir.join("file"), "x").unwrap();
    let recorder = Arc::new(Recorder::default());
    let ctx = context(recorder.clone(), Arc::new(NoProgress));
    let error = tool(&dir)
        .await
        .execute(json!({"command": "pwd", "workdir": "file"}), &ctx)
        .await
        .unwrap_err();
    assert_eq!(error.message, "Unable to execute command: pwd");
    assert_eq!(recorder.actions(), ["bash"]);
}

#[tokio::test]
async fn advisory_warnings_do_not_request_approval() {
    let (_g, dir) = temp();
    let (_o, outside) = temp();
    std::fs::write(outside.join("secret.txt"), "s").unwrap();
    let recorder = Recorder::deny("external_directory");
    let ctx = context(recorder.clone(), Arc::new(NoProgress));
    let out = tool(&dir)
        .await
        .execute(
            json!({"command": format!("cat {}", outside.join("secret.txt").display())}),
            &ctx,
        )
        .await
        .unwrap();
    assert_eq!(recorder.actions(), ["bash"]);
    assert_eq!(out.structured, json!({"exit": 0, "truncated": false}));
    assert_eq!(
        texts(&out)[1],
        format!(
            "Warnings:\n- Command argument references external directory {}/*. Bash runs with host-user filesystem, process, and network authority; this scan is advisory only.\n\nCommand exited with code 0.",
            outside.display()
        )
    );
}

#[tokio::test]
async fn non_zero_exit_stderr_and_capture_truncation() {
    let (_g, dir) = temp();
    let ctx = context(Arc::new(Recorder::default()), Arc::new(NoProgress));
    let out = tool(&dir)
        .await
        .execute(json!({"command": "echo HEAD; echo TAIL >&2; exit 7"}), &ctx)
        .await
        .unwrap();
    assert_eq!(out.structured, json!({"exit": 7, "truncated": false}));
    assert!(texts(&out)[0].contains("HEAD") && texts(&out)[0].contains("TAIL"));
    assert_eq!(texts(&out)[1], "Command exited with code 7.");
    let big = format!("head -c {} /dev/zero | tr '\\0' a", MAX_CAPTURE_BYTES + 10);
    let out = tool(&dir)
        .await
        .execute(json!({"command": big}), &ctx)
        .await
        .unwrap();
    assert_eq!(out.structured, json!({"exit": 0, "truncated": true}));
    assert!(
        texts(&out)[0].ends_with("a\n\n[output capture truncated at the in-memory safety limit]")
    );
}

#[tokio::test]
async fn timeout_settles_and_kills_descendants() {
    let (_g, dir) = temp();
    let ctx = context(Arc::new(Recorder::default()), Arc::new(NoProgress));
    let started = Instant::now();
    let script = "sleep 60 & echo $! > pid; wait";
    let out = tool(&dir)
        .await
        .execute(json!({"command": script, "timeout": 300}), &ctx)
        .await
        .unwrap();
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(out.structured, json!({"truncated": false, "timeout": true}));
    assert_eq!(
        texts(&out),
        [
            "Command exceeded timeout of 300 ms. Retry with a larger timeout if the command is expected to take longer.",
            "Command timed out before completion."
        ]
    );
    let pid = std::fs::read_to_string(dir.join("pid"))
        .unwrap()
        .trim()
        .to_string();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let alive = std::process::Command::new("kill")
        .args(["-0", &pid])
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
        .success();
    assert!(!alive, "background child {pid} survived timeout");
}

#[tokio::test]
async fn cancellation_and_input_validation() {
    let (_g, dir) = temp();
    let ctx = context(Arc::new(Recorder::default()), Arc::new(NoProgress));
    let cancel = ctx.cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        cancel.cancel();
    });
    let error = tool(&dir)
        .await
        .execute(json!({"command": "sleep 30"}), &ctx)
        .await
        .unwrap_err();
    assert_eq!(error.message, "Unable to execute command: sleep 30");
    let fresh = context(Arc::new(Recorder::default()), Arc::new(NoProgress));
    let invalid = tool(&dir)
        .await
        .execute(json!({"command": "x", "timeout": 600001}), &fresh)
        .await
        .unwrap_err();
    assert_eq!(
        invalid.message,
        "Invalid tool input: Expected a value less than or equal to 600000, got 600001\n  at [\"timeout\"]"
    );
}
