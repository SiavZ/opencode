//! Purpose: regression tests for shared provider lifecycle, tool streaming and content helpers.
//! Scope: unit tests of the actual shared implementation.
//! Uses:
//! rust/crates/
//! `-- provider-adapters/src/shared.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- provider-adapters/src/shared.rs
//! Invariants: preserves lifecycle event ordering, streamed input assembly and baseline messages.
//! Contract tests: this module, run through opencode-provider-adapters package tests.

use super::*;

#[test]
fn lifecycle_finish_closes_reasoning_then_text() {
    let mut lc = Lifecycle::default();
    let mut ev = Vec::new();
    lc.text_delta(&mut ev, "t", "hi");
    lc.reasoning_delta(&mut ev, "r", "think", None);
    lc.finish(&mut ev, FinishReason::Stop, None, None);
    let kinds: Vec<&str> = ev.iter().map(LlmEvent::kind).collect();
    assert_eq!(
        kinds,
        [
            "step-start",
            "text-start",
            "text-delta",
            "reasoning-start",
            "reasoning-delta",
            "reasoning-end",
            "text-end",
            "step-finish",
            "finish"
        ]
    );
}

#[test]
fn tool_stream_accumulates_and_parses() {
    let mut ts = ToolStream::<u32>::default();
    let a = ts
        .append_or_start("r", 0, Some("c1"), Some("f"), "{\"a\"", "missing")
        .unwrap();
    assert_eq!(a.len(), 2);
    let b = ts
        .append_or_start("r", 0, None, None, ":1}", "missing")
        .unwrap();
    assert_eq!(b.len(), 1);
    let done = ts.finish_all("r").unwrap();
    assert_eq!(
        done[1],
        LlmEvent::ToolCall {
            id: "c1".into(),
            name: "f".into(),
            input: serde_json::json!({"a":1}),
            provider_executed: None,
            provider_metadata: None
        }
    );
    let err = ts
        .append_or_start("r", 1, None, None, "x", "Missing tool")
        .unwrap_err();
    assert_eq!(err.reason.message(), "Missing tool");
    assert_eq!(
        parse_tool_input("r", "f", "").unwrap(),
        serde_json::json!({})
    );
}

#[test]
fn content_helpers_match_baseline_messages() {
    assert_eq!(
        unsupported_content("r", "user", &["text", "media", "tool-call"])
            .reason
            .message(),
        "r user messages only support text, media, and tool-call content for now"
    );
    assert_eq!(
        wrap_system_update("<a&b>"),
        "<system-update>\n&lt;a&amp;b&gt;\n</system-update>"
    );
}
