//! Purpose: legacy Session response schema shared by list, create and get.
//! Scope: pure protocol schema.
//! Uses:
//! rust/crates/
//! `-- protocol/src/ir.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- protocol/src/legacy/mod.rs
//! Invariants: legacy Session has directory, slug and version, not the V2 location envelope.
//! Contract tests: protocol/tests/registry.rs

use crate::ir::{self, Definition, Schema};

pub fn definitions() -> Vec<Definition> {
    vec![
        ir::def(
            "LegacySession",
            ir::object(vec![
                ir::req("id", ir::id("^ses", "SessionID")),
                ir::req("slug", ir::string()),
                ir::req("projectID", ir::string()),
                ir::opt("workspaceID", ir::id("^wrk", "WorkspaceID")),
                ir::req("directory", ir::string()),
                ir::opt("path", ir::string()),
                ir::opt("parentID", ir::id("^ses", "SessionID")),
                ir::opt(
                    "summary",
                    ir::object(vec![
                        ir::req("additions", ir::number()),
                        ir::req("deletions", ir::number()),
                        ir::req("files", ir::number()),
                        ir::opt("diffs", ir::array(ir::unknown())),
                    ]),
                ),
                ir::opt("cost", ir::number()),
                ir::opt(
                    "tokens",
                    ir::object(vec![
                        ir::req("input", ir::number()),
                        ir::req("output", ir::number()),
                        ir::req("reasoning", ir::number()),
                        ir::req(
                            "cache",
                            ir::object(vec![
                                ir::req("read", ir::number()),
                                ir::req("write", ir::number()),
                            ]),
                        ),
                    ]),
                ),
                ir::opt("share", ir::object(vec![ir::req("url", ir::string())])),
                ir::req("title", ir::string()),
                ir::opt("agent", ir::string()),
                ir::opt("model", model()),
                ir::req("version", ir::string()),
                ir::opt("metadata", ir::record(ir::string(), ir::unknown())),
                ir::req(
                    "time",
                    ir::object(vec![
                        ir::req("created", timestamp()),
                        ir::req("updated", timestamp()),
                        ir::opt("compacting", timestamp()),
                        ir::opt("archived", ir::number()),
                    ]),
                ),
                ir::opt("permission", permissions()),
                ir::opt(
                    "revert",
                    ir::object(vec![
                        ir::req("messageID", ir::id("^msg", "MessageID")),
                        ir::opt("partID", ir::id("^prt", "PartID")),
                        ir::opt("snapshot", ir::string()),
                        ir::opt("diff", ir::string()),
                    ]),
                ),
            ]),
        ),
        ir::def(
            "LegacyNotFoundError",
            ir::object(vec![
                ir::req("name", ir::lit("NotFoundError")),
                ir::req("data", ir::object(vec![ir::req("message", ir::string())])),
            ]),
        ),
        ir::def(
            "LegacyBadRequest",
            ir::object(vec![ir::req("_tag", ir::lit("BadRequest"))]),
        ),
    ]
}

pub fn model() -> Schema {
    ir::object(vec![
        ir::req("id", ir::string()),
        ir::req("providerID", ir::string()),
        ir::opt("variant", ir::string()),
    ])
}

pub fn permissions() -> Schema {
    ir::array(ir::object(vec![
        ir::req("permission", ir::string()),
        ir::req("pattern", ir::string()),
        ir::req("action", ir::lits(&["allow", "deny", "ask"])),
    ]))
}

fn timestamp() -> Schema {
    Schema::Number {
        int: true,
        checks: ir::Checks {
            minimum: Some(0.0),
            ..Default::default()
        },
        from_string: false,
    }
}
