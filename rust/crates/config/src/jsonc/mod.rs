//! Purpose: JSONC entry points: tolerant parse (`jsonc-parser.parse` with trailing commas),
//! TypeScript-compatible error reports (`ConfigParse.jsonc`), and comment-preserving edits.
//! Scope: module root for the JSONC port.
//! Uses:
//! rust/crates/
//! |-- config/src/jsonc/edit.rs
//! |-- config/src/jsonc/format.rs
//! |-- config/src/jsonc/scanner.rs
//! `-- config/src/jsonc/visit.rs
//! External:
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! |-- config/src/current.rs
//! |-- config/src/legacy/load.rs
//! |-- config/src/legacy/update.rs
//! `-- config/src/lib.rs
//! Invariants: offsets, line and column numbers are UTF-16 based, exactly like the baseline.
//! Contract tests: config/tests/jsonc.rs

mod edit;
mod format;
mod scanner;
mod visit;

pub use edit::{EditError, modify};
pub use visit::{ErrorCode, ParseError, js_number, number_value};

use serde_json::Value;

/// Parse JSONC with `allowTrailingComma: true`. Returns the (possibly partial) value and every
/// error the baseline parser would report.
pub fn parse(text: &str) -> (Option<Value>, Vec<ParseError>) {
    let units: Vec<u16> = text.encode_utf16().collect();
    let mut errors = Vec::new();
    let tree = visit::parse_tree(&units, &mut errors, true);
    (tree.map(|node| node.to_value()), errors)
}

/// Parse JSONC and fail with the exact `ConfigJsonError.message` text on any error.
pub fn parse_strict(text: &str) -> Result<Value, String> {
    let (value, errors) = parse(text);
    if errors.is_empty() {
        return Ok(value.unwrap_or(Value::Null));
    }
    Err(error_message(text, &errors))
}

/// `ConfigParse.jsonc` message body for a set of parse errors.
pub fn error_message(text: &str, errors: &[ParseError]) -> String {
    let units: Vec<u16> = text.encode_utf16().collect();
    let lines: Vec<&str> = text.split('\n').collect();
    let issues = errors
        .iter()
        .map(|error| {
            let before = String::from_utf16_lossy(&units[..error.offset.min(units.len())]);
            let segments: Vec<&str> = before.split('\n').collect();
            let line = segments.len();
            let column = segments
                .last()
                .map(|last| last.encode_utf16().count())
                .unwrap_or(0)
                + 1;
            let head = format!("{} at line {}, column {}", error.error.name(), line, column);
            match lines.get(line - 1) {
                Some(problem) if !problem.is_empty() => {
                    format!(
                        "{head}\n   Line {line}: {problem}\n{}^",
                        " ".repeat(column + 9)
                    )
                }
                _ => head,
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("\n--- JSONC Input ---\n{text}\n--- Errors ---\n{issues}\n--- End ---")
}
