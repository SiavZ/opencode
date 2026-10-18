//! Purpose: fixture tests for the legacy prompt path: `resolvePromptParts`, `createUserMessage`
//! part resolution, agent/model selection, noReply, loop join, cancel with `lastAssistant` fallback,
//! tools permission override, revert cleanup, V1 projector usage accounting and share/unshare.
//! Scope: test-only, real SQLite + event store + legacy projectors, fixture agents/provider/loop.
//! Uses:
//! rust/crates/
//! |-- session-runtime/src/legacy/mod.rs
//! |-- session-runtime/tests/support/legacy.rs
//! `-- session-runtime/tests/support/sql.rs
//! External:
//! `-- serde_json::json
//! Used by:
//! `-- none, entrypoint
//! Invariants: fixture-labeled doubles only for agents, provider, loop, tools and share remote.
//! Contract tests: session-runtime/tests/legacy_prompt.rs

#[path = "support/sql.rs"]
mod sql;
#[path = "support/legacy.rs"]
mod support;

use std::sync::Arc;

use opencode_session_runtime::legacy::types::{LegacyError, PromptInput};
use serde_json::{Value, json};
use support::{SESSION, setup};
use tokio::sync::Notify;

fn text(text: &str) -> serde_json::Map<String, Value> {
    serde_json::from_value(json!({ "type": "text", "text": text })).unwrap()
}

fn input(parts: Vec<serde_json::Map<String, Value>>) -> PromptInput {
    PromptInput {
        session_id: SESSION.into(),
        parts,
        no_reply: Some(true),
        ..PromptInput::default()
    }
}

#[tokio::test]
async fn resolve_prompt_parts_dedupes_files_dirs_and_agents() {
    let ctx = setup().await;
    std::fs::write(ctx.worktree.path().join("a.txt"), "x").unwrap();
    std::fs::create_dir(ctx.worktree.path().join("src")).unwrap();
    let parts = ctx
        .legacy
        .resolve_prompt_parts(
            "see @a.txt and @src, again @a.txt, ask @explore, ignore @missing x@y",
        )
        .await
        .unwrap();
    let parts: Vec<Value> = parts.into_iter().map(Value::Object).collect();
    let root = ctx.worktree.path().to_string_lossy();
    assert_eq!(
        parts[0],
        json!({ "type": "text", "text": "see @a.txt and @src, again @a.txt, ask @explore, ignore @missing x@y" })
    );
    assert_eq!(parts[1]["filename"], "a.txt");
    assert_eq!(parts[1]["mime"], "text/plain");
    assert_eq!(parts[1]["url"], format!("file://{root}/a.txt"));
    assert_eq!(parts[2]["mime"], "application/x-directory");
    assert_eq!(parts[3], json!({ "type": "agent", "name": "explore" }));
    assert_eq!(parts.len(), 4);
}

#[tokio::test]
async fn no_reply_prompt_persists_user_message_and_updates_agent_model() {
    let ctx = setup().await;
    let result = ctx.legacy.prompt(input(vec![text("hello")])).await.unwrap();
    assert_eq!(result.info["role"], "user");
    assert_eq!(result.info["agent"], "build");
    assert_eq!(
        result.info["model"],
        json!({ "providerID": "test", "modelID": "model" })
    );
    assert!(
        ctx.run_loop.runs.lock().unwrap().is_empty(),
        "noReply never runs the loop"
    );
    let messages = ctx.messages().await;
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].parts[0]["text"], "hello");
    assert!(
        messages[0].parts[0]["id"]
            .as_str()
            .unwrap()
            .starts_with("prt_")
    );
    let session = ctx.legacy.store().get(SESSION).await.unwrap();
    assert_eq!(session["agent"], "build");
    assert_eq!(
        session["model"],
        json!({ "id": "model", "providerID": "test", "variant": "default" })
    );
    assert_eq!(
        *ctx.tools.hooks.lock().unwrap(),
        vec!["chat.message".to_string()]
    );
}

