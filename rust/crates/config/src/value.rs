//! Purpose: JSON value helpers: remeda `mergeDeep` semantics, the legacy array-concatenating
//! merge, and per-value provenance tracking across merged sources.
//! Scope: pure values.
//! Uses:
//! `-- none
//! External:
//! |-- serde_json::Map
//! |-- serde_json::Value
//! `-- serde_json::to_string_pretty
//! Used by:
//! rust/crates/
//! |-- config/src/legacy/load.rs
//! |-- config/src/legacy/parse.rs
//! |-- config/src/legacy/steps.rs
//! |-- config/src/legacy/update.rs
//! `-- config/src/lib.rs
//! Invariants: plain objects merge recursively, everything else (arrays included) is replaced;
//! key order follows `{ ...target, ...source }`; provenance resolves by longest recorded prefix.
//! Contract tests: config/tests/legacy_load.rs

use serde_json::{Map, Value};
use std::collections::BTreeMap;

pub fn is_record(value: &Value) -> bool {
    value.is_object()
}

/// remeda `mergeDeep(target, source)`.
pub fn merge_deep(target: &Value, source: &Value) -> Value {
    let (Some(target), Some(source)) = (target.as_object(), source.as_object()) else {
        return source.clone();
    };
    Value::Object(merge_maps(target, source))
}

fn merge_maps(target: &Map<String, Value>, source: &Map<String, Value>) -> Map<String, Value> {
    let mut out = target.clone();
    for (key, value) in source {
        let merged = match (target.get(key), value) {
            (Some(Value::Object(left)), Value::Object(right)) => {
                Value::Object(merge_maps(left, right))
            }
            _ => value.clone(),
        };
        out.insert(key.clone(), merged);
    }
    out
}

/// `mergeConfigConcatArrays`: deep merge, then `instructions` become the ordered union.
pub fn merge_concat(target: &Value, source: &Value) -> Value {
    let mut merged = merge_deep(target, source);
    if let (Some(Value::Array(left)), Some(Value::Array(right))) =
        (target.get("instructions"), source.get("instructions"))
    {
        let mut seen: Vec<Value> = Vec::new();
        for item in left.iter().chain(right) {
            if !seen.contains(item) {
                seen.push(item.clone());
            }
        }
        merged["instructions"] = Value::Array(seen);
    }
    merged
}

/// Which source last wrote each value of a merged config. Keys are JSON paths.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Provenance {
    entries: BTreeMap<Vec<String>, String>,
}

impl Provenance {
    /// Record that `source` was merged at `prefix` (deep-merge aware: only leaves that the
    /// source actually replaced are attributed to it).
    pub fn record(&mut self, prefix: &[String], target: &Value, source: &Value, origin: &str) {
        let (Some(target), Some(source)) = (target.as_object(), source.as_object()) else {
            self.set(prefix, origin);
            return;
        };
        for (key, value) in source {
            let mut path = prefix.to_vec();
            path.push(key.clone());
            match (target.get(key), value) {
                (Some(left @ Value::Object(_)), Value::Object(_)) => {
                    self.record(&path, left, value, origin)
                }
                _ => self.set(&path, origin),
            }
        }
    }

    /// Attribute the subtree at `path` to `origin`, dropping finer-grained records below it.
    pub fn set(&mut self, path: &[String], origin: &str) {
        self.entries
            .retain(|key, _| !(key.len() > path.len() && key.starts_with(path)));
        self.entries.insert(path.to_vec(), origin.to_string());
    }

    /// The source of the value at `path` (longest recorded prefix).
    pub fn source(&self, path: &[&str]) -> Option<&str> {
        (0..=path.len()).rev().find_map(|len| {
            let key: Vec<String> = path[..len]
                .iter()
                .map(|segment| segment.to_string())
                .collect();
            self.entries.get(&key).map(String::as_str)
        })
    }

    pub fn entries(&self) -> impl Iterator<Item = (&Vec<String>, &String)> {
        self.entries.iter()
    }
}

/// `JSON.stringify(value, null, 2)`.
pub fn stringify_pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default()
}
