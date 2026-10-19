//! Purpose: port of packages/codemode/test/enumeration.test.ts (Object.keys / for...in over tools).
//! Scope: one Rust test per TS `test(...)`.
//! Uses:
//! rust/crates/
//! |-- codemode/src/lib.rs
//! `-- codemode/tests/common/mod.rs
//! External:
//! |-- serde_json::Value
//! |-- serde_json::json
//! `-- tokio::test
//! Used by:
//! `-- none, entrypoint
//! Invariants: tests drive the real interpreter through the public API.
//! Contract tests: self

mod common;

use common::{run_tools, tool};
use opencode_codemode::{Diagnostic, DiagnosticKind, Schema, ToolTree};
use serde_json::{Value as Json, json};

fn echo(description: &str) -> opencode_codemode::Definition {
    tool(
        description,
        Schema::object([("value", Schema::string())]),
        Some(Schema::string().into()),
        |input| Ok(input["value"].clone()),
    )
}

fn tools() -> ToolTree {
    ToolTree::new()
        .namespace(
            "github",
            ToolTree::new()
                .tool("list_issues", echo("List issues"))
                .tool("get_issue", echo("Get one issue")),
        )
        .namespace(
            "memory",
            ToolTree::new().tool("search", echo("Search memory")),
        )
        .namespace(
            "playwright",
            ToolTree::new().tool("navigate", echo("Navigate somewhere")),
        )
}

async fn value(code: &str) -> Json {
    let result = run_tools(code, tools()).await;
    assert!(result.ok, "expected success, got {:?}", result.error);
    result.value.unwrap_or(Json::Null)
}

async fn error(code: &str) -> Diagnostic {
    let result = run_tools(code, tools()).await;
    assert!(!result.ok, "expected failure, got {:?}", result.value);
    result.error.expect("diagnostic")
}

#[tokio::test]
async fn enumerates_top_level_namespaces() {
    assert_eq!(
        value(
            "const namespaces = Object.keys(tools)\nreturn { namespaces, count: namespaces.length }"
        )
        .await,
        json!({ "namespaces": ["github", "memory", "playwright", "$codemode"], "count": 4 })
    );
}

#[tokio::test]
async fn enumerates_tool_names_at_a_nested_namespace() {
    assert_eq!(
        value("return Object.keys(tools.github)").await,
        json!(["list_issues", "get_issue"])
    );
}

#[tokio::test]
async fn a_callable_tool_is_a_leaf() {
    assert_eq!(
        value("return Object.keys(tools.github.list_issues)").await,
        json!([])
    );
}

#[tokio::test]
async fn the_discovery_namespace_enumerates_search() {
    assert_eq!(
        value("return Object.keys(tools.$codemode)").await,
        json!(["search"])
    );
}

#[tokio::test]
async fn an_unknown_namespace_is_unknown_tool() {
    let failure = error("return Object.keys(tools.nonexistent)").await;
    assert_eq!(failure.kind, DiagnosticKind::UnknownTool);
    assert!(
        failure
            .message
            .contains("Unknown tool namespace 'nonexistent'")
    );
    assert!(
        failure
            .suggestions
            .unwrap_or_default()
            .join(" ")
            .contains("Object.keys(tools)")
    );
}

#[tokio::test]
async fn object_values_entries_on_tool_references_explain_idioms() {
    for method in ["values", "entries"] {
        let failure = error(&format!("return Object.{method}(tools)")).await;
        assert_eq!(failure.kind, DiagnosticKind::InvalidDataValue);
        assert!(failure.message.contains(&format!(
            "Object.{method}(...) cannot read tool references: they are not plain data. Use Object.keys(tools) for names, or tools.$codemode.search({{ query }}) for signatures."
        )));
    }
    assert!(
        error("return Object.entries(tools.github)")
            .await
            .message
            .contains("Use Object.keys(tools) for names")
    );
}

#[tokio::test]
async fn object_keys_over_arrays_returns_index_strings() {
    assert_eq!(
        value(r#"return Object.keys(["a", "b", "c"])"#).await,
        json!(["0", "1", "2"])
    );
    assert_eq!(value("return Object.keys([])").await, json!([]));
}

#[tokio::test]
async fn objects_keep_their_own_keys() {
    assert_eq!(
        value("return Object.keys({ a: 1, b: 2 })").await,
        json!(["a", "b"])
    );
}

#[tokio::test]
async fn non_object_inputs_still_fail_clearly() {
    assert!(
        error(r#"return Object.keys("nope")"#)
            .await
            .message
            .contains("Object.keys expects a data object or array")
    );
}

#[tokio::test]
async fn for_in_iterates_object_keys_with_break_continue() {
    assert_eq!(
        value(
            r#"
      const seen = []
      for (const key in { a: 1, b: 2, c: 3, d: 4 }) {
        if (key === "b") continue
        if (key === "d") break
        seen.push(key)
      }
      return seen
    "#
        )
        .await,
        json!(["a", "c"])
    );
}

#[tokio::test]
async fn for_in_iterates_index_strings_over_arrays() {
    assert_eq!(
        value(
            r#"
      const indexes = []
      for (const i in ["x", "y", "z"]) {
        if (i === "2") break
        indexes.push(i)
      }
      return indexes
    "#
        )
        .await,
        json!(["0", "1"])
    );
}

#[tokio::test]
async fn for_in_supports_let_and_bare_identifiers() {
    assert_eq!(
        value("let last = \"\"\nfor (let key in { a: 1, b: 2 }) last = key\nreturn last").await,
        json!("b")
    );
    assert_eq!(
        value("let key = \"before\"\nfor (key in { only: 1 }) {}\nreturn key").await,
        json!("only")
    );
}

#[tokio::test]
async fn for_in_enumerates_the_tool_tree() {
    assert_eq!(
        value(
            r#"
      const names = []
      for (const ns in tools) {
        for (const name in tools[ns]) names.push(ns + "." + name)
      }
      return names
    "#
        )
        .await,
        json!([
            "github.list_issues",
            "github.get_issue",
            "memory.search",
            "playwright.navigate",
            "$codemode.search"
        ])
    );
}

#[tokio::test]
async fn for_in_over_unsupported_values_hints_at_for_of() {
    for expression in [
        r#""text""#,
        "new Map([[1, 2]])",
        "new Set([1])",
        "42",
        "null",
    ] {
        let failure = error(&format!(
            r#"for (const key in {expression}) {{}}; return "no""#
        ))
        .await;
        assert!(
            failure
                .message
                .contains("for...in requires a plain object, array, or tools reference")
        );
        assert!(
            failure
                .message
                .contains("Use for...of for arrays/strings/Maps/Sets, or Object.keys(value)")
        );
    }
}
