//! Purpose: remaining legacy Session mutation and history endpoint schemas.
//! Scope: static registry construction.
//! Uses:
//! rust/crates/
//! |-- protocol/src/ir.rs
//! |-- protocol/src/legacy/build.rs
//! |-- protocol/src/legacy/schema.rs
//! `-- protocol/src/operation.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- protocol/src/legacy/mod.rs
//! Invariants: raw fork has an optional body; prompt_async has no response body.
//! Contract tests: protocol/tests/registry.rs, server/tests/http_legacy.rs

use super::build::{endpoint, optional, query};
use super::schema;
use crate::ir::{self, Schema};
use crate::operation::{Endpoint, Method};

pub(super) fn endpoints() -> Vec<Endpoint> {
    let mut endpoints = vec![
        endpoint(
            "session.children",
            Method::Get,
            "/session/:sessionID/children",
        ),
        endpoint("session.todo", Method::Get, "/session/:sessionID/todo"),
        query(
            endpoint("session.diff", Method::Get, "/session/:sessionID/diff"),
            vec![ir::opt("messageID", ir::id("^msg", "MessageID"))],
        ),
        endpoint("session.delete", Method::Delete, "/session/:sessionID"),
        endpoint("session.update", Method::Patch, "/session/:sessionID").payload(ir::object(vec![
            ir::opt("title", ir::string()),
            ir::opt("metadata", ir::record(ir::string(), ir::unknown())),
            ir::opt("permission", schema::permissions()),
            ir::opt("time", ir::object(vec![ir::opt("archived", ir::number())])),
        ])),
        optional(
            endpoint("session.fork", Method::Post, "/session/:sessionID/fork"),
            ir::object(vec![ir::opt("messageID", ir::id("^msg", "MessageID"))]),
        ),
        endpoint("session.init", Method::Post, "/session/:sessionID/init").payload(ir::object(
            vec![
                ir::req("modelID", ir::string()),
                ir::req("providerID", ir::string()),
                ir::req("messageID", ir::id("^msg", "MessageID")),
            ],
        )),
        endpoint("session.share", Method::Post, "/session/:sessionID/share"),
        endpoint(
            "session.unshare",
            Method::Delete,
            "/session/:sessionID/share",
        ),
        endpoint(
            "session.summarize",
            Method::Post,
            "/session/:sessionID/summarize",
        )
        .payload(ir::object(vec![
            ir::req("providerID", ir::string()),
            ir::req("modelID", ir::string()),
            ir::opt("auto", ir::boolean()),
        ])),
        endpoint(
            "session.prompt",
            Method::Post,
            "/session/:sessionID/message",
        )
        .payload(prompt()),
        endpoint(
            "session.prompt_async",
            Method::Post,
            "/session/:sessionID/prompt_async",
        )
        .payload(prompt())
        .no_content(),
        endpoint(
            "session.command",
            Method::Post,
            "/session/:sessionID/command",
        )
        .payload(ir::object(vec![
            ir::opt("messageID", ir::id("^msg", "MessageID")),
            ir::opt("agent", ir::string()),
            ir::opt("model", ir::string()),
            ir::req("arguments", ir::string()),
            ir::req("command", ir::string()),
            ir::opt("variant", ir::string()),
            ir::opt("parts", ir::array(file_part())),
        ])),
        endpoint("session.shell", Method::Post, "/session/:sessionID/shell").payload(ir::object(
            vec![
                ir::opt("messageID", ir::id("^msg", "MessageID")),
                ir::req("agent", ir::string()),
                ir::opt("model", model()),
                ir::req("command", ir::string()),
            ],
        )),
        endpoint("session.revert", Method::Post, "/session/:sessionID/revert").payload(ir::object(
            vec![
                ir::req("messageID", ir::id("^msg", "MessageID")),
                ir::opt("partID", ir::id("^prt", "PartID")),
            ],
        )),
        endpoint(
            "session.unrevert",
            Method::Post,
            "/session/:sessionID/unrevert",
        ),
        endpoint(
            "permission.respond",
            Method::Post,
            "/session/:sessionID/permissions/:permissionID",
        )
        .payload(ir::object(vec![ir::req(
            "response",
            ir::lits(&["once", "always", "reject"]),
        )])),
        endpoint(
            "session.deleteMessage",
            Method::Delete,
            "/session/:sessionID/message/:messageID",
        ),
        endpoint(
            "part.delete",
            Method::Delete,
            "/session/:sessionID/message/:messageID/part/:partID",
        ),
        endpoint(
            "part.update",
            Method::Patch,
            "/session/:sessionID/message/:messageID/part/:partID",
        )
        .payload(ir::object(vec![
            ir::req("id", ir::id("^prt", "PartID")),
            ir::req("sessionID", ir::id("^ses", "SessionID")),
            ir::req("messageID", ir::id("^msg", "MessageID")),
            ir::req("type", ir::string()),
        ])),
    ];
    for endpoint in &mut endpoints {
        endpoint.errors.push(crate::operation::ErrorResponse {
            status: 400,
            schema: ir::r("LegacyBadRequest"),
        });
        endpoint.errors.push(crate::operation::ErrorResponse {
            status: 404,
            schema: ir::r("LegacyNotFoundError"),
        });
    }
    endpoints
}

fn model() -> Schema {
    ir::object(vec![
        ir::req("providerID", ir::string()),
        ir::req("modelID", ir::string()),
    ])
}

fn file_part() -> Schema {
    ir::object(vec![
        ir::opt("id", ir::id("^prt", "PartID")),
        ir::req("type", ir::lit("file")),
        ir::req("mime", ir::string()),
        ir::opt("filename", ir::string()),
        ir::req("url", ir::string()),
        ir::opt("source", ir::unknown()),
    ])
}

fn prompt() -> Schema {
    ir::object(vec![
        ir::opt("messageID", ir::id("^msg", "MessageID")),
        ir::opt("model", model()),
        ir::opt("agent", ir::string()),
        ir::opt("noReply", ir::boolean()),
        ir::opt("tools", ir::record(ir::string(), ir::boolean())),
        ir::opt("format", ir::unknown()),
        ir::opt("system", ir::string()),
        ir::opt("variant", ir::string()),
        ir::req(
            "parts",
            ir::array(ir::union(vec![
                ir::object(vec![
                    ir::opt("id", ir::id("^prt", "PartID")),
                    ir::req("type", ir::lit("text")),
                    ir::req("text", ir::string()),
                    ir::opt("synthetic", ir::boolean()),
                    ir::opt("ignored", ir::boolean()),
                    ir::opt("time", ir::unknown()),
                    ir::opt("metadata", ir::unknown()),
                ]),
                file_part(),
                ir::object(vec![
                    ir::opt("id", ir::id("^prt", "PartID")),
                    ir::req("type", ir::lit("agent")),
                    ir::req("name", ir::string()),
                    ir::opt("source", ir::unknown()),
                ]),
                ir::object(vec![
                    ir::opt("id", ir::id("^prt", "PartID")),
                    ir::req("type", ir::lit("subtask")),
                    ir::req("prompt", ir::string()),
                    ir::req("description", ir::string()),
                    ir::req("agent", ir::string()),
                    ir::opt("model", model()),
                    ir::opt("command", ir::string()),
                ]),
            ])),
        ),
    ])
}
