//! Purpose: emit OpenAPI 3.1 from the actual Protocol operation registry.
//! Scope: pure registry transformation, no baseline files or runtime services.
//! Uses: protocol/src/operation.rs, protocol/src/ir.rs, protocol/src/openapi/schema.rs, protocol/src/openapi/endpoint.rs
//! External: serde_json
//! Used by: protocol/src/lib.rs, server/src/app.rs
//! Invariants: later APIs replace duplicate method/path entries; component references remain resolvable.
//! Contract tests: protocol/tests/openapi.rs
mod components;
mod endpoint;
mod schema;

use crate::operation::Api;
use serde_json::{Map, Value, json};

/// Merge APIs in registration order without consulting generated snapshots.
pub fn document(apis: &[&Api]) -> Value {
    let mut paths = Map::new();
    let mut components = Map::new();
    let mut tags = Vec::new();
    for api in apis {
        let names = components::merge(api, &mut components);
        for group in &api.groups {
            let name = group.title.as_ref().unwrap_or(&api.title);
            let mut tag = json!({"name":name});
            if let Some(description) = &group.description {
                tag["description"] = json!(description);
            }
            tags.push(tag);
            for operation in &group.endpoints {
                let path = operation
                    .path
                    .split('/')
                    .map(|segment| {
                        if segment == "*" {
                            return "{wildcard}".into();
                        }
                        segment
                            .strip_prefix(':')
                            .map(|name| format!("{{{name}}}"))
                            .unwrap_or_else(|| segment.into())
                    })
                    .collect::<Vec<_>>()
                    .join("/");
                let item = paths.entry(path).or_insert_with(|| json!({}));
                item[operation.method.as_str().to_ascii_lowercase()] =
                    components::rewrite(endpoint::emit(operation, name), &names);
            }
        }
    }
    let info = apis
        .first()
        .map(|api| {
            let mut info = json!({"title":api.title,"version":api.version});
            if let Some(description) = &api.description {
                info["description"] = json!(description);
            }
            info
        })
        .unwrap_or_else(|| json!({"title":"opencode","version":"0.0.0"}));
    json!({"openapi":"3.1.0","info":info,"paths":paths,"components":{"schemas":components},"tags":tags})
}
