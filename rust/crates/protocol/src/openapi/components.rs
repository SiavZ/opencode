//! Purpose: merge per-API definitions without conflating same-named incompatible schemas.
//! Scope: component naming and reference rewriting for OpenAPI composition.
//! Uses: protocol/src/operation.rs, protocol/src/openapi/schema.rs
//! External: serde_json
//! Used by: protocol/src/openapi.rs
//! Invariants: collision suffixes are deterministic and each API retains its own reference meanings.
//! Contract tests: protocol/tests/openapi.rs
use super::schema;
use crate::operation::Api;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn merge(api: &Api, components: &mut Map<String, Value>) -> BTreeMap<String, String> {
    let values = api
        .definitions
        .iter()
        .map(|definition| {
            let mut value = schema::emit(&definition.schema);
            if let Some(title) = &definition.title {
                value["title"] = json!(title);
            }
            if let Some(description) = &definition.description {
                value["description"] = json!(description);
            }
            (definition.name.clone(), value)
        })
        .collect::<Vec<_>>();
    let mut names = values
        .iter()
        .map(|(name, _)| (name.clone(), name.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut reserved = components
        .keys()
        .chain(names.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    // A renamed child can make an otherwise identical parent distinct on the next pass.
    loop {
        let mut changed = false;
        for (name, value) in &values {
            let emitted = rewrite(value.clone(), &names);
            if components
                .get(&names[name])
                .is_none_or(|existing| *existing == emitted)
            {
                continue;
            }
            let mut suffix = 1;
            while reserved.contains(&format!("{name}{suffix}")) {
                suffix += 1;
            }
            let renamed = format!("{name}{suffix}");
            reserved.insert(renamed.clone());
            names.insert(name.clone(), renamed);
            changed = true;
        }
        if !changed {
            break;
        }
    }
    for (name, value) in values {
        components.insert(names[&name].clone(), rewrite(value, &names));
    }
    names
}

pub(super) fn rewrite(value: Value, names: &BTreeMap<String, String>) -> Value {
    match value {
        Value::Object(mut fields) => {
            if let Some(Value::String(reference)) = fields.get_mut("$ref")
                && let Some(name) = reference.strip_prefix("#/components/schemas/")
            {
                let decoded = name.replace("~1", "/").replace("~0", "~");
                if let Some(renamed) = names.get(&decoded) {
                    *reference = format!(
                        "#/components/schemas/{}",
                        renamed.replace('~', "~0").replace('/', "~1")
                    );
                }
            }
            Value::Object(
                fields
                    .into_iter()
                    .map(|(key, value)| (key, rewrite(value, names)))
                    .collect(),
            )
        }
        Value::Array(values) => Value::Array(
            values
                .into_iter()
                .map(|value| rewrite(value, names))
                .collect(),
        ),
        value => value,
    }
}
