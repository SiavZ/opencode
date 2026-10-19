//! Purpose: Effect-Schema-compatible input decoding shared by the search and shell leaf tools
//! (`Schema.String`, `optional`, `PositiveInt` issue messages) and legacy invalid-argument text.
//! Scope: pure JSON validation.
//! Uses:
//! rust/crates/
//! |-- tool-api/src/lib.rs
//! `-- tools/src/files/read_common.rs
//! External:
//! |-- serde_json::Map
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! |-- tools/src/search/core.rs
//! |-- tools/src/search/legacy.rs
//! |-- tools/src/search/mod.rs
//! |-- tools/src/shell/core.rs
//! `-- tools/src/shell/legacy.rs
//! Invariants: messages match Effect `formatUnknown` issue text with `\n  at ["key"]` paths.
//! Contract tests: tools/tests/search_tools.rs

use crate::files::read_common::format_unknown;
use opencode_tool_api::ToolFailure;
use serde_json::{Map, Value};

const MAX_SAFE: f64 = 9_007_199_254_740_991.0;

pub struct Object<'a>(&'a Map<String, Value>);

pub fn object(input: &Value) -> Result<Object<'_>, String> {
    match input {
        Value::Object(map) => Ok(Object(map)),
        other => Err(format!("Expected object, got {}", format_unknown(other))),
    }
}

fn at(key: &str) -> String {
    format!("\n  at [\"{key}\"]")
}

impl Object<'_> {
    pub fn string(&self, key: &str) -> Result<String, String> {
        match self.0.get(key) {
            None => Err(format!("Missing key{}", at(key))),
            Some(Value::String(value)) => Ok(value.clone()),
            Some(other) => Err(format!(
                "Expected string, got {}{}",
                format_unknown(other),
                at(key)
            )),
        }
    }

    pub fn optional_string(&self, key: &str) -> Result<Option<String>, String> {
        match self.0.get(key) {
            None => Ok(None),
            Some(Value::String(value)) => Ok(Some(value.clone())),
            Some(other) => Err(format!(
                "Expected string | undefined, got {}{}",
                format_unknown(other),
                at(key)
            )),
        }
    }

    /// Optional `PositiveInt` with an optional inclusive maximum.
    pub fn optional_positive(
        &self,
        key: &str,
        maximum: Option<u64>,
    ) -> Result<Option<u64>, String> {
        let Some(value) = self.0.get(key) else {
            return Ok(None);
        };
        let shown = format_unknown(value);
        let Some(number) = value.as_f64().filter(|_| value.is_number()) else {
            return Err(format!(
                "Expected number | undefined, got {shown}{}",
                at(key)
            ));
        };
        if number.fract() != 0.0 || number.abs() > MAX_SAFE {
            return Err(format!("Expected an integer, got {shown}{}", at(key)));
        }
        if number <= 0.0 {
            return Err(format!(
                "Expected a value greater than 0, got {shown}{}",
                at(key)
            ));
        }
        if let Some(maximum) = maximum
            && number > maximum as f64
        {
            return Err(format!(
                "Expected a value less than or equal to {maximum}, got {shown}{}",
                at(key)
            ));
        }
        Ok(Some(number as u64))
    }
}

/// Legacy `Tool.define` invalid-argument failure.
pub fn legacy_invalid(tool: &str, detail: impl std::fmt::Display) -> ToolFailure {
    ToolFailure::new(format!(
        "The {tool} tool was called with invalid arguments: SchemaError({detail}).\nPlease rewrite the input so it satisfies the expected schema."
    ))
}

/// Omit absent keys the way JSON serialization drops `undefined` metadata fields.
pub fn metadata(entries: Vec<(&str, Option<Value>)>) -> Map<String, Value> {
    entries
        .into_iter()
        .filter_map(|(key, value)| value.map(|value| (key.to_string(), value)))
        .collect()
}
