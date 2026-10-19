//! Purpose: runner step allowance and overflow transition contract (FIXTURE provider): max-steps
//! last turn (no tools, toolChoice none, MAX_STEPS assistant message, tool calls rejected), step
//! reset when steering input promotes (session-runner.test.ts:3112-3161), overflow compaction
//! retry and the second-overflow error.
//! Scope: integration test over real SQLite through `LocalSessionExecution`.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- session-api/src/lib.rs
//! |-- session-runtime/src/runner/max_steps.rs
//! |-- session-runtime/tests/support/provider.rs
//! `-- session-runtime/tests/support/sql.rs
//! External:
//! |-- tokio::spawn
//! `-- tokio::test
//! Used by:
//! `-- none, entrypoint
//! Invariants: fixture-only provider and compaction port.
//! Contract tests: session-runtime/tests/runner_turn_steps.rs

#[path = "support/sql.rs"]
mod sql;
#[path = "support/provider.rs"]
mod support;

use std::sync::Arc;

use opencode_llm_api::{ContentPart, MessageRole, ToolChoiceType};
use opencode_session_api::{Delivery, SessionError};
use opencode_session_runtime::runner::max_steps::MAX_STEPS_PROMPT;
use support::*;

#[tokio::test]
async fn last_step_disables_tools_and_appends_the_max_steps_prompt() {
    let t = Harness::new().await;
    let _steps = t.set_steps(1);
    t.prompt("Only once", Delivery::Steer, false).await;
    t.llm
        .script(vec![calls(vec![tool_call("call-late", "late", None)])]);
    t.resume().await.unwrap();

    let requests = t.llm.requests();
    assert_eq!(requests.len(), 1, "a rejected tool call does not continue");
    let request = &requests[0];
    assert!(request.tools.is_empty());
    assert_eq!(
        request.tool_choice.as_ref().unwrap().kind,
        ToolChoiceType::None
    );
    let last = request.messages.last().unwrap();
    assert_eq!(last.role, MessageRole::Assistant);
    assert!(matches!(&last.content[0], ContentPart::Text { text, .. } if text == MAX_STEPS_PROMPT));
    assert!(t.executions().is_empty());
    let assistant = &t.context().await[1];
    let tool = &assistant["content"][0];
    assert_eq!(tool["state"]["status"], "error");
    assert_eq!(
        tool["state"]["error"]["message"],
        "Tools are disabled after the maximum agent steps"
    );
}

#[tokio::test]
async fn step_allowance_bounds_tool_continuations() {
    let t = Harness::new().await;
    let _steps = t.set_steps(2);
    t.prompt("Loop", Delivery::Steer, false).await;
    t.llm.script(vec![
        calls(vec![tool_call("call-1", "one", None)]),
        calls(vec![tool_call("call-2", "two", None)]),
        stop(),
    ]);
    t.resume().await.unwrap();
    let requests = t.llm.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].tool_choice.is_none());
    assert_eq!(
        requests[1].tool_choice.as_ref().unwrap().kind,
        ToolChoiceType::None
    );
    assert_eq!(t.executions(), ["one"]);
}

#[tokio::test]
async fn resets_the_configured_step_allowance_when_steering_input_promotes() {
    let t = Arc::new(Harness::new().await);
    let _steps = t.set_steps(2);
    t.prompt("Start work", Delivery::Steer, false).await;
    t.llm.script(vec![
        calls(vec![tool_call("call-before-steer", "before", None)]),
        calls(vec![tool_call("call-after-steer", "after", None)]),
        stop(),
    ]);
    let (started, gate) = t.llm.gate_next();
    let run = tokio::spawn({
        let t = t.clone();
        async move { t.resume().await }
    });
    started.wait().await;
    t.prompt("Change direction", Delivery::Steer, true).await;
    gate.open();
    run.await.unwrap().unwrap();

    let requests = t.llm.requests();
    assert_eq!(requests.len(), 3);
    assert!(requests[1].tool_choice.is_none());
    assert!(!requests[1].tools.is_empty());
    assert_eq!(
        requests[2].tool_choice.as_ref().unwrap().kind,
        ToolChoiceType::None
    );
    assert_eq!(t.executions(), ["before", "after"]);
}

#[tokio::test]
async fn overflow_before_assistant_start_compacts_and_retries_once() {
    let compaction = Arc::new(CountingCompaction::default());
    let t = Harness::with_compaction(compaction.clone()).await;
    t.prompt("Big", Delivery::Steer, false).await;
    t.llm
        .script(vec![events(vec![provider_error("too long", true)]), stop()]);
    t.resume().await.unwrap();

    assert_eq!(t.llm.requests().len(), 2);
    assert_eq!(*compaction.overflow_calls.lock().unwrap(), 1);
    assert_eq!(
        t.types().await,
        ["user", "assistant"],
        "the withheld overflow is not projected"
    );
    assert_eq!(t.context().await[1]["finish"], "stop");
}

#[tokio::test]
async fn second_overflow_after_overflow_compaction_is_published_as_a_failure() {
    let compaction = Arc::new(CountingCompaction::default());
    let t = Harness::with_compaction(compaction.clone()).await;
    t.prompt("Big", Delivery::Steer, false).await;
    t.llm.script(vec![
        events(vec![provider_error("too long", true)]),
        events(vec![provider_error("still too long", true)]),
    ]);
    t.resume().await.unwrap();

    assert_eq!(t.llm.requests().len(), 2);
    assert_eq!(
        *compaction.overflow_calls.lock().unwrap(),
        1,
        "no recovery on the retry path"
    );
    let assistant = &t.context().await[1];
    assert_eq!(assistant["finish"], "error");
    assert_eq!(assistant["error"]["message"], "still too long");
}

#[tokio::test]
async fn explicit_resume_surfaces_provider_stream_failures() {
    let t = Harness::new().await;
    t.prompt("Boom", Delivery::Steer, false).await;
    t.llm.script(vec![Script::Events(vec![
        Ok(step_start()),
        Err(opencode_llm_api::LlmError::invalid_output(
            "fake/fake-route",
            "bad frame",
            None,
        )),
    ])]);
    let error = t.resume().await.unwrap_err();
    assert!(matches!(error, SessionError::Run { .. }), "{error:?}");
    let assistant = &t.context().await[1];
    assert_eq!(assistant["finish"], "error");
    assert_eq!(assistant["error"]["message"], "bad frame");
}
