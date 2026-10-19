//! Purpose: legacy workspace orchestration and event synchronization route schemas.
//! Scope: static protocol construction.
//! Uses:
//! rust/crates/
//! |-- protocol/src/ir.rs
//! |-- protocol/src/legacy/build.rs
//! `-- protocol/src/operation.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- protocol/src/legacy/mod.rs
//! Invariants: workspace branch and extra remain nullable; replay requires at least one event.
//! Contract tests: protocol/tests/registry.rs

use super::build::{endpoint, group};
use crate::ir::{self, Schema};
use crate::operation::{Endpoint, Group, Method};

pub(super) fn groups() -> Vec<Group> {
    vec![
        group(
            "workspace",
            vec![
                endpoint(
                    "experimental.workspace.adapter.list",
                    Method::Get,
                    "/experimental/workspace/adapter",
                ),
                endpoint(
                    "experimental.workspace.list",
                    Method::Get,
                    "/experimental/workspace",
                ),
                endpoint(
                    "experimental.workspace.create",
                    Method::Post,
                    "/experimental/workspace",
                )
                .payload(ir::object(vec![
                    ir::opt("id", ir::id("^wrk", "WorkspaceID")),
                    ir::req("type", ir::string()),
                    ir::req("branch", ir::union(vec![Schema::Null, ir::string()])),
                    ir::opt("extra", ir::unknown()),
                ])),
                endpoint(
                    "experimental.workspace.syncList",
                    Method::Post,
                    "/experimental/workspace/sync-list",
                )
                .no_content(),
                endpoint(
                    "experimental.workspace.status",
                    Method::Get,
                    "/experimental/workspace/status",
                ),
                endpoint(
                    "experimental.workspace.remove",
                    Method::Delete,
                    "/experimental/workspace/:id",
                ),
                endpoint(
                    "experimental.workspace.warp",
                    Method::Post,
                    "/experimental/workspace/warp",
                )
                .payload(ir::object(vec![
                    ir::req(
                        "id",
                        ir::union(vec![Schema::Null, ir::id("^wrk", "WorkspaceID")]),
                    ),
                    ir::req("sessionID", ir::id("^ses", "SessionID")),
                    ir::opt("copyChanges", ir::boolean()),
                ]))
                .no_content(),
            ],
        ),
        group(
            "controlPlane",
            vec![
                Endpoint::new(
                    "experimental.controlPlane.moveSession",
                    "experimental.controlPlane.moveSession",
                    Method::Post,
                    "/experimental/control-plane/move-session",
                )
                .payload(ir::object(vec![
                    ir::req("sessionID", ir::id("^ses", "SessionID")),
                    ir::req(
                        "destination",
                        ir::object(vec![ir::req(
                            "directory",
                            crate::definitions::absolute_path(),
                        )]),
                    ),
                    ir::opt("moveChanges", ir::boolean()),
                ]))
                .no_content(),
            ],
        ),
        group(
            "sync",
            vec![
                endpoint("sync.start", Method::Post, "/sync/start"),
                endpoint("sync.replay", Method::Post, "/sync/replay").payload(ir::object(vec![
                    ir::req("directory", ir::string()),
                    ir::req(
                        "events",
                        Schema::Tuple {
                            elements: vec![event()],
                            rest: Some(Box::new(event())),
                        },
                    ),
                ])),
                endpoint("sync.steal", Method::Post, "/sync/steal").payload(ir::object(vec![
                    ir::req("sessionID", ir::id("^ses", "SessionID")),
                ])),
                endpoint("sync.history.list", Method::Post, "/sync/history")
                    .payload(ir::record(ir::string(), nonnegative())),
            ],
        ),
    ]
}

fn nonnegative() -> Schema {
    Schema::Number {
        int: true,
        checks: ir::Checks {
            minimum: Some(0.0),
            ..Default::default()
        },
        from_string: false,
    }
}

fn event() -> Schema {
    ir::object(vec![
        ir::req("id", ir::string()),
        ir::req("aggregateID", ir::string()),
        ir::req("seq", nonnegative()),
        ir::req("type", ir::string()),
        ir::req("data", ir::record(ir::string(), ir::unknown())),
    ])
}
