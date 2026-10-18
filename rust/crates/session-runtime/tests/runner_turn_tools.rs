//! Purpose: runner tool settlement and interruption contract (FIXTURE provider): local calls are
//! published before settlement and settled concurrently, results reach the next turn, provider-
//! executed calls are not re-executed, stale running tools fail before a new drain, and
//! interrupting an active turn fails unsettled tools and the active assistant.
//! Scope: integration test over real SQLite through `LocalSessionExecution`.
//! Uses:
//! rust/crates/
//! |-- session-runtime/src/execution.rs
//! |-- session-runtime/src/runner/turn.rs
//! |-- session-runtime/tests/support/provider.rs
//! `-- session-runtime/tests/support/sql.rs
//! External:
//! `-- tokio
//! Used by:
//! `-- none, entrypoint
//! Invariants: fixture-only provider; tools run through the real opencode-tool-api registry.
//! Contract tests: session-runtime/tests/runner_turn_tools.rs

#[path = "support/sql.rs"]
mod sql;
#[path = "support/provider.rs"]
mod support;

use std::sync::Arc;
use std::time::Duration;

use opencode_llm_api::{ContentPart, MessageRole};
use opencode_session_api::Delivery;
use opencode_session_api::operations::{SessionExecution, SessionService};
use opencode_session_runtime::runner::SessionRunner;
use support::*;
use tokio::sync::Barrier;

#[tokio::test]
async fn settles_local_tool_calls_concurrently_and_continues_with_results() {
    let t = Harness::new().await;
    *t.echo.barrier.lock().unwrap() = Some(Arc::new(Barrier::new(2)));
    t.prompt("Use tools", Delivery::Steer, false).await;
    t.llm.script(vec![
        calls(vec![
            tool_call("call-a", "a", None),
            tool_call("call-b", "b", None),
        ]),
        stop(),
    ]);
    t.resume().await.unwrap();

    let requests = t.llm.requests();
    assert_eq!(
        requests.len(),
        2,
        "tool continuation is a second provider turn"
    );
    let mut executions = t.executions();
    executions.sort();
    assert_eq!(executions, ["a", "b"]);
    let results: Vec<_> = requests[1]
        .messages
        .iter()
        .filter(|message| message.role == MessageRole::Tool)
        .flat_map(|message| &message.content)
        .filter_map(|part| match part {
            ContentPart::ToolResult(result) => Some(result.id.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(results, ["call-a", "call-b"]);
    let tools = &t.context().await[1]["content"];
    assert_eq!(tools[0]["state"]["status"], "completed");
    assert_eq!(tools[1]["state"]["status"], "completed");
}

#[tokio::test]
async fn provider_executed_calls_are_not_settled_locally() {
    let t = Harness::new().await;
    t.prompt("Hosted", Delivery::Steer, false).await;
    t.llm.script(vec![calls(vec![tool_call(
        "call-hosted",
        "hosted",
        Some(true),
    )])]);
    t.resume().await.unwrap();

    assert_eq!(
        t.llm.requests().len(),
        1,
        "hosted calls do not need continuation"
    );
    assert!(t.executions().is_empty());
    let tool = &t.context().await[1]["content"][0];
    assert_eq!(tool["state"]["status"], "error");
    assert_eq!(
        tool["state"]["error"]["message"],
        "Provider did not return a tool result"
    );
}

#[tokio::test]
async fn interrupting_a_turn_fails_running_tools_and_the_active_assistant() {
    let t = Arc::new(Harness::new().await);
    *t.echo.hang.lock().unwrap() = true;
    t.prompt("Hang", Delivery::Steer, false).await;
    t.llm.script(vec![Script::Hang(vec![
        step_start(),
        tool_call("call-hang", "hang", None),
    ])]);
    let run = tokio::spawn({
        let t = t.clone();
        async move { t.resume().await }
    });
    for _ in 0..200 {
        if !t.executions().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(t.executions(), ["hang"]);
    t.service.interrupt(&t.sid()).await;
    assert!(run.await.unwrap().is_err());
    assert!(t.execution.active().await.is_empty());

    let assistant = &t.context().await[1];
    assert_eq!(assistant["finish"], "error");
    assert_eq!(assistant["error"]["message"], "Provider turn interrupted");
    let tool = &assistant["content"][0];
    assert_eq!(tool["state"]["status"], "error");
    assert_eq!(
        tool["state"]["error"]["message"],
        "Tool execution interrupted"
    );
}

#[tokio::test]
async fn a_new_drain_fails_tools_left_running_by_a_dropped_owner() {
    let t = Arc::new(Harness::new().await);
    *t.echo.hang.lock().unwrap() = true;
    t.prompt("Hang", Delivery::Steer, false).await;
    t.llm.script(vec![Script::Hang(vec![
        step_start(),
        tool_call("call-stale", "stale", None),
    ])]);
    // Simulates a crashed owner: the drain future is dropped without settling anything.
    let owner = tokio::spawn({
        let t = t.clone();
        async move { t.runner.run(&t.sid(), true).await }
    });
    for _ in 0..200 {
        if !t.executions().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    owner.abort();
    let _ = owner.await;
    assert_eq!(
        t.context().await[1]["content"][0]["state"]["status"],
        "running"
    );

    *t.echo.hang.lock().unwrap() = false;
    t.llm.script(vec![stop()]);
    t.resume().await.unwrap();
    let tool = &t.context().await[1]["content"][0];
    assert_eq!(tool["state"]["status"], "error");
    assert_eq!(
        tool["state"]["error"]["message"],
        "Tool execution interrupted"
    );
    assert_eq!(t.llm.requests().len(), 1);
}

#[tokio::test]
async fn idle_interrupt_is_a_no_op() {
    let t = Harness::new().await;
    t.service.interrupt(&t.sid()).await;
    assert!(t.llm.requests().is_empty());
}
