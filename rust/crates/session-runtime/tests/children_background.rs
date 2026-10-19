//! Purpose: verify task-tool background and cancellation behavior: background start/extend, the single
//! synthetic parent notification, foreground abort, promotion, and hierarchical cancel without orphans.
//! Scope: integration tests over the real `ComposedChildHost` and migrated SQLite. `FixtureOps` (agent
//! lookup, permission ask, tool metadata, legacy SessionPrompt ops) is fixture-only.
//! Uses:
//! rust/crates/
//! |-- session-runtime/src/background.rs
//! |-- session-runtime/src/children.rs
//! |-- session-runtime/tests/children/fixture.rs
//! `-- session-runtime/tests/support/sql.rs
//! External:
//! |-- serde_json::json
//! |-- tokio::spawn
//! |-- tokio::sync::Semaphore
//! |-- tokio::test
//! |-- tokio::time::sleep
//! `-- tokio_util::sync::CancellationToken
//! Used by:
//! `-- none, entrypoint
//! Invariants: tests call `Children`, never copied orchestration logic. Fixture prompt runs are gated by
//! semaphores so start, extend, cancel and promotion orderings are deterministic.
//! Contract tests: session-runtime/tests/children_background.rs

#[path = "children/fixture.rs"]
mod fixture;
#[path = "support/sql.rs"]
mod sql;

use std::sync::Arc;
use std::time::Duration;

