//! Purpose: surviving legacy file stores: the JSON key/value store with its file migrations
//! (packages/opencode/src/storage/storage.ts) and legacy provider auth (packages/opencode/src/auth/index.ts).
//! Scope: module root plus the JS-compatible JSON text writer they share.
//! Uses:
//! rust/crates/
//! |-- storage-sqlite/src/legacy_files/auth.rs
//! |-- storage-sqlite/src/legacy_files/migrate.rs
//! `-- storage-sqlite/src/legacy_files/store.rs
//! External:
//! |-- serde_json::Number
//! |-- serde_json::Value
//! |-- serde_json::from_str
//! |-- serde_json::to_string_pretty
//! `-- u32::MAX
//! Used by:
//! rust/crates/
//! |-- storage-sqlite/src/legacy_files/auth.rs
//! |-- storage-sqlite/src/legacy_files/migrate.rs
//! |-- storage-sqlite/src/legacy_files/store.rs
//! |-- storage-sqlite/src/lib.rs
//! |-- storage-sqlite/tests/credentials.rs
//! `-- storage-sqlite/tests/legacy_json.rs
//! Invariants: files are written as `JSON.stringify(value, null, 2)`: two-space indent and JS number
//! text (integral floats print without `.0`). Never touches anything outside the given data dir.
//! Contract tests: storage-sqlite/tests/legacy_json.rs, storage-sqlite/tests/credentials.rs

pub mod auth;
mod migrate;
pub mod store;

pub use auth::{AuthInfo, LegacyAuth};
pub use store::LegacyJsonStore;

use serde_json::{Number, Value};

/// `JSON.stringify(value, null, 2)`.
pub fn js_pretty(value: &Value) -> String {
    serde_json::to_string_pretty(&js_normalize(value)).unwrap_or_else(|_| "null".into())
}

/// Integer-like keys (canonical array indices) come first in ascending order in every JS object,
/// then the remaining keys in insertion order.
fn js_index(key: &str) -> Option<u32> {
    let index: u32 = key.parse().ok()?;
    (index != u32::MAX && index.to_string() == key).then_some(index)
}

/// Rewrite a value the way JS holds and prints it: integral floats without a fraction, -0 as 0,
/// non-finite as null, object keys in JS property order. Other floats keep ryu's shortest form,
/// which equals JS for |x| in [1e-7, 1e21).
pub(crate) fn js_normalize(value: &Value) -> Value {
    match value {
        Value::Number(n) => match n.as_f64() {
            Some(f) if n.is_f64() && f.fract() == 0.0 && f.abs() < 9.007_199_254_740_992e15 => {
                Value::Number(Number::from(f as i64))
            }
            Some(f) if n.is_f64() && !f.is_finite() => Value::Null,
            _ => value.clone(),
        },
        Value::Array(items) => Value::Array(items.iter().map(js_normalize).collect()),
        Value::Object(map) => {
            let mut indexed: Vec<(u32, &String, &Value)> = map
                .iter()
                .filter_map(|(k, v)| js_index(k).map(|i| (i, k, v)))
                .collect();
            indexed.sort_by_key(|(i, _, _)| *i);
            let named = map.iter().filter(|(k, _)| js_index(k).is_none());
            Value::Object(
                indexed
                    .into_iter()
                    .map(|(_, k, v)| (k, v))
                    .chain(named)
                    .map(|(k, v)| (k.clone(), js_normalize(v)))
                    .collect(),
            )
        }
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pretty_matches_json_stringify() {
        let value: Value =
            serde_json::from_str(r#"{"a":1.0,"b":[],"c":{},"d":[0.5,-0.0,2],"10":1,"2":2}"#)
                .unwrap();
        assert_eq!(
            js_pretty(&value),
            "{\n  \"2\": 2,\n  \"10\": 1,\n  \"a\": 1,\n  \"b\": [],\n  \"c\": {},\n  \"d\": [\n    0.5,\n    0,\n    2\n  ]\n}"
        );
    }
}
