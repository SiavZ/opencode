//! Purpose: conformance of the current Server API registry against the baseline OpenAPI document
//! (`packages/sdk/openapi.json`): every `v2.*` operation (method, path, params, query, payload,
//! success, errors, tag, docs) and every named component in their closure.
//! Scope: fixture-based (reads the committed baseline OpenAPI JSON), no I/O beyond that file.
//! Uses:
//! rust/crates/
//! |-- protocol/src/current/mod.rs
//! |-- protocol/src/ir.rs
//! |-- protocol/src/operation.rs
//! `-- protocol/tests/current_support/mod.rs
//! External:
//! |-- current::api
//! |-- serde_json::Value
//! `-- serde_json::json
//! Used by:
//! `-- none, entrypoint
//! Invariants: the IR is compared through a minimal Effect-style JSON Schema rendering
//! (`render`); documented baseline rendering quirks are listed in `KNOWN_DIVERGENCES`.
//! Contract tests: protocol/tests/current_registry.rs

mod current_support;

use current_support::*;
use opencode_protocol::current;
use opencode_protocol::ir::{Optionality, Schema};
use opencode_protocol::operation::{Endpoint, Middleware, SuccessEncoding};
use serde_json::{Value, json};
use std::collections::BTreeSet;

#[test]
fn every_v2_operation_is_registered_with_method_and_path() {
    let doc = baseline();
    let api = current::api();
    let ops = v2_operations(&doc);
    assert_eq!(ops.len(), api.endpoints().count(), "operation count");
    for (method, path, op) in &ops {
        let id = op["operationId"].as_str().unwrap();
        let (group, e) = api
            .endpoints()
            .find(|(_, e)| e.operation_id == id)
            .unwrap_or_else(|| panic!("{id} not registered"));
        assert_eq!(e.method.as_str(), method, "{id} method");
        assert_eq!(&openapi_path(&e.path), path, "{id} path");
        let tag = group.title.as_deref().unwrap_or("opencode HttpApi");
        assert_eq!(op["tags"][0], json!(tag), "{id} tag");
        assert_eq!(
            op.get("summary"),
            e.summary.as_ref().map(|s| json!(s)).as_ref(),
            "{id} summary"
        );
        assert_eq!(
            op.get("description"),
            e.description.as_ref().map(|s| json!(s)).as_ref(),
            "{id} description"
        );
    }
}

#[test]
fn group_order_matches_make_api() {
    let ids: Vec<_> = current::api()
        .groups
        .iter()
        .map(|g| g.identifier.trim_start_matches("server."))
        .collect();
    assert_eq!(
        ids,
        [
            "health",
            "location",
            "agent",
            "session",
            "message",
            "model",
            "provider",
            "integration",
            "credential",
            "permission",
            "fs",
            "command",
            "skill",
            "event",
            "pty",
            "question",
            "reference",
            "projectCopy"
        ]
    );
}

#[test]
fn params_query_and_bodies_match_baseline() {
    let doc = baseline();
    let api = current::api();
    for (_, _, op) in v2_operations(&doc) {
        let id = op["operationId"].as_str().unwrap();
        let e = api
            .endpoints()
            .map(|(_, e)| e)
            .find(|e| e.operation_id == id)
            .unwrap();
        let params = op["parameters"].as_array().cloned().unwrap_or_default();
        let named = |place: &str| -> Vec<(String, bool)> {
            params
                .iter()
                .filter(|p| p["in"] == place)
                .map(|p| {
                    (
                        p["name"].as_str().unwrap().to_string(),
                        p["required"] == json!(true),
                    )
                })
                .collect()
        };
        let path_params: Vec<_> = e.params.iter().map(|f| (f.name.clone(), true)).collect();
        assert_eq!(path_params, named("path"), "{id} path params");
        for p in params.iter().filter(|p| p["in"] == "path") {
            let f = e
                .params
                .iter()
                .find(|f| p["name"] == json!(f.name))
                .unwrap();
            assert_eq!(render(&f.schema), p["schema"], "{id} param {}", f.name);
        }
        if !divergent(id) {
            let query: Vec<_> = e
                .query
                .iter()
                .map(|f| (f.name.clone(), f.optionality == Optionality::Required))
                .collect();
            assert_eq!(query, named("query"), "{id} query");
            for p in params.iter().filter(|p| p["in"] == "query") {
                let name = p["name"].as_str().unwrap();
                assert_eq!(
                    p.get("style") == Some(&json!("deepObject")),
                    e.deep_object_query.iter().any(|d| d == name),
                    "{id} deepObject {name}"
                );
            }
        }
        let body = &op["requestBody"]["content"]["application/json"]["schema"];
        match &e.payload {
            Some(s) => assert_schema(&format!("{id} payload"), &render(s), body),
            None => assert!(body.is_null(), "{id} has a baseline payload"),
        }
        check_success(id, e, &op);
    }
}

