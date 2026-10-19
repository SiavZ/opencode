//! Purpose: tests for legacy command templating (`SessionPrompt.command`, `Command.hints`,
//! `ConfigMarkdown.files/shell`) and the pure template helpers, plus `child_reply` mapping for the
//! ChildHost seam.
//! Scope: test-only; command shell substitution spawns `/bin/sh` in a temp directory.
//! Uses:
//! rust/crates/
//! |-- session-runtime/src/legacy/ports.rs
//! |-- session-runtime/src/legacy/prompt.rs
//! |-- session-runtime/src/legacy/template.rs
//! |-- session-runtime/src/legacy/types.rs
//! |-- session-runtime/tests/support/legacy.rs
//! `-- session-runtime/tests/support/sql.rs
//! External:
//! |-- serde_json::from_value
//! |-- serde_json::json
//! `-- tokio::test
//! Used by:
//! `-- none, entrypoint
//! Invariants: expected strings are derived by hand from the baseline JS regex semantics.
//! Contract tests: session-runtime/tests/legacy_command.rs

#[path = "support/sql.rs"]
mod sql;
#[path = "support/legacy.rs"]
mod support;

use opencode_session_runtime::legacy::ports::CommandInfo;
use opencode_session_runtime::legacy::prompt::child_reply;
use opencode_session_runtime::legacy::template::{
    expand_command, file_references, hints, is_default_title, parse_model, replace_first_js,
    split_arguments,
};
use opencode_session_runtime::legacy::types::{CommandInput, LegacyError, WithParts};
use serde_json::json;
use support::{Options, SESSION, setup_with};

#[test]
fn file_references_follow_file_regex() {
    assert_eq!(
        file_references("read @src/a.ts, @./b.md. and `@x` me@mail.com @.env @dir/"),
        vec!["src/a.ts", "./b.md", ".env", "dir/"]
    );
    assert_eq!(file_references("@a.b.c.d"), vec!["a.b.c.d"]);
    assert_eq!(file_references("@"), vec![""]);
}

#[test]
fn argument_splitting_and_placeholders() {
    assert_eq!(
        split_arguments("one \"two three\" 'four' [Image 1] five"),
        vec!["one", "two three", "four", "[Image 1]", "five"]
    );
    assert_eq!(expand_command("a=$1 b=$2", "x y z"), "a=x b=y z");
    assert_eq!(expand_command("a=$1 b=$3", "x"), "a=x b=");
    assert_eq!(expand_command("all: $ARGUMENTS", "p q"), "all: p q");
    assert_eq!(expand_command("plain", "extra args"), "plain\n\nextra args");
    assert_eq!(expand_command("plain", "   "), "plain");
    assert_eq!(
        expand_command("$ARGUMENTS", "cost $& x"),
        "cost $ARGUMENTS x"
    );
    assert_eq!(
        hints("use $2 then $1 and $ARGUMENTS $1"),
        vec!["$1", "$2", "$ARGUMENTS"]
    );
}

#[test]
fn small_helpers_match_baseline() {
    assert_eq!(
        parse_model("openai/gpt/4o"),
        ("openai".into(), "gpt/4o".into())
    );
    assert!(is_default_title("New session - 2025-01-02T03:04:05.678Z"));
    assert!(is_default_title("Child session - 2025-01-02T03:04:05.678Z"));
    assert!(!is_default_title("New session - 2025-01-02"));
    assert_eq!(
        replace_first_js("a ${x} b ${x}", "${x}", "[$$]"),
        "a [$] b ${x}"
    );
}

fn command(name: &str, template: &str) -> CommandInfo {
    CommandInfo {
        name: name.into(),
        template: template.into(),
        ..CommandInfo::default()
    }
}

fn options(commands: Vec<CommandInfo>) -> Options {
    Options {
        commands,
        ..Options::default()
    }
}

fn input(name: &str, arguments: &str) -> CommandInput {
    CommandInput {
        session_id: SESSION.into(),
        command: name.into(),
        arguments: arguments.into(),
        ..CommandInput::default()
    }
}

#[tokio::test]
async fn unknown_command_fails_with_available_hint() {
    let ctx = setup_with(options(vec![command("init", "x")])).await;
    assert_eq!(
        ctx.legacy.command(input("nope", "")).await.unwrap_err(),
        LegacyError::Unknown("Command not found: \"nope\". Available commands: init".into())
    );
}

#[cfg(unix)]
#[tokio::test]
async fn command_expands_arguments_and_shell_then_prompts() {
    let ctx = setup_with(options(vec![command(
        "greet",
        "Hello $1! !`echo from-shell`",
    )]))
    .await;
    let result = ctx.legacy.command(input("greet", "world")).await.unwrap();
    assert_eq!(result.info["id"], "msg_reply", "command runs the loop");
    let messages = ctx.messages().await;
    assert_eq!(messages[0].parts[0]["text"], "Hello world! from-shell");
    assert_eq!(messages[0].info["agent"], "build");
    assert_eq!(
        *ctx.tools.hooks.lock().unwrap(),
        vec![
            "command.execute.before".to_string(),
            "chat.message".to_string()
        ]
    );
}

#[tokio::test]
async fn subagent_command_sends_one_subtask_part() {
    let mut cmd = command("review", "Review $ARGUMENTS");
    cmd.agent = Some("explore".into());
    cmd.description = Some("review changes".into());
    let ctx = setup_with(options(vec![cmd])).await;
    ctx.legacy.command(input("review", "HEAD")).await.unwrap();
    let messages = ctx.messages().await;
    let part = &messages[0].parts[0];
    assert_eq!(part["type"], "subtask");
    assert_eq!(part["agent"], "explore");
    assert_eq!(part["prompt"], "Review HEAD");
    assert_eq!(part["description"], "review changes");
    assert_eq!(part["command"], "review");
    assert_eq!(
        part["model"],
        json!({ "providerID": "test", "modelID": "model" })
    );
    assert_eq!(
        messages[0].info["agent"], "build",
        "the user message keeps the caller agent"
    );
}

#[tokio::test]
async fn command_model_not_found_publishes_error() {
    let mut cmd = command("m", "x");
    cmd.model = Some("nope/missing".into());
    let ctx = setup_with(options(vec![cmd])).await;
    let error = ctx.legacy.command(input("m", "")).await.unwrap_err();
    assert!(matches!(error, LegacyError::ModelNotFound { .. }));
    assert!(ctx.messages().await.is_empty());
}

#[test]
fn child_reply_prefers_assistant_error_then_failed_tool_then_text() {
    let result: WithParts = serde_json::from_value(json!({
        "info": { "role": "assistant", "error": { "name": "APIError", "data": { "message": "rate" } } },
        "parts": [
            { "type": "text", "text": "first" },
            { "type": "tool", "state": { "status": "error", "error": "bad" } },
            { "type": "text", "text": "last" },
        ],
    }))
    .unwrap();
    let reply = child_reply(&result);
    assert_eq!(reply.error.as_deref(), Some("rate"));
    assert_eq!(reply.failed_tool.as_deref(), Some("bad"));
    assert_eq!(reply.text.as_deref(), Some("last"));
    let named: WithParts = serde_json::from_value(json!({
        "info": { "role": "assistant", "error": { "name": "MessageAbortedError", "data": {} } },
        "parts": [],
    }))
    .unwrap();
    assert_eq!(
        child_reply(&named).error.as_deref(),
        Some("MessageAbortedError")
    );
}
