//! Purpose: MCP and LSP parts of V2-compat lowering (`v2-compat.ts` `normalizeMcp`,
//! `normalizeServer`, `lowerServer`, `lowerTimeout`, `normalizeLsp`).
//! Scope: pure transform.
//! Uses:
//! rust/crates/
//! |-- config/src/legacy/compat.rs
//! `-- config/src/schema/v1.rs
//! External:
//! `-- serde_json::{Map, Value}
//! Used by:
//! rust/crates/
//! `-- config/src/legacy/compat.rs
//! Invariants: flat V1 entries win over enveloped `servers` entries with the same name; a global
//! timeout lowers to `experimental.mcp_timeout` only when catalog == execution and no startup.
//! Contract tests: config/tests/compat_fixtures.rs

use super::compat::{Diags, Out, conflict, decode_value, path, prefer_legacy, unsupported};
use crate::schema::{self, Shape, opt, record, req, strukt, union};
use serde_json::{Map, Value};

fn timeout_shape() -> Shape {
    strukt(vec![
        opt("startup", Shape::PositiveInt),
        opt("catalog", Shape::PositiveInt),
        opt("execution", Shape::PositiveInt),
    ])
}

fn server_shape() -> Shape {
    let oauth = strukt(vec![
        opt("client_id", Shape::String),
        opt("client_secret", Shape::String),
        opt("scope", Shape::String),
        opt("callback_port", Shape::IntBetween(1, 65535)),
        opt("redirect_uri", Shape::String),
    ]);
    union(vec![
        strukt(vec![
            req("type", Shape::Literals(&["local"])),
            req("command", schema::array(Shape::String)),
            opt("cwd", Shape::String),
            opt("environment", record(Shape::String)),
            opt("disabled", Shape::Bool),
            opt("codemode", Shape::Bool),
            opt("timeout", timeout_shape()),
        ]),
        strukt(vec![
            req("type", Shape::Literals(&["remote"])),
            req("url", Shape::String),
            opt("headers", record(Shape::String)),
            opt("oauth", union(vec![oauth, Shape::False])),
            opt("disabled", Shape::Bool),
            opt("codemode", Shape::Bool),
            opt("timeout", timeout_shape()),
        ]),
    ])
}

/// Object-valued entries can be servers literally named "type" or "enabled".
fn is_direct_server(value: &Map<String, Value>) -> bool {
    ["type", "enabled"].iter().any(|key| {
        value
            .get(*key)
            .is_some_and(|v| v.is_null() || !v.is_object())
    })
}

fn lower_timeout(input: &Value) -> Option<Value> {
    if input.get("startup").is_some() {
        return None;
    }
    let (catalog, execution) = (input.get("catalog")?, input.get("execution")?);
    (catalog == execution).then(|| catalog.clone())
}

fn lower_server(input: &Value) -> Value {
    let mut result = input.as_object().cloned().unwrap_or_default();
    let disabled = input.get("disabled") == Some(&Value::Bool(true));
    result.insert("enabled".into(), Value::Bool(!disabled));
    result.remove("disabled");
    result.remove("codemode");
    result.remove("timeout");
    if let Some(timeout) = input.get("timeout").and_then(lower_timeout) {
        result.insert("timeout".into(), timeout);
    }
    if input.get("type") == Some(&Value::from("remote"))
        && let Some(oauth) = input.get("oauth").and_then(Value::as_object)
    {
        let mut lowered = Map::new();
        for (from, to) in [
            ("client_id", "clientId"),
            ("client_secret", "clientSecret"),
            ("scope", "scope"),
            ("callback_port", "callbackPort"),
            ("redirect_uri", "redirectUri"),
        ] {
            if let Some(value) = oauth.get(from) {
                lowered.insert(to.into(), value.clone());
            }
        }
        result.insert("oauth".into(), Value::Object(lowered));
    }
    Value::Object(result)
}

fn normalize_server(input: &Value, p: Vec<String>, diags: &mut Diags) -> Option<Value> {
    let server = decode_value(&server_shape(), input, p.clone(), diags)?;
    if server.get("codemode").is_some() {
        unsupported([p.clone(), path(&["codemode"])].concat(), diags);
    }
    if let Some(timeout) = server.get("timeout")
        && lower_timeout(timeout).is_none()
        && timeout.as_object().is_some_and(|t| !t.is_empty())
    {
        unsupported([p.clone(), path(&["timeout"])].concat(), diags);
    }
    let Some(enabled) = input.get("enabled") else {
        return Some(lower_server(&server));
    };
    if let Some(disabled) = server.get("disabled")
        && enabled == disabled
    {
        conflict([p, path(&["disabled"])].concat(), diags);
    }
    let mut lowered = lower_server(&server);
    lowered["enabled"] = enabled.clone();
    Some(lowered)
}

