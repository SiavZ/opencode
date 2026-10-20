//! Purpose: verify public OpenAPI output against registry operations and baseline wire shapes.
//! Scope: public document integration tests, no production snapshot loading.
//! Uses: protocol/src/openapi.rs, protocol/src/current/mod.rs, protocol/src/legacy/mod.rs
//! External: serde_json
//! Used by: cargo test
//! Invariants: every operation and reference survives emission, empty stubs cannot pass.
//! Contract tests: this file
use opencode_protocol::{current, ir, legacy, openapi, operation};
use serde_json::{Value, json};

#[test]
fn baseline_path_and_operation_inventory_is_not_a_stub() {
    let actual = openapi::document(&[legacy::api(), current::api()]);
    let baseline: Value =
        serde_json::from_str(include_str!("../../../../packages/sdk/openapi.json")).unwrap();
    for (path, methods) in baseline["paths"].as_object().unwrap() {
        for (method, endpoint) in methods.as_object().unwrap() {
            // The SDK snapshot keeps this trailing wildcard literal; the emitter
            // represents it as an OpenAPI path parameter.
            let emitted_path = if path.ends_with("/*") {
                path.replace("/*", "/{wildcard}")
            } else {
                path.clone()
            };
            assert_eq!(
                actual["paths"][&emitted_path][method]["operationId"], endpoint["operationId"],
                "{method} {path}"
            );
        }
    }
}

#[test]
fn emits_payload_encodings_deep_query_and_middleware_status_unions() {
    let endpoint = operation::Endpoint::new(
        "example",
        "example",
        operation::Method::Post,
        "/example/:id/*",
    )
    .params(vec![ir::req("id", ir::string())])
    .query(vec![ir::opt(
        "location",
        ir::object(vec![ir::opt("directory", ir::string())]),
    )])
    .deep("location")
    .headers(vec![ir::opt("x-test", ir::string())])
    .payload(ir::string())
    .payload_encoding(operation::PayloadEncoding::Text)
    .sse(ir::r("Event"))
    .no_content()
    .websocket()
    .error(400, ir::r("FirstError"))
    .middleware(operation::Middleware::SchemaError)
    .middleware(operation::Middleware::SchemaError)
    .deprecated();
    let api = operation::Api {
        identifier: "test".into(),
        title: "test".into(),
        version: "1".into(),
        description: None,
        definitions: vec![],
        groups: vec![operation::Group::new("group", Some("tag"), None).endpoint(endpoint)],
    };
    let document = openapi::document(&[&api]);
    let operation = &document["paths"]["/example/{id}/{wildcard}"]["post"];
    assert_eq!(operation["parameters"][1]["style"], "deepObject");
    assert_eq!(operation["parameters"][1]["explode"], true);
    assert_eq!(operation["parameters"][3]["name"], "wildcard");
    assert_eq!(
        operation["requestBody"]["content"]["text/plain"]["schema"]["type"],
        "string"
    );
    assert_eq!(
        operation["responses"]["200"]["content"]["text/event-stream"]["schema"]["$ref"],
        "#/components/schemas/Event"
    );
    assert!(operation["responses"]["204"]["content"].is_null());
    assert!(operation["responses"]["101"]["content"].is_null());
    assert_eq!(
        operation["responses"]["400"]["content"]["application/json"]["schema"]["anyOf"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(operation["deprecated"], true);
}

#[test]
fn emits_every_registered_operation_and_resolvable_component() {
    let apis = [current::api(), legacy::api()];
    let document = openapi::document(&apis);
    let mut count = 0;
    for api in apis {
        for (_, endpoint) in api.endpoints() {
            let path = endpoint
                .path
                .split('/')
                .map(|part| {
                    part.strip_prefix(':')
                        .map(|name| format!("{{{name}}}"))
                        .unwrap_or_else(|| {
                            if part == "*" {
                                "{wildcard}".into()
                            } else {
                                part.into()
                            }
                        })
                })
                .collect::<Vec<_>>()
                .join("/");
            let emitted = &document["paths"][path][endpoint.method.as_str().to_ascii_lowercase()];
            assert_eq!(emitted["operationId"], endpoint.operation_id);
            assert!(
                emitted["responses"]
                    .as_object()
                    .is_some_and(|map| !map.is_empty())
            );
            count += 1;
        }
    }
    assert!(count >= 187);
    assert!(document["paths"].as_object().unwrap().len() >= 160);
    check_refs(&document, &document);
    assert_eq!(document, openapi::document(&apis));
}

fn check_refs(value: &Value, document: &Value) {
    match value {
        Value::Object(fields) => {
            if let Some(reference) = fields.get("$ref").and_then(Value::as_str) {
                assert!(
                    document
                        .pointer(reference.strip_prefix('#').unwrap())
                        .is_some(),
                    "unresolved {reference}"
                );
            }
            fields
                .values()
                .for_each(|value| check_refs(value, document));
        }
        Value::Array(values) => values.iter().for_each(|value| check_refs(value, document)),
        _ => {}
    }
}

#[test]
fn real_current_health_matches_baseline_response_and_error_shapes() {
    let document = openapi::document(&[current::api()]);
    let baseline: Value =
        serde_json::from_str(include_str!("../../../../packages/sdk/openapi.json")).unwrap();
    let actual = &document["paths"]["/api/health"]["get"];
    let expected = &baseline["paths"]["/api/health"]["get"];
    for key in [
        "operationId",
        "summary",
        "description",
        "parameters",
        "responses",
    ] {
        assert_eq!(actual[key], expected[key], "health {key}");
    }
    assert!(document["paths"]["/auth/{providerID}"].is_null());
}

#[test]
fn preserves_optional_nullable_defaults_constraints_and_variadic_tuples() {
    let api = operation::Api {
        identifier: "test".into(),
        title: "test".into(),
        version: "1".into(),
        description: None,
        groups: vec![],
        definitions: vec![ir::def(
            "Example",
            ir::object(vec![
                ir::req("id", ir::id("^ses", "SessionID")),
                ir::exact("nullable", ir::union(vec![ir::string(), ir::Schema::Null]))
                    .default_value(json!(null)),
                ir::req(
                    "tuple",
                    ir::Schema::Tuple {
                        elements: vec![ir::int()],
                        rest: Some(Box::new(ir::boolean())),
                    },
                ),
                ir::opt("never", ir::Schema::Never),
            ]),
        )],
    };
    let document = openapi::document(&[&api]);
    let schema = &document["components"]["schemas"]["Example"];
    assert_eq!(schema["required"], json!(["id", "tuple"]));
    assert_eq!(schema["properties"]["id"]["pattern"], "^ses");
    assert_eq!(schema["properties"]["nullable"]["default"], json!(null));
    assert_eq!(schema["properties"]["nullable"]["anyOf"][1]["type"], "null");
    assert_eq!(
        schema["properties"]["tuple"]["prefixItems"][0]["type"],
        "integer"
    );
    assert_eq!(schema["properties"]["tuple"]["items"]["type"], "boolean");
    assert_eq!(schema["properties"]["never"], json!({"not":{}}));
}
