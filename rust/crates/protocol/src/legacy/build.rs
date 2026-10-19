//! Purpose: shared legacy endpoint constructors preserving routing middleware and schema fields.
//! Scope: static protocol construction.
//! Uses:
//! rust/crates/
//! |-- protocol/src/ir.rs
//! `-- protocol/src/operation.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- protocol/src/legacy/mod.rs
//! Invariants: instance operations resolve workspace before decoding; global operations do not.
//! Contract tests: protocol/tests/registry.rs

use crate::ir::{self, Field, Schema};
use crate::operation::{Endpoint, Group, Method, Middleware};

pub(super) fn endpoint(name: &str, method: Method, path: &str) -> Endpoint {
    Endpoint::new(name, name, method, path)
        .params(
            path.split('/')
                .filter_map(|part| part.strip_prefix(':'))
                .map(|name| {
                    ir::req(
                        name,
                        match name {
                            "sessionID" => ir::id("^ses", "SessionID"),
                            "messageID" => ir::id("^msg", "MessageID"),
                            "partID" => ir::id("^prt", "PartID"),
                            "ptyID" => ir::id("^pty", "PtyID"),
                            "workspaceID" => ir::id("^wrk", "WorkspaceID"),
                            _ => ir::string(),
                        },
                    )
                })
                .collect(),
        )
        .query(super::routing())
        .middleware(Middleware::LegacyInstance)
        .ok(ir::unknown())
}

pub(super) fn group(name: &str, endpoints: Vec<Endpoint>) -> Group {
    let mut group = Group::new(name, Some(name), None);
    group.legacy = true;
    group.endpoints = endpoints
        .into_iter()
        .map(|mut endpoint| {
            if endpoint
                .successes
                .iter()
                .any(|success| success.encoding == crate::operation::SuccessEncoding::Void)
            {
                endpoint
                    .successes
                    .retain(|success| success.encoding == crate::operation::SuccessEncoding::Void);
            }
            endpoint
        })
        .collect();
    group
}

pub(super) fn query(mut endpoint: Endpoint, fields: Vec<Field>) -> Endpoint {
    endpoint.query.extend(fields);
    endpoint
}

pub(super) fn optional(mut endpoint: Endpoint, payload: Schema) -> Endpoint {
    endpoint = endpoint.payload(payload);
    endpoint.payload_required = false;
    endpoint
}
