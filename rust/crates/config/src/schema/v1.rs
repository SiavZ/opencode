//! Purpose: legacy V1 config schema (`packages/core/src/v1/config/**`) as decoder shapes,
//! including the agent and permission normalizing transforms and the custom-LSP check.
//! Scope: schema values.
//! Uses:
//! rust/crates/
//! |-- config/src/schema/mod.rs
//! `-- config/src/schema/v1_provider.rs
//! External:
//! `-- serde_json::{Map, Value}
//! Used by:
//! rust/crates/
//! |-- config/src/legacy/compat.rs
//! |-- config/src/legacy/load.rs
//! |-- config/src/legacy/markdown.rs
//! `-- config/src/migrate.rs
//! Invariants: field sets and literals match the baseline `ConfigV1.Info`; agent normalize moves
//! unknown keys into `options` and `tools` into `permission`, `maxSteps` into `steps`.
//! Contract tests: config/tests/compat_fixtures.rs

pub use super::v1_provider::provider;
use super::{Shape, array, opt, record, req, strukt, union};
use serde_json::{Map, Value};
use std::sync::OnceLock;

pub const ACTIONS: &[&str] = &["ask", "allow", "deny"];

pub const BUILTIN_LSP: &[&str] = &[
    "deno",
    "typescript",
    "vue",
    "eslint",
    "oxlint",
    "biome",
    "gopls",
    "ruby-lsp",
    "ty",
    "pyright",
    "elixir-ls",
    "zls",
    "csharp",
    "razor",
    "fsharp",
    "sourcekit-lsp",
    "rust",
    "clangd",
    "svelte",
    "astro",
    "jdtls",
    "kotlin-ls",
    "yaml-ls",
    "lua-ls",
    "php intelephense",
    "prisma",
    "dart",
    "ocaml-lsp",
    "bash",
    "terraform",
    "texlab",
    "dockerfile",
    "gleam",
    "clojure-lsp",
    "nixd",
    "tinymist",
    "haskell-language-server",
    "julials",
];

fn strings() -> Shape {
    array(Shape::String)
}

fn string_map() -> Shape {
    record(Shape::String)
}

fn is_hex_color(text: &str) -> bool {
    text.len() == 7 && text.starts_with('#') && text[1..].chars().all(|c| c.is_ascii_hexdigit())
}

pub fn color() -> Shape {
    union(vec![
        Shape::Pattern(is_hex_color, "^#[0-9a-fA-F]{6}$"),
        Shape::Literals(&[
            "primary",
            "secondary",
            "accent",
            "success",
            "warning",
            "error",
            "info",
        ]),
    ])
}

fn permission_rule() -> Shape {
    union(vec![
        Shape::Literals(ACTIONS),
        record(Shape::Literals(ACTIONS)),
    ])
}

/// `ConfigPermissionV1.Info`: a bare action becomes `{ "*": action }`.
pub fn permission() -> Shape {
    let rule = permission_rule;
    let action = || Shape::Literals(ACTIONS);
    let object = Shape::Struct(
        vec![
            opt("read", rule()),
            opt("edit", rule()),
            opt("glob", rule()),
            opt("grep", rule()),
            opt("list", rule()),
            opt("bash", rule()),
            opt("task", rule()),
            opt("external_directory", rule()),
            opt("todowrite", action()),
            opt("question", action()),
            opt("webfetch", action()),
            opt("websearch", action()),
            opt("lsp", rule()),
            opt("doom_loop", action()),
            opt("skill", rule()),
        ],
        Some(Box::new(rule())),
    );
    union(vec![
        Shape::Map(Box::new(action()), |value| {
            Value::Object(Map::from_iter([("*".into(), value)]))
        }),
        object,
    ])
}

const AGENT_KNOWN: &[&str] = &[
    "name",
    "model",
    "variant",
    "prompt",
    "description",
    "temperature",
    "top_p",
    "mode",
    "hidden",
    "color",
    "steps",
    "maxSteps",
    "options",
    "permission",
    "disable",
    "tools",
];