#[tokio::test]
async fn unknown_agent_publishes_error_and_fails_with_hint() {
    let ctx = setup().await;
    let mut request = input(vec![text("hi")]);
    request.agent = Some("ghost".into());
    let error = ctx.legacy.prompt(request).await.unwrap_err();
    assert_eq!(
        error,
        LegacyError::Unknown(
            "Agent not found: \"ghost\". Available agents: build, plan, explore, denied".into()
        )
    );
    assert!(ctx.messages().await.is_empty());
}

#[tokio::test]
async fn file_text_part_reads_through_read_tool_with_range() {
    let ctx = setup().await;
    let path = ctx.worktree.path().join("f.rs");
    std::fs::write(&path, "fn main() {}").unwrap();
    let url = format!("file://{}?start=3&end=7", path.display());
    let part = serde_json::from_value(
        json!({ "type": "file", "mime": "text/plain", "url": url, "filename": "f.rs" }),
    )
    .unwrap();
    let result = ctx.legacy.prompt(input(vec![part])).await.unwrap();
    let texts: Vec<&str> = result
        .parts
        .iter()
        .filter_map(|p| p.get("text").and_then(Value::as_str))
        .collect();
    assert_eq!(
        texts[0],
        format!(
            "Called the Read tool with the following input: {{\"filePath\":\"{}\",\"offset\":3,\"limit\":5}}",
            path.display()
        )
    );
    assert_eq!(texts[1], "<file>contents</file>");
    assert_eq!(result.parts[2]["type"], "file");
    let reads = ctx.tools.reads.lock().unwrap();
    assert_eq!(reads[0].1.model.as_ref().unwrap()["id"], "model");
}

#[tokio::test]
async fn read_failure_publishes_error_and_degrades_to_synthetic_text() {
    let ctx = setup().await;
    *ctx.tools.fail_read.lock().unwrap() = Some("boom".into());
    let path = ctx.worktree.path().join("g.txt");
    std::fs::write(&path, "x").unwrap();
    let part = serde_json::from_value(json!({ "type": "file", "mime": "text/plain", "url": format!("file://{}", path.display()) })).unwrap();
    let result = ctx.legacy.prompt(input(vec![part])).await.unwrap();
    assert_eq!(result.parts.len(), 2);
    assert_eq!(
        result.parts[1]["text"],
        format!(
            "Read tool failed to read {} with the following error: boom",
            path.display()
        )
    );
}

#[tokio::test]
async fn binary_file_is_inlined_as_data_url_and_data_text_is_decoded() {
    let ctx = setup().await;
    let path = ctx.worktree.path().join("img.png");
    std::fs::write(&path, b"hello").unwrap();
    let file = serde_json::from_value(json!({ "type": "file", "mime": "image/png", "url": format!("file://{}", path.display()), "filename": "img.png" })).unwrap();
    let data = serde_json::from_value(json!({ "type": "file", "mime": "text/plain", "url": "data:text/plain;base64,aGk=", "filename": "n.txt" })).unwrap();
    let result = ctx.legacy.prompt(input(vec![file, data])).await.unwrap();
    assert_eq!(result.parts[1]["url"], "data:image/png;base64,aGVsbG8=");
    assert_eq!(result.parts[1]["filename"], "img.png");
    assert_eq!(
        result.parts[2]["text"],
        "Called the Read tool with the following input: {\"filePath\":\"n.txt\"}"
    );
    assert_eq!(result.parts[3]["text"], "hi");
}

#[tokio::test]
async fn agent_part_adds_task_hint_with_deny_suffix() {
    let ctx = setup().await;
    let part = serde_json::from_value(json!({ "type": "agent", "name": "explore" })).unwrap();
    let mut request = input(vec![part]);
    request.agent = Some("denied".into());
    let result = ctx.legacy.prompt(request).await.unwrap();
    assert_eq!(
        result.parts[1]["text"],
        " Use the above message and context to generate a prompt and call the task tool with subagent: explore . Invoked by user; guaranteed to exist."
    );
}

