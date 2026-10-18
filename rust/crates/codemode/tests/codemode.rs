//! Purpose: port of packages/codemode/test/codemode.test.ts (host boundary, observation, console,
//! output budget, schema flexibility, public contract, discovery, limits).
//! Scope: one Rust test per TS `test(...)`. Effect-specific host failure shapes map onto
//! `ToolError` variants: typed failures and defects are `ToolError::Internal`, interruption is
//! `ToolError::Interrupted`, and a host value that throws while copied maps to output that
//! violates the data contract.
//! Uses:
//! rust/crates/
//! |-- codemode/src/lib.rs
//! `-- codemode/tests/common/mod.rs
//! External:
//! |-- serde_json::json
//! `-- tokio_util::sync::CancellationToken
//! Used by:
//! `-- none, entrypoint
//! Invariants: tests drive the real runtime through the public API.
//! Contract tests: self

mod common;

use common::{empty_struct, run, run_with, runtime_value, tool};
use opencode_codemode::{
    ConfigError, Definition, DiagnosticKind, ExecutionLimits, ExecutionResult, Options, Runtime,
    Schema, ToolCallOutcome, ToolError, ToolTree, tool_error,
};
use serde_json::{Value as Json, json};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

fn host(definition: Definition) -> Runtime {
    Runtime::new(Options {
        tools: ToolTree::new().namespace("host", ToolTree::new().tool("call", definition)),
        ..Options::default()
    })
    .expect("runtime")
}

async fn run_host(definition: Definition) -> ExecutionResult {
    runtime_value(&host(definition), "return await tools.host.call({})").await
}

fn diagnostic(result: &ExecutionResult) -> (DiagnosticKind, String) {
    let error = result.error.clone().expect("diagnostic");
    (error.kind, error.message)
}

fn round_trip(result: &ExecutionResult) {
    let text = serde_json::to_string(result).expect("serialize");
    let back: ExecutionResult = serde_json::from_str(&text).expect("deserialize");
    assert_eq!(&back, result);
}

#[tokio::test]
async fn preserves_explicit_safe_tool_failures() {
    let result = run_host(tool(
        "Fail safely",
        empty_struct(),
        Some(Schema::string().into()),
        |_| Err(tool_error("Authorized request was refused")),
    ))
    .await;
    assert_eq!(
        diagnostic(&result),
        (
            DiagnosticKind::ToolFailure,
            "Authorized request was refused".into()
        )
    );
    assert_eq!(result.error.as_ref().and_then(|error| error.location), None);
}

#[tokio::test]
async fn sanitizes_unknown_host_failures_and_defects() {
    for secret in [
        "Authorization: Bearer typed-secret",
        "postgres://user:defect-secret@example.invalid",
    ] {
        let result = run_host(tool(
            "Fail internally",
            empty_struct(),
            Some(Schema::string().into()),
            move |_| Err(ToolError::Internal(secret.into())),
        ))
        .await;
        assert_eq!(
            diagnostic(&result),
            (DiagnosticKind::ToolFailure, "Tool execution failed".into())
        );
        let text = serde_json::to_string(&result).expect("serialize");
        assert!(
            !text.contains("typed-secret")
                && !text.contains("defect-secret")
                && !text.contains("Authorization: Bearer")
        );
    }
}

#[tokio::test]
async fn sanitizes_invalid_host_output() {
    let result = run_host(tool(
        "Return invalid output",
        empty_struct(),
        Some(Schema::object([("safe", Schema::string())]).into()),
        |_| Ok(json!({ "safe": 1, "secret": "invalid-output-secret" })),
    ))
    .await;
    assert_eq!(
        diagnostic(&result),
        (
            DiagnosticKind::InvalidToolOutput,
            "Invalid output from tool 'host.call'.".into()
        )
    );
    assert!(
        !serde_json::to_string(&result)
            .expect("serialize")
            .contains("invalid-output-secret")
    );
}

#[tokio::test]
async fn sanitizes_host_output_that_fails_while_being_copied() {
    let result = run_host(tool(
        "Return hostile output",
        empty_struct(),
        Some(Schema::unknown().into()),
        |_| Ok(json!({ "__proto__": { "polluted": "host-output-secret" } })),
    ))
    .await;
    assert_eq!(
        diagnostic(&result),
        (
            DiagnosticKind::InvalidToolOutput,
            "Invalid output from tool 'host.call'.".into()
        )
    );
    assert!(
        !serde_json::to_string(&result)
            .expect("serialize")
            .contains("host-output-secret")
    );
}

#[tokio::test]
async fn caught_tool_failures_are_error_values() {
    let runtime = host(tool(
        "Refuse",
        empty_struct(),
        Some(Schema::string().into()),
        |_| Err(tool_error("Refused")),
    ));
    let result = runtime_value(
        &runtime,
        r#"
        try {
          await tools.host.call({})
          return "no"
        } catch (e) {
          return { isError: e instanceof Error, message: e.message }
        }
      "#,
    )
    .await;
    assert_eq!(
        result.value,
        Some(json!({ "isError": true, "message": "Refused" }))
    );
}

#[tokio::test]
async fn propagates_host_interruption() {
    let runtime = host(tool(
        "Interrupt",
        empty_struct(),
        Some(Schema::string().into()),
        |_| Err(ToolError::Interrupted),
    ));
    let outcome = runtime
        .execute("return await tools.host.call({})", CancellationToken::new())
        .await;
    assert_eq!(outcome, Err(opencode_codemode::Interrupted));
}

