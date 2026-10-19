//! Purpose: legacy experimental capabilities, tool, worktree and Session discovery routes.
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
//! Invariants: optional worktree creation body differs from required removal and reset bodies.
//! Contract tests: protocol/tests/registry.rs

use super::build::{endpoint, group, optional, query};
use crate::ir;
use crate::operation::{Group, Method};

pub(super) fn group_definition() -> Group {
    group(
        "experimental",
        vec![
            endpoint(
                "experimental.capabilities.get",
                Method::Get,
                "/experimental/capabilities",
            ),
            endpoint(
                "experimental.console.get",
                Method::Get,
                "/experimental/console",
            ),
            endpoint(
                "experimental.console.listOrgs",
                Method::Get,
                "/experimental/console/orgs",
            ),
            endpoint(
                "experimental.console.switchOrg",
                Method::Post,
                "/experimental/console/switch",
            )
            .payload(ir::object(vec![
                ir::req("accountID", ir::string()),
                ir::req("orgID", ir::string()),
            ])),
            query(
                endpoint("tool.list", Method::Get, "/experimental/tool"),
                vec![
                    ir::req("provider", ir::string()),
                    ir::req("model", ir::string()),
                ],
            ),
            endpoint("tool.ids", Method::Get, "/experimental/tool/ids"),
            endpoint("worktree.list", Method::Get, "/experimental/worktree"),
            optional(
                endpoint("worktree.create", Method::Post, "/experimental/worktree"),
                ir::object(vec![
                    ir::opt("name", ir::string()),
                    ir::opt("startCommand", ir::string()),
                ]),
            ),
            endpoint("worktree.remove", Method::Delete, "/experimental/worktree")
                .payload(ir::object(vec![ir::req("directory", ir::string())])),
            endpoint(
                "worktree.reset",
                Method::Post,
                "/experimental/worktree/reset",
            )
            .payload(ir::object(vec![ir::req("directory", ir::string())])),
            query(
                endpoint(
                    "experimental.session.list",
                    Method::Get,
                    "/experimental/session",
                ),
                vec![
                    ir::opt("roots", ir::lits(&["true", "false"])),
                    ir::opt("start", super::numeric()),
                    ir::opt("cursor", super::numeric()),
                    ir::opt("search", ir::string()),
                    ir::opt("limit", super::numeric()),
                    ir::opt("archived", ir::lits(&["true", "false"])),
                ],
            ),
            endpoint(
                "experimental.session.background",
                Method::Post,
                "/experimental/session/:sessionID/background",
            ),
            endpoint(
                "experimental.resource.list",
                Method::Get,
                "/experimental/resource",
            ),
            endpoint(
                "experimental.projectCopy.generateName",
                Method::Post,
                "/experimental/project/:projectID/copy/generate-name",
            )
            .payload(ir::object(vec![ir::opt("context", ir::string())])),
        ],
    )
}