fn normalize_agent(value: Value) -> Value {
    let Value::Object(mut agent) = value else {
        return value;
    };
    let mut options = agent
        .get("options")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    for (key, entry) in &agent {
        if !AGENT_KNOWN.contains(&key.as_str()) {
            options.insert(key.clone(), entry.clone());
        }
    }
    let mut permission = Map::new();
    if let Some(tools) = agent.get("tools").and_then(Value::as_object) {
        for (tool, enabled) in tools {
            let action = if enabled.as_bool() == Some(true) {
                "allow"
            } else {
                "deny"
            };
            let key = if matches!(tool.as_str(), "write" | "edit" | "patch") {
                "edit"
            } else {
                tool.as_str()
            };
            permission.insert(key.into(), Value::String(action.into()));
        }
    }
    if let Some(existing) = agent.get("permission").and_then(Value::as_object) {
        for (key, entry) in existing {
            permission.insert(key.clone(), entry.clone());
        }
    }
    let steps = agent
        .get("steps")
        .or_else(|| agent.get("maxSteps"))
        .cloned();
    agent.insert("options".into(), Value::Object(options));
    agent.insert("permission".into(), Value::Object(permission));
    if let Some(steps) = steps {
        agent.insert("steps".into(), steps);
    }
    Value::Object(agent)
}

/// `ConfigAgentV1.Info`.
pub fn agent() -> Shape {
    let fields = vec![
        opt("model", Shape::String),
        opt("variant", Shape::String),
        opt("temperature", Shape::Finite),
        opt("top_p", Shape::Finite),
        opt("prompt", Shape::String),
        opt("tools", record(Shape::Bool)),
        opt("disable", Shape::Bool),
        opt("mode", Shape::Literals(&["subagent", "primary", "all"])),
        opt("hidden", Shape::Bool),
        opt("options", record(Shape::Any)),
        opt("color", color()),
        opt("steps", Shape::PositiveInt),
        opt("maxSteps", Shape::PositiveInt),
        opt("permission", permission()),
    ];
    Shape::Map(
        Box::new(Shape::Struct(fields, Some(Box::new(Shape::Any)))),
        normalize_agent,
    )
}

pub fn command() -> Shape {
    strukt(vec![
        req("template", Shape::String),
        opt("description", Shape::String),
        opt("agent", Shape::String),
        opt("model", Shape::String),
        opt("variant", Shape::String),
        opt("subtask", Shape::Bool),
    ])
}

pub fn mcp_server() -> Shape {
    let local = strukt(vec![
        req("type", Shape::Literals(&["local"])),
        req("command", strings()),
        opt("cwd", Shape::String),
        opt("environment", string_map()),
        opt("enabled", Shape::Bool),
        opt("timeout", Shape::PositiveInt),
    ]);
    let oauth = strukt(vec![
        opt("clientId", Shape::String),
        opt("clientSecret", Shape::String),
        opt("scope", Shape::String),
        opt("callbackPort", Shape::IntBetween(1, 65535)),
        opt("redirectUri", Shape::String),
    ]);
    let remote = strukt(vec![
        req("type", Shape::Literals(&["remote"])),
        req("url", Shape::String),
        opt("enabled", Shape::Bool),
        opt("headers", string_map()),
        opt("oauth", union(vec![oauth, Shape::False])),
        opt("timeout", Shape::PositiveInt),
    ]);
    union(vec![local, remote])
}

fn formatter() -> Shape {
    let entry = strukt(vec![
        opt("disabled", Shape::Bool),
        opt("command", strings()),
        opt("environment", string_map()),
        opt("extensions", strings()),
    ]);
    union(vec![Shape::Bool, record(entry)])
}

pub fn lsp_entry() -> Shape {
    union(vec![
        disabled_true(),
        strukt(vec![
            req("command", strings()),
            opt("extensions", strings()),
            opt("disabled", Shape::Bool),
            opt("env", string_map()),
            opt("initialization", record(Shape::Any)),
        ]),
    ])
}

/// `Schema.Struct({ disabled: Schema.Literal(true) })`.
pub fn disabled_true() -> Shape {
    Shape::Check(
        Box::new(strukt(vec![req("disabled", Shape::Bool)])),
        |value| {
            (value.get("disabled") != Some(&Value::Bool(true))).then(|| "Expected true".to_string())
        },
    )
}

fn lsp() -> Shape {
    Shape::Check(
        Box::new(union(vec![Shape::Bool, record(lsp_entry())])),
        |value| {
            let map = value.as_object()?;
            let ok = map.iter().all(|(id, entry)| {
                entry.get("disabled") == Some(&Value::Bool(true))
                    || BUILTIN_LSP.contains(&id.as_str())
                    || entry.get("extensions").is_some_and(|ext| !ext.is_null())
            });
            (!ok).then(|| "For custom LSP servers, 'extensions' array is required.".to_string())
        },
    )
}

pub fn attachment() -> Shape {
    let image = strukt(vec![
        opt("auto_resize", Shape::Bool),
        opt("max_width", Shape::PositiveInt),
        opt("max_height", Shape::PositiveInt),
        opt("max_base64_bytes", Shape::PositiveInt),
    ]);
    strukt(vec![opt("image", image)])
}