pub(super) fn normalize_mcp(input: &Out, result: &mut Out, diags: &mut Diags) {
    let Some(mcp) = input.get("mcp").and_then(Value::as_object) else {
        return;
    };
    let nested = mcp.get("servers").and_then(Value::as_object);
    let envelope = nested.is_some_and(|nested| !is_direct_server(nested));
    let timeout_record = mcp.get("timeout").and_then(Value::as_object);
    let timeout = mcp
        .get("timeout")
        .and_then(|t| schema::decode(&timeout_shape(), t).ok());
    let global_timeout = timeout.is_some()
        && timeout_record.is_some_and(|record| {
            !is_direct_server(record)
                && (record.is_empty()
                    || ["startup", "catalog", "execution"]
                        .iter()
                        .any(|k| record.contains_key(*k)))
        });
    let mut servers = Map::new();
    for (name, value) in mcp {
        if (name == "servers" && envelope) || (name == "timeout" && global_timeout) {
            continue;
        }
        let p = vec!["mcp".to_string(), name.clone()];
        let native = value.as_object().is_some_and(|record| {
            let oauth = record.get("oauth").and_then(Value::as_object);
            record.contains_key("disabled")
                || record.contains_key("codemode")
                || record
                    .get("timeout")
                    .is_some_and(|t| t.is_object() || t.is_array() || t.is_null())
                || oauth.is_some_and(|oauth| {
                    [
                        "client_id",
                        "client_secret",
                        "callback_port",
                        "redirect_uri",
                    ]
                    .iter()
                    .any(|k| oauth.contains_key(*k))
                })
        });
        // Keep invalid flat entries for the final V1 decoder rather than sanitizing them.
        let lowered = if native {
            normalize_server(value, p, diags).unwrap_or_else(|| value.clone())
        } else {
            value.clone()
        };
        servers.insert(name.clone(), lowered);
    }
    if let (true, Some(nested)) = (envelope, nested) {
        for (name, value) in nested {
            let p = vec!["mcp".to_string(), "servers".into(), name.clone()];
            if let Some(existing) = servers.get(name) {
                if existing != value {
                    conflict(p, diags);
                }
                continue;
            }
            let record = value.as_object();
            if record.is_some_and(|r| {
                r.get("enabled").is_some_and(Value::is_boolean) && !r.contains_key("disabled")
            }) {
                servers.insert(name.clone(), value.clone());
                continue;
            }
            if let Some(server) = normalize_server(value, p, diags) {
                servers.insert(name.clone(), server);
            }
        }
    }
    result.insert("mcp".into(), Value::Object(servers));
    let (true, Some(timeout)) = (global_timeout, timeout) else {
        return;
    };
    let Some(value) = lower_timeout(&timeout) else {
        if timeout.as_object().is_some_and(|t| !t.is_empty()) {
            unsupported(path(&["mcp", "timeout"]), diags);
        }
        return;
    };
    let existing = result.get("experimental");
    if existing.is_some() && !existing.is_some_and(Value::is_object) {
        return;
    }
    let mut experimental = existing
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    prefer_legacy(
        &mut experimental,
        "mcp_timeout",
        value,
        path(&["mcp", "timeout"]),
        diags,
    );
    result.insert("experimental".into(), Value::Object(experimental));
}

pub(super) fn normalize_lsp(input: &Out, result: &mut Out, diags: &mut Diags) {
    let Some(lsp) = input.get("lsp").and_then(Value::as_object) else {
        return;
    };
    let entry_shape = crate::schema::v1::lsp_entry();
    let mut out = Map::new();
    for (name, value) in lsp {
        let keep = crate::schema::v1::BUILTIN_LSP.contains(&name.as_str())
            || match schema::decode(&entry_shape, value) {
                Err(_) => true,
                Ok(entry) => {
                    entry.get("disabled") == Some(&Value::Bool(true))
                        || entry.get("extensions").is_some_and(|e| !e.is_null())
                }
            };
        if keep {
            out.insert(name.clone(), value.clone());
        } else {
            unsupported(vec!["lsp".into(), name.clone()], diags);
        }
    }
    result.insert("lsp".into(), Value::Object(out));
}
