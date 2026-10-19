//! Purpose: model-facing output bounding (`boundOutput`): truncate oversized results and logs to
//! a UTF-8 byte budget with explicit markers.
//! Scope: runtime.ts utf8Truncate/boundOutput (lines 3400-3465).
//! Uses:
//! rust/crates/
//! |-- codemode/src/run.rs
//! `-- codemode/src/tools/render.rs
//! External:
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! |-- codemode/src/lib.rs
//! `-- codemode/src/run.rs
//! Invariants: truncation never fails the execution and never splits a code point.
//! Contract tests: codemode/tests/codemode.rs

use crate::run::ExecutionResult;
use crate::tools::render::json_stringify;
use serde_json::Value as Json;

fn truncate(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

pub(crate) fn bound(mut result: ExecutionResult, max_bytes: usize) -> ExecutionResult {
    let mut truncated = false;
    let mut value_bytes = 0;
    if result.ok {
        let serialized = json_stringify(result.value.as_ref().unwrap_or(&Json::Null));
        let bytes = serialized.len();
        if bytes > max_bytes {
            truncated = true;
            result.value = Some(Json::String(format!(
                "{} [result truncated: {bytes} bytes exceeds the {max_bytes}-byte output limit; return a smaller value]",
                truncate(&serialized, max_bytes)
            )));
            value_bytes = max_bytes;
        } else {
            value_bytes = bytes;
        }
    }
    let logs = result.logs.take().unwrap_or_default();
    let budget = max_bytes.saturating_sub(value_bytes);
    let mut used = 0;
    let mut kept = Vec::new();
    for line in &logs {
        let cost = line.len() + 1;
        if used + cost > budget {
            break;
        }
        used += cost;
        kept.push(line.clone());
    }
    if kept.len() < logs.len() {
        truncated = true;
        kept.push(format!(
            "[logs truncated: showing {} of {} lines]",
            kept.len(),
            logs.len()
        ));
    }
    result.logs = (!kept.is_empty()).then_some(kept);
    if truncated {
        result.truncated = Some(true);
    }
    result
}
