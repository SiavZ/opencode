//! Purpose: legacy provider authentication and MCP route declarations.
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
//! Invariants: OAuth authorize is decoded by the raw HTTP boundary; credentials retain canonical wire fields.
//! Contract tests: protocol/tests/registry.rs, server/tests/http_legacy.rs

use super::build::{endpoint, group};
use crate::ir;
use crate::operation::{Endpoint, Group, Method};

pub(super) fn groups() -> Vec<Group> {
    vec![
        group(
            "provider",
            vec![
                endpoint("provider.list", Method::Get, "/provider"),
                endpoint("provider.auth", Method::Get, "/provider/auth"),
                endpoint(
                    "provider.oauth.authorize",
                    Method::Post,
                    "/provider/:providerID/oauth/authorize",
                )
                .payload(ir::object(vec![
                    ir::req("method", ir::number()),
                    ir::opt("inputs", ir::record(ir::string(), ir::string())),
                ])),
                endpoint(
                    "provider.oauth.callback",
                    Method::Post,
                    "/provider/:providerID/oauth/callback",
                )
                .payload(ir::object(vec![
                    ir::req("method", ir::number()),
                    ir::opt("code", ir::string()),
                ])),
            ],
        ),
        group(
            "control",
            vec![
                Endpoint::new("auth.set", "auth.set", Method::Put, "/auth/:providerID")
                    .params(vec![ir::req("providerID", ir::string())])
                    .payload(ir::union(vec![
                        ir::object(vec![
                            ir::req("type", ir::lit("oauth")),
                            ir::req("refresh", ir::string()),
                            ir::req("access", ir::string()),
                            ir::req("expires", nonnegative()),
                            ir::opt("accountId", ir::string()),
                            ir::opt("enterpriseUrl", ir::string()),
                        ]),
                        ir::object(vec![
                            ir::req("type", ir::lit("api")),
                            ir::req("key", ir::string()),
                            ir::opt("metadata", ir::record(ir::string(), ir::string())),
                        ]),
                        ir::object(vec![
                            ir::req("type", ir::lit("wellknown")),
                            ir::req("key", ir::string()),
                            ir::req("token", ir::string()),
                        ]),
                    ]))
                    .ok(ir::boolean()),
                Endpoint::new(
                    "auth.remove",
                    "auth.remove",
                    Method::Delete,
                    "/auth/:providerID",
                )
                .params(vec![ir::req("providerID", ir::string())])
                .ok(ir::boolean()),
                Endpoint::new("app.log", "app.log", Method::Post, "/log")
                    .query(super::routing())
                    .payload(ir::object(vec![
                        ir::req("service", ir::string()),
                        ir::req("level", ir::lits(&["debug", "info", "error", "warn"])),
                        ir::req("message", ir::string()),
                        ir::opt("extra", ir::record(ir::string(), ir::unknown())),
                    ]))
                    .ok(ir::boolean()),
            ],
        ),
        group(
            "mcp",
            vec![
                endpoint("mcp.status", Method::Get, "/mcp"),
                endpoint("mcp.add", Method::Post, "/mcp").payload(ir::object(vec![
                    ir::req("name", ir::string()),
                    ir::req(
                        "config",
                        ir::union(vec![
                            ir::object(vec![
                                ir::req("type", ir::lit("local")),
                                ir::req("command", ir::array(ir::string())),
                                ir::opt("environment", ir::record(ir::string(), ir::string())),
                                ir::opt("enabled", ir::boolean()),
                                ir::opt("timeout", ir::number()),
                            ]),
                            ir::object(vec![
                                ir::req("type", ir::lit("remote")),
                                ir::req("url", ir::string()),
                                ir::opt("enabled", ir::boolean()),
                                ir::opt("headers", ir::record(ir::string(), ir::string())),
                                ir::opt(
                                    "oauth",
                                    ir::union(vec![
                                        ir::lit(false),
                                        ir::record(ir::string(), ir::unknown()),
                                    ]),
                                ),
                                ir::opt("timeout", ir::number()),
                            ]),
                        ]),
                    ),
                ])),
                endpoint("mcp.auth.start", Method::Post, "/mcp/:name/auth"),
                endpoint(
                    "mcp.auth.callback",
                    Method::Post,
                    "/mcp/:name/auth/callback",
                )
                .payload(ir::object(vec![ir::req("code", ir::string())])),
                endpoint(
                    "mcp.auth.authenticate",
                    Method::Post,
                    "/mcp/:name/auth/authenticate",
                ),
                endpoint("mcp.auth.remove", Method::Delete, "/mcp/:name/auth"),
                endpoint("mcp.connect", Method::Post, "/mcp/:name/connect"),
                endpoint("mcp.disconnect", Method::Post, "/mcp/:name/disconnect"),
            ],
        ),
    ]
}

fn nonnegative() -> ir::Schema {
    ir::Schema::Number {
        int: true,
        checks: ir::Checks {
            minimum: Some(0.0),
            ..Default::default()
        },
        from_string: false,
    }
}
