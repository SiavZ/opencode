//! Purpose: native PTY, permission and question legacy HTTP schemas.
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
//! Invariants: permission replies are enumerated; terminal sizes are positive integers.
//! Contract tests: protocol/tests/registry.rs, server/tests/http_legacy.rs

use super::build::{endpoint, group};
use crate::ir;
use crate::operation::{Group, Method};

pub(super) fn groups() -> Vec<Group> {
    vec![
        group(
            "permission",
            vec![
                endpoint("permission.list", Method::Get, "/permission"),
                endpoint(
                    "permission.reply",
                    Method::Post,
                    "/permission/:requestID/reply",
                )
                .payload(ir::object(vec![
                    ir::req("reply", ir::lits(&["once", "always", "reject"])),
                    ir::opt("message", ir::string()),
                ])),
            ],
        ),
        group(
            "question",
            vec![
                endpoint("question.list", Method::Get, "/question"),
                endpoint("question.reply", Method::Post, "/question/:requestID/reply").payload(
                    ir::object(vec![ir::req("answers", ir::array(ir::array(ir::string())))]),
                ),
                endpoint(
                    "question.reject",
                    Method::Post,
                    "/question/:requestID/reject",
                ),
            ],
        ),
        group(
            "pty",
            vec![
                endpoint("pty.shells", Method::Get, "/pty/shells"),
                endpoint("pty.list", Method::Get, "/pty"),
                endpoint("pty.create", Method::Post, "/pty").payload(ir::object(vec![
                    ir::opt("command", ir::string()),
                    ir::opt("args", ir::array(ir::string())),
                    ir::opt("cwd", ir::string()),
                    ir::opt("title", ir::string()),
                    ir::opt("env", ir::record(ir::string(), ir::string())),
                ])),
                endpoint("pty.get", Method::Get, "/pty/:ptyID"),
                endpoint("pty.update", Method::Put, "/pty/:ptyID").payload(ir::object(vec![
                    ir::opt("title", ir::string()),
                    ir::opt(
                        "size",
                        ir::object(vec![
                            ir::req("rows", crate::definitions::positive_int()),
                            ir::req("cols", crate::definitions::positive_int()),
                        ]),
                    ),
                ])),
                endpoint("pty.remove", Method::Delete, "/pty/:ptyID"),
                endpoint("pty.connect", Method::Get, "/pty/:ptyID/connect")
                    .query(vec![ir::opt("cursor", ir::string()), ir::opt("ticket", ir::string())])
                    .ok(ir::boolean()),
                endpoint(
                    "pty.connectToken",
                    Method::Post,
                    "/pty/:ptyID/connect-token",
                ),
            ],
        ),
    ]
}
