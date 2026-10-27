//! Purpose: registry contract tests ported from packages/core/test/session-runner-tool-registry.test.ts
//! (scoped overlay, stale advertisement, in-flight survival, wholly disabled filtering, failures).
//! Scope: fixture tools through the public registry API.
//! Uses:
//! rust/crates/
//! |-- permissions/src/lib.rs
//! `-- tool-api/src/lib.rs
//! External:
//! |-- async_trait::async_trait
//! |-- serde_json::Value
//! |-- serde_json::json
//! |-- tokio::test
//! `-- tokio_util::sync::CancellationToken
//! Used by:
//! `-- none, entrypoint
//! Invariants: none.
//! Contract tests: tool-api/tests/registry.rs

use async_trait::async_trait;
use opencode_permissions::{AssertInput, Effect, PermissionError, Rule};
use opencode_tool_api::*;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

// Boundary fixture: exposes forwarded context without implementing retention policy.
struct ContextBounder(bool);

#[async_trait]
impl OutputBounder for ContextBounder {
    async fn bound(
        &self,
        session_id: &str,
        tool_call_id: &str,
        output: ToolOutput,
    ) -> Result<BoundOutput, String> {
        if self.0 {
            return Err("retention unavailable".into());
        }
        Ok(BoundOutput {
            output,
            output_paths: vec![format!("{session_id}/{tool_call_id}")],
        })
    }
}

#[tokio::test]
async fn rebound_output_uses_location_bounder_and_preserves_context_and_failures() {
    let application = ToolRegistry::new(Arc::new(PassthroughBounder));
    let location = application.fork(Arc::new(ContextBounder(false)));
    let output = ToolOutput::new(
        json!({"hook":"after"}),
        vec![ToolContent::text("rewritten output")],
    );
    let bounded = location.bound(output.clone(), &context()).await.unwrap();
    assert_eq!(bounded.output, output);
    assert_eq!(bounded.output_paths, ["ses_1/call_1"]);
    assert!(
        application
            .bound(output.clone(), &context())
            .await
            .unwrap()
            .output_paths
            .is_empty()
    );
    let failed = application.fork(Arc::new(ContextBounder(true)));
    assert_eq!(
        failed.bound(output, &context()).await.unwrap_err(),
        "retention unavailable"
    );
}

struct Echo(&'static str);

#[async_trait]
impl Tool for Echo {
    fn description(&self) -> &str {
        self.0
    }
    fn input_schema(&self) -> Value {
        json!({"type":"object","properties":{"text":{"type":"string"}},"required":["text"]})
    }
    async fn execute(
        &self,
        input: Value,
        _context: &ToolContext,
    ) -> Result<ToolOutput, ToolFailure> {
        let text = input
            .get("text")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolFailure::invalid_input("text is required"))?;
        Ok(ToolOutput::new(
            json!({}),
            vec![ToolContent::text(format!("{}:{text}", self.0))],
        ))
    }
}

struct Allow;

#[async_trait]
impl Authorizer for Allow {
    async fn assert(&self, _input: AssertInput) -> Result<(), PermissionError> {
        Ok(())
    }
}

fn context() -> ToolContext {
    ToolContext {
        session_id: "ses_1".into(),
        agent: "build".into(),
        assistant_message_id: "msg_1".into(),
        tool_call_id: "call_1".into(),
        cancel: CancellationToken::new(),
        authorizer: Arc::new(Allow),
        progress: Arc::new(NoProgress),
    }
}

fn call(name: &str) -> ExecuteInput {
    ExecuteInput {
        name: name.into(),
        input: json!({"text":"x"}),
    }
}

#[tokio::test]
async fn location_overrides_application_and_stale_calls_fail() {
    let registry = ToolRegistry::new(Arc::new(PassthroughBounder));
    let _app = registry
        .register_application(vec![(
            "echo".into(),
            Arc::new(Echo("app")) as Arc<dyn Tool>,
        )])
        .unwrap();
    let first = registry.materialize(&[]);
    assert_eq!(
        first.settle(call("echo"), &context()).await.unwrap().result,
        ToolResultValue::Text {
            value: json!("app:x")
        }
    );

    let local = registry
        .register(vec![(
            "echo".into(),
            Arc::new(Echo("local")) as Arc<dyn Tool>,
        )])
        .unwrap();
    let second = registry.materialize(&[]);
    assert_eq!(
        first.settle(call("echo"), &context()).await.unwrap().result,
        ToolResultValue::error("Stale tool call: echo")
    );
    assert_eq!(
        second
            .settle(call("echo"), &context())
            .await
            .unwrap()
            .result,
        ToolResultValue::Text {
            value: json!("local:x")
        }
    );
    drop(local);
    assert_eq!(
        second
            .settle(call("echo"), &context())
            .await
            .unwrap()
            .result,
        ToolResultValue::error("Stale tool call: echo")
    );
    assert_eq!(
        registry
            .materialize(&[])
            .settle(call("missing"), &context())
            .await
            .unwrap()
            .result,
        ToolResultValue::error("Unknown tool: missing")
    );
}

