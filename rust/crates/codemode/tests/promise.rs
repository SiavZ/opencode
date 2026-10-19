//! Purpose: port of packages/codemode/test/promise.test.ts (first-class promise values).
//! Scope: eager tool calls, await, combinators, race interruption, timeouts, unsupported surface.
//! Uses:
//! rust/crates/
//! |-- codemode/src/lib.rs
//! `-- codemode/tests/common/mod.rs
//! External:
//! |-- serde_json::Value
//! |-- serde_json::json
//! |-- tokio::test
//! `-- tokio::time::sleep
//! Used by:
//! `-- none, entrypoint
//! Invariants: interruption is observed through drop guards on the host futures.
//! Contract tests: self

mod common;

use common::{async_tool, empty_struct, run_with};
use opencode_codemode::{
    DiagnosticKind, ExecutionLimits, ExecutionResult, Options, Schema, ToolTree, tool_error,
};
use serde_json::{Value as Json, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Default)]
struct Trace {
    starts: Vec<i64>,
    active: i64,
    max_active: i64,
    completed: i64,
    interrupted: i64,
}

type Shared = Arc<Mutex<Trace>>;

/// Decrements `active` and counts an interruption unless the call completed.
struct Guard {
    trace: Shared,
    done: bool,
}

impl Drop for Guard {
    fn drop(&mut self) {
        if !self.done
            && let Ok(mut trace) = self.trace.lock()
        {
            trace.active -= 1;
            trace.interrupted += 1;
        }
    }
}

fn tools(trace: &Shared) -> ToolTree {
    let trace = trace.clone();
    let sleepy = async_tool(
        "Echo an id after a delay",
        Schema::object([
            ("id", Schema::number()),
            ("ms", Schema::optional(Schema::number())),
        ]),
        Some(Schema::number().into()),
        move |input: Json, _context| {
            let trace = trace.clone();
            async move {
                let id = input["id"].as_i64().unwrap_or_default();
                let ms = input.get("ms").and_then(Json::as_u64).unwrap_or(20);
                {
                    let mut state = trace.lock().map_err(|_| tool_error("poisoned"))?;
                    state.starts.push(id);
                    state.active += 1;
                    state.max_active = state.max_active.max(state.active);
                }
                let mut guard = Guard {
                    trace: trace.clone(),
                    done: false,
                };
                tokio::time::sleep(Duration::from_millis(ms)).await;
                guard.done = true;
                let mut state = trace.lock().map_err(|_| tool_error("poisoned"))?;
                state.active -= 1;
                state.completed += 1;
                Ok(json!(id))
            }
        },
    );
    let failing = async_tool(
        "Always refuse",
        empty_struct(),
        Some(Schema::string().into()),
        |_, _| async { Err(tool_error("Lookup refused")) },
    );
    ToolTree::new().namespace(
        "host",
        ToolTree::new().tool("sleepy", sleepy).tool("fail", failing),
    )
}

async fn run(code: &str, trace: &Shared, limits: ExecutionLimits) -> ExecutionResult {
    run_with(
        code,
        &Options {
            tools: tools(trace),
            limits,
            ..Options::default()
        },
    )
    .await
}

fn trace() -> Shared {
    Arc::new(Mutex::new(Trace::default()))
}

async fn value(code: &str) -> Json {
    value_traced(code, &trace()).await
}

async fn value_traced(code: &str, trace: &Shared) -> Json {
    let result = run(code, trace, ExecutionLimits::default()).await;
    assert!(result.ok, "expected success, got {:?}", result.error);
    result.value.unwrap_or(Json::Null)
}

async fn error(code: &str) -> opencode_codemode::Diagnostic {
    error_limits(code, ExecutionLimits::default()).await
}

async fn error_limits(code: &str, limits: ExecutionLimits) -> opencode_codemode::Diagnostic {
    let result = run(code, &trace(), limits).await;
    assert!(!result.ok, "expected failure, got {:?}", result.value);
    result.error.expect("diagnostic")
}

fn read(trace: &Shared) -> std::sync::MutexGuard<'_, Trace> {
    trace.lock().expect("trace lock")
}

#[tokio::test]
async fn an_unawaited_tool_call_starts_eagerly_in_call_order() {
    let trace = trace();
    let result = value_traced(
        r#"
        const a = tools.host.sleepy({ id: 1, ms: 40 })
        const b = tools.host.sleepy({ id: 2, ms: 40 })
        const rb = await b
        const ra = await a
        return [ra, rb]
      "#,
        &trace,
    )
    .await;
    assert_eq!(result, json!([1, 2]));
    assert_eq!(read(&trace).starts, vec![1, 2]);
    assert!(read(&trace).max_active > 1);
}

