//! Purpose: null-prototype data containers (objects with JS key order, arrays with own extras).
//! Scope: storage and enumeration order only.
//! Uses:
//! rust/crates/
//! `-- codemode/src/value/mod.rs
//! External:
//! |-- indexmap::IndexMap
//! `-- u32::MAX
//! Used by:
//! rust/crates/
//! |-- codemode/src/interp/ops.rs
//! |-- codemode/src/tools/text.rs
//! `-- codemode/src/value/mod.rs
//! Invariants: enumeration follows JS own-key order (array-index keys ascending, then insertion).
//! Contract tests: codemode/tests/parity.rs

use super::Value;
use indexmap::IndexMap;

/// A plain data object. `brand` marks error values constructed by `new Error(...)` so
/// `instanceof` can recognize them; spreading or copying loses it, like losing the prototype.
#[derive(Clone, Default)]
pub(crate) struct ObjectData {
    pub props: IndexMap<String, Value>,
    pub brand: Option<&'static str>,
}

impl ObjectData {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_pairs(pairs: impl IntoIterator<Item = (String, Value)>) -> Self {
        let mut data = Self::new();
        for (key, value) in pairs {
            data.set(key, value);
        }
        data
    }

    pub fn error(name: &'static str, message: String) -> Self {
        let mut data = Self::new();
        data.set("name".into(), Value::str(name));
        data.set("message".into(), Value::str(message));
        data.brand = Some(name);
        data
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.props.get(key)
    }

    pub fn set(&mut self, key: String, value: Value) {
        self.props.insert(key, value);
    }

    pub fn has(&self, key: &str) -> bool {
        self.props.contains_key(key)
    }

    /// Own enumerable keys in JS order.
    pub fn keys(&self) -> Vec<String> {
        js_key_order(self.props.keys().cloned())
    }

    pub fn entries(&self) -> Vec<(String, Value)> {
        self.keys()
            .into_iter()
            .filter_map(|key| self.props.get(&key).cloned().map(|value| (key, value)))
            .collect()
    }
}

/// Array storage plus own non-index properties (regex match `index`/`groups`).
#[derive(Clone, Default)]
pub(crate) struct ArrayData {
    pub items: Vec<Value>,
    pub extra: IndexMap<String, Value>,
}

impl ArrayData {
    pub fn new(items: Vec<Value>) -> Self {
        Self {
            items,
            extra: IndexMap::new(),
        }
    }
}

pub(crate) fn is_array_index(key: &str) -> bool {
    !key.is_empty()
        && key.bytes().all(|byte| byte.is_ascii_digit())
        && (key == "0" || !key.starts_with('0'))
        && key.parse::<u32>().is_ok_and(|index| index < u32::MAX)
}

/// Orders keys like `Object.keys`: integer-like keys ascending, then the rest in insertion order.
pub(crate) fn js_key_order(keys: impl IntoIterator<Item = String>) -> Vec<String> {
    let (mut indexes, rest): (Vec<String>, Vec<String>) =
        keys.into_iter().partition(|key| is_array_index(key));
    indexes.sort_by_key(|key| key.parse::<u32>().unwrap_or(u32::MAX));
    indexes.extend(rest);
    indexes
}
