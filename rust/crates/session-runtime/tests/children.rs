//! Purpose: verify task-tool child Session creation (depth gate, permission inheritance, parentID,
//! model/variant, foreground join, reply failures, resume, typed unwired blockers) through the public
//! `Children` API, the real `ComposedChildHost` and migrated SQLite files.
//! Scope: integration tests. Session rows, the legacy `session.created` projector, the `message` table
//! and the event store are real. `FixtureOps` (agent lookup, permission ask, tool metadata and legacy
//! SessionPrompt ops) is fixture-only: those producers are owned by other nodes and not composed yet.
//! Uses:
//! rust/crates/
//! |-- session-runtime/src/background.rs
//! |-- session-runtime/src/children.rs
//! |-- session-runtime/tests/children/fixture.rs
//! `-- session-runtime/tests/support/sql.rs
//! External:
//! |-- serde_json::{json, Value}
//! |-- tokio
//! `-- tokio_util::sync::CancellationToken
//! Used by:
//! `-- none, entrypoint
//! Invariants: tests call `Children`, never copied orchestration logic. Background, abort, promotion
//! and cancel-tree coverage lives in `children_background.rs`.
//! Contract tests: session-runtime/tests/children.rs

#[path = "children/fixture.rs"]
mod fixture;
#[path = "support/sql.rs"]
mod sql;

use std::sync::Arc;

use fixture::{FixtureOps, PARENT, child_of, default_ops, input, model, settings, setup};
use opencode_session_runtime::background::{BackgroundJobs, JobStatus};
use opencode_session_runtime::children::{
    ChildError, ChildLocation, ChildReply, Children, ComposedChildHost, LegacySessions, TaskInput,
    TaskSettings, UNWIRED_AGENT, UNWIRED_PROMPT, UnwiredChildOps,
};
use serde_json::{Value as Json, json};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn foreground_creates_child_with_inherited_permission_and_returns_result() {
    let ctx = setup(default_ops()).await;
    let settings = TaskSettings {
        primary_tools: vec!["bash".into(), "task".into()],
        ..settings()
    };
    let output = ctx
        .children
        .run(&settings, input("general"), CancellationToken::new())
        .await
        .unwrap();
    let child = child_of(&output);
    assert!(child.starts_with("ses_"));
    let (parent_id, agent, title, permission) = ctx.row(&child).await;
    assert_eq!(parent_id.as_deref(), Some(PARENT));
    assert_eq!(agent.as_deref(), Some("general"));
    assert_eq!(title, "Find files (@general subagent)");
    // Parent deny + external_directory rules, default todowrite/task denies, then primary tool denies
    // not already present (the `task` deny is deduplicated).
    let permission: Json = serde_json::from_str(&permission.unwrap()).unwrap();
    assert_eq!(
        permission,
        json!([
            {"permission": "edit", "pattern": "secret/*", "action": "deny"},
            {"permission": "external_directory", "pattern": "/tmp/*", "action": "allow"},
            {"permission": "todowrite", "pattern": "*", "action": "deny"},
            {"permission": "todowrite", "pattern": "*", "action": "deny"},
            {"permission": "task", "pattern": "*", "action": "deny"},
            {"permission": "bash", "pattern": "*", "action": "deny"}
        ])
    );
    assert_eq!(
        ctx.ops.asks.lock().unwrap().clone(),
        vec![("task".to_owned(), "general".to_owned())]
    );
    // Calling message model and variant are inherited when the agent has no model.
    let run = ctx.ops.prompts.lock().unwrap()[0].clone();
    assert_eq!(run.child, child);
    assert_eq!(run.model, model("openai", "gpt-call"));
    assert_eq!(run.variant.as_deref(), Some("high"));
    assert_eq!(run.agent, "general");
    assert_eq!(
        serde_json::to_string(&output.metadata).unwrap(),
        format!(
            r#"{{"parentSessionId":"{PARENT}","sessionId":"{child}","model":{{"modelID":"gpt-call","providerID":"openai"}}}}"#
        )
    );
    assert_eq!(output.title, "Find files");
    assert_eq!(
        output.output,
        format!(
            "<task id=\"{child}\" state=\"completed\">\n<task_result>\ndone: look around\n</task_result>\n</task>"
        )
    );
    assert_eq!(
        ctx.children.jobs().get(&child).unwrap().status,
        JobStatus::Completed
    );
}