/// `ConfigReference.Info` (shared by V1 and V2).
pub fn references() -> Shape {
    let git = strukt(vec![
        req("repository", Shape::String),
        opt("branch", Shape::String),
        opt("description", Shape::String),
        opt("hidden", Shape::Bool),
    ]);
    let local = strukt(vec![
        req("path", Shape::String),
        opt("description", Shape::String),
        opt("hidden", Shape::Bool),
    ]);
    record(union(vec![Shape::String, git, local]))
}

/// `ConfigExperimental.Policy`.
pub fn policy() -> Shape {
    strukt(vec![
        req("action", Shape::Literals(&["provider.use"])),
        req("effect", Shape::Literals(&["allow", "deny"])),
        req("resource", Shape::String),
    ])
}

fn build_info() -> Shape {
    let plugin_spec = union(vec![
        Shape::String,
        Shape::Check(Box::new(array(Shape::Any)), plugin_tuple),
    ]);
    strukt(vec![
        opt("$schema", Shape::String),
        opt("shell", Shape::String),
        opt(
            "logLevel",
            Shape::Literals(&["DEBUG", "INFO", "WARN", "ERROR"]),
        ),
        opt(
            "server",
            strukt(vec![
                opt("port", Shape::PositiveInt),
                opt("hostname", Shape::String),
                opt("mdns", Shape::Bool),
                opt("mdnsDomain", Shape::String),
                opt("cors", strings()),
            ]),
        ),
        opt("command", record(command())),
        opt(
            "skills",
            strukt(vec![opt("paths", strings()), opt("urls", strings())]),
        ),
        opt("references", references()),
        opt("reference", references()),
        opt("watcher", strukt(vec![opt("ignore", strings())])),
        opt("snapshot", Shape::Bool),
        opt("plugin", array(plugin_spec)),
        opt("share", Shape::Literals(&["manual", "auto", "disabled"])),
        opt("autoshare", Shape::Bool),
        opt(
            "autoupdate",
            union(vec![Shape::Bool, Shape::Literals(&["notify"])]),
        ),
        opt("disabled_providers", strings()),
        opt("enabled_providers", strings()),
        opt("model", Shape::String),
        opt("small_model", Shape::String),
        opt("default_agent", Shape::String),
        opt("subagent_depth", Shape::NonNegativeInt),
        opt("username", Shape::String),
        opt("mode", record(agent())),
        opt("agent", record(agent())),
        opt("provider", record(provider())),
        opt(
            "mcp",
            record(union(vec![
                mcp_server(),
                strukt(vec![req("enabled", Shape::Bool)]),
            ])),
        ),
        opt("formatter", formatter()),
        opt("lsp", lsp()),
        opt("instructions", strings()),
        opt("layout", Shape::Literals(&["auto", "stretch"])),
        opt("permission", permission()),
        opt("tools", record(Shape::Bool)),
        opt("attachment", attachment()),
        opt("enterprise", strukt(vec![opt("url", Shape::String)])),
        opt(
            "tool_output",
            strukt(vec![
                opt("max_lines", Shape::PositiveInt),
                opt("max_bytes", Shape::PositiveInt),
            ]),
        ),
        opt(
            "compaction",
            strukt(vec![
                opt("auto", Shape::Bool),
                opt("prune", Shape::Bool),
                opt("tail_turns", Shape::NonNegativeInt),
                opt("preserve_recent_tokens", Shape::NonNegativeInt),
                opt("reserved", Shape::NonNegativeInt),
            ]),
        ),
        opt(
            "experimental",
            strukt(vec![
                opt("disable_paste_summary", Shape::Bool),
                opt("batch_tool", Shape::Bool),
                opt("openTelemetry", Shape::Bool),
                opt("primary_tools", strings()),
                opt("continue_loop_on_deny", Shape::Bool),
                opt("mcp_timeout", Shape::PositiveInt),
                opt("policies", array(policy())),
            ]),
        ),
    ])
}

/// `[string, Record<string, unknown>]` plugin tuple.
fn plugin_tuple(value: &Value) -> Option<String> {
    let items = value.as_array()?;
    let ok = items.len() == 2 && items[0].is_string() && items[1].is_object();
    (!ok).then(|| "Expected [string, object]".to_string())
}

/// `ConfigV1.Info`.
pub fn info() -> &'static Shape {
    static INFO: OnceLock<Shape> = OnceLock::new();
    INFO.get_or_init(build_info)
}

/// `ConfigV1.WellKnown`.
pub fn well_known() -> Shape {
    strukt(vec![
        opt("config", Shape::Any),
        opt("remote_config", Shape::Any),
    ])
}
