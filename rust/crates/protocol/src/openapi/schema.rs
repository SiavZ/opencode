//! Purpose: transform every structural SchemaIR variant into JSON Schema 2020-12.
//! Scope: pure OpenAPI component and inline schema encoding.
//! Uses: protocol/src/ir.rs
//! External: serde_json
//! Used by: protocol/src/openapi.rs, protocol/src/openapi/endpoint.rs
//! Invariants: references use component keys, optional keys remain distinct from nullable values.
//! Contract tests: protocol/tests/openapi.rs
use crate::ir::{Field, Optionality, Schema};
use serde_json::{Map, Value, json};

pub(super) fn emit(schema: &Schema) -> Value {
    match schema {
        Schema::Ref { name } => {
            json!({"$ref":format!("#/components/schemas/{}", name.replace('~', "~0").replace('/', "~1"))})
        }
        Schema::String { checks, .. } | Schema::Number { checks, .. } => {
            let kind = match schema {
                Schema::String { .. } => "string",
                Schema::Number { int: true, .. } => "integer",
                _ => "number",
            };
            let mut value = json!({"type":kind});
            for (name, check) in serde_json::to_value(checks)
                .expect("checks serialize")
                .as_object()
                .expect("checks object")
            {
                value[name] = check
                    .as_f64()
                    .filter(|number| {
                        number.fract() == 0.0
                            && *number >= i64::MIN as f64
                            && *number < i64::MAX as f64
                    })
                    .map(|number| json!(number as i64))
                    .unwrap_or_else(|| check.clone());
            }
            value
        }
        Schema::Boolean => json!({"type":"boolean"}),
        Schema::Null => json!({"type":"null"}),
        Schema::Unknown => json!({}),
        Schema::Never => json!({"not":{}}),
        Schema::Literal { value } => {
            json!({"type":match value {Value::Null=>"null",Value::Bool(_)=>"boolean",Value::Number(_)=>"number",Value::String(_)=>"string",Value::Array(_)=>"array",Value::Object(_)=>"object"},"enum":[value]})
        }
        Schema::Array { items } => json!({"type":"array","items":emit(items)}),
        Schema::Tuple { elements, rest } => {
            let mut value = json!({"type":"array","prefixItems":elements.iter().map(emit).collect::<Vec<_>>(),"minItems":elements.len()});
            value["items"] = rest.as_ref().map(|item| emit(item)).unwrap_or(json!(false));
            if rest.is_none() {
                value["maxItems"] = json!(elements.len());
            }
            value
        }
        Schema::Record { key, value } => {
            let mut result = json!({"type":"object","additionalProperties":emit(value)});
            if !matches!(key.as_ref(), Schema::String { checks, .. } if checks.is_empty()) {
                result["propertyNames"] = emit(key);
            }
            result
        }
        Schema::Struct { fields, index } => {
            let properties = fields
                .iter()
                .map(|field| (field.name.clone(), field_schema(field)))
                .collect::<Map<_, _>>();
            let required = fields
                .iter()
                .filter(|field| field.optionality == Optionality::Required)
                .map(|field| field.name.clone())
                .collect::<Vec<_>>();
            let mut value = json!({"type":"object","properties":properties,"additionalProperties":index.as_ref().map(|(_,value)| emit(value)).unwrap_or(json!(false))});
            if !required.is_empty() {
                value["required"] = json!(required);
            }
            value
        }
        Schema::Union { members } => {
            let literals = members
                .iter()
                .map(|member| match member {
                    Schema::Literal { value } if value.is_string() => Some(value.clone()),
                    _ => None,
                })
                .collect::<Option<Vec<_>>>();
            match literals {
                Some(values) if !values.is_empty() => json!({"type":"string","enum":values}),
                _ => json!({"anyOf":members.iter().map(emit).collect::<Vec<_>>()}),
            }
        }
        Schema::Bytes => json!({"type":"string","format":"binary"}),
        Schema::DateTimeMillis => json!({"type":"number"}),
        Schema::Described {
            description,
            schema,
        } => {
            let mut value = emit(schema);
            value["description"] = json!(description);
            value
        }
    }
}

pub(super) fn field_schema(field: &Field) -> Value {
    let mut value = emit(&field.schema);
    if let Some(description) = &field.description {
        value["description"] = json!(description);
    }
    if let Some(default) = &field.default {
        value["default"] = default.clone();
    }
    value
}
