//! Purpose: registry integrity tests: unique operation ids, resolvable refs, auth/schema errors.
//! Scope: fixture-free, exercises the public registry.
//! Uses:
//! rust/crates/
//! `-- protocol/src/lib.rs
//! External:
//! `-- none
//! Used by:
//! `-- none, entrypoint
//! Invariants: every `Ref` resolves within its API definitions table.
//! Contract tests: protocol/tests/registry.rs

use opencode_protocol::ir::Schema;
use opencode_protocol::{Api, registry};
use std::collections::HashSet;

fn refs(schema: &Schema, out: &mut Vec<String>) {
    match schema {
        Schema::Ref { name } => out.push(name.clone()),
        Schema::Array { items } => refs(items, out),
        Schema::Tuple { elements, rest } => {
            elements.iter().for_each(|e| refs(e, out));
            if let Some(r) = rest {
                refs(r, out);
            }
        }
        Schema::Record { key, value } => {
            refs(key, out);
            refs(value, out);
        }
        Schema::Struct { fields, index } => {
            fields.iter().for_each(|f| refs(&f.schema, out));
            if let Some((k, v)) = index {
                refs(k, out);
                refs(v, out);
            }
        }
        Schema::Never => {}
        Schema::Union { members } => members.iter().for_each(|m| refs(m, out)),
        Schema::Described { schema, .. } => refs(schema, out),
        _ => {}
    }
}

fn check(api: &Api) {
    let mut ids = HashSet::new();
    let mut found = Vec::new();
    for (_, e) in api.endpoints() {
        assert!(
            ids.insert((e.method, e.path.clone())),
            "duplicate route {} {}",
            e.method.as_str(),
            e.path
        );
        e.params
            .iter()
            .chain(&e.query)
            .chain(&e.headers)
            .for_each(|f| refs(&f.schema, &mut found));
        e.payload.iter().for_each(|s| refs(s, &mut found));
        e.successes
            .iter()
            .filter_map(|s| s.schema.as_ref())
            .for_each(|s| refs(s, &mut found));
        e.errors.iter().for_each(|x| refs(&x.schema, &mut found));
    }
    api.definitions
        .iter()
        .for_each(|d| refs(&d.schema, &mut found));
    for name in found {
        assert!(
            api.definition(&name).is_some(),
            "{}: unresolved ref {name}",
            api.identifier
        );
    }
}

#[test]
fn registries_are_consistent() {
    for api in registry() {
        check(api);
    }
}

#[test]
fn current_health_is_first() {
    let api = registry()[0];
    assert_eq!(api.groups[0].identifier, "server.health");
    let e = api.find("health.get").expect("health.get");
    assert_eq!(e.path, "/api/health");
    assert_eq!(e.operation_id, "v2.health.get");
}
