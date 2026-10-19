//! Purpose: runner turn loop contract over real SQLite (FIXTURE provider): single turn request
//! shape (headers, promptCacheKey, system, history), advisory force=false exit, one stream per
//! turn, provider errors as terminal step failures, steering mid-drain, steer batch coalescing,
//! queue promotion at idle and FIFO queue order (session-runner.test.ts:1877-1965, 2053-2094,
//! 2195-2233, 3163-3184).
//! Scope: integration test: `LocalSessionService` -> `LocalSessionExecution` -> `LocationRunners`
//! -> `LlmSessionRunner` with the scripted `LlmClient` from tests/support/provider.rs.
//! Uses:
//! rust/crates/
//! |-- session-api/src/lib.rs
//! |-- session-api/src/operations.rs
//! |-- session-runtime/src/runner/mod.rs
//! |-- session-runtime/src/runner/turn.rs
//! |-- session-runtime/tests/support/provider.rs
//! `-- session-runtime/tests/support/sql.rs
//! External:
//! |-- serde_json::json
//! |-- tokio::spawn
//! |-- tokio::test
//! `-- tokio::time::sleep
//! Used by:
//! `-- none, entrypoint
//! Invariants: fixture-only provider; durable state is the real projected `session_message` rows.
//! Contract tests: session-runtime/tests/runner_turn.rs

#[path = "support/sql.rs"]
mod sql;
#[path = "support/provider.rs"]
mod support;

use std::sync::Arc;

use opencode_session_api::Delivery;
use opencode_session_api::operations::SessionExecution;
use opencode_session_runtime::runner::SessionRunner;
use opencode_session_runtime::runner::turn::prompt_cache_key;
use serde_json::json;
use support::*;

#[tokio::test]
async fn single_turn_builds_the_baseline_request_and_projects_the_assistant() {
    let t = Harness::new().await;
    t.prompt("Hello", Delivery::Steer, false).await;
    t.llm.script(vec![stop()]);
    t.resume().await.unwrap();

    let requests = t.llm.requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(user_texts(request), ["Hello"]);
    let headers = request.http.as_ref().unwrap().headers.as_ref().unwrap();
    let keys: Vec<_> = headers.keys().cloned().collect();
    assert_eq!(
        keys,
        [
            "x-opencode-session-id",
            "x-session-affinity",
            "X-Session-Id"
        ]
    );
    assert!(headers.values().all(|value| value == SESSION));
    assert_eq!(
        serde_json::to_value(request.provider_options.as_ref().unwrap()).unwrap(),
        json!({ "openai": { "promptCacheKey": SESSION } })
    );
    assert!(
        request.system.is_empty(),
        "empty agent system and baseline are dropped"
    );
    assert!(request.tool_choice.is_none());
    assert_eq!(request.tools.len(), 1);
    assert_eq!(request.tools[0].name, "echo");
    assert_eq!(t.types().await, ["user", "assistant"]);
    let assistant = &t.context().await[1];
    assert_eq!(assistant["finish"], "stop");
    assert_eq!(assistant["agent"], "build");
    assert_eq!(
        assistant["model"],
        json!({ "id": "fake-model", "providerID": "fake" })
    );
}

#[test]
fn prompt_cache_key_strips_only_canonical_session_ids() {
    let canonical = format!("ses_{}", "a1".repeat(32));
    assert_eq!(prompt_cache_key(&canonical), &canonical[4..]);
    assert_eq!(prompt_cache_key("ses_short"), "ses_short");
    let upper = format!("ses_{}", "A1".repeat(32));
    assert_eq!(prompt_cache_key(&upper), upper);
}

#[tokio::test]
async fn advisory_run_without_pending_input_returns_without_a_provider_call() {
    let t = Harness::new().await;
    t.llm.script(vec![stop()]);
    t.runner.run(&t.sid(), false).await.unwrap();
    assert!(t.llm.requests().is_empty());
    assert!(t.types().await.is_empty());

    // An explicit run performs one provider attempt even with nothing eligible.
    t.runner.run(&t.sid(), true).await.unwrap();
    assert_eq!(t.llm.requests().len(), 1);
}

#[tokio::test]
async fn advisory_wake_drains_an_admitted_prompt_once() {
    let t = Harness::new().await;
    t.llm.script(vec![stop(), stop()]);
    t.prompt("Wake me", Delivery::Steer, false).await;
    t.runner.run(&t.sid(), false).await.unwrap();
    assert_eq!(t.llm.requests().len(), 1);
    t.runner.run(&t.sid(), false).await.unwrap();
    assert_eq!(t.llm.requests().len(), 1, "nothing pending after promotion");
}