#[tokio::test]
async fn wholly_disabled_tools_are_hidden_and_names_validated() {
    let registry = ToolRegistry::new(Arc::new(PassthroughBounder));
    let _r = registry
        .register(vec![
            ("a".into(), Arc::new(Echo("a")) as Arc<dyn Tool>),
            ("b".into(), Arc::new(Echo("b")) as Arc<dyn Tool>),
        ])
        .unwrap();
    let m = registry.materialize(&[Rule::new("b", "*", Effect::Deny)]);
    assert_eq!(m.names(), vec!["a"]);
    assert_eq!(m.definitions()[0].name, "a");
    let error = registry
        .register(vec![("9bad".into(), Arc::new(Echo("x")) as Arc<dyn Tool>)])
        .unwrap_err();
    assert_eq!(error.message, "Invalid tool name: 9bad");
    assert!(validate_name("a_b-C9"));
    assert!(!validate_name(&"a".repeat(65)));
}

#[tokio::test]
async fn failures_become_error_results() {
    let registry = ToolRegistry::new(Arc::new(PassthroughBounder));
    let _r = registry
        .register(vec![("echo".into(), Arc::new(Echo("e")) as Arc<dyn Tool>)])
        .unwrap();
    let settled = registry
        .materialize(&[])
        .settle(
            ExecuteInput {
                name: "echo".into(),
                input: json!({}),
            },
            &context(),
        )
        .await
        .unwrap();
    assert_eq!(
        settled.result,
        ToolResultValue::error("Invalid tool input: text is required")
    );
    assert_eq!(settled.output, None);
}

#[tokio::test]
async fn forks_share_live_application_tools_but_keep_location_overlays_isolated() {
    let application = ToolRegistry::new(Arc::new(PassthroughBounder));
    let first = application.fork(Arc::new(PassthroughBounder));
    let second = application.fork(Arc::new(PassthroughBounder));
    let app = application
        .register_application(vec![(
            "echo".into(),
            Arc::new(Echo("app")) as Arc<dyn Tool>,
        )])
        .unwrap();
    let advertised = first.materialize(&[]);
    assert_eq!(advertised.names(), vec!["echo"]);
    assert_eq!(second.materialize(&[]).names(), vec!["echo"]);

    let local = first
        .register(vec![(
            "echo".into(),
            Arc::new(Echo("local")) as Arc<dyn Tool>,
        )])
        .unwrap();
    assert_eq!(
        advertised
            .settle(call("echo"), &context())
            .await
            .unwrap()
            .result,
        ToolResultValue::error("Stale tool call: echo")
    );
    assert_eq!(
        second
            .materialize(&[])
            .settle(call("echo"), &context())
            .await
            .unwrap()
            .result,
        ToolResultValue::Text {
            value: json!("app:x")
        }
    );
    assert_eq!(
        first
            .materialize(&[])
            .settle(call("echo"), &context())
            .await
            .unwrap()
            .result,
        ToolResultValue::Text {
            value: json!("local:x")
        }
    );
    drop(app);
    assert!(application.materialize(&[]).names().is_empty());
    assert!(second.materialize(&[]).names().is_empty());
    assert_eq!(first.materialize(&[]).names(), vec!["echo"]);
    drop(local);
    assert!(first.materialize(&[]).names().is_empty());
}

#[tokio::test]
async fn sibling_application_replacement_invalidates_earlier_advertisements() {
    let application = ToolRegistry::new(Arc::new(PassthroughBounder));
    let first = application.fork(Arc::new(PassthroughBounder));
    let second = application.fork(Arc::new(PassthroughBounder));
    let initial = first
        .register_application(vec![(
            "echo".into(),
            Arc::new(Echo("old")) as Arc<dyn Tool>,
        )])
        .unwrap();
    let advertised = second.materialize(&[]);
    let replacement = application
        .register_application(vec![(
            "echo".into(),
            Arc::new(Echo("new")) as Arc<dyn Tool>,
        )])
        .unwrap();
    assert_eq!(
        advertised
            .settle(call("echo"), &context())
            .await
            .unwrap()
            .result,
        ToolResultValue::error("Stale tool call: echo")
    );
    assert_eq!(
        second
            .materialize(&[])
            .settle(call("echo"), &context())
            .await
            .unwrap()
            .result,
        ToolResultValue::Text {
            value: json!("new:x")
        }
    );
    drop(replacement);
    assert_eq!(
        second
            .materialize(&[])
            .settle(call("echo"), &context())
            .await
            .unwrap()
            .result,
        ToolResultValue::Text {
            value: json!("old:x")
        }
    );
    drop(initial);
    assert!(first.materialize(&[]).names().is_empty());
}