fn lookup() -> Definition {
    tool(
        "Look up a value",
        Schema::object([("query", Schema::string())]),
        Some(Schema::string().into()),
        |input| {
            if input["query"] == "boom" {
                Err(tool_error("Lookup refused"))
            } else {
                Ok(input["query"].clone())
            }
        },
    )
}

#[tokio::test]
async fn reports_the_tools_actually_invoked() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let sink = calls.clone();
    let options = Options {
        tools: ToolTree::new().namespace("context", ToolTree::new().tool("lookup", lookup())),
        on_tool_call_start: Some(Arc::new(move |call| {
            sink.lock().expect("lock").push(call.clone())
        })),
        ..Options::default()
    };
    let result = run_with(
        "if (false) await tools.context.lookup({ query: \"not called\" })\nreturn await tools.context.lookup({ query: \"deployment failure\" })",
        &options,
    )
    .await;
    assert!(result.ok);
    let calls = calls.lock().expect("lock");
    assert_eq!(calls.len(), 1);
    assert_eq!(
        (calls[0].index, calls[0].name.as_str(), &calls[0].input),
        (
            0,
            "context.lookup",
            &json!({ "query": "deployment failure" })
        )
    );
}

#[tokio::test]
async fn observes_settled_calls_with_outcome_and_duration() {
    let events = Arc::new(Mutex::new(Vec::<Json>::new()));
    let (start_sink, end_sink) = (events.clone(), events.clone());
    let runtime = Runtime::new(Options {
        tools: ToolTree::new().namespace("context", ToolTree::new().tool("lookup", lookup())),
        on_tool_call_start: Some(Arc::new(move |call| {
            start_sink.lock().expect("lock").push(json!({ "phase": "start", "index": call.index, "name": call.name }));
        })),
        on_tool_call_end: Some(Arc::new(move |call| {
            assert!(call.duration_ms >= 0.0);
            let mut event = json!({
                "phase": "end",
                "index": call.index,
                "name": call.name,
                "outcome": if call.outcome == ToolCallOutcome::Success { "success" } else { "failure" },
            });
            if let Some(message) = &call.message {
                event["message"] = json!(message);
            }
            end_sink.lock().expect("lock").push(event);
        })),
        ..Options::default()
    })
    .expect("runtime");
    assert!(
        runtime_value(
            &runtime,
            r#"return await tools.context.lookup({ query: "ok" })"#
        )
        .await
        .ok
    );
    assert!(
        !runtime_value(
            &runtime,
            r#"return await tools.context.lookup({ query: "boom" })"#
        )
        .await
        .ok
    );
    assert_eq!(
        *events.lock().expect("lock"),
        vec![
            json!({ "phase": "start", "index": 0, "name": "context.lookup" }),
            json!({ "phase": "end", "index": 0, "name": "context.lookup", "outcome": "success" }),
            json!({ "phase": "start", "index": 0, "name": "context.lookup" }),
            json!({ "phase": "end", "index": 0, "name": "context.lookup", "outcome": "failure", "message": "Lookup refused" }),
        ]
    );
}

