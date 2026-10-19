//! Purpose: current (V2) config schema `Config.Info` from `packages/core/src/config.ts:29-107`
//! and `packages/core/src/config/*.ts` as decoder shapes.
//! Scope: schema values.
//! Uses:
//! rust/crates/
//! |-- config/src/schema/mod.rs
//! `-- config/src/schema/v1.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- config/src/schema/mod.rs
//! Invariants: field names, literals and integer constraints match the baseline classes.
//! Contract tests: config/tests/current.rs

use super::v1::{color, disabled_true, policy, references};
use super::{Shape, array, opt, record, req, strukt, union};
use std::sync::OnceLock;

fn strings() -> Shape {
    array(Shape::String)
}

fn string_map() -> Shape {
    record(Shape::String)
}

pub fn request() -> Vec<super::Field> {
    vec![
        opt("headers", string_map()),
        opt("body", record(Shape::Any)),
    ]
}

fn permission_rules() -> Shape {
    array(strukt(vec![
        req("action", Shape::String),
        req("resource", Shape::String),
        req("effect", Shape::Literals(&["allow", "deny", "ask"])),
    ]))
}

fn agent() -> Shape {
    strukt(vec![
        opt("model", Shape::String),
        opt("variant", Shape::String),
        opt("request", strukt(request())),
        opt("system", Shape::String),
        opt("description", Shape::String),
        opt("mode", Shape::Literals(&["subagent", "primary", "all"])),
        opt("hidden", Shape::Bool),
        opt("color", color()),
        opt("steps", Shape::PositiveInt),
        opt("disabled", Shape::Bool),
        opt("permissions", permission_rules()),
    ])
}

fn mcp() -> Shape {
    let timeout = || {
        strukt(vec![
            opt("startup", Shape::PositiveInt),
            opt("request", Shape::PositiveInt),
        ])
    };
    let local = strukt(vec![
        req("type", Shape::Literals(&["local"])),
        req("command", strings()),
        opt("cwd", Shape::String),
        opt("environment", string_map()),
        opt("disabled", Shape::Bool),
        opt("timeout", timeout()),
    ]);
    let oauth = strukt(vec![
        opt("client_id", Shape::String),
        opt("client_secret", Shape::String),
        opt("scope", Shape::String),
        opt("callback_port", Shape::IntBetween(1, 65535)),
        opt("redirect_uri", Shape::String),
    ]);
    let remote = strukt(vec![
        req("type", Shape::Literals(&["remote"])),
        req("url", Shape::String),
        opt("headers", string_map()),
        opt("oauth", union(vec![oauth, Shape::False])),
        opt("disabled", Shape::Bool),
        opt("timeout", timeout()),
    ]);
    strukt(vec![
        opt("timeout", timeout()),
        opt("servers", record(union(vec![local, remote]))),
    ])
}

fn provider_api() -> (Shape, Shape) {
    let aisdk = vec![
        req("type", Shape::Literals(&["aisdk"])),
        req("package", Shape::String),
        opt("url", Shape::String),
        opt("settings", record(Shape::Any)),
    ];
    let native = vec![
        req("type", Shape::Literals(&["native"])),
        opt("url", Shape::String),
        req("settings", record(Shape::Any)),
    ];
    let api = union(vec![strukt(aisdk.clone()), strukt(native.clone())]);
    let model_api = union(vec![
        strukt([vec![opt("id", Shape::String)], aisdk].concat()),
        strukt([vec![opt("id", Shape::String)], native].concat()),
        strukt(vec![req("id", Shape::String)]),
    ]);
    (api, model_api)
}