#[tokio::test]
async fn projects_provider_errors_as_terminal_assistant_step_failures() {
    let t = Harness::new().await;
    t.prompt("Fail durably", Delivery::Steer, false).await;
    t.llm.script(vec![events(vec![
        step_start(),
        provider_error("Provider unavailable", false),
    ])]);
    t.resume().await.unwrap();

    assert_eq!(t.llm.requests().len(), 1);
    let context = t.context().await;
    assert_eq!(context[0]["type"], "user");
    assert_eq!(context[0]["text"], "Fail durably");
    assert_eq!(context[1]["type"], "assistant");
    assert_eq!(context[1]["finish"], "error");
    assert_eq!(
        context[1]["error"],
        json!({ "type": "unknown", "message": "Provider unavailable" })
    );
}

#[tokio::test]
async fn steers_an_active_provider_turn_with_newly_recorded_prompts() {
    let t = Arc::new(Harness::new().await);
    t.prompt("Start working", Delivery::Steer, false).await;
    t.llm.script(vec![stop(), stop()]);
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
    assert_eq!(requests.len(), 2, "exactly one stream per provider turn");
    assert_eq!(user_texts(&requests[0]), ["Start working"]);
    assert_eq!(
        user_texts(&requests[1]),
        ["Start working", "Change direction"]
    );
    assert_eq!(t.types().await, ["user", "assistant", "user", "assistant"]);
}

#[tokio::test]
async fn coalesces_multiple_active_steering_prompts_into_one_continuation_turn() {
    let t = Arc::new(Harness::new().await);
    t.prompt("Start working", Delivery::Steer, false).await;
    t.llm.script(vec![stop(), stop()]);
    let (started, gate) = t.llm.gate_next();

    let run = tokio::spawn({
        let t = t.clone();
        async move { t.resume().await }
    });
    started.wait().await;
    t.prompt("First steer", Delivery::Steer, true).await;
    t.prompt("Second steer", Delivery::Steer, true).await;
    gate.open();
    run.await.unwrap().unwrap();

    let requests = t.llm.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        user_texts(&requests[1]),
        ["Start working", "First steer", "Second steer"]
    );
    t.execution.wake(&t.sid()).await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(
        t.llm.requests().len(),
        2,
        "advisory wake finds nothing eligible"
    );
}

#[tokio::test]
async fn promotes_queued_input_after_continuation_ends() {
    let t = Arc::new(Harness::new().await);
    t.prompt("Start working", Delivery::Steer, false).await;
    t.llm.script(vec![
        calls(vec![tool_call("call-echo", "hello", None)]),
        stop(),
        stop(),
    ]);
    let (started, gate) = t.llm.gate_next();

    let run = tokio::spawn({
        let t = t.clone();
        async move { t.resume().await }
    });
    started.wait().await;
    t.prompt("Wait until continuation ends", Delivery::Queue, true)
        .await;
    gate.open();
    run.await.unwrap().unwrap();

    let requests = t.llm.requests();
    assert_eq!(requests.len(), 3);
    assert_eq!(user_texts(&requests[0]), ["Start working"]);
    assert_eq!(user_texts(&requests[1]), ["Start working"]);
    assert_eq!(
        user_texts(&requests[2]),
        ["Start working", "Wait until continuation ends"]
    );
}

#[tokio::test]
async fn promotes_queued_inputs_one_at_a_time_in_fifo_order() {
    let t = Arc::new(Harness::new().await);
    t.prompt("Start working", Delivery::Steer, false).await;
    t.llm.script(vec![stop(), stop(), stop()]);
    let (started, gate) = t.llm.gate_next();

    let run = tokio::spawn({
        let t = t.clone();
        async move { t.resume().await }
    });
    started.wait().await;
    t.prompt("Queue first", Delivery::Queue, true).await;
    t.prompt("Queue second", Delivery::Queue, true).await;
    gate.open();
    run.await.unwrap().unwrap();

    let requests = t.llm.requests();
    assert_eq!(requests.len(), 3);
    assert_eq!(user_texts(&requests[0]), ["Start working"]);
    assert_eq!(user_texts(&requests[1]), ["Start working", "Queue first"]);
    assert_eq!(
        user_texts(&requests[2]),
        ["Start working", "Queue first", "Queue second"]
    );
}

#[tokio::test]
async fn advisory_wake_promotes_a_lone_queued_input() {
    let t = Harness::new().await;
    t.prompt("Queued", Delivery::Queue, false).await;
    t.llm.script(vec![stop()]);
    t.runner.run(&t.sid(), false).await.unwrap();
    let requests = t.llm.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(user_texts(&requests[0]), ["Queued"]);
}
