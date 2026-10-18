//! Purpose: structural schema IR mirroring the subset of Effect Schema the baseline wire contract
//! uses, so codegen (OpenAPI, TS clients) and validation are derived from one Rust definition.
//! Scope: pure data plus small builder helpers. No serialization of wire values.
//! Uses:
//! `-- none
//! External:
//! `-- serde::{Deserialize, Serialize}
//! Used by:
//! rust/crates/
//! |-- protocol/src/current/mod.rs
//! |-- protocol/src/errors.rs
//! |-- protocol/src/legacy/mod.rs
//! `-- protocol/src/operation.rs
//! Invariants: `Ref` names resolve in the owning `Api::definitions` table. Field order is the
//! baseline declaration order. Optional (`optionalKey`) vs `exact` (omit, never undefined) vs
//! nullable are distinct, matching Effect's encoded representations.
//! Contract tests: protocol/tests/registry.rs

use serde::{Deserialize, Serialize};

/// One schema node. Named schemas are `Ref`s into the API definitions table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Schema {
    /// Reference to a named definition (Effect `identifier` annotation).
    Ref {
        name: String,
    },
    String {
        #[serde(skip_serializing_if = "Checks::is_empty")]
        checks: Checks,
        /// Brand names (Effect `Schema.brand`). Brands do not change the wire shape.
        #[serde(skip_serializing_if = "Vec::is_empty")]
        brands: Vec<String>,
    },
    Number {
        int: bool,
        #[serde(skip_serializing_if = "Checks::is_empty")]
        checks: Checks,
        /// Encoded as a decimal string (Effect `NumberFromString`, query/params).
        from_string: bool,
    },
    Boolean,
    /// JSON `null`.
    Null,
    /// Any JSON value (Effect `Unknown`).
    Unknown,
    Literal {
        value: serde_json::Value,
    },
    Array {
        items: Box<Schema>,
    },
    Tuple {
        elements: Vec<Schema>,
        rest: Option<Box<Schema>>,
    },
    Record {
        key: Box<Schema>,
        value: Box<Schema>,
    },
    Struct {
        fields: Vec<Field>,
        /// Index signature (Effect `StructWithRest` with a `Record`): (key, value).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<(Box<Schema>, Box<Schema>)>,
    },
    Union {
        members: Vec<Schema>,
    },
    /// No value is valid (Effect `Never`).
    Never,
    /// Raw bytes (Effect `Uint8Array` with `asUint8Array`).
    Bytes,
    /// `DateTimeUtc` encoded as epoch milliseconds (`DateTimeUtcFromMillis`).
    DateTimeMillis,
    /// Annotated wrapper carrying documentation on an inline schema.
    Described {
        description: String,
        schema: Box<Schema>,
    },
}

/// String/number refinements. Empty when absent.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(default)]
pub struct Checks {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_length: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_length: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimum: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclusive_minimum: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maximum: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclusive_maximum: Option<f64>,
    /// OpenAPI `format`, e.g. `uuid`, `date-time`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_items: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_items: Option<u64>,
}

impl Checks {
    pub fn is_empty(&self) -> bool {
        *self == Checks::default()
    }
}

/// Field optionality in the encoded form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Optionality {
    Required,
    /// `Schema.optional`: key may be absent or `undefined`.
    Optional,
    /// `Schema.optionalKey` / schema `optional` helper: key absent, never `undefined`.
    Exact,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Field {
    pub name: String,
    pub optionality: Optionality,
    pub schema: Schema,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Encoded default (Effect `withDecodingDefault`), rendered as OpenAPI `default`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
}

/// A named entry in the definitions table, in registration order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Definition {
    /// OpenAPI component key (may carry a numeric suffix, e.g. `UnknownError1`).
    pub name: String,
    /// Effect `identifier` annotation when it differs from `name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identifier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub schema: Schema,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

// Builder helpers. Short names keep registry modules readable.

pub fn r(name: &str) -> Schema {
    Schema::Ref {
        name: name.to_string(),
    }
}

pub fn string() -> Schema {
    Schema::String {
        checks: Checks::default(),
        brands: Vec::new(),
    }
}

/// Branded string with a prefix pattern, e.g. `id("^ses", "SessionID")`.
pub fn id(pattern: &str, brand: &str) -> Schema {
    Schema::String {
        checks: Checks {
            pattern: Some(pattern.to_string()),
            ..Checks::default()
        },
        brands: vec![brand.to_string()],
    }
}

pub fn branded(brand: &str) -> Schema {
    Schema::String {
        checks: Checks::default(),
        brands: vec![brand.to_string()],
    }
}

pub fn number() -> Schema {
    Schema::Number {
        int: false,
        checks: Checks::default(),
        from_string: false,
    }
}

pub fn int() -> Schema {
    Schema::Number {
        int: true,
        checks: Checks::default(),
        from_string: false,
    }
}

/// Query/path integer decoded from a string with optional bounds.
pub fn int_from_string(minimum: Option<f64>, maximum: Option<f64>) -> Schema {
    Schema::Number {
        int: true,
        checks: Checks {
            minimum,
            maximum,
            ..Checks::default()
        },
        from_string: true,
    }
}

pub fn boolean() -> Schema {
    Schema::Boolean
}

pub fn unknown() -> Schema {
    Schema::Unknown
}

pub fn lit(value: impl Into<serde_json::Value>) -> Schema {
    Schema::Literal {
        value: value.into(),
    }
}

pub fn lits(values: &[&str]) -> Schema {
    Schema::Union {
        members: values.iter().map(|v| lit(*v)).collect(),
    }
}

pub fn array(items: Schema) -> Schema {
    Schema::Array {
        items: Box::new(items),
    }
}

pub fn record(key: Schema, value: Schema) -> Schema {
    Schema::Record {
        key: Box::new(key),
        value: Box::new(value),
    }
}

pub fn union(members: Vec<Schema>) -> Schema {
    Schema::Union { members }
}

pub fn object(fields: Vec<Field>) -> Schema {
    Schema::Struct {
        fields,
        index: None,
    }
}

pub fn described(description: &str, schema: Schema) -> Schema {
    Schema::Described {
        description: description.to_string(),
        schema: Box::new(schema),
    }
}

pub fn req(name: &str, schema: Schema) -> Field {
    Field {
        name: name.to_string(),
        optionality: Optionality::Required,
        schema,
        description: None,
        default: None,
    }
}

pub fn opt(name: &str, schema: Schema) -> Field {
    Field {
        name: name.to_string(),
        optionality: Optionality::Optional,
        schema,
        description: None,
        default: None,
    }
}

pub fn exact(name: &str, schema: Schema) -> Field {
    Field {
        name: name.to_string(),
        optionality: Optionality::Exact,
        schema,
        description: None,
        default: None,
    }
}

impl Field {
    pub fn default_value(mut self, value: serde_json::Value) -> Self {
        self.default = Some(value);
        self
    }

    pub fn doc(mut self, description: &str) -> Self {
        self.description = Some(description.to_string());
        self
    }
}

pub fn def(name: &str, schema: Schema) -> Definition {
    Definition {
        name: name.to_string(),
        identifier: None,
        title: None,
        schema,
        description: None,
    }
}