fn provider() -> Shape {
    let (api, model_api) = provider_api();
    let cost = strukt(vec![
        opt(
            "tier",
            strukt(vec![
                req("type", Shape::Literals(&["context"])),
                req("size", Shape::Int),
            ]),
        ),
        req("input", Shape::Finite),
        req("output", Shape::Finite),
        opt(
            "cache",
            strukt(vec![
                opt("read", Shape::Finite),
                opt("write", Shape::Finite),
            ]),
        ),
    ]);
    let model = strukt(vec![
        opt("family", Shape::String),
        opt("name", Shape::String),
        opt("api", model_api),
        opt(
            "capabilities",
            strukt(vec![
                req("tools", Shape::Bool),
                req("input", strings()),
                req("output", strings()),
            ]),
        ),
        opt(
            "request",
            strukt([request(), vec![opt("variant", Shape::String)]].concat()),
        ),
        opt(
            "variants",
            array(strukt([vec![req("id", Shape::String)], request()].concat())),
        ),
        opt("cost", union(vec![cost.clone(), array(cost)])),
        opt("disabled", Shape::Bool),
        opt(
            "limit",
            strukt(vec![
                opt("context", Shape::Int),
                opt("input", Shape::Int),
                opt("output", Shape::Int),
            ]),
        ),
    ]);
    strukt(vec![
        opt("name", Shape::String),
        opt("env", strings()),
        opt("api", api),
        opt("request", strukt(request())),
        opt("models", record(model)),
    ])
}

fn build_info() -> Shape {
    let formatter_entry = strukt(vec![
        opt("disabled", Shape::Bool),
        opt("command", strings()),
        opt("environment", string_map()),
        opt("extensions", strings()),
    ]);
    let lsp_server = strukt(vec![
        req("command", strings()),
        opt("extensions", strings()),
        opt("disabled", Shape::Bool),
        opt("env", string_map()),
        opt("initialization", record(Shape::Any)),
    ]);
    let plugin = union(vec![
        Shape::String,
        strukt(vec![
            req("package", Shape::String),
            opt("options", record(Shape::Any)),
        ]),
    ]);
    let command = strukt(vec![
        req("template", Shape::String),
        opt("description", Shape::String),
        opt("agent", Shape::String),
        opt("model", Shape::String),
        opt("variant", Shape::String),
        opt("subtask", Shape::Bool),
    ]);
    strukt(vec![
        opt("$schema", Shape::String),
        opt("shell", Shape::String),
        opt("model", Shape::String),
        opt("default_agent", Shape::String),
        opt(
            "autoupdate",
            union(vec![Shape::Bool, Shape::Literals(&["notify"])]),
        ),
        opt("share", Shape::Literals(&["manual", "auto", "disabled"])),
        opt("enterprise", strukt(vec![opt("url", Shape::String)])),
        opt("username", Shape::String),
        opt("permissions", permission_rules()),
        opt("agents", record(agent())),
        opt("snapshots", Shape::Bool),
        opt("watcher", strukt(vec![opt("ignore", strings())])),
        opt(
            "formatter",
            union(vec![Shape::Bool, record(formatter_entry)]),
        ),
        opt(
            "lsp",
            union(vec![
                Shape::Bool,
                record(union(vec![disabled_true(), lsp_server])),
            ]),
        ),
        opt("attachments", super::v1::attachment()),
        opt(
            "tool_output",
            strukt(vec![
                opt("max_lines", Shape::PositiveInt),
                opt("max_bytes", Shape::PositiveInt),
            ]),
        ),
        opt("mcp", mcp()),
        opt(
            "compaction",
            strukt(vec![
                opt("auto", Shape::Bool),
                opt("prune", Shape::Bool),
                opt("keep", strukt(vec![opt("tokens", Shape::NonNegativeInt)])),
                opt("buffer", Shape::NonNegativeInt),
            ]),
        ),
        opt("skills", strings()),
        opt("commands", record(command)),
        opt("instructions", strings()),
        opt("references", references()),
        opt("plugins", array(plugin)),
        opt(
            "experimental",
            strukt(vec![opt("policies", array(policy()))]),
        ),
        opt("providers", record(provider())),
    ])
}

/// `Config.Info`.
pub fn info() -> &'static Shape {
    static INFO: OnceLock<Shape> = OnceLock::new();
    INFO.get_or_init(build_info)
}