#[tokio::test]
async fn mcp_resource_part_expands_text_and_filters_binaries() {
    let ctx = setup().await;
    let part = serde_json::from_value(json!({
        "type": "file", "mime": "text/plain", "url": "mcp://x", "filename": "doc",
        "source": { "type": "resource", "clientName": "c", "uri": "res://a", "text": { "value": "", "start": 0, "end": 0 } },
    })).unwrap();
    let result = ctx.legacy.prompt(input(vec![part])).await.unwrap();
    let texts: Vec<&str> = result
        .parts
        .iter()
        .filter_map(|p| p.get("text").and_then(Value::as_str))
        .collect();
    assert_eq!(texts[0], "Reading MCP resource: doc (res://a)");
    assert_eq!(texts[1], "text of res://a");
    assert_eq!(
        texts[2],
        "[Binary MCP resource attached: res.png (image/png)]"
    );
    assert_eq!(
        texts[3],
        "[Binary MCP resource omitted: doc (application/zip, 5 B) is not a supported attachment type]"
    );
    assert_eq!(result.parts[3]["url"], "data:image/png;base64,aGVsbG8=");
}

#[tokio::test]
async fn tools_map_overrides_session_permission() {
    let ctx = setup().await;
    let mut request = input(vec![text("x")]);
    request.tools = Some(serde_json::from_value(json!({ "bash": false, "read": true })).unwrap());
    ctx.legacy.prompt(request).await.unwrap();
    let session = ctx.legacy.store().get(SESSION).await.unwrap();
    assert_eq!(
        session["permission"],
        json!([
            { "permission": "bash", "action": "deny", "pattern": "*" },
            { "permission": "read", "action": "allow", "pattern": "*" },
        ])
    );
}

