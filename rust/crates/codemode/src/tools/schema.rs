//! Purpose: validating tool schemas (the Rust analogue of the Effect Schema subset CodeMode tools
//! use) plus their JSON Schema emission for signature rendering.
//! Scope: schema construction, decode/validate, JSON Schema document emission.
//! Uses:
//! rust/crates/
//! |-- codemode/src/value/convert.rs
//! `-- codemode/src/value/number.rs
//! External:
//! |-- serde_json::Map
//! |-- serde_json::Value
//! |-- serde_json::json
//! `-- serde_json::to_string
//! Used by:
//! rust/crates/
//! |-- codemode/src/lib.rs
//! |-- codemode/src/tools/discovery.rs
//! `-- codemode/src/tools/mod.rs
//! Invariants: decode returns the decoded (Type-side) value; `json_schema(false)` describes the
//! encoded side and `json_schema(true)` the decoded side, like `Schema.toType`.
//! Contract tests: codemode/tests/codemode.rs, codemode/tests/signature.rs

use serde_json::{Map, Value as Json, json};

/// A validating schema. Construct with the associated functions (`Schema::string()`,
/// `Schema::object(...)`, ...).
#[derive(Clone, Debug)]
pub enum Schema {
    String,
    Number,
    Boolean,
    Null,
    Unknown,
    /// Safe integer with optional inclusive/exclusive lower bounds.
    Int {
        greater_than: Option<f64>,
        at_least: Option<f64>,
    },
    /// Encoded as a numeric string, decoded to a number.
    NumberFromString,
    Literal(Json),
    Array(Box<Schema>),
    NullOr(Box<Schema>),
    Struct(Vec<Field>),
    Described(Box<Schema>, String),
}

#[derive(Clone, Debug)]
pub struct Field {
    pub name: String,
    pub schema: Schema,
    pub optional: bool,
}

impl Schema {
    pub fn string() -> Self {
        Self::String
    }
    pub fn number() -> Self {
        Self::Number
    }
    pub fn boolean() -> Self {
        Self::Boolean
    }
    pub fn unknown() -> Self {
        Self::Unknown
    }
    pub fn int() -> Self {
        Self::Int {
            greater_than: None,
            at_least: None,
        }
    }
    pub fn positive_int() -> Self {
        Self::Int {
            greater_than: Some(0.0),
            at_least: None,
        }
    }
    pub fn non_negative_int() -> Self {
        Self::Int {
            greater_than: None,
            at_least: Some(0.0),
        }
    }
    pub fn number_from_string() -> Self {
        Self::NumberFromString
    }
    pub fn literal(value: Json) -> Self {
        Self::Literal(value)
    }
    pub fn array(item: Schema) -> Self {
        Self::Array(Box::new(item))
    }
    pub fn null_or(item: Schema) -> Self {
        Self::NullOr(Box::new(item))
    }
    /// `Schema.Struct({...})` from `(name, schema)` pairs; use [`Schema::optional`] for optional keys.
    pub fn object(fields: impl IntoIterator<Item = (impl Into<String>, Schema)>) -> Self {
        Self::Struct(
            fields
                .into_iter()
                .map(|(name, schema)| match schema {
                    Self::Described(inner, marker) if marker == OPTIONAL_MARKER => Field {
                        name: name.into(),
                        schema: *inner,
                        optional: true,
                    },
                    schema => Field {
                        name: name.into(),
                        schema,
                        optional: false,
                    },
                })
                .collect(),
        )
    }
    /// `Schema.optionalKey(schema)`, meaningful only as an [`Schema::object`] field.
    pub fn optional(schema: Schema) -> Self {
        Self::Described(Box::new(schema), OPTIONAL_MARKER.into())
    }
    /// `schema.annotate({ description })`.
    pub fn describe(self, description: impl Into<String>) -> Self {
        Self::Described(Box::new(self), description.into())
    }

    /// Validates and decodes an encoded value.
    pub fn decode(&self, value: &Json) -> Result<Json, String> {
        self.decode_at(value, &mut Vec::new())
    }