#[tokio::test]
async fn agent_model_wins_and_drops_variant_and_own_rules_skip_default_denies() {
    let ctx = setup(default_ops()).await;
    let output = ctx
        .children
        .run(&settings(), input("pinned"), CancellationToken::new())
        .await
        .unwrap();
    let child = child_of(&output);
    let run = ctx.ops.prompts.lock().unwrap()[0].clone();
    assert_eq!(run.model, model("anthropic", "claude-pinned"));
    assert_eq!(run.variant, None);
    assert_eq!(
        serde_json::to_string(&output.metadata["model"]).unwrap(),
        r#"{"providerID":"anthropic","modelID":"claude-pinned"}"#
    );
    let permission: Json = serde_json::from_str(&ctx.row(&child).await.3.unwrap()).unwrap();
    // The pinned agent mentions todowrite, so only the parent's deny remains plus the task deny.
    assert_eq!(
        permission,
        json!([
            {"permission": "edit", "pattern": "secret/*", "action": "deny"},
            {"permission": "external_directory", "pattern": "/tmp/*", "action": "allow"},
            {"permission": "todowrite", "pattern": "*", "action": "deny"},
            {"permission": "task", "pattern": "*", "action": "deny"}
        ])
    );
}

#[tokio::test]
async fn depth_gate_counts_ancestors_before_asking_or_creating() {
    let ctx = setup(default_ops()).await;
    let first = ctx
        .children
        .run(&settings(), input("general"), CancellationToken::new())
        .await
        .unwrap();
    let child = child_of(&first);
    ctx.assistant(&child, "msg_child", None).await;
    let nested = TaskInput {
        session_id: child.clone(),
        message_id: "msg_child".into(),
        ..input("general")
    };
    let before = ctx.sessions().await;
    let error = ctx
        .children
        .run(&settings(), nested.clone(), CancellationToken::new())
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Subagent depth limit reached (1). Increase \"subagent_depth\" to allow nested subagents."
    );
    assert_eq!(ctx.ops.asks.lock().unwrap().len(), 1);
    assert_eq!(ctx.sessions().await, before);

    let deeper = TaskSettings {
        subagent_depth: Some(2),
        ..settings()
    };
    let grandchild = child_of(
        &ctx.children
            .run(&deeper, nested, CancellationToken::new())
            .await
            .unwrap(),
    );
    assert_eq!(
        ctx.row(&grandchild).await.0.as_deref(),
        Some(child.as_str())
    );
    // Grandchild variant comes from its calling message, which has none.
    assert_eq!(ctx.ops.prompts.lock().unwrap()[1].variant, None);
    let limit = TaskSettings {
        subagent_depth: Some(0),
        ..settings()
    };
    assert!(
        ctx.children
            .run(&limit, input("general"), CancellationToken::new())
            .await
            .unwrap_err()
            .to_string()
            .starts_with("Subagent depth limit reached (0).")
    );
}

#[tokio::test]
async fn validation_failures_do_not_create_children() {
    let ctx = setup(default_ops()).await;
    let token = CancellationToken::new;
    let background = TaskInput {
        background: Some(true),
        ..input("general")
    };
    let disabled = TaskSettings {
        background_subagents: false,
        ..settings()
    };
    assert_eq!(
        ctx.children
            .run(&disabled, background, token())
            .await
            .unwrap_err()
            .to_string(),
        "Background subagents require OPENCODE_EXPERIMENTAL_BACKGROUND_SUBAGENTS=true"
    );
    assert_eq!(
        ctx.children
            .run(&settings(), input("nobody"), token())
            .await
            .unwrap_err()
            .to_string(),
        "Unknown agent type: nobody is not a valid agent type"
    );
    let missing = TaskInput {
        session_id: "ses_missing".into(),
        ..input("general")
    };
    assert_eq!(
        ctx.children
            .run(&settings(), missing, token())
            .await
            .unwrap_err(),
        ChildError::NotFound("Session not found: ses_missing".into())
    );
    assert_eq!(ctx.sessions().await, 1);
    // Bypass skips the ask; an unknown task_id falls back to creating a fresh child.
    let bypass = TaskInput {
        bypass_agent_check: true,
        task_id: Some("ses_gone".into()),
        ..input("general")
    };
    let asks = ctx.ops.asks.lock().unwrap().len();
    ctx.children
        .run(&settings(), bypass, token())
        .await
        .unwrap();
    assert_eq!(ctx.ops.asks.lock().unwrap().len(), asks);
    assert_eq!(ctx.sessions().await, 2);
}