// console capture
#[tokio::test]
async fn captures_console_output_as_logs() {
    let result = run(r#"
        const returned = console.log("Thread info:", { name: "Demo", count: 2 })
        console.warn("careful")
        return returned
      "#)
    .await;
    assert!(result.ok);
    assert_eq!(result.value, Some(Json::Null));
    assert_eq!(
        result.logs,
        Some(vec![
            r#"Thread info: {"name":"Demo","count":2}"#.to_string(),
            "[warn] careful".to_string()
        ])
    );
    assert!(result.tool_calls.is_empty());
    round_trip(&result);
}

#[tokio::test]
async fn keeps_logs_captured_before_failures() {
    let result = run("console.log(\"before failure\")\nthrow new Error(\"boom\")").await;
    assert_eq!(result.logs, Some(vec!["before failure".to_string()]));
    assert_eq!(diagnostic(&result).1, "Uncaught: boom");
}

#[tokio::test]
async fn prints_nan_and_infinity_literally() {
    let result = run("console.log(NaN)\nconsole.log(Infinity, -Infinity)\nconsole.log({ ratio: NaN, bounds: [Infinity] })\nreturn null").await;
    assert!(result.ok);
    assert_eq!(
        result.logs,
        Some(vec![
            "NaN".into(),
            "Infinity -Infinity".into(),
            r#"{"ratio":NaN,"bounds":[Infinity]}"#.into()
        ])
    );
}

#[tokio::test]
async fn renders_sandbox_values_nested_in_logged_containers() {
    let result = run(
        "console.log({ m: new Map([[\"a\", 1]]), when: new Date(0), r: /ab/g, s: new Set([1, 2]) })\nconsole.log([new Date(0)])\nreturn null",
    )
    .await;
    assert!(result.ok);
    assert_eq!(
        result.logs,
        Some(vec![
            r#"{"m":Map(1) [["a",1]],"when":1970-01-01T00:00:00.000Z,"r":/ab/g,"s":Set(2) [1,2]}"#
                .into(),
            "[1970-01-01T00:00:00.000Z]".into(),
        ])
    );
}

#[tokio::test]
async fn console_formatting_is_total() {
    let result = run(
        "const m = new Map()\nm.set(\"self\", m)\nconsole.log({ box: m })\nconsole.log({ fn: (x) => x, ok: 1 })\nreturn null",
    )
    .await;
    assert!(result.ok);
    assert_eq!(
        result.logs,
        Some(vec![
            r#"{"box":Map(1) [["self",[Circular]]]}"#.into(),
            r#"{"fn":[CodeMode reference],"ok":1}"#.into()
        ])
    );
}

#[tokio::test]
async fn console_table_renders_sandbox_value_cells() {
    let result = run("console.table([{ when: new Date(0), n: NaN }])\nreturn null").await;
    assert!(result.ok);
    assert_eq!(
        result.logs,
        Some(vec![
            "(index)\twhen\tn\n0\t1970-01-01T00:00:00.000Z\tNaN".into()
        ])
    );
}

#[tokio::test]
async fn captures_console_dir_and_table() {
    let result = run(r#"
        console.dir({ nested: { ok: true } })
        console.table([
          { name: "Kit", count: 1, hidden: "x" },
          { name: "Olive", count: 2, hidden: "y" }
        ], ["name", "count"])
        return "done"
      "#)
    .await;
    assert_eq!(result.value, Some(json!("done")));
    assert_eq!(
        result.logs,
        Some(vec![
            r#"{"nested":{"ok":true}}"#.into(),
            "(index)\tname\tcount\n0\tKit\t1\n1\tOlive\t2".into()
        ])
    );
    assert!(result.tool_calls.is_empty());
}

// output budget
#[tokio::test]
async fn absent_max_output_bytes_means_no_truncation() {
    let result = run(r#"console.log("z".repeat(50_000)); return "x".repeat(100_000)"#).await;
    assert!(result.ok);
    assert_eq!(result.truncated, None);
    assert_eq!(result.value, Some(json!("x".repeat(100_000))));
    assert_eq!(result.logs, Some(vec!["z".repeat(50_000)]));
}

fn limited(max_output_bytes: f64) -> Options {
    Options {
        limits: ExecutionLimits {
            max_output_bytes: Some(max_output_bytes),
            ..ExecutionLimits::default()
        },
        ..Options::default()
    }
}

#[tokio::test]
async fn truncates_an_oversized_result_with_a_marker() {
    let result = run_with(
        &format!(r#"return {{ data: "{}" }}"#, "x".repeat(200)),
        &limited(40.0),
    )
    .await;
    assert!(result.ok);
    assert_eq!(result.truncated, Some(true));
    let text = result
        .value
        .as_ref()
        .and_then(Json::as_str)
        .expect("string value")
        .to_string();
    let rest = text.strip_prefix(r#"{"data":""#).expect("prefix");
    let (xs, marker) = rest.split_at(rest.find(' ').expect("marker"));
    assert!(!xs.is_empty() && xs.chars().all(|ch| ch == 'x'));
    let marker = marker
        .strip_prefix(" [result truncated: ")
        .expect("marker start");
    let (bytes, tail) = marker.split_at(marker.find(' ').expect("bytes"));
    assert!(bytes.parse::<usize>().is_ok());
    assert_eq!(
        tail,
        " bytes exceeds the 40-byte output limit; return a smaller value]"
    );
    round_trip(&result);
}

#[tokio::test]
async fn keeps_leading_logs_within_budget_and_marks_the_cut() {
    let result = run_with(
        &format!(
            "console.log(\"first line\")\nconsole.log(\"{}\")\nreturn \"ok\"",
            "y".repeat(200)
        ),
        &limited(40.0),
    )
    .await;
    assert_eq!(result.value, Some(json!("ok")));
    assert_eq!(result.truncated, Some(true));
    assert_eq!(
        result.logs,
        Some(vec![
            "first line".into(),
            "[logs truncated: showing 1 of 2 lines]".into()
        ])
    );
}

#[tokio::test]
async fn does_not_mark_results_within_the_budget() {
    let result = run("console.log(\"fits\")\nreturn { fits: true }").await;
    assert_eq!(
        result,
        ExecutionResult {
            ok: true,
            value: Some(json!({ "fits": true })),
            error: None,
            logs: Some(vec!["fits".into()]),
            truncated: None,
            tool_calls: Vec::new(),
        }
    );
}

// schema flexibility
#[tokio::test]
async fn accepts_render_only_json_schema_input_and_omitted_output() {
    let observed = Arc::new(Mutex::new(Vec::new()));
    let sink = observed.clone();
    let call = tool(
        "Call an adapter-described tool",
        json!({ "type": "object", "properties": { "id": { "type": "string" }, "count": { "type": "number" } }, "required": ["id"] }),
        None,
        move |input| {
            sink.lock().expect("lock").push(input.clone());
            Ok(json!({ "echoed": input }))
        },
    );
    let runtime = Runtime::new(Options {
        tools: ToolTree::new().namespace("adapter", ToolTree::new().tool("call", call)),
        ..Options::default()
    })
    .expect("runtime");
    assert_eq!(runtime.catalog().len(), 1);
    assert_eq!(runtime.catalog()[0].path, "adapter.call");
    assert_eq!(
        runtime.catalog()[0].description,
        "Call an adapter-described tool"
    );
    assert_eq!(
        runtime.catalog()[0].signature,
        "tools.adapter.call(input: {\n  id: string,\n  count?: number,\n}): Promise<unknown>"
    );
    let result = runtime_value(&runtime, "return await tools.adapter.call({ id: 42 })").await;
    assert_eq!(result.value, Some(json!({ "echoed": { "id": 42 } })));
    assert_eq!(*observed.lock().expect("lock"), vec![json!({ "id": 42 })]);
}

#[tokio::test]
async fn renders_json_schema_outputs_and_defs_refs() {
    let lookup = tool(
        "Look up a user",
        json!({ "type": "object", "properties": { "login": { "type": "string" } }, "required": ["login"] }),
        Some(
            json!({
                "$ref": "#/$defs/User",
                "$defs": { "User": { "type": "object", "properties": { "login": { "type": "string" }, "id": { "type": "number" } }, "required": ["login", "id"] } },
            })
            .into(),
        ),
        |_| Ok(json!({ "login": "kit", "id": 7 })),
    );
    let runtime = Runtime::new(Options {
        tools: ToolTree::new().namespace("users", ToolTree::new().tool("lookup", lookup)),
        ..Options::default()
    })
    .expect("runtime");
    assert_eq!(
        runtime.catalog()[0].signature,
        "tools.users.lookup(input: {\n  login: string,\n}): Promise<{\n  login: string,\n  id: number,\n}>"
    );
    let result = runtime_value(
        &runtime,
        r#"return await tools.users.lookup({ login: "kit" })"#,
    )
    .await;
    assert_eq!(result.value, Some(json!({ "login": "kit", "id": 7 })));
}

#[tokio::test]
async fn omitted_output_renders_unknown() {
    let ping = tool(
        "Ping",
        Schema::object([("host", Schema::string())]),
        None,
        |_| Ok(json!("pong")),
    );
    let runtime = Runtime::new(Options {
        tools: ToolTree::new().namespace("net", ToolTree::new().tool("ping", ping)),
        ..Options::default()
    })
    .expect("runtime");
    assert_eq!(
        runtime.catalog()[0].signature,
        "tools.net.ping(input: {\n  host: string,\n}): Promise<unknown>"
    );
    assert_eq!(
        runtime_value(
            &runtime,
            r#"return await tools.net.ping({ host: "example.test" })"#
        )
        .await
        .value,
        Some(json!("pong"))
    );
}

// public contract
fn order_lookup() -> Definition {
    tool(
        "Look up an order by ID",
        Schema::object([("id", Schema::string())]),
        Some(Schema::object([("id", Schema::string()), ("status", Schema::string())]).into()),
        |input| Ok(json!({ "id": input["id"], "status": "open" })),
    )
}

fn orders() -> ToolTree {
    ToolTree::new().namespace("orders", ToolTree::new().tool("lookup", order_lookup()))
}

const SOURCE: &str = r#"return await tools.orders.lookup({ id: "order_42" })"#;
const ORDER_SIGNATURE: &str = "tools.orders.lookup(input: {\n  id: string,\n}): Promise<{\n  id: string,\n  status: string,\n}>";

fn orders_runtime(budget: Option<f64>) -> Runtime {
    Runtime::new(Options {
        tools: orders(),
        catalog_budget: budget,
        ..Options::default()
    })
    .expect("runtime")
}

#[tokio::test]
async fn keeps_one_shot_and_reusable_execution_equivalent() {
    let one_shot = run_with(
        SOURCE,
        &Options {
            tools: orders(),
            ..Options::default()
        },
    )
    .await;
    let reusable = runtime_value(&orders_runtime(None), SOURCE).await;
    assert_eq!(reusable, one_shot);
    round_trip(&reusable);
}

#[tokio::test]
async fn inlines_a_complete_small_catalog() {
    let runtime = orders_runtime(None);
    assert_eq!(runtime.catalog()[0].signature, ORDER_SIGNATURE);
    let instructions = runtime.instructions();
    assert!(instructions.contains("Available tools (COMPLETE list"));
    assert!(instructions.contains("- orders (1 tool)"));
    assert!(instructions.contains(&format!("  - {ORDER_SIGNATURE} // Look up an order by ID")));
    assert!(!instructions.contains("$codemode"));
    let result = runtime_value(
        &runtime,
        r#"return await tools.$codemode.search({ query: "order" })"#,
    )
    .await;
    assert_eq!(
        result.value,
        Some(json!({
            "items": [{ "path": "tools.orders.lookup", "description": "Look up an order by ID", "signature": ORDER_SIGNATURE }],
            "remaining": 0,
            "next": null,
        }))
    );
}

#[tokio::test]
async fn renders_bracket_notation_for_non_identifier_tool_names() {
    let resolve = tool(
        "Resolve a library ID",
        Schema::object([("libraryName", Schema::string())]),
        Some(Schema::string().into()),
        |input| {
            Ok(json!(format!(
                "/resolved/{}",
                input["libraryName"].as_str().unwrap_or_default()
            )))
        },
    );
    let runtime = Runtime::new(Options {
        tools: ToolTree::new().namespace(
            "context7",
            ToolTree::new().tool("resolve-library-id", resolve),
        ),
        ..Options::default()
    })
    .expect("runtime");
    let signature = "tools.context7[\"resolve-library-id\"](input: {\n  libraryName: string,\n}): Promise<string>";
    assert_eq!(runtime.catalog()[0].path, "context7.resolve-library-id");
    assert_eq!(runtime.catalog()[0].signature, signature);
    assert!(runtime.instructions().contains(signature));
    let search = runtime_value(
        &runtime,
        r#"return await tools.$codemode.search({ query: "resolve library id" })"#,
    )
    .await;
    assert_eq!(
        search.value,
        Some(json!({
            "items": [{ "path": "tools.context7[\"resolve-library-id\"]", "description": "Resolve a library ID", "signature": signature }],
            "remaining": 0,
            "next": null,
        }))
    );
    let call = runtime_value(
        &runtime,
        r#"return await tools.context7["resolve-library-id"]({ libraryName: "TypeScript" })"#,
    )
    .await;
    assert_eq!(call.value, Some(json!("/resolved/TypeScript")));
    let exact = runtime_value(
        &runtime,
        r#"return await tools.$codemode.search({ query: 'tools.context7["resolve-library-id"]' })"#,
    )
    .await;
    let value = exact.value.expect("value");
    assert_eq!(
        (value["remaining"].clone(), value["next"].clone()),
        (json!(0), Json::Null)
    );
}

#[test]
fn instructions_use_markdown_sections_with_placeholders() {
    let instructions = orders_runtime(None).instructions().to_string();
    let position = |needle: &str| instructions.find(needle).expect(needle);
    assert!(position("## Workflow") < position("## Rules"));
    assert!(position("## Rules") < position("## Language"));
    assert!(position("## Language") < position("\n## Available tools (COMPLETE list"));
    for expected in [
        "Return only the fields you need",
        "avoid returning large raw payloads",
        "Do not infer or normalize tool names",
        "bracket notation and quotes are part of the path",
        "surrounding agent tools are not available",
        "Only Code Mode tools listed here and internal runtime tools",
        "`const result = await tools.<namespace>.<tool>(input)`",
        "Return only the fields you need from structured results",
        "check that it is a non-null object and not an array",
        "1. Pick a tool from the list under `## Available tools`",
    ] {
        assert!(instructions.contains(expected), "{expected}");
    }
    for forbidden in [
        "JSON.parse(res)",
        "result.<field>",
        "data.<field>",
        "total_count",
        "list_issues",
        "tools.orders.lookup({",
        "Browse one namespace",
    ] {
        assert!(!instructions.contains(forbidden), "{forbidden}");
    }
    let partial = orders_runtime(Some(0.0)).instructions().to_string();
    for expected in [
        "1. If needed, discover tools: `return await tools.$codemode.search({ query: \"<intent + key nouns>\" })`.",
        "In the next execution, copy a returned path exactly",
        "Only Code Mode tools listed here or returned by `tools.$codemode.search` and internal runtime tools",
        "- Browse one namespace: `await tools.$codemode.search({ query: \"\", namespace: \"<name>\" })`.",
        "repeat the same search with `offset: next.offset`",
        "  limit?: number,\n  offset?: number,",
    ] {
        assert!(partial.contains(expected), "{expected}");
    }
    assert!(!partial.contains("total_count"));
    assert!(!partial.contains("tools.orders.lookup({"));
}

#[test]
fn language_section_does_not_overclaim() {
    let instructions = orders_runtime(None).instructions().to_string();
    assert!(instructions.contains("restricted JavaScript language for calling tools"));
    assert!(instructions.contains("not a general-purpose runtime"));
    assert!(!instructions.contains("Standard modern JavaScript works"));
    assert!(!instructions.contains("TypeScript type annotations"));
    for missing in [
        "Modules/imports",
        "classes",
        "generators",
        "fetch",
        "promise chaining",
    ] {
        assert!(instructions.contains(missing));
    }
    assert!(instructions.contains("URL, URLSearchParams, and URI encoding helpers"));
    assert!(!instructions.contains("host globals"));
    assert!(instructions.contains("Use Code Mode tools for external operations"));
    assert!(instructions.contains(
        "Dates and URLs serialize to strings at data boundaries; Map/Set/RegExp/URLSearchParams serialize to `{}`."
    ));
}

#[test]
fn zero_tools_keep_minimal_sections() {
    let runtime = Runtime::new(Options::default()).expect("runtime");
    let instructions = runtime.instructions();
    assert!(instructions.contains("No tools are currently available."));
    assert!(instructions.contains("## Language"));
    assert!(instructions.contains("## Available tools"));
    assert!(!instructions.contains("## Workflow"));
    assert!(!instructions.contains("## Rules"));
    assert!(!instructions.contains("$codemode"));
}

#[tokio::test]
async fn uses_one_ranked_search_for_large_catalogs() {
    let sent = || Some(Schema::object([("sent", Schema::boolean())]).into());
    let upload = tool(
        "Upload one readable local file to the current Discord thread",
        Schema::object([("path", Schema::string())]),
        sent(),
        |_| Ok(json!({ "sent": true })),
    );
    let generate = tool(
        "Generate an image and upload it to the current Discord thread",
        Schema::object([("prompt", Schema::string())]),
        sent(),
        |_| Ok(json!({ "sent": true })),
    );
    let runtime = Runtime::new(Options {
        tools: ToolTree::new()
            .namespace(
                "thread",
                ToolTree::new()
                    .tool("uploadFile", upload)
                    .tool("generateImage", generate),
            )
            .namespace("orders", ToolTree::new().tool("lookup", order_lookup())),
        catalog_budget: Some(0.0),
        ..Options::default()
    })
    .expect("runtime");
    let instructions = runtime.instructions();
    assert!(instructions.contains(
        "Available tools (PARTIAL - 0 of 3 shown; find the rest with tools.$codemode.search)"
    ));
    assert!(instructions.contains("- thread (2 tools, none shown)"));
    assert!(instructions.contains("- orders (1 tool, none shown)"));
    assert!(instructions.contains("$codemode.search"));
    assert!(!instructions.contains("tools.thread.uploadFile(input"));
    let result = runtime_value(
        &runtime,
        r#"return await tools.$codemode.search({ query: "send message attachment upload file to current Discord thread", limit: 2 })"#,
    )
    .await;
    assert_eq!(
        result.value,
        Some(json!({
            "items": [
                {
                    "path": "tools.thread.uploadFile",
                    "description": "Upload one readable local file to the current Discord thread",
                    "signature": "tools.thread.uploadFile(input: {\n  path: string,\n}): Promise<{\n  sent: boolean,\n}>",
                },
                {
                    "path": "tools.thread.generateImage",
                    "description": "Generate an image and upload it to the current Discord thread",
                    "signature": "tools.thread.generateImage(input: {\n  prompt: string,\n}): Promise<{\n  sent: boolean,\n}>",
                },
            ],
            "remaining": 0,
            "next": null,
        }))
    );
    assert_eq!(
        result
            .tool_calls
            .iter()
            .map(|call| call.name.as_str())
            .collect::<Vec<_>>(),
        vec!["$codemode.search"]
    );
    let variants = runtime_value(
        &runtime,
        r#"return await Promise.all([tools.$codemode.search({ query: "file" }), tools.$codemode.search({ query: "image" })])"#,
    )
    .await;
    let values = variants.value.expect("value");
    assert_eq!(
        values[0]["items"][0]["path"],
        json!("tools.thread.uploadFile")
    );
    assert_eq!(
        values[1]["items"][0]["path"],
        json!("tools.thread.generateImage")
    );
    let removed = runtime_value(
        &runtime,
        r#"return await tools.$codemode.describe({ path: "thread.uploadFile" })"#,
    )
    .await;
    assert_eq!(diagnostic(&removed).0, DiagnosticKind::UnknownTool);
}

#[tokio::test]
async fn search_defaults_to_ten_results_and_resolves_exact_paths() {
    let many = (0..14).fold(ToolTree::new(), |tree, index| {
        tree.tool(
            format!("tool{index}"),
            tool(
                &format!("Numbered tool {index}"),
                Schema::object([("id", Schema::string())]),
                Some(Schema::string().into()),
                |_| Ok(json!("ok")),
            ),
        )
    });
    let runtime = Runtime::new(Options {
        tools: ToolTree::new().namespace("many", many),
        ..Options::default()
    })
    .expect("runtime");
    let browse = runtime_value(&runtime, "return await tools.$codemode.search({})")
        .await
        .value
        .expect("value");
    assert_eq!(browse["items"].as_array().map(Vec::len), Some(10));
    assert_eq!(browse["remaining"], json!(4));
    assert_eq!(browse["next"], json!({ "offset": 10 }));
    for query in ["many.tool13", "tools.many.tool13"] {
        let exact = runtime_value(
            &runtime,
            &format!(
                "return await tools.$codemode.search({{ query: {} }})",
                json!(query)
            ),
        )
        .await;
        assert_eq!(
            exact.value,
            Some(json!({
                "items": [{ "path": "tools.many.tool13", "description": "Numbered tool 13", "signature": "tools.many.tool13(input: {\n  id: string,\n}): Promise<string>" }],
                "remaining": 0,
                "next": null,
            }))
        );
    }
}

fn simple(description: &str) -> Definition {
    tool(
        description,
        Schema::object([("id", Schema::string())]),
        Some(Schema::string().into()),
        |_| Ok(json!("ok")),
    )
}

fn paths(value: &Json) -> Vec<String> {
    value["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| item["path"].as_str().unwrap_or_default().to_string())
        .collect()
}

#[tokio::test]
async fn scopes_search_to_one_namespace() {
    let runtime = Runtime::new(Options {
        tools: ToolTree::new()
            .namespace(
                "github",
                ToolTree::new()
                    .tool("list_issues", simple("List issues"))
                    .tool("create_issue", simple("Create an issue")),
            )
            .namespace(
                "linear",
                ToolTree::new().tool("list_issues", simple("List Linear issues")),
            ),
        ..Options::default()
    })
    .expect("runtime");
    let browse = runtime_value(
        &runtime,
        r#"return await tools.$codemode.search({ query: "", namespace: "github" })"#,
    )
    .await
    .value
    .expect("value");
    assert_eq!(browse["remaining"], json!(0));
    assert_eq!(
        paths(&browse),
        vec!["tools.github.create_issue", "tools.github.list_issues"]
    );
    let scoped = runtime_value(
        &runtime,
        r#"return await tools.$codemode.search({ query: "issues", namespace: "linear" })"#,
    )
    .await
    .value
    .expect("value");
    assert_eq!(scoped["remaining"], json!(0));
    assert_eq!(paths(&scoped)[0], "tools.linear.list_issues");
    let invalid = runtime_value(
        &runtime,
        r#"return await tools.$codemode.search({ query: "issues", namespace: 7 })"#,
    )
    .await;
    assert_eq!(diagnostic(&invalid).0, DiagnosticKind::InvalidToolInput);
}

#[tokio::test]
async fn matches_input_parameter_names_and_substrings() {
    let upload = tool(
        "Send a document to the workspace",
        json!({ "type": "object", "properties": { "attachment": { "type": "string", "description": "Local path of the payload to send" } }, "required": ["attachment"] }),
        None,
        |_| Ok(json!("ok")),
    );
    let other = tool(
        "Rename the workspace",
        Schema::object([("name", Schema::string())]),
        Some(Schema::string().into()),
        |_| Ok(json!("ok")),
    );
    let runtime = Runtime::new(Options {
        tools: ToolTree::new().namespace(
            "files",
            ToolTree::new().tool("upload", upload).tool("other", other),
        ),
        ..Options::default()
    })
    .expect("runtime");
    for query in ["attachment", "docum"] {
        let value = runtime_value(
            &runtime,
            &format!("return await tools.$codemode.search({{ query: \"{query}\" }})"),
        )
        .await
        .value
        .expect("value");
        assert_eq!(value["remaining"], json!(0));
        assert_eq!(paths(&value)[0], "tools.files.upload");
    }
}

#[tokio::test]
async fn a_plural_query_term_matches_singular_text() {
    let runtime = Runtime::new(Options {
        tools: ToolTree::new()
            .namespace(
                "tracker",
                ToolTree::new().tool("fetch_all", simple("Fetch every open issue in the project")),
            )
            .namespace(
                "github",
                ToolTree::new().tool("list_issues", simple("List issues")),
            )
            .namespace(
                "misc",
                ToolTree::new().tool("rename", simple("Rename the workspace")),
            ),
        ..Options::default()
    })
    .expect("runtime");
    let plural = runtime_value(
        &runtime,
        r#"return await tools.$codemode.search({ query: "issues", namespace: "tracker" })"#,
    )
    .await
    .value
    .expect("value");
    assert_eq!(plural["remaining"], json!(0));
    assert_eq!(paths(&plural)[0], "tools.tracker.fetch_all");
    let ranked = runtime_value(
        &runtime,
        r#"return await tools.$codemode.search({ query: "issues" })"#,
    )
    .await
    .value
    .expect("value");
    assert_eq!(ranked["remaining"], json!(0));
    assert_eq!(
        paths(&ranked),
        vec!["tools.github.list_issues", "tools.tracker.fetch_all"]
    );
}

#[tokio::test]
async fn empty_query_lists_everything_alphabetically() {
    let plain = |description: &str| {
        tool(
            description,
            empty_struct(),
            Some(Schema::string().into()),
            |_| Ok(json!("ok")),
        )
    };
    let runtime = Runtime::new(Options {
        tools: ToolTree::new()
            .namespace("zeta", ToolTree::new().tool("last", plain("Last")))
            .namespace(
                "alpha",
                ToolTree::new()
                    .tool("beta", plain("Middle"))
                    .tool("aardvark", plain("First")),
            ),
        ..Options::default()
    })
    .expect("runtime");
    let browse = runtime_value(&runtime, "return await tools.$codemode.search({})")
        .await
        .value
        .expect("value");
    assert_eq!(
        paths(&browse),
        vec![
            "tools.alpha.aardvark",
            "tools.alpha.beta",
            "tools.zeta.last"
        ]
    );
    assert_eq!(
        (browse["remaining"].clone(), browse["next"].clone()),
        (json!(0), Json::Null)
    );
    let middle = runtime_value(
        &runtime,
        "return await tools.$codemode.search({ limit: 1, offset: 1 })",
    )
    .await
    .value
    .expect("value");
    assert_eq!(paths(&middle), vec!["tools.alpha.beta"]);
    assert_eq!(
        (middle["remaining"].clone(), middle["next"].clone()),
        (json!(1), json!({ "offset": 2 }))
    );
    let exhausted = runtime_value(
        &runtime,
        "return await tools.$codemode.search({ limit: 1, offset: 3 })",
    )
    .await;
    assert_eq!(
        exhausted.value,
        Some(json!({ "items": [], "remaining": 0, "next": null }))
    );
}

#[test]
fn inlines_round_robin_across_namespaces() {
    let cheap = || {
        tool(
            "Cheap",
            Schema::object([("q", Schema::string())]),
            Some(Schema::string().into()),
            |_| Ok(json!("ok")),
        )
    };
    let expensive = tool(
        "An expensive tool whose description alone consumes far more than the remaining inline catalog byte budget for this runtime",
        Schema::object([
            ("someRatherLongParameterName", Schema::string()),
            ("anotherEvenLongerParameterName", Schema::number()),
        ]),
        Some(Schema::string().into()),
        |_| Ok(json!("ok")),
    );
    let runtime = Runtime::new(Options {
        tools: ToolTree::new()
            .namespace(
                "alpha",
                ToolTree::new()
                    .tool("cheap", cheap())
                    .tool("expensive", expensive),
            )
            .namespace("beta", ToolTree::new().tool("cheap", cheap())),
        catalog_budget: Some(40.0),
        ..Options::default()
    })
    .expect("runtime");
    let instructions = runtime.instructions();
    assert!(instructions.contains(
        "Available tools (PARTIAL - 2 of 3 shown; find the rest with tools.$codemode.search)"
    ));
    assert!(instructions.contains("- alpha (2 tools, 1 shown)"));
    assert!(
        instructions
            .contains("  - tools.alpha.cheap(input: {\n  q: string,\n}): Promise<string> // Cheap")
    );
    assert!(!instructions.contains("tools.alpha.expensive("));
    assert!(instructions.contains("- beta (1 tool)"));
    assert!(
        instructions
            .contains("  - tools.beta.cheap(input: {\n  q: string,\n}): Promise<string> // Cheap")
    );
    assert!(instructions.contains("$codemode.search"));
}

#[test]
fn charges_inline_jsdoc_against_the_budget() {
    let documented = tool(
        "Look up a record",
        json!({ "type": "object", "properties": { "id": { "type": "string", "description": "A detailed identifier description. ".repeat(20) } }, "required": ["id"] }),
        None,
        |_| Ok(json!("ok")),
    );
    let runtime = Runtime::new(Options {
        tools: ToolTree::new().namespace("records", ToolTree::new().tool("lookup", documented)),
        catalog_budget: Some(40.0),
        ..Options::default()
    })
    .expect("runtime");
    assert!(
        runtime.catalog()[0]
            .signature
            .contains("/** A detailed identifier description.")
    );
    assert!(runtime.instructions().contains(
        "Available tools (PARTIAL - 0 of 1 shown; find the rest with tools.$codemode.search)"
    ));
    assert!(
        !runtime
            .instructions()
            .contains("tools.records.lookup(input:")
    );
}

#[tokio::test]
async fn decodes_tool_input_and_output() {
    let observed = Arc::new(Mutex::new(Vec::new()));
    let (run_sink, hook_sink) = (observed.clone(), observed.clone());
    let double = tool(
        "Double a number",
        Schema::object([("value", Schema::number_from_string())]),
        Some(Schema::number_from_string().into()),
        move |input| {
            let value = input["value"].as_f64().unwrap_or_default();
            run_sink.lock().expect("lock").push(input["value"].clone());
            Ok(json!(format!("{}", value * 2.0)))
        },
    );
    let runtime = Runtime::new(Options {
        tools: ToolTree::new().namespace("math", ToolTree::new().tool("double", double)),
        on_tool_call_start: Some(Arc::new(move |call| {
            hook_sink.lock().expect("lock").push(call.input.clone())
        })),
        ..Options::default()
    })
    .expect("runtime");
    let success = runtime_value(
        &runtime,
        r#"return await tools.math.double({ value: "21" })"#,
    )
    .await;
    assert_eq!(success.value, Some(json!(42)));
    assert_eq!(
        success
            .tool_calls
            .iter()
            .map(|call| call.name.as_str())
            .collect::<Vec<_>>(),
        vec!["math.double"]
    );
    assert_eq!(
        *observed.lock().expect("lock"),
        vec![json!({ "value": 21 }), json!(21)]
    );
    let invalid = runtime_value(&runtime, "return await tools.math.double({ value: 21 })").await;
    assert_eq!(diagnostic(&invalid).0, DiagnosticKind::InvalidToolInput);
    assert_eq!(observed.lock().expect("lock").len(), 2);
}

#[tokio::test]
async fn returns_json_safe_data_and_normalizes_undefined() {
    let result = run("return { top: undefined, nested: [1, undefined] }").await;
    assert_eq!(
        result.value,
        Some(json!({ "top": null, "nested": [1, null] }))
    );
    assert!(result.tool_calls.is_empty());
    round_trip(&result);
}

#[tokio::test]
async fn rejects_invalid_configuration_and_discovery_limits() {
    for limits in [
        ExecutionLimits {
            timeout_ms: Some(0.0),
            ..ExecutionLimits::default()
        },
        ExecutionLimits {
            timeout_ms: Some(f64::INFINITY),
            ..ExecutionLimits::default()
        },
        ExecutionLimits {
            max_tool_calls: Some(-1.0),
            ..ExecutionLimits::default()
        },
        ExecutionLimits {
            max_output_bytes: Some(-1.0),
            ..ExecutionLimits::default()
        },
    ] {
        let outcome = opencode_codemode::execute(
            "return 1",
            &Options {
                limits,
                ..Options::default()
            },
            CancellationToken::new(),
        )
        .await;
        assert!(matches!(outcome, Err(ConfigError::InvalidLimit(..))));
    }
    assert!(matches!(
        Runtime::new(Options {
            tools: orders(),
            catalog_budget: Some(-1.0),
            ..Options::default()
        }),
        Err(ConfigError::InvalidCatalogBudget)
    ));
    let result = runtime_value(
        &orders_runtime(Some(0.0)),
        r#"return await tools.$codemode.search({ query: "order", limit: 0.5 })"#,
    )
    .await;
    assert_eq!(diagnostic(&result).0, DiagnosticKind::InvalidToolInput);
    for offset in ["-1", "0.5", "9007199254740992", "\"1\""] {
        let invalid = runtime_value(
            &orders_runtime(None),
            &format!(
                r#"return await tools.$codemode.search({{ query: "order", offset: {offset} }})"#
            ),
        )
        .await;
        assert_eq!(
            diagnostic(&invalid).0,
            DiagnosticKind::InvalidToolInput,
            "{offset}"
        );
    }
}

#[tokio::test]
async fn enforces_the_tool_call_limit() {
    let options = Options {
        tools: orders(),
        limits: ExecutionLimits {
            max_tool_calls: Some(0.0),
            ..ExecutionLimits::default()
        },
        ..Options::default()
    };
    assert_eq!(
        diagnostic(&run_with(SOURCE, &options).await).0,
        DiagnosticKind::ToolCallLimitExceeded
    );
}

#[tokio::test]
async fn no_default_timeout_or_call_budget() {
    let counter = tool(
        "Count invocations",
        empty_struct(),
        Some(Schema::number().into()),
        |_| Ok(json!(1)),
    );
    let result = run_with(
        "let total = 0\nfor (let i = 0; i < 150; i += 1) total += await tools.host.count({})\nreturn total",
        &Options { tools: ToolTree::new().namespace("host", ToolTree::new().tool("count", counter)), ..Options::default() },
    )
    .await;
    assert_eq!(result.value, Some(json!(150)));
    assert_eq!(result.tool_calls.len(), 150);
}

#[tokio::test]
async fn the_timeout_interrupts_a_busy_loop() {
    let started = std::time::Instant::now();
    let result = run_with(
        "while (true) {}",
        &Options {
            limits: ExecutionLimits {
                timeout_ms: Some(200.0),
                ..ExecutionLimits::default()
            },
            ..Options::default()
        },
    )
    .await;
    let (kind, message) = diagnostic(&result);
    assert_eq!(kind, DiagnosticKind::TimeoutExceeded);
    assert!(message.contains("timed out after 200ms"));
    assert!(started.elapsed().as_millis() < 3_000);
}

#[test]
fn reserves_the_discovery_namespace() {
    let tools =
        ToolTree::new().namespace("$codemode", ToolTree::new().tool("lookup", order_lookup()));
    let error = Runtime::new(Options {
        tools,
        ..Options::default()
    })
    .expect_err("reserved");
    assert!(
        error
            .to_string()
            .contains("reserved for CodeMode discovery tools")
    );
}
