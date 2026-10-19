//! Purpose: legacy PublicApi registry for implemented global, event and Session boundaries.
//! Scope: static registry construction.
//! Uses:
//! rust/crates/
//! |-- protocol/src/ir.rs
//! |-- protocol/src/legacy/schema.rs
//! `-- protocol/src/operation.rs
//! External:
//! |-- ir::array
//! |-- ir::boolean
//! |-- ir::id
//! |-- ir::int_from_string
//! |-- ir::lit
//! |-- ir::lits
//! |-- ir::object
//! |-- ir::opt
//! |-- ir::r
//! |-- ir::req
//! |-- ir::string
//! `-- ir::unknown
//! Used by:
//! rust/crates/
//! `-- protocol/src/lib.rs
//! Invariants: legacy wire envelopes are independent of current API; unsupported families are not advertised.
//! Contract tests: protocol/tests/registry.rs

mod build;
mod core;
mod experimental;
mod integration;
mod native;
mod schema;
mod session;
mod tui;
mod workspace;

use crate::ir::{self, Field, Schema};
use crate::operation::{Api, Endpoint, Group, Method, Middleware};
use std::sync::OnceLock;

pub fn api() -> &'static Api {
    static API: OnceLock<Api> = OnceLock::new();
    API.get_or_init(|| {
        let mut global = Group::new("global", Some("global"), None)
            .endpoint(
                Endpoint::new(
                    "global.health",
                    "global.health",
                    Method::Get,
                    "/global/health",
                )
                .ok(ir::object(vec![
                    ir::req("healthy", ir::lit(true)),
                    ir::req("version", ir::string()),
                ])),
            )
            .endpoint(
                Endpoint::new("global.event", "global.event", Method::Get, "/global/event")
                    .sse(ir::unknown()),
            );
        global.legacy = true;
        let mut event = Group::new("event", Some("event"), None).endpoint(
            Endpoint::new("event.subscribe", "event.subscribe", Method::Get, "/event")
                .query(routing())
                .middleware(Middleware::LegacyInstance)
                .sse(ir::unknown()),
        );
        event.legacy = true;
        let mut session = Group::new("session", Some("session"), None);
        session.legacy = true;
        let list_query = routing()
            .into_iter()
            .chain([
                ir::opt("scope", ir::lit("project")),
                ir::opt("path", ir::string()),
                ir::opt("roots", ir::lits(&["true", "false"])),
                ir::opt("start", numeric()),
                ir::opt("search", ir::string()),
                ir::opt("limit", numeric()),
            ])
            .collect();
        session.endpoints.push(
            Endpoint::new("session.list", "session.list", Method::Get, "/session")
                .query(list_query)
                .middleware(Middleware::LegacyInstance)
                .ok(ir::array(ir::r("LegacySession"))),
        );
        let mut create =
            Endpoint::new("session.create", "session.create", Method::Post, "/session")
                .query(routing())
                .payload(ir::object(vec![
                    ir::opt("parentID", ir::id("^ses", "SessionID")),
                    ir::opt("title", ir::string()),
                    ir::opt("agent", ir::string()),
                    ir::opt("model", schema::model()),
                    ir::opt("metadata", ir::record(ir::string(), ir::unknown())),
                    ir::opt("workspaceID", ir::id("^wrk", "WorkspaceID")),
                    ir::opt("permission", schema::permissions()),
                ]))
                .middleware(Middleware::LegacyInstance)
                .ok(ir::r("LegacySession"))
                .error(400, ir::r("LegacyBadRequest"));
        create.payload_required = false;
        session.endpoints.push(create);
        for (name, method, path) in [
            ("get", Method::Get, "/session/:sessionID"),
            ("status", Method::Get, "/session/status"),
            ("messages", Method::Get, "/session/:sessionID/message"),
            (
                "message",
                Method::Get,
                "/session/:sessionID/message/:messageID",
            ),
            ("abort", Method::Post, "/session/:sessionID/abort"),
        ] {
            let params = ["sessionID", "messageID"]
                .into_iter()
                .filter(|name| path.contains(&format!(":{name}")))
                .map(|name| {
                    ir::req(
                        name,
                        ir::id(if name == "sessionID" { "^ses" } else { "^msg" }, name),
                    )
                })
                .collect();
            let query = if name == "messages" {
                routing()
                    .into_iter()
                    .chain([
                        ir::opt("limit", ir::int_from_string(Some(0.0), None)),
                        ir::opt("before", ir::string()),
                    ])
                    .collect()
            } else {
                routing()
            };
            session.endpoints.push(
                Endpoint::new(
                    &format!("session.{name}"),
                    &format!("session.{name}"),
                    method,
                    path,
                )
                .params(params)
                .query(query)
                .middleware(Middleware::LegacyInstance)
                .ok(if name == "get" {
                    ir::r("LegacySession")
                } else if name == "abort" {
                    ir::boolean()
                } else {
                    ir::unknown()
                })
                .error(400, ir::r("LegacyBadRequest"))
                .error(404, ir::r("LegacyNotFoundError")),
            );
        }
        global.endpoints.extend(core::global());
        session.endpoints.extend(session::endpoints());
        let mut groups = vec![global, event, build::group("session", session.endpoints)];
        groups.extend(core::groups());
        groups.extend(integration::groups());
        groups.extend(native::groups());
        groups.push(experimental::group_definition());
        groups.push(tui::group_definition());
        groups.extend(workspace::groups());
        let mut definitions = schema::definitions();
        for endpoint in groups.iter_mut().flat_map(|group| &mut group.endpoints) {
            // Constructors start with an unknown 200; a later explicit schema replaces it.
            let mut statuses = std::collections::HashSet::new();
            endpoint.successes.reverse();
            endpoint
                .successes
                .retain(|success| statuses.insert(success.status));
            endpoint.successes.reverse();
            let name = match endpoint.name.as_str() {
                "auth.set" => "Auth",
                "config.update" | "global.config.update" => "Config",
                "part.update" => "Part",
                "worktree.create" => "WorktreeCreateInput",
                "worktree.remove" => "WorktreeRemoveInput",
                "worktree.reset" => "WorktreeResetInput",
                _ => continue,
            };
            if let Some(payload) = endpoint.payload.take() {
                if !definitions.iter().any(|definition| definition.name == name) {
                    definitions.push(ir::def(name, payload));
                }
                endpoint.payload = Some(ir::r(name));
            }
        }
        Api {
            identifier: "opencode".into(),
            title: "opencode".into(),
            version: "1.0.0".into(),
            description: Some("opencode api".into()),
            groups,
            definitions,
        }
    })
}

fn routing() -> Vec<Field> {
    vec![
        ir::opt("directory", ir::string()),
        ir::opt("workspace", ir::string()),
    ]
}

fn numeric() -> Schema {
    Schema::Number {
        int: false,
        checks: Default::default(),
        from_string: true,
    }
}