#[tokio::test]
async fn prompt_runs_loop_and_concurrent_prompts_join_one_run() {
    let ctx = setup().await;
    let gate = Arc::new(Notify::new());
    *ctx.run_loop.gate.lock().unwrap() = Some(gate.clone());
    let mut first = input(vec![text("a")]);
    first.no_reply = None;
    let mut second = input(vec![text("b")]);
    second.no_reply = None;
    let started = ctx.run_loop.started.notified();
    let a = tokio::spawn({
        let legacy = ctx.legacy.clone();
        async move { legacy.prompt(first).await }
    });
    started.await;
    assert!(ctx.legacy.run_state().busy(SESSION));
    let b = tokio::spawn({
        let legacy = ctx.legacy.clone();
        async move { legacy.prompt(second).await }
    });
    // Wait until the second prompt wrote its user message, then release the single run.
    while ctx.messages().await.len() < 2 {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    gate.notify_one();
    let (a, b) = (a.await.unwrap().unwrap(), b.await.unwrap().unwrap());
    assert_eq!(a.info["id"], "msg_reply");
    assert_eq!(b.info["id"], "msg_reply");
    assert_eq!(ctx.run_loop.runs.lock().unwrap().len(), 1, "joined one run");
    assert!(!ctx.legacy.run_state().busy(SESSION));
}

#[tokio::test]
async fn cancel_interrupts_run_and_returns_last_message() {
    let ctx = setup().await;
    *ctx.run_loop.gate.lock().unwrap() = Some(Arc::new(Notify::new()));
    let mut request = input(vec![text("a")]);
    request.no_reply = None;
    let started = ctx.run_loop.started.notified();
    let run = tokio::spawn({
        let legacy = ctx.legacy.clone();
        async move { legacy.prompt(request).await }
    });
    started.await;
    ctx.legacy.cancel(SESSION).await;
    let result = run.await.unwrap().unwrap();
    assert_eq!(
        result.info["role"], "user",
        "lastAssistant falls back to the newest message"
    );
    assert!(!ctx.legacy.run_state().busy(SESSION));
    assert_eq!(
        ctx.legacy.run_state().status().get(SESSION),
        json!({ "type": "idle" })
    );
}

#[tokio::test]
async fn cancel_idle_session_is_noop() {
    let ctx = setup().await;
    ctx.legacy.cancel(SESSION).await;
    assert!(!ctx.legacy.run_state().busy(SESSION));
}

#[tokio::test]
async fn unwired_loop_fails_typed() {
    use opencode_session_runtime::legacy::ports::{LegacyLoop, UnwiredLoop};
    let error = UnwiredLoop.run(SESSION).await.unwrap_err();
    assert!(matches!(error, LegacyError::NotYetPorted(_)));
}

#[tokio::test]
async fn revert_cleanup_drops_reverted_messages_before_prompt() {
    let ctx = setup().await;
    let first = ctx.legacy.prompt(input(vec![text("keep")])).await.unwrap();
    let second = ctx.legacy.prompt(input(vec![text("drop")])).await.unwrap();
    assert_ne!(first.info["id"], second.info["id"]);
    ctx.legacy
        .store()
        .patch(
            SESSION,
            serde_json::from_value(json!({ "revert": { "messageID": second.info["id"] } }))
                .unwrap(),
        )
        .await
        .unwrap();
    ctx.legacy.prompt(input(vec![text("new")])).await.unwrap();
    let texts: Vec<String> = ctx
        .messages()
        .await
        .iter()
        .map(|m| m.parts[0]["text"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(texts, vec!["keep", "new"]);
    assert!(
        ctx.legacy
            .store()
            .get(SESSION)
            .await
            .unwrap()
            .get("revert")
            .is_none()
    );
}

#[tokio::test]
async fn step_finish_part_usage_is_applied_and_reversed() {
    let ctx = setup().await;
    let user = ctx.legacy.prompt(input(vec![text("x")])).await.unwrap();
    let store = ctx.legacy.store();
    let part = |cost: f64, input: f64| {
        serde_json::from_value(json!({
            "id": "prt_step", "messageID": user.info["id"], "sessionID": SESSION,
            "type": "step-finish", "reason": "stop", "cost": cost,
            "tokens": { "input": input, "output": 2, "reasoning": 1, "cache": { "read": 3, "write": 4 } },
        }))
        .unwrap()
    };
    store.update_part(&part(0.5, 10.0)).await.unwrap();
    store.update_part(&part(0.25, 20.0)).await.unwrap();
    let session = store.get(SESSION).await.unwrap();
    assert_eq!(session["cost"], json!(0.25));
    assert_eq!(session["tokens"]["input"], json!(20.0));
    store
        .remove_part(SESSION, user.info["id"].as_str().unwrap(), "prt_step")
        .await
        .unwrap();
    let session = store.get(SESSION).await.unwrap();
    assert_eq!(session["cost"], json!(0.0));
    assert_eq!(session["tokens"]["cache"]["write"], json!(0.0));
}

#[tokio::test]
async fn share_and_unshare_round_trip_and_disabled_gate() {
    let ctx = setup().await;
    let url = ctx.legacy.share(SESSION).await.unwrap();
    assert_eq!(url, format!("https://share.test/{SESSION}"));
    assert_eq!(
        ctx.legacy.store().get(SESSION).await.unwrap()["share"],
        json!({ "url": url })
    );
    ctx.legacy.unshare(SESSION).await.unwrap();
    // Baseline quirk: `sessionRow` maps an absent share to `share_url: undefined`, which Drizzle's
    // `.set()` skips, so unshare publishes `session.updated` without the share but the stored URL
    // column is left untouched (packages/core/src/session/projector.ts:55).
    let shared = ctx.legacy.store().get(SESSION).await.unwrap();
    assert_eq!(shared["share"], json!({ "url": url }));
    assert_eq!(
        *ctx.share.removed.lock().unwrap(),
        vec![SESSION.to_string()]
    );

    let disabled = support::setup_with(support::Options {
        config: opencode_session_runtime::legacy::LegacyConfig {
            share: Some("disabled".into()),
            ..Default::default()
        },
        ..Default::default()
    })
    .await;
    assert_eq!(
        disabled.legacy.share(SESSION).await.unwrap_err(),
        LegacyError::Unknown("Sharing is disabled in configuration".into())
    );
}

#[tokio::test]
async fn unwired_share_fails_typed() {
    use opencode_session_runtime::legacy::ports::{ShareRemote, UnwiredShare};
    assert!(matches!(
        UnwiredShare.create(SESSION).await,
        Err(opencode_session_runtime::legacy::ports::PortError::NotYetPorted(_))
    ));
}