    fn decode_at(&self, value: &Json, path: &mut Vec<String>) -> Result<Json, String> {
        let mismatch = |expected: &str, path: &[String]| {
            let rendered = serde_json::to_string(value).unwrap_or_default();
            let at = if path.is_empty() {
                String::new()
            } else {
                format!("\n  at [{}]", path.join("]["))
            };
            Err(format!("Expected {expected}, got {rendered}{at}"))
        };
        match self {
            Self::String => {
                if value.is_string() {
                    Ok(value.clone())
                } else {
                    mismatch("string", path)
                }
            }
            Self::Number => {
                if value.is_number() {
                    Ok(value.clone())
                } else {
                    mismatch("number", path)
                }
            }
            Self::Boolean => {
                if value.is_boolean() {
                    Ok(value.clone())
                } else {
                    mismatch("boolean", path)
                }
            }
            Self::Null => {
                if value.is_null() {
                    Ok(Json::Null)
                } else {
                    mismatch("null", path)
                }
            }
            Self::Unknown => Ok(value.clone()),
            Self::Literal(expected) => {
                if value == expected {
                    Ok(value.clone())
                } else {
                    mismatch(&expected.to_string(), path)
                }
            }
            Self::Int {
                greater_than,
                at_least,
            } => {
                let Some(number) = value.as_f64() else {
                    return mismatch("integer", path);
                };
                let safe = number.fract() == 0.0 && number.abs() <= 9_007_199_254_740_991.0;
                if !safe {
                    return mismatch("integer", path);
                }
                if greater_than.is_some_and(|bound| number <= bound) {
                    return mismatch(
                        &format!("a value greater than {}", greater_than.unwrap_or_default()),
                        path,
                    );
                }
                if at_least.is_some_and(|bound| number < bound) {
                    return mismatch(
                        &format!(
                            "a value greater than or equal to {}",
                            at_least.unwrap_or_default()
                        ),
                        path,
                    );
                }
                Ok(value.clone())
            }
            Self::NumberFromString => {
                let Some(text) = value.as_str() else {
                    return mismatch("string", path);
                };
                let number = crate::value::number::parse_number(text);
                if number.is_nan() && text.trim() != "NaN" {
                    return mismatch("a numeric string", path);
                }
                Ok(crate::value::convert::json_number(number))
            }
            Self::Array(item) => {
                let Some(items) = value.as_array() else {
                    return mismatch("array", path);
                };
                let mut out = Vec::with_capacity(items.len());
                for (index, entry) in items.iter().enumerate() {
                    path.push(index.to_string());
                    out.push(item.decode_at(entry, path)?);
                    path.pop();
                }
                Ok(Json::Array(out))
            }
            Self::NullOr(item) => {
                if value.is_null() {
                    Ok(Json::Null)
                } else {
                    item.decode_at(value, path)
                }
            }
            Self::Described(inner, _) => inner.decode_at(value, path),
            Self::Struct(fields) => {
                let Some(object) = value.as_object() else {
                    return mismatch("object", path);
                };
                let mut out = Map::new();
                for field in fields {
                    match object.get(&field.name) {
                        None if field.optional => {}
                        None => {
                            path.push(serde_json::to_string(&field.name).unwrap_or_default());
                            let at = format!("\n  at [{}]", path.join("]["));
                            path.pop();
                            return Err(format!("Missing key{at}"));
                        }
                        Some(entry) => {
                            path.push(serde_json::to_string(&field.name).unwrap_or_default());
                            out.insert(field.name.clone(), field.schema.decode_at(entry, path)?);
                            path.pop();
                        }
                    }
                }
                Ok(Json::Object(out))
            }
        }
    }

    /// JSON Schema for the encoded side (`decoded = false`) or decoded side (`decoded = true`).
    pub fn json_schema(&self, decoded: bool) -> Json {
        match self {
            Self::String => json!({ "type": "string" }),
            Self::Number => json!({ "type": "number" }),
            Self::Boolean => json!({ "type": "boolean" }),
            Self::Null => json!({ "type": "null" }),
            Self::Unknown => json!({}),
            Self::Int { .. } => json!({ "type": "integer" }),
            Self::NumberFromString => {
                if decoded {
                    json!({ "type": "number" })
                } else {
                    json!({ "type": "string" })
                }
            }
            Self::Literal(value) => json!({ "const": value }),
            Self::Array(item) => json!({ "type": "array", "items": item.json_schema(decoded) }),
            Self::NullOr(item) => {
                json!({ "anyOf": [item.json_schema(decoded), { "type": "null" }] })
            }
            Self::Described(inner, description) => {
                let mut schema = inner.json_schema(decoded);
                if description != OPTIONAL_MARKER
                    && let Some(object) = schema.as_object_mut()
                {
                    object.insert("description".into(), Json::String(description.clone()));
                }
                schema
            }
            Self::Struct(fields) => {
                let properties: Map<String, Json> = fields
                    .iter()
                    .map(|field| (field.name.clone(), field.schema.json_schema(decoded)))
                    .collect();
                let required: Vec<Json> = fields
                    .iter()
                    .filter(|field| !field.optional)
                    .map(|field| Json::String(field.name.clone()))
                    .collect();
                let mut schema = Map::new();
                schema.insert("type".into(), json!("object"));
                schema.insert("properties".into(), Json::Object(properties));
                if !required.is_empty() {
                    schema.insert("required".into(), Json::Array(required));
                }
                schema.insert("additionalProperties".into(), Json::Bool(false));
                Json::Object(schema)
            }
        }
    }
}

const OPTIONAL_MARKER: &str = "\u{0}optional";
