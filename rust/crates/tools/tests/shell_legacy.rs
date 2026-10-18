//! Purpose: legacy shell tool conformance with real processes (port of packages/opencode/test/tool/
//! shell.test.ts execution, permission, abort, timeout, streaming and truncation cases).
//! Scope: live shells in temp dirs; output retention writes into a temp tool-output directory.
//! Uses:
//! rust/crates/
//! |-- platform-native/src/process/mod.rs
//! |-- tools/src/shell/legacy.rs
//! |-- tools/src/shell/output.rs
//! `-- tools/tests/search_support.rs
//! External:
//! `-- serde_json::json
//! Used by:
//! `-- none, entrypoint
//! Invariants: abort/timeout preserve collected output; retained files hold the full stream.
//! Contract tests: this integration suite
#![cfg(unix)]

#[path = "search_support.rs"]
mod support;

use opencode_platform_native::process::NativeProcessRunner;
use opencode_tool_api::{NoProgress, Tool};
use opencode_tools::shell::output::DirectoryRetention;
use opencode_tools::shell::{LegacyShellConfig, LegacyShellTool, NoShellEnv};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use support::{Progress, Recorder, context, temp, write};

fn tool(dir: &Path, retention: &Path, timeout: u64) -> LegacyShellTool {
    LegacyShellTool::new(
        Arc::new(NativeProcessRunner),
        Arc::new(DirectoryRetention::new(retention)),
        Arc::new(NoShellEnv),
        LegacyShellConfig {
            shell: Some("/bin/sh".into()),
            default_timeout_ms: timeout,
            tmp: "/tmp/opencode".into(),
            home: dir.join("home"),
            directory: dir.to_path_buf(),
            worktree: dir.to_path_buf(),
        },
    )
}