use fixture::{ASSISTANT, FixtureOps, PARENT, child_of, default_ops, input, settings, setup};
use opencode_session_runtime::background::JobStatus;
use opencode_session_runtime::children::{
    BACKGROUND_STARTED, BACKGROUND_UPDATED, ChildError, ChildReply, TaskInput, TaskSettings,
    TaskState, render_output,
};
use serde_json::json;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn background_start_extend_and_synthetic_parent_notification() {
    let gate = Arc::new(Semaphore::new(0));
    let ctx = setup(FixtureOps {
        gate: Some(gate.clone()),
        ..default_ops()
    })
    .await;
    let background = TaskInput {
        background: Some(true),
        ..input("general")
    };
    let started = ctx
        .children
        .run(&settings(), background.clone(), CancellationToken::new())
        .await
        .unwrap();
    let child = child_of(&started);
    assert_eq!(started.metadata["background"], json!(true));
    assert_eq!(started.metadata["jobId"], json!(child));
    assert_eq!(
        started.output,
        render_output(
            &child,
            TaskState::Running,
            Some("Background task started"),
            BACKGROUND_STARTED
        )
    );
    // Same task_id while running: extends the job instead of starting a new one.
    let more = TaskInput {
        task_id: Some(child.clone()),
        prompt: "and more".into(),
        ..background
    };
    let updated = ctx
        .children
        .run(&settings(), more, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        updated.output,
        render_output(
            &child,
            TaskState::Running,
            Some("Background task updated"),
            BACKGROUND_UPDATED
        )
    );
    assert!(ctx.ops.parents.lock().unwrap().is_empty());
    gate.add_permits(2);
    let parents = ctx.parents(1).await;
    assert_eq!(parents.len(), 1);
    let (session, agent, variant, text) = parents[0].clone();
    assert_eq!(session, PARENT);
    // Parent's current agent from its Session row, calling variant.
    assert_eq!(agent, "build");
    assert_eq!(variant.as_deref(), Some("high"));
    assert_eq!(
        text,
        render_output(
            &child,
            TaskState::Completed,
            Some("Background task completed: Find files"),
            "done: and more"
        )
    );
    ctx.children.shutdown().await;
    assert_eq!(ctx.ops.parents.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn background_failure_notifies_error_and_cancel_notifies_nothing() {
    let gate = Arc::new(Semaphore::new(0));
    let ops = FixtureOps {
        gate: Some(gate.clone()),
        ..default_ops()
    };
    ops.replies.lock().unwrap().push_back(ChildReply {
        error: Some("boom".into()),
        ..ChildReply::default()
    });
    let ctx = setup(ops).await;
    let background = TaskInput {
        background: Some(true),
        ..input("general")
    };
    let failing = child_of(
        &ctx.children
            .run(&settings(), background.clone(), CancellationToken::new())
            .await
            .unwrap(),
    );
    gate.add_permits(1);
    let parents = ctx.parents(1).await;
    assert_eq!(
        parents[0].3,
        render_output(
            &failing,
            TaskState::Error,
            Some("Background task failed: Find files"),
            &format!("Subagent failed (task_id: {failing}): boom")
        )
    );
    let cancelled = child_of(
        &ctx.children
            .run(&settings(), background, CancellationToken::new())
            .await
            .unwrap(),
    );
    ctx.children.cancel_tree(PARENT).await;
    assert_eq!(
        ctx.children.jobs().get(&cancelled).unwrap().status,
        JobStatus::Cancelled
    );
    assert!(ctx.ops.cancels.lock().unwrap().contains(&cancelled));
    ctx.children.shutdown().await;
    assert_eq!(ctx.ops.parents.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn foreground_abort_cancels_child_and_job() {
    let gate = Arc::new(Semaphore::new(0));
    let ctx = setup(FixtureOps {
        gate: Some(gate),
        ..default_ops()
    })
    .await;
    let abort = CancellationToken::new();
    let children = ctx.children.clone();
    let token = abort.clone();
    let task =
        tokio::spawn(async move { children.run(&settings(), input("general"), token).await });
    while ctx.ops.prompts.lock().unwrap().is_empty() {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let child = ctx.ops.prompts.lock().unwrap()[0].child.clone();
    abort.cancel();
    assert_eq!(task.await.unwrap(), Err(ChildError::Cancelled));
    assert_eq!(
        ctx.children.jobs().get(&child).unwrap().status,
        JobStatus::Cancelled
    );
    assert!(ctx.ops.cancels.lock().unwrap().contains(&child));
}

#[tokio::test]
async fn promotion_returns_background_result_then_notifies() {
    let gate = Arc::new(Semaphore::new(0));
    let ctx = setup(FixtureOps {
        gate: Some(gate.clone()),
        ..default_ops()
    })
    .await;
    let children = ctx.children.clone();
    let task = tokio::spawn(async move {
        children
            .run(&settings(), input("general"), CancellationToken::new())
            .await
    });
    while ctx.ops.prompts.lock().unwrap().is_empty() {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let child = ctx.ops.prompts.lock().unwrap()[0].child.clone();
    ctx.children.jobs().promote(&child).await.unwrap();
    let output = task.await.unwrap().unwrap();
    assert_eq!(output.metadata["background"], json!(true));
    assert_eq!(output.metadata["jobId"], json!(child));
    assert!(
        output
            .output
            .contains("<summary>Background task started</summary>")
    );
    // on_promote republished tool metadata with background + jobId.
    let last = ctx.ops.metadata.lock().unwrap().last().cloned().unwrap();
    assert_eq!(last["jobId"], json!(child));
    gate.add_permits(1);
    let parents = ctx.parents(1).await;
    assert_eq!(parents.len(), 1);
    assert!(parents[0].3.contains("state=\"completed\""));
}

#[tokio::test]
async fn cancel_tree_cancels_descendants_but_not_siblings() {
    let gate = Arc::new(Semaphore::new(0));
    let ctx = setup(FixtureOps {
        gate: Some(gate.clone()),
        ..default_ops()
    })
    .await;
    let deep = TaskSettings {
        subagent_depth: Some(3),
        ..settings()
    };
    let background = |session: &str, message: &str| TaskInput {
        session_id: session.into(),
        message_id: message.into(),
        background: Some(true),
        ..input("general")
    };
    let child = child_of(
        &ctx.children
            .run(
                &deep,
                background(PARENT, ASSISTANT),
                CancellationToken::new(),
            )
            .await
            .unwrap(),
    );
    ctx.assistant(&child, "msg_c", None).await;
    let grandchild = child_of(
        &ctx.children
            .run(&deep, background(&child, "msg_c"), CancellationToken::new())
            .await
            .unwrap(),
    );
    ctx.exec("INSERT INTO session (id, project_id, slug, directory, title, version, time_created, time_updated) VALUES ('ses_other', 'global', 's', '/project', 't', 'v', 1, 1)", vec![]).await;
    ctx.assistant("ses_other", "msg_o", None).await;
    let sibling = child_of(
        &ctx.children
            .run(
                &deep,
                background("ses_other", "msg_o"),
                CancellationToken::new(),
            )
            .await
            .unwrap(),
    );
    ctx.children.cancel_tree(PARENT).await;
    let status = |id: &str| ctx.children.jobs().get(id).unwrap().status;
    assert_eq!(status(&child), JobStatus::Cancelled);
    assert_eq!(status(&grandchild), JobStatus::Cancelled);
    assert_eq!(status(&sibling), JobStatus::Running);
    // Cancel hooks reached both descendants, so no child prompt outlives its ancestor.
    let cancels = ctx.ops.cancels.lock().unwrap().clone();
    assert!(cancels.contains(&child) && cancels.contains(&grandchild));
    gate.add_permits(3);
    let parents = ctx.parents(1).await;
    assert_eq!(parents.len(), 1);
    assert_eq!(parents[0].0, "ses_other");
    ctx.children.shutdown().await;
}
