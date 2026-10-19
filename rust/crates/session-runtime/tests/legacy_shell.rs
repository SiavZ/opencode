//! Purpose: tests for the legacy shell path (`SessionPrompt.shell`) with the real native process
//! runner: durable transcript, streamed-to-completion output, cancel with abort marker and
//! `lastAssistant` result, busy rejection, process failure, and `Shell.args` per shell family.
//! Scope: test-only; spawns `/bin/sh` in a temp directory (no network, no user data).
//! Uses:
//! rust/crates/
//! |-- session-runtime/src/legacy/shell.rs
//! |-- session-runtime/src/legacy/types.rs
//! |-- session-runtime/tests/support/legacy.rs
//! `-- session-runtime/tests/support/sql.rs
//! External:
//! |-- serde_json::json
//! |-- tokio::spawn
//! |-- tokio::test
//! |-- tokio::time::sleep
//! `-- tokio::time::timeout
//! Used by:
//! `-- none, entrypoint
//! Invariants: live local process tests (unix only); fixtures for agents/provider/loop.
//! Contract tests: session-runtime/tests/legacy_shell.rs
#![cfg(unix)]

#[path = "support/sql.rs"]
mod sql;
#[path = "support/legacy.rs"]
mod support;

use std::time::Duration;

use opencode_session_runtime::legacy::shell::shell_args;
use opencode_session_runtime::legacy::types::{LegacyError, ShellInput};
use serde_json::json;
use support::{SESSION, setup};

fn shell(command: &str) -> ShellInput {
    ShellInput {
        session_id: SESSION.into(),
        agent: "build".into(),
        command: command.into(),
        ..ShellInput::default()
    }
}

#[tokio::test]
async fn shell_writes_transcript_and_completes_tool_part() {
    let ctx = setup().await;
    let result = ctx
        .legacy
        .shell(shell("echo hi; echo err 1>&2"))
        .await
        .unwrap();
    assert_eq!(result.info["role"], "assistant");
    assert!(result.info["time"]["completed"].is_number());
    let part = &result.parts[0];
    assert_eq!(part["tool"], "bash");
    assert_eq!(part["state"]["status"], "completed");
    assert_eq!(
        part["state"]["input"],
        json!({ "command": "echo hi; echo err 1>&2" })
    );
    let output = part["state"]["output"].as_str().unwrap();
    assert!(
        output.contains("hi\n") && output.contains("err\n"),
        "{output}"
    );
    assert_eq!(part["state"]["metadata"]["output"], part["state"]["output"]);
    assert_eq!(part["state"]["title"], "");

    let messages = ctx.messages().await;
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].info["role"], "user");
    assert_eq!(
        messages[0].parts[0]["text"],
        "The following tool was executed by the user"
    );
    assert_eq!(messages[0].parts[0]["synthetic"], true);
    assert_eq!(messages[1].info["parentID"], messages[0].info["id"]);
    assert_eq!(messages[1].parts[0]["state"]["status"], "completed");
    assert!(!ctx.legacy.run_state().busy(SESSION));
}

#[tokio::test]
async fn shell_runs_in_instance_directory_with_dumb_term() {
    let ctx = setup().await;
    let result = ctx.legacy.shell(shell("pwd; echo $TERM")).await.unwrap();
    let output = result.parts[0]["state"]["output"]
        .as_str()
        .unwrap()
        .to_owned();
    let root = std::fs::canonicalize(ctx.worktree.path()).unwrap();
    assert!(output.contains(&*root.to_string_lossy()), "{output}");
    assert!(output.ends_with("dumb\n"), "{output}");
}

#[tokio::test]
async fn cancel_marks_abort_and_returns_last_assistant() {
    let ctx = setup().await;
    let run = tokio::spawn({
        let legacy = ctx.legacy.clone();
        async move { legacy.shell(shell("echo start; sleep 30")).await }
    });
    while !ctx.legacy.run_state().busy(SESSION) {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    // A second shell while busy is rejected.
    assert_eq!(
        ctx.legacy.shell(shell("true")).await.unwrap_err(),
        LegacyError::Busy(SESSION.into())
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    let started = std::time::Instant::now();
    ctx.legacy.cancel(SESSION).await;
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "cancel kills the process"
    );
    let result = tokio::time::timeout(Duration::from_secs(10), run)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(result.info["role"], "assistant");
    let part = &ctx.messages().await[1].parts[0];
    assert_eq!(part["state"]["status"], "completed");
    let output = part["state"]["output"].as_str().unwrap();
    assert!(
        output.ends_with("\n\n<metadata>\nUser aborted the command\n</metadata>"),
        "{output}"
    );
    assert!(!ctx.legacy.run_state().busy(SESSION));
}

#[tokio::test]
async fn shell_with_unknown_agent_fails_and_releases_session() {
    let ctx = setup().await;
    let mut input = shell("true");
    input.agent = "ghost".into();
    let error = ctx.legacy.shell(input).await.unwrap_err();
    assert!(
        matches!(error, LegacyError::Unknown(message) if message.starts_with("Agent not found: \"ghost\""))
    );
    assert!(!ctx.legacy.run_state().busy(SESSION));
    assert!(ctx.messages().await.is_empty());
}

#[test]
fn shell_args_match_baseline_families() {
    assert_eq!(shell_args("/usr/bin/fish", "ls", "/w"), vec!["-c", "ls"]);
    assert_eq!(shell_args("/bin/sh", "ls", "/w"), vec!["-c", "ls"]);
    assert_eq!(
        shell_args("pwsh", "ls", "/w"),
        vec!["-NoProfile", "-Command", "ls"]
    );
    let bash = shell_args("/bin/bash", "echo \"x\"", "/w");
    assert_eq!(bash[0], "-l");
    assert_eq!(
        bash[2],
        "\n        shopt -s expand_aliases\n        [[ -f ~/.bashrc ]] && source ~/.bashrc >/dev/null 2>&1 || true\n        cd -- \"$1\"\n        eval \"echo \\\"x\\\"\"\n      "
    );
    assert_eq!(&bash[3..], &["opencode", "/w"]);
    assert!(shell_args("/bin/zsh", "ls", "/w")[2].contains("ZDOTDIR"));
}
