//! Purpose: legacy project, file, instance and configuration route declarations.
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
//! Invariants: filesystem query requirements and numeric bounds match the TypeScript route schemas.
//! Contract tests: protocol/tests/registry.rs, server/tests/http_legacy.rs

use super::build::{endpoint, group, query};
use crate::ir;
use crate::operation::{Endpoint, Group, Method};

pub(super) fn groups() -> Vec<Group> {
    vec![
        group(
            "config",
            vec![
                endpoint("config.get", Method::Get, "/config"),
                endpoint("config.update", Method::Patch, "/config")
                    .payload(ir::record(ir::string(), ir::unknown())),
                endpoint("config.providers", Method::Get, "/config/providers"),
            ],
        ),
        group(
            "project",
            vec![
                endpoint("project.list", Method::Get, "/project"),
                endpoint("project.current", Method::Get, "/project/current"),
                endpoint("project.initGit", Method::Post, "/project/git/init"),
                endpoint("project.update", Method::Patch, "/project/:projectID").payload(
                    ir::object(vec![
                        ir::opt("name", ir::string()),
                        ir::opt(
                            "icon",
                            ir::object(vec![
                                ir::opt("url", ir::string()),
                                ir::opt("override", ir::string()),
                                ir::opt("color", ir::string()),
                            ]),
                        ),
                        ir::opt("commands", ir::object(vec![ir::opt("start", ir::string())])),
                    ]),
                ),
                endpoint(
                    "project.directories",
                    Method::Get,
                    "/project/:projectID/directories",
                ),
            ],
        ),
        group(
            "file",
            vec![
                query(
                    endpoint("find.text", Method::Get, "/find"),
                    vec![ir::req("pattern", ir::string())],
                ),
                query(
                    endpoint("find.files", Method::Get, "/find/file"),
                    vec![
                        ir::req("query", ir::string()),
                        ir::opt("dirs", ir::lits(&["true", "false"])),
                        ir::opt("type", ir::lits(&["file", "directory"])),
                        ir::opt("limit", ir::int_from_string(Some(1.0), Some(200.0))),
                    ],
                ),
                query(
                    endpoint("find.symbols", Method::Get, "/find/symbol"),
                    vec![ir::req("query", ir::string())],
                ),
                query(
                    endpoint("file.list", Method::Get, "/file"),
                    vec![ir::req("path", ir::string())],
                ),
                query(
                    endpoint("file.read", Method::Get, "/file/content"),
                    vec![ir::req("path", ir::string())],
                ),
                endpoint("file.status", Method::Get, "/file/status"),
            ],
        ),
        group(
            "instance",
            vec![
                endpoint("instance.dispose", Method::Post, "/instance/dispose"),
                endpoint("path.get", Method::Get, "/path"),
                endpoint("vcs.get", Method::Get, "/vcs"),
                endpoint("vcs.status", Method::Get, "/vcs/status"),
                query(
                    endpoint("vcs.diff", Method::Get, "/vcs/diff"),
                    vec![
                        ir::req("mode", ir::lits(&["git", "branch"])),
                        ir::opt("context", ir::int_from_string(Some(0.0), None)),
                    ],
                ),
                endpoint("vcs.diff.raw", Method::Get, "/vcs/diff/raw"),
                endpoint("vcs.apply", Method::Post, "/vcs/apply")
                    .payload(ir::object(vec![ir::req("patch", ir::string())])),
                endpoint("command.list", Method::Get, "/command"),
                endpoint("app.agents", Method::Get, "/agent"),
                endpoint("app.skills", Method::Get, "/skill"),
                endpoint("lsp.status", Method::Get, "/lsp"),
                endpoint("formatter.status", Method::Get, "/formatter"),
            ],
        ),
    ]
}

pub(super) fn global() -> Vec<Endpoint> {
    [
        Endpoint::new(
            "global.config.get",
            "global.config.get",
            Method::Get,
            "/global/config",
        )
        .ok(ir::unknown()),
        Endpoint::new(
            "global.config.update",
            "global.config.update",
            Method::Patch,
            "/global/config",
        )
        .payload(ir::record(ir::string(), ir::unknown()))
        .ok(ir::unknown()),
        Endpoint::new(
            "global.dispose",
            "global.dispose",
            Method::Post,
            "/global/dispose",
        )
        .ok(ir::boolean()),
        Endpoint::new(
            "global.upgrade",
            "global.upgrade",
            Method::Post,
            "/global/upgrade",
        )
        .payload(ir::object(vec![ir::req("target", ir::string())]))
        .ok(ir::unknown()),
    ]
    .into()
}
