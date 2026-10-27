//! Purpose: emit operation parameters, encoded bodies and middleware response unions.
//! Scope: pure OpenAPI operation objects.
//! Uses: protocol/src/operation.rs, protocol/src/ir.rs, protocol/src/openapi/schema.rs
//! External: serde_json
//! Used by: protocol/src/openapi.rs
//! Invariants: status collisions are unions, never silently discarded error alternatives.
//! Contract tests: protocol/tests/openapi.rs
use super::schema;
use crate::ir::{self, Optionality, Schema};
use crate::operation::{Endpoint, Middleware, PayloadEncoding, SuccessEncoding};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

pub(super) fn emit(endpoint: &Endpoint, tag: &str) -> Value {
    let mut parameters = Vec::new();
    for (location, fields) in [
        ("path", &endpoint.params),
        ("query", &endpoint.query),
        ("header", &endpoint.headers),
    ] {
        for field in fields {
            let mut value = json!({"name":field.name,"in":location,"schema":schema::field_schema(field),"required":location == "path" || field.optionality == Optionality::Required});
            if location == "query" && endpoint.deep_object_query.contains(&field.name) {
                value["style"] = json!("deepObject");
                value["explode"] = json!(true);
            }
            parameters.push(value);
        }
    }
    if endpoint.path.split('/').any(|segment| segment == "*")
        && !endpoint.params.iter().any(|field| field.name == "wildcard")
    {
        parameters.push(
            json!({"name":"wildcard","in":"path","required":true,"schema":{"type":"string"}}),
        );
    }
    let mut responses = Map::new();
    for success in &endpoint.successes {
        let description = success
            .description
            .as_deref()
            .unwrap_or(match success.encoding {
                SuccessEncoding::Sse => "Event stream",
                SuccessEncoding::Void => "No Content",
                SuccessEncoding::WebSocket => "Switching Protocols",
                _ => "Success",
            });
        let mut response = json!({"description":description});
        let media = match success.encoding {
            SuccessEncoding::Json => Some("application/json"),
            SuccessEncoding::Sse => Some("text/event-stream"),
            SuccessEncoding::Bytes => Some("application/octet-stream"),
            _ => None,
        };
        if let Some(media) = media {
            response["content"] = json!({media:{"schema":success.schema.as_ref().map(schema::emit).unwrap_or_else(|| schema::emit(&Schema::Bytes))}});
        }
        responses.insert(success.status.to_string(), response);
    }
    let mut errors = endpoint
        .errors
        .iter()
        .map(|error| (error.status, error.schema.clone()))
        .collect::<Vec<_>>();
    for middleware in &endpoint.middleware {
        match middleware {
            Middleware::Auth => errors.push((401, ir::r("UnauthorizedError"))),
            Middleware::SchemaError => errors.push((400, ir::r("InvalidRequestError"))),
            Middleware::SessionLocation => {
                errors.push((400, ir::r("InvalidRequestError")));
                errors.push((404, ir::r("SessionNotFoundError")));
            }
            _ => {}
        }
    }
    let mut statuses = BTreeMap::<u16, Vec<Schema>>::new();
    for (status, error) in errors {
        let entries = statuses.entry(status).or_default();
        if !entries.contains(&error) {
            entries.push(error);
        }
    }
    for (status, schemas) in statuses {
        let description = schemas
            .iter()
            .map(|schema| match schema {
                Schema::Ref { name } => name.as_str(),
                _ => "Error",
            })
            .collect::<Vec<_>>()
            .join(" | ");
        let value = if schemas.len() == 1 {
            schema::emit(&schemas[0])
        } else {
            schema::emit(&Schema::Union { members: schemas })
        };
        responses.insert(
            status.to_string(),
            json!({"description":description,"content":{"application/json":{"schema":value}}}),
        );
    }
    let mut value = json!({"tags":[tag],"operationId":endpoint.operation_id,"parameters":parameters,"security":[],"responses":responses});
    if let Some(summary) = &endpoint.summary {
        value["summary"] = json!(summary);
    }
    if let Some(description) = &endpoint.description {
        value["description"] = json!(description);
    }
    if endpoint.deprecated {
        value["deprecated"] = json!(true);
    }
    if let Some(payload) = &endpoint.payload {
        let media = match endpoint.payload_encoding {
            PayloadEncoding::Json => "application/json",
            PayloadEncoding::Multipart => "multipart/form-data",
            PayloadEncoding::Form => "application/x-www-form-urlencoded",
            PayloadEncoding::Text => "text/plain",
            PayloadEncoding::Bytes => "application/octet-stream",
        };
        value["requestBody"] = json!({"required":endpoint.payload_required,"content":{media:{"schema":schema::emit(payload)}}});
    }
    value
}