#[tokio::test]
async fn awaiting_the_same_promise_twice_settles_once() {
    let result = run(
        "const p = tools.host.sleepy({ id: 7 })\nconst x = await p\nconst y = await p\nreturn [x, y]",
        &trace(),
        ExecutionLimits::default(),
    )
    .await;
    assert!(result.ok);
    assert_eq!(result.value, Some(json!([7, 7])));
    assert_eq!(result.tool_calls.len(), 1);
    assert_eq!(result.tool_calls[0].name, "host.sleepy");
}

#[tokio::test]
async fn await_of_a_non_promise_is_a_passthrough() {
    assert_eq!(value("return await 42").await, json!(42));
    assert_eq!(value(r#"const x = await "s"; return x"#).await, json!("s"));
    assert_eq!(value("return await null").await, Json::Null);
    assert_eq!(value("return (await [1, 2]).length").await, json!(2));
}

#[tokio::test]
async fn returning_an_unawaited_tool_call_resolves_it() {
    assert_eq!(value("return tools.host.sleepy({ id: 9 })").await, json!(9));
}

#[tokio::test]
async fn typeof_a_promise_is_object_and_console_renders_it() {
    let result = run(
        "const p = Promise.resolve(1)\nconsole.log(p)\nreturn typeof p",
        &trace(),
        ExecutionLimits::default(),
    )
    .await;
    assert!(result.ok);
    assert_eq!(result.value, Some(json!("object")));
    assert_eq!(
        result.logs,
        Some(vec!["[Promise (await it to get its value)]".to_string()])
    );
}

#[tokio::test]
async fn an_awaited_failure_is_catchable() {
    assert_eq!(
        value(
            r#"
      const p = tools.host.fail({})
      try {
        await p
        return "no"
      } catch (e) {
        return e.message
      }
    "#
        )
        .await,
        json!("Lookup refused")
    );
}

#[tokio::test]
async fn a_fire_and_forget_call_completes_before_the_execution_ends() {
    let trace = trace();
    let result = value_traced(
        "tools.host.sleepy({ id: 1, ms: 30 })\nreturn \"done\"",
        &trace,
    )
    .await;
    assert_eq!(result, json!("done"));
    assert_eq!(read(&trace).completed, 1);
    assert_eq!(read(&trace).interrupted, 0);
}

#[tokio::test]
async fn a_never_awaited_failing_call_surfaces_as_unhandled_rejection() {
    let diagnostic = error("tools.host.fail({})\nreturn \"done\"").await;
    assert_eq!(diagnostic.kind, DiagnosticKind::ToolFailure);
    assert!(
        diagnostic
            .message
            .contains("Unhandled rejection from an un-awaited tool call")
    );
    assert!(diagnostic.message.contains("Lookup refused"));
    assert!(
        diagnostic
            .suggestions
            .unwrap_or_default()
            .join(" ")
            .contains("await tools.ns.tool(...)")
    );
}

#[tokio::test]
async fn returning_an_unawaited_promise_inside_data_is_a_diagnostic() {
    let diagnostic = error("return { result: tools.host.sleepy({ id: 1 }) }").await;
    assert_eq!(diagnostic.kind, DiagnosticKind::InvalidDataValue);
    assert!(diagnostic.message.contains("un-awaited Promise"));
    assert!(diagnostic.message.contains("await tools.ns.tool(...)"));
}

#[tokio::test]
async fn passing_an_unawaited_promise_as_a_tool_argument_is_a_diagnostic() {
    let diagnostic =
        error("return await tools.host.sleepy({ id: tools.host.sleepy({ id: 1 }) })").await;
    assert_eq!(diagnostic.kind, DiagnosticKind::InvalidDataValue);
    assert!(diagnostic.message.contains("un-awaited Promise"));
}

#[tokio::test]
async fn json_stringify_of_a_promise_is_a_diagnostic() {
    let diagnostic = error("return JSON.stringify(Promise.resolve(1))").await;
    assert_eq!(diagnostic.kind, DiagnosticKind::InvalidDataValue);
    assert!(diagnostic.message.contains("un-awaited Promise"));
}

#[tokio::test]
async fn operators_reject_promise_operands() {
    assert_eq!(
        error("return Promise.resolve(1) + 1").await.kind,
        DiagnosticKind::InvalidDataValue
    );
}

#[tokio::test]
async fn promise_all_mixes_promises_and_plain_values() {
    assert_eq!(
        value(r#"return await Promise.all([tools.host.sleepy({ id: 1 }), "plain", tools.host.sleepy({ id: 2 }), 42])"#).await,
        json!([1, "plain", 2, 42])
    );
}

#[tokio::test]
async fn promise_all_accepts_prebuilt_arrays_and_spread() {
    assert_eq!(
        value(
            r#"
      const calls = []
      calls.push(tools.host.sleepy({ id: 1 }))
      calls.push(7)
      const more = [tools.host.sleepy({ id: 2 })]
      const batch = [...calls, ...more, "x"]
      return await Promise.all(batch)
    "#
        )
        .await,
        json!([1, 7, 2, "x"])
    );
}

#[tokio::test]
async fn runs_items_map_tool_calls_in_parallel() {
    let trace = trace();
    let result = value_traced(
        "const ids = [1, 2, 3, 4]\nreturn await Promise.all(ids.map((id) => tools.host.sleepy({ id, ms: 40 })))",
        &trace,
    )
    .await;
    assert_eq!(result, json!([1, 2, 3, 4]));
    assert!(read(&trace).max_active > 1);
}

#[tokio::test]
async fn caps_live_tool_call_concurrency_at_eight() {
    let trace = trace();
    let result = value_traced(
        r#"
        const ids = []
        for (let i = 0; i < 20; i += 1) ids.push(i)
        const results = await Promise.all(ids.map((id) => tools.host.sleepy({ id, ms: 10 })))
        return results.length
      "#,
        &trace,
    )
    .await;
    assert_eq!(result, json!(20));
    assert!(read(&trace).max_active > 1);
    assert!(read(&trace).max_active <= 8);
}

#[tokio::test]
async fn promise_all_resolves_the_empty_array() {
    assert_eq!(value("return await Promise.all([])").await, json!([]));
}

#[tokio::test]
async fn promise_all_rejects_with_the_first_failure() {
    assert_eq!(
        value(
            r#"
      try {
        await Promise.all([tools.host.sleepy({ id: 1 }), tools.host.fail({})])
        return "no"
      } catch (e) {
        return e.message
      }
    "#
        )
        .await,
        json!("Lookup refused")
    );
}

#[tokio::test]
async fn promise_all_non_collection_is_a_clear_error() {
    assert!(
        error("return await Promise.all(42)")
            .await
            .message
            .contains("Promise.all expects an array")
    );
}

#[tokio::test]
async fn exceeding_max_tool_calls_inside_promise_all() {
    let diagnostic = error_limits(
        "return await Promise.all([tools.host.sleepy({ id: 1 }), tools.host.sleepy({ id: 2 }), tools.host.sleepy({ id: 3 })])",
        ExecutionLimits { max_tool_calls: Some(2.0), ..ExecutionLimits::default() },
    )
    .await;
    assert_eq!(diagnostic.kind, DiagnosticKind::ToolCallLimitExceeded);
}

#[tokio::test]
async fn all_settled_reports_outcomes_with_normalized_reasons() {
    assert_eq!(
        value(
            r#"
      return await Promise.allSettled([
        tools.host.sleepy({ id: 5 }),
        tools.host.fail({}),
        "plain",
        Promise.reject(new Error("boom")),
      ])
    "#
        )
        .await,
        json!([
            { "status": "fulfilled", "value": 5 },
            { "status": "rejected", "reason": { "name": "Error", "message": "Lookup refused" } },
            { "status": "fulfilled", "value": "plain" },
            { "status": "rejected", "reason": { "name": "Error", "message": "boom" } },
        ])
    );
}

#[tokio::test]
async fn all_settled_never_rejects_for_program_failures() {
    let result = run(
        "const settled = await Promise.allSettled([tools.host.fail({}), tools.host.fail({})])\nreturn settled.filter((s) => s.status === \"rejected\").length",
        &trace(),
        ExecutionLimits::default(),
    )
    .await;
    assert!(result.ok);
    assert_eq!(result.value, Some(json!(2)));
}

#[tokio::test]
async fn race_first_settlement_wins_and_losers_are_interrupted() {
    let trace = trace();
    let result = value_traced(
        r#"
        const fast = tools.host.sleepy({ id: 1, ms: 10 })
        const slow = tools.host.sleepy({ id: 2, ms: 5000 })
        return await Promise.race([fast, slow])
      "#,
        &trace,
    )
    .await;
    assert_eq!(result, json!(1));
    assert_eq!(read(&trace).interrupted, 1);
    assert_eq!(read(&trace).completed, 1);
}

#[tokio::test]
async fn awaiting_an_interrupted_loser_is_catchable() {
    assert_eq!(
        value(
            r#"
      const fast = tools.host.sleepy({ id: 1, ms: 10 })
      const slow = tools.host.sleepy({ id: 2, ms: 5000 })
      const winner = await Promise.race([fast, slow])
      try {
        await slow
        return "no"
      } catch (e) {
        return { winner, caught: e.message }
      }
    "#
        )
        .await,
        json!({ "winner": 1, "caught": "This tool call was interrupted because another value settled a Promise.race first." })
    );
}

#[tokio::test]
async fn a_rejection_can_win_the_race() {
    assert_eq!(
        value(
            r#"
      try {
        await Promise.race([tools.host.fail({}), tools.host.sleepy({ id: 1, ms: 5000 })])
        return "no"
      } catch (e) {
        return e.message
      }
    "#
        )
        .await,
        json!("Lookup refused")
    );
}

#[tokio::test]
async fn a_plain_value_wins_over_pending_promises() {
    let trace = trace();
    assert_eq!(
        value_traced(
            r#"return await Promise.race([tools.host.sleepy({ id: 1, ms: 5000 }), "immediate"])"#,
            &trace
        )
        .await,
        json!("immediate")
    );
    assert_eq!(read(&trace).interrupted, 1);
}

#[tokio::test]
async fn an_empty_race_is_a_clear_error() {
    assert!(
        error("return await Promise.race([])")
            .await
            .message
            .contains("never settle")
    );
}

#[tokio::test]
async fn resolve_wraps_values_and_passes_promises_through() {
    assert_eq!(value("return await Promise.resolve(42)").await, json!(42));
    assert_eq!(
        value(r#"return await Promise.resolve(Promise.resolve("nested"))"#).await,
        json!("nested")
    );
    assert_eq!(
        value("return await Promise.resolve(tools.host.sleepy({ id: 3 }))").await,
        json!(3)
    );
}

#[tokio::test]
async fn reject_produces_a_promise_whose_await_throws() {
    assert_eq!(
        value(r#"try { await Promise.reject("nope"); return "no" } catch (e) { return e }"#).await,
        json!("nope")
    );
}

#[tokio::test]
async fn the_execution_timeout_interrupts_in_flight_calls() {
    let trace = trace();
    let result = run(
        r#"
        const a = tools.host.sleepy({ id: 1, ms: 60000 })
        const b = tools.host.sleepy({ id: 2, ms: 60000 })
        return await a
      "#,
        &trace,
        ExecutionLimits {
            timeout_ms: Some(100.0),
            ..ExecutionLimits::default()
        },
    )
    .await;
    assert!(!result.ok);
    assert_eq!(
        result.error.expect("diagnostic").kind,
        DiagnosticKind::TimeoutExceeded
    );
    assert_eq!(read(&trace).starts, vec![1, 2]);
    assert_eq!(read(&trace).interrupted, 2);
    assert_eq!(read(&trace).completed, 0);
}

#[tokio::test]
async fn the_timeout_interrupts_calls_inside_promise_all() {
    let trace = trace();
    let result = run(
        "return await Promise.all([tools.host.sleepy({ id: 1, ms: 60000 }), tools.host.sleepy({ id: 2, ms: 60000 })])",
        &trace,
        ExecutionLimits { timeout_ms: Some(100.0), ..ExecutionLimits::default() },
    )
    .await;
    assert!(!result.ok);
    assert_eq!(
        result.error.expect("diagnostic").kind,
        DiagnosticKind::TimeoutExceeded
    );
    assert_eq!(read(&trace).interrupted, 2);
}

#[tokio::test]
async fn then_catch_finally_give_an_await_instead_error() {
    for method in ["then", "catch", "finally"] {
        let diagnostic = error(&format!(
            "return tools.host.sleepy({{ id: 1 }}).{method}((x) => x)"
        ))
        .await;
        assert_eq!(diagnostic.kind, DiagnosticKind::UnsupportedSyntax);
        assert!(
            diagnostic
                .message
                .contains(&format!("Promise.prototype.{method} is not supported"))
        );
        assert!(diagnostic.message.contains("await"));
    }
}

#[tokio::test]
async fn other_property_reads_on_a_promise_hint_at_await() {
    let diagnostic = error("return tools.host.sleepy({ id: 1 }).value").await;
    assert_eq!(diagnostic.kind, DiagnosticKind::InvalidDataValue);
    assert!(diagnostic.message.contains("un-awaited Promise"));
    assert!(diagnostic.message.contains("await it first"));
}

#[tokio::test]
async fn unknown_promise_statics_list_what_is_available() {
    let diagnostic = error("return await Promise.any([tools.host.sleepy({ id: 1 })])").await;
    assert!(diagnostic.message.contains("Promise.any is not available"));
    assert!(diagnostic.message.contains("Promise.allSettled"));
}

#[tokio::test]
async fn new_promise_points_at_tool_calls() {
    let diagnostic = error("return new Promise((resolve) => resolve(1))").await;
    assert_eq!(diagnostic.kind, DiagnosticKind::UnsupportedSyntax);
    assert!(
        diagnostic
            .message
            .contains("new Promise(...) is not supported")
    );
    assert!(diagnostic.message.contains("already return promises"));
}