fn check_success(id: &str, e: &Endpoint, op: &Value) {
    if divergent(id) {
        return;
    }
    let s = &e.successes[0];
    let response = &op["responses"][s.status.to_string()];
    assert!(response.is_object(), "{id} success status {}", s.status);
    match s.encoding {
        SuccessEncoding::Json => assert_schema(
            &format!("{id} success"),
            &render(s.schema.as_ref().unwrap()),
            &response["content"]["application/json"]["schema"],
        ),
        SuccessEncoding::Void => assert!(response.get("content").is_none(), "{id} 204 body"),
        SuccessEncoding::Bytes => assert!(
            response["content"]["application/octet-stream"].is_object(),
            "{id} octet-stream"
        ),
        SuccessEncoding::Sse => {
            let mut found = BTreeSet::new();
            refs(
                &response["content"]["text/event-stream"]["schema"],
                &mut found,
            );
            let mut ours = BTreeSet::new();
            refs(&render(s.schema.as_ref().unwrap()), &mut ours);
            assert_eq!(ours, found, "{id} sse data");
        }
        SuccessEncoding::WebSocket => {}
    }
}

#[test]
fn errors_match_baseline_including_middleware() {
    let doc = baseline();
    let api = current::api();
    for (_, _, op) in v2_operations(&doc) {
        let id = op["operationId"].as_str().unwrap();
        let e = api
            .endpoints()
            .map(|(_, e)| e)
            .find(|e| e.operation_id == id)
            .unwrap();
        let mut expected = BTreeSet::new();
        for (status, response) in op["responses"].as_object().unwrap() {
            if status.starts_with('2') {
                continue;
            }
            let mut names = BTreeSet::new();
            refs(response, &mut names);
            expected.extend(names.into_iter().map(|n| (status.clone(), n)));
        }
        let mut ours = BTreeSet::new();
        let mut add = |status: u16, name: &str| {
            ours.insert((status.to_string(), component_key(name).to_string()));
        };
        for err in &e.errors {
            let Schema::Ref { name } = &err.schema else {
                panic!("{id}: inline error schema")
            };
            add(err.status, name);
        }
        for m in &e.middleware {
            match m {
                Middleware::Auth if !e.path.starts_with("/experimental/") => {
                    add(401, "UnauthorizedError")
                }
                Middleware::SchemaError => add(400, "InvalidRequestError"),
                Middleware::SessionLocation => {
                    add(400, "InvalidRequestError");
                    add(404, "SessionNotFoundError");
                }
                _ => {}
            }
        }
        assert_eq!(ours, expected, "{id} errors");
    }
}

#[test]
fn session_scoped_endpoints_use_session_location() {
    for (_, e) in current::api().endpoints() {
        let scoped = e.path.starts_with("/api/session/:sessionID");
        assert_eq!(
            e.middleware.contains(&Middleware::SessionLocation),
            scoped,
            "{} SessionLocation",
            e.name
        );
        if scoped {
            assert!(!e.middleware.contains(&Middleware::Location), "{}", e.name);
        }
        let last = &e.middleware[e.middleware.len() - 2..];
        assert_eq!(
            last,
            [Middleware::Auth, Middleware::SchemaError],
            "{}",
            e.name
        );
    }
}

#[test]
fn component_definitions_match_baseline() {
    let doc = baseline();
    let components = doc["components"]["schemas"].as_object().unwrap();
    let api = current::api();
    let mut closure = BTreeSet::new();
    for (_, _, op) in v2_operations(&doc) {
        refs(&op, &mut closure);
    }
    let mut todo: Vec<String> = closure.iter().cloned().collect();
    while let Some(name) = todo.pop() {
        if EVENT_OWNED.contains(&name.as_str()) {
            continue;
        }
        let mut next = BTreeSet::new();
        refs(&components[&name], &mut next);
        for n in next {
            if closure.insert(n.clone()) {
                todo.push(n);
            }
        }
    }
    let mut checked = 0;
    for name in closure
        .iter()
        .filter(|n| !EVENT_OWNED.contains(&n.as_str()))
    {
        let ours = if name == "UnknownError1" {
            "UnknownError"
        } else {
            name.as_str()
        };
        let def = api
            .definition(ours)
            .unwrap_or_else(|| panic!("component {name} not defined"));
        assert_schema(name, &render(&def.schema), &components[name]);
        checked += 1;
    }
    assert!(checked > 90, "only {checked} components checked");
}

#[test]
fn definition_names_are_unique_and_identifiers_keep_dots() {
    let api = current::api();
    let mut names = BTreeSet::new();
    for d in &api.definitions {
        assert!(
            names.insert(d.name.clone()),
            "duplicate definition {}",
            d.name
        );
    }
    let info = api.definition("SessionV2Info").unwrap();
    assert_eq!(info.identifier.as_deref(), Some("SessionV2.Info"));
}