#[tokio::test]
async fn denied_ask_and_non_assistant_caller_fail() {
    let ctx = setup(FixtureOps {
        deny_ask: true,
        ..default_ops()
    })
    .await;
    assert_eq!(
        ctx.children
            .run(&settings(), input("general"), CancellationToken::new())
            .await
            .unwrap_err(),
        ChildError::PermissionDenied("rejected".into())
    );
    assert_eq!(ctx.sessions().await, 1);

    let ctx = setup(default_ops()).await;
    ctx.exec(
        "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES ('msg_user', ?, 1, 1, '{\"role\":\"user\"}')",
        vec![PARENT.into()],
    )
    .await;
    let user = TaskInput {
        message_id: "msg_user".into(),
        ..input("general")
    };
    assert_eq!(
        ctx.children
            .run(&settings(), user, CancellationToken::new())
            .await
            .unwrap_err(),
        ChildError::Failed("Not an assistant message".into())
    );
}

#[tokio::test]
async fn child_reply_errors_map_to_subagent_failures_and_resume_reuses_child() {
    let ops = default_ops();
    ops.replies.lock().unwrap().extend([
        ChildReply {
            error: Some("rate limited".into()),
            ..ChildReply::default()
        },
        ChildReply {
            failed_tool: Some("bash exploded".into()),
            text: Some("ignored".into()),
            ..ChildReply::default()
        },
    ]);
    let ctx = setup(ops).await;
    let first = ctx
        .children
        .run(&settings(), input("general"), CancellationToken::new())
        .await
        .unwrap_err();
    let ChildError::Failed(text) = first else {
        panic!("{first:?}")
    };
    let child = text
        .strip_prefix("Subagent failed (task_id: ")
        .and_then(|rest| rest.strip_suffix("): rate limited"))
        .unwrap()
        .to_owned();
    let resumed = TaskInput {
        task_id: Some(child.clone()),
        ..input("general")
    };
    assert_eq!(
        ctx.children
            .run(&settings(), resumed.clone(), CancellationToken::new())
            .await
            .unwrap_err(),
        ChildError::Failed(format!("Subagent failed (task_id: {child}): bash exploded"))
    );
    let output = ctx
        .children
        .run(&settings(), resumed, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(child_of(&output), child);
    assert_eq!(ctx.sessions().await, 2);
}

#[tokio::test]
async fn unwired_composition_fails_typed() {
    let h = sql::open().await;
    h.seed_session(PARENT).await;
    let sessions = LegacySessions::new(
        h.events.clone(),
        ChildLocation {
            project_id: "global".into(),
            directory: "/project".into(),
            path: None,
            workspace_id: None,
            version: "test".into(),
        },
    );
    let host = Arc::new(ComposedChildHost::new(sessions, Arc::new(UnwiredChildOps)));
    let children = Children::new(host, BackgroundJobs::new());
    let bypass = TaskInput {
        bypass_agent_check: true,
        ..input("general")
    };
    assert_eq!(
        children
            .run(&settings(), bypass, CancellationToken::new())
            .await
            .unwrap_err(),
        ChildError::NotYetPorted(UNWIRED_AGENT)
    );
    // Missing prompt ops fail before any job starts, like "TaskTool requires promptOps".
    let ctx = setup(FixtureOps {
        unwired_prompt: true,
        ..default_ops()
    })
    .await;
    assert_eq!(
        ctx.children
            .run(&settings(), input("general"), CancellationToken::new())
            .await
            .unwrap_err(),
        ChildError::NotYetPorted(UNWIRED_PROMPT)
    );
    assert!(ctx.children.jobs().list().is_empty());
}
