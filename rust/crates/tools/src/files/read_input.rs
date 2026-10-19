//! Purpose: decode read-tool inputs for both generations with the baseline Effect Schema issue
//! messages (first issue only, `path` suffix, JS number formatting).
//! Scope: pure validation of untrusted tool-call JSON.
//! Uses:
//! rust/crates/
//! `-- tools/src/files/read_common.rs
//! External:
//! |-- serde_json::Map
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! |-- tools/src/files/mod.rs
//! |-- tools/src/files/read_core.rs
//! `-- tools/src/files/read_legacy.rs
//! Invariants: Core `offset`/`limit` are optional safe positive integers (limit <= 2000), Legacy
//! `offset`/`limit` are optional safe non-negative integers. Unknown keys are ignored.
//! Contract tests: tools/tests/files_read.rs

use super::read_common::{MAX_READ_LINES, format_unknown};
use serde_json::{Map, Value};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadInput {
    pub path: String,
    pub offset: Option<u64>,
    pub limit: Option<u64>,
}

const MAX_SAFE: f64 = 9_007_199_254_740_991.0;

/// Core `{ path, offset?: PositiveInt, limit?: PositiveInt <= 2000 }`.
pub fn decode_core(input: &Value) -> Result<ReadInput, String> {
    decode(input, "path", 1)
}

/// Legacy `{ filePath, offset?: NonNegativeInt, limit?: NonNegativeInt }`.
pub fn decode_legacy(input: &Value) -> Result<ReadInput, String> {
    decode(input, "filePath", 0)
}

fn decode(input: &Value, key: &str, minimum: u64) -> Result<ReadInput, String> {
    let Value::Object(object) = input else {
        return Err(format!("Expected object, got {}", format_unknown(input)));
    };
    let path = match object.get(key) {
        None => return Err(format!("Missing key\n  at [\"{key}\"]")),
        Some(Value::String(path)) => path.clone(),
        Some(other) => {
            return Err(format!(
                "Expected string, got {}\n  at [\"{key}\"]",
                format_unknown(other)
            ));
        }
    };
    let maximum = (minimum == 1).then_some(MAX_READ_LINES as u64);
    Ok(ReadInput {
        path,
        offset: integer(object, "offset", minimum, None)?,
        limit: integer(object, "limit", minimum, maximum)?,
    })
}

fn integer(
    object: &Map<String, Value>,
    key: &str,
    minimum: u64,
    maximum: Option<u64>,
) -> Result<Option<u64>, String> {
    let Some(value) = object.get(key) else {
        return Ok(None);
    };
    let at = format!("\n  at [\"{key}\"]");
    let Some(number) = value.as_f64().filter(|_| value.is_number()) else {
        return Err(format!(
            "Expected number | undefined, got {}{at}",
            format_unknown(value)
        ));
    };
    let shown = format_unknown(value);
    if number.fract() != 0.0 || number.abs() > MAX_SAFE {
        return Err(format!("Expected an integer, got {shown}{at}"));
    }
    if minimum == 1 && number <= 0.0 {
        return Err(format!("Expected a value greater than 0, got {shown}{at}"));
    }
    if number < 0.0 {
        return Err(format!(
            "Expected a value greater than or equal to 0, got {shown}{at}"
        ));
    }
    let number = number as u64;
    if let Some(maximum) = maximum.filter(|maximum| number > *maximum) {
        return Err(format!(
            "Expected a value less than or equal to {maximum}, got {shown}{at}"
        ));
    }
    Ok(Some(number))
}