fn output(value: &opencode_tool_api::ToolOutput) -> String {
    value.structured["output"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn basic_exit_codes_stderr_and_metadata() {
    let (_g, dir) = temp();
    let ctx = context(Arc::new(Recorder::default()), Arc::new(NoProgress));
    let shell = tool(&dir, &dir.join("out"), 120_000);
    assert_eq!(shell.shell(), "/bin/sh");
    let result = shell
        .execute(json!({"command": "echo test"}), &ctx)
        .await
        .unwrap();
    assert_eq!(
        result.structured["metadata"],
        json!({"output": "test\n", "exit": 0, "truncated": false})
    );
    assert_eq!(result.structured["title"], "echo test");
    assert_eq!(output(&result), "test\n");
    let both = shell
        .execute(
            json!({"command": "echo stdout_msg && echo stderr_msg >&2"}),
            &ctx,
        )
        .await
        .unwrap();
    assert!(output(&both).contains("stdout_msg") && output(&both).contains("stderr_msg"));
    let failed = shell
        .execute(json!({"command": "exit 42"}), &ctx)
        .await
        .unwrap();
    assert_eq!(failed.structured["metadata"]["exit"], 42);
    assert_eq!(output(&failed), "(no output)");
    std::fs::create_dir(dir.join("sub")).unwrap();
    let pwd = shell
        .execute(json!({"command": "pwd", "workdir": "sub"}), &ctx)
        .await
        .unwrap();
    assert_eq!(output(&pwd).trim(), dir.join("sub").to_string_lossy());
}

#[tokio::test]
async fn permission_requests_and_external_workdir() {
    let (_g, root) = temp();
    let dir = root.join("project");
    write(&dir.join("x"), "");
    write(&root.join("outer/outside.txt"), "x");
    let shell = tool(&dir, &root.join("out"), 120_000);
    let recorder = Arc::new(Recorder::default());
    let ctx = context(recorder.clone(), Arc::new(NoProgress));
    let file = root.join("outer/outside.txt");
    shell
        .execute(json!({"command": format!("cat {}", file.display())}), &ctx)
        .await
        .unwrap();
    let external = recorder.find("external_directory").unwrap();
    let glob = format!("{}/*", root.join("outer").display());
    assert_eq!(external.resources, std::slice::from_ref(&glob));
    assert_eq!(external.save, Some(vec![glob.clone()]));
    assert_eq!(
        Value::Object(external.metadata.unwrap()),
        json!({"command": format!("cat {}", file.display()), "directories": [root.join("outer").to_string_lossy()], "patterns": [glob]})
    );
    let bash = recorder.find("bash").unwrap();
    assert_eq!(bash.resources, [format!("cat {}", file.display())]);
    assert_eq!(bash.save, Some(vec!["cat *".to_string()]));
    let recorder = Arc::new(Recorder::default());
    let ctx = context(recorder.clone(), Arc::new(NoProgress));
    shell
        .execute(json!({"command": "cd ."}), &ctx)
        .await
        .unwrap();
    assert!(recorder.asked.lock().unwrap().is_empty());
    shell
        .execute(
            json!({"command": "pwd", "workdir": root.join("outer").to_string_lossy()}),
            &ctx,
        )
        .await
        .unwrap();
    assert_eq!(recorder.actions(), ["external_directory", "bash"]);
}

#[tokio::test]
async fn denial_prevents_spawn() {
    let (_g, dir) = temp();
    let shell = tool(&dir, &dir.join("out"), 120_000);
    let denied = Recorder::deny("bash");
    let ctx = context(denied.clone(), Arc::new(NoProgress));
    let marker = dir.join("marker");
    let error = shell
        .execute(
            json!({"command": format!("touch {}", marker.display())}),
            &ctx,
        )
        .await
        .unwrap_err();
    assert_eq!(error.message, "permission declined");
    assert!(!marker.exists());
}

#[tokio::test]
async fn abort_preserves_output_and_kills_process() {
    let (_g, dir) = temp();
    let progress = Arc::new(Progress::default());
    let ctx = context(Arc::new(Recorder::default()), progress.clone());
    let cancel = ctx.cancel.clone();
    let watcher = progress.clone();
    tokio::spawn(async move {
        loop {
            let seen = watcher
                .updates
                .lock()
                .unwrap()
                .iter()
                .any(|(_, m)| m["output"].as_str().unwrap_or("").contains("before"));
            if seen {
                cancel.cancel();
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });
    let shell = tool(&dir, &dir.join("out"), 120_000);
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        shell.execute(json!({"command": "echo before && sleep 30"}), &ctx),
    )
    .await
    .unwrap()
    .unwrap();
    let text = output(&result);
    assert!(text.contains("before"));
    assert!(text.ends_with("\n\n<shell_metadata>\nUser aborted the command\n</shell_metadata>"));
    assert_eq!(result.structured["metadata"]["exit"], Value::Null);
}

#[tokio::test]
async fn timeout_terminates_and_uses_default_timeout() {
    let (_g, dir) = temp();
    let ctx = context(Arc::new(Recorder::default()), Arc::new(NoProgress));
    let shell = tool(&dir, &dir.join("out"), 500);
    assert!(
        shell
            .description()
            .contains("commands will time out after 500ms")
    );
    let result = shell
        .execute(json!({"command": "sleep 60 & echo $! > pid; wait"}), &ctx)
        .await
        .unwrap();
    let text = output(&result);
    assert!(text.contains("shell tool terminated command after exceeding timeout 500 ms"));
    assert!(text.contains("retry with a larger timeout value in milliseconds"));
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
    assert!(!alive, "descendant survived timeout");
    let explicit = tool(&dir, &dir.join("out"), 120_000)
        .execute(json!({"command": "sleep 60", "timeout": 300}), &ctx)
        .await
        .unwrap();
    assert!(output(&explicit).contains("exceeding timeout 300 ms"));
}

#[tokio::test]
async fn streams_metadata_progressively() {
    let (_g, dir) = temp();
    let progress = Arc::new(Progress::default());
    let ctx = context(Arc::new(Recorder::default()), progress.clone());
    let result = tool(&dir, &dir.join("out"), 120_000)
        .execute(
            json!({"command": "echo first && sleep 0.1 && echo second"}),
            &ctx,
        )
        .await
        .unwrap();
    assert!(output(&result).contains("first") && output(&result).contains("second"));
    let updates: Vec<String> = progress
        .updates
        .lock()
        .unwrap()
        .iter()
        .map(|(_, m)| m["output"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(updates.first().map(String::as_str), Some(""));
    assert!(updates.iter().filter(|u| !u.is_empty()).count() > 1);
}

#[tokio::test]
async fn truncation_by_lines_and_bytes_with_full_output_saved() {
    let (_g, dir) = temp();
    let retention = dir.join("tool-output");
    let ctx = context(Arc::new(Recorder::default()), Arc::new(NoProgress));
    let shell = tool(&dir, &retention, 120_000);
    let lines = shell
        .execute(json!({"command": "seq 1 2100"}), &ctx)
        .await
        .unwrap();
    assert_eq!(lines.structured["metadata"]["truncated"], true);
    let text = output(&lines);
    assert!(text.starts_with("...output truncated...\n\nFull output saved to: "));
    let saved = lines.structured["metadata"]["outputPath"].as_str().unwrap();
    assert!(Path::new(saved).starts_with(&retention));
    assert!(
        Path::new(saved)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("tool_")
    );
    let content = std::fs::read_to_string(saved).unwrap();
    let all: Vec<&str> = content.trim().split('\n').collect();
    assert_eq!(all.len(), 2100);
    assert_eq!((all[0], all[2099]), ("1", "2100"));
    assert!(text.ends_with("2100\n"));
    let bytes = shell
        .execute(
            json!({"command": format!("head -c {} /dev/zero | tr '\\0' a", 50 * 1024 + 10000)}),
            &ctx,
        )
        .await
        .unwrap();
    assert_eq!(bytes.structured["metadata"]["truncated"], true);
    let saved = bytes.structured["metadata"]["outputPath"].as_str().unwrap();
    assert_eq!(std::fs::metadata(saved).unwrap().len(), 50 * 1024 + 10000);
    let small = shell
        .execute(json!({"command": "echo 1"}), &ctx)
        .await
        .unwrap();
    assert_eq!(small.structured["metadata"]["truncated"], false);
    assert!(small.structured["metadata"].get("outputPath").is_none());
}

#[tokio::test]
async fn invalid_arguments_use_legacy_message() {
    let (_g, dir) = temp();
    let ctx = context(Arc::new(Recorder::default()), Arc::new(NoProgress));
    let error = tool(&dir, &dir.join("out"), 1000)
        .execute(json!({"command": "x", "timeout": -1}), &ctx)
        .await
        .unwrap_err();
    assert_eq!(
        error.message,
        "The bash tool was called with invalid arguments: SchemaError(Expected a value greater than 0, got -1\n  at [\"timeout\"]).\nPlease rewrite the input so it satisfies the expected schema."
    );
}
