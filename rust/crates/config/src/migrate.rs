//! Purpose: V1 to V2 config migration (`packages/core/src/v1/config/migrate.ts`): `isV1`,
//! `migrate`, `migrateAgent`, MCP/provider/model lowering.
//! Scope: pure transforms over decoded V1 JSON.
//! Uses:
//! rust/crates/
//! `-- config/src/provider_options.rs
//! External:
//! `-- serde_json::{Map, Value, json}
//! Used by:
//! rust/crates/
//! |-- config/src/current.rs
//! `-- config/src/plugins.rs
//! Invariants: `undefined` fields are omitted (never `null`), key order follows the baseline
//! object literals, limits are truncated and clamped to the JS safe-integer range.
//! Contract tests: config/tests/current.rs

use crate::provider_options;
use serde_json::{Map, Value, json};

const V1_KEYS: &[&str] = &[
    "logLevel",
    "server",
    "command",
    "reference",
    "snapshot",
    "plugin",
    "autoshare",
    "disabled_providers",
    "enabled_providers",
    "small_model",
    "mode",
    "agent",
    "provider",
    "permission",
    "tools",
    "attachment",
    "layout",
];

/// `ConfigMigrateV1.isV1`.
pub fn is_v1(input: &Value) -> bool {
    input
        .as_object()
        .is_some_and(|map| map.keys().any(|key| V1_KEYS.contains(&key.as_str())))
}

/// Build an object from optional entries, dropping `None` like `JSON.stringify` drops undefined.
pub fn obj(entries: Vec<(&str, Option<Value>)>) -> Value {
    Value::Object(
        entries
            .into_iter()
            .filter_map(|(key, value)| value.map(|value| (key.to_string(), value)))
            .collect(),
    )
}

fn get(value: &Value, key: &str) -> Option<Value> {
    value.get(key).cloned()
}

/// `ConfigMigrateV1.migrate`.
pub fn migrate(info: &Value) -> Value {
    let share = get(info, "share")
        .or_else(|| (info.get("autoshare") == Some(&Value::Bool(true))).then(|| json!("auto")));
    let compaction = info.get("compaction").map(|compaction| {
        obj(vec![
            ("auto", get(compaction, "auto")),
            ("prune", get(compaction, "prune")),
            (
                "keep",
                Some(obj(vec![(
                    "tokens",
                    get(compaction, "preserve_recent_tokens"),
                )])),
            ),
            ("buffer", get(compaction, "reserved")),
        ])
    });
    let skills = info.get("skills").map(|skills| {
        let mut out = Vec::new();
        for key in ["paths", "urls"] {
            out.extend(
                skills
                    .get(key)
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default(),
            );
        }
        Value::Array(out)
    });
    let plugins = info.get("plugin").and_then(Value::as_array).map(|plugins| {
        Value::Array(
            plugins
                .iter()
                .map(|plugin| match plugin {
                    Value::Array(tuple) => obj(vec![
                        ("package", tuple.first().cloned()),
                        ("options", tuple.get(1).cloned()),
                    ]),
                    other => other.clone(),
                })
                .collect(),
        )
    });
    let experimental = info
        .get("experimental")
        .and_then(|experimental| experimental.get("policies"))
        .map(|policies| json!({ "policies": policies }));
    obj(vec![
        ("$schema", get(info, "$schema")),
        ("shell", get(info, "shell")),
        ("model", get(info, "model")),
        ("default_agent", get(info, "default_agent")),
        ("autoupdate", get(info, "autoupdate")),
        ("share", share),
        ("enterprise", get(info, "enterprise")),
        ("username", get(info, "username")),
        (
            "permissions",
            permissions(info.get("permission"), info.get("tools")),
        ),
        ("agents", agents(info)),
        ("snapshots", get(info, "snapshot")),
        ("watcher", get(info, "watcher")),
        ("formatter", get(info, "formatter")),
        ("lsp", get(info, "lsp")),
        ("attachments", get(info, "attachment")),
        ("tool_output", get(info, "tool_output")),
        ("mcp", mcp(info)),
        ("compaction", compaction),
        ("skills", skills),
        ("commands", get(info, "command")),
        ("instructions", get(info, "instructions")),
        (
            "references",
            get(info, "references").or_else(|| get(info, "reference")),
        ),
        ("plugins", plugins),
        ("experimental", experimental),
        ("providers", providers(info.get("provider"))),
    ])
}

fn normalize_action(action: &str) -> &str {
    if action == "write" || action == "patch" {
        "edit"
    } else {
        action
    }
}

fn permissions(info: Option<&Value>, tools: Option<&Value>) -> Option<Value> {
    let mut rules = Vec::new();
    for (action, enabled) in tools.and_then(Value::as_object).into_iter().flatten() {
        let effect = if enabled.as_bool() == Some(true) {
            "allow"
        } else {
            "deny"
        };
        rules
            .push(json!({ "action": normalize_action(action), "resource": "*", "effect": effect }));
    }
    for (action, rule) in info.and_then(Value::as_object).into_iter().flatten() {
        match rule {
            Value::String(effect) => {
                rules.push(json!({ "action": action, "resource": "*", "effect": effect }))
            }
            Value::Object(map) => {
                for (resource, effect) in map {
                    rules.push(json!({ "action": action, "resource": resource, "effect": effect }));
                }
            }
            _ => {}
        }
    }
    (!rules.is_empty()).then_some(Value::Array(rules))
}

fn agents(info: &Value) -> Option<Value> {
    let mut entries: Vec<(String, Value)> = Vec::new();
    for (name, agent) in info
        .get("agent")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        entries.push((name.clone(), agent.clone()));
    }
    for (name, agent) in info
        .get("mode")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        let mut agent = agent.clone();
        if let Some(map) = agent.as_object_mut() {
            map.insert("mode".into(), json!("primary"));
        }
        entries.push((name.clone(), agent));
    }
    if entries.is_empty() {
        return None;
    }
    let mut out = Map::new();
    for (name, agent) in entries {
        if !agent.is_null() {
            out.insert(name, migrate_agent(&agent));
        }
    }
    Some(Value::Object(out))
}

/// `ConfigMigrateV1.migrateAgent`.
pub fn migrate_agent(info: &Value) -> Value {
    let mut body = info
        .get("options")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    for key in ["temperature", "top_p"] {
        if let Some(value) = info.get(key) {
            body.insert(key.into(), value.clone());
        }
    }
    obj(vec![
        ("model", get(info, "model")),
        ("variant", get(info, "variant")),
        (
            "request",
            (!body.is_empty()).then(|| json!({ "body": body })),
        ),
        ("system", get(info, "prompt")),
        ("description", get(info, "description")),
        ("mode", get(info, "mode")),
        ("hidden", get(info, "hidden")),
        ("color", get(info, "color")),
        ("steps", get(info, "steps")),
        ("disabled", get(info, "disable")),
        ("permissions", permissions(info.get("permission"), None)),
    ])
}

fn mcp(info: &Value) -> Option<Value> {
    let mut servers = Map::new();
    for (name, server) in info
        .get("mcp")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        if server.get("type").is_some() {
            servers.insert(name.clone(), migrate_mcp(server));
        }
    }
    let timeout = info
        .get("experimental")
        .and_then(|e| e.get("mcp_timeout"))
        .cloned();
    let has_timeout = timeout
        .as_ref()
        .and_then(Value::as_f64)
        .is_some_and(|n| n != 0.0);
    if !has_timeout && servers.is_empty() {
        return None;
    }
    Some(obj(vec![
        (
            "timeout",
            timeout.map(|timeout| json!({ "request": timeout })),
        ),
        ("servers", Some(Value::Object(servers))),
    ]))
}

fn migrate_mcp(info: &Value) -> Value {
    let disabled = info
        .get("enabled")
        .and_then(Value::as_bool)
        .map(|enabled| Value::Bool(!enabled));
    let timeout = info
        .get("timeout")
        .map(|timeout| json!({ "request": timeout }));
    if info.get("type") == Some(&json!("local")) {
        return obj(vec![
            ("type", get(info, "type")),
            ("command", get(info, "command")),
            ("cwd", get(info, "cwd")),
            ("environment", get(info, "environment")),
            ("disabled", disabled),
            ("timeout", timeout),
        ]);
    }
    let oauth = match info.get("oauth") {
        Some(Value::Object(_)) => info.get("oauth").map(|oauth| {
            obj(vec![
                ("client_id", get(oauth, "clientId")),
                ("client_secret", get(oauth, "clientSecret")),
                ("scope", get(oauth, "scope")),
                ("callback_port", get(oauth, "callbackPort")),
                ("redirect_uri", get(oauth, "redirectUri")),
            ])
        }),
        other => other.cloned(),
    };
    obj(vec![
        ("type", get(info, "type")),
        ("url", get(info, "url")),
        ("headers", get(info, "headers")),
        ("oauth", oauth),
        ("disabled", disabled),
        ("timeout", timeout),
    ])
}

fn providers(info: Option<&Value>) -> Option<Value> {
    let map = info?.as_object()?;
    Some(Value::Object(
        map.iter()
            .map(|(name, provider)| (name.clone(), migrate_provider(provider)))
            .collect(),
    ))
}

fn migrate_provider(info: &Value) -> Value {
    let npm = info.get("npm").and_then(Value::as_str);
    let lowerer = provider_options::get(npm);
    let options = info
        .get("options")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let lowered = lowerer.provider(&options);
    let url = info
        .get("api")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or(lowered.url.clone());
    let api = npm.filter(|npm| !npm.is_empty()).map(|npm| {
        obj(vec![
            ("type", Some(json!("aisdk"))),
            ("package", Some(json!(npm))),
            ("url", url.map(Value::String)),
            (
                "settings",
                Some(Value::Object(lowered.settings.clone().unwrap_or_default())),
            ),
        ])
    });
    let request = info.get("options").map(|_| {
        obj(vec![
            ("headers", lowered.headers.clone().map(Value::Object)),
            ("body", lowered.body.clone().map(Value::Object)),
        ])
    });
    let models = info.get("models").and_then(Value::as_object).map(|models| {
        Value::Object(
            models
                .iter()
                .map(|(name, model)| (name.clone(), migrate_model(model, npm)))
                .collect(),
        )
    });
    obj(vec![
        ("name", get(info, "name")),
        ("env", get(info, "env")),
        ("api", api),
        ("request", request),
        ("models", models),
    ])
}

fn cost_entry(cost: &Value, tier: Option<Value>) -> Value {
    obj(vec![
        ("tier", tier),
        ("input", get(cost, "input")),
        ("output", get(cost, "output")),
        (
            "cache",
            Some(obj(vec![
                ("read", get(cost, "cache_read")),
                ("write", get(cost, "cache_write")),
            ])),
        ),
    ])
}

fn migrate_model(info: &Value, package: Option<&str>) -> Value {
    let provider_npm = info
        .get("provider")
        .and_then(|p| p.get("npm"))
        .and_then(Value::as_str);
    let lowerer = provider_options::get(provider_npm.or(package));
    let request = info
        .get("options")
        .and_then(Value::as_object)
        .map(|options| Value::Object(lowerer.request(options)));
    let costs = info.get("cost").map(|cost| {
        let mut out = vec![cost_entry(cost, None)];
        if let Some(over) = cost.get("context_over_200k") {
            out.push(cost_entry(
                over,
                Some(json!({ "type": "context", "size": 200_000 })),
            ));
        }
        Value::Array(out)
    });
    let modalities = info.get("modalities");
    let input = modalities.and_then(|m| m.get("input")).cloned();
    let output = modalities.and_then(|m| m.get("output")).cloned();
    let capabilities = (info.get("tool_call").is_some() || input.is_some() || output.is_some())
        .then(|| {
            json!({
                "tools": info.get("tool_call").cloned().unwrap_or(Value::Bool(false)),
                "input": input.unwrap_or(json!([])),
                "output": output.unwrap_or(json!([])),
            })
        });
    let api = match provider_npm.filter(|npm| !npm.is_empty()) {
        Some(npm) => Some(obj(vec![
            ("id", get(info, "id")),
            ("type", Some(json!("aisdk"))),
            ("package", Some(json!(npm))),
            (
                "url",
                info.get("provider").and_then(|p| p.get("api")).cloned(),
            ),
            ("settings", Some(json!({}))),
        ])),
        None => info.get("id").map(|id| json!({ "id": id })),
    };
    let headers = get(info, "headers");
    let model_request = (headers.is_some() || request.is_some())
        .then(|| obj(vec![("headers", headers), ("body", request)]));
    let variants = info
        .get("variants")
        .and_then(Value::as_object)
        .map(|variants| {
            Value::Array(
                variants
                    .iter()
                    .map(|(id, options)| {
                        let body = lowerer.request(options.as_object().unwrap_or(&Map::new()));
                        json!({ "id": id, "body": body })
                    })
                    .collect(),
            )
        });
    let limit = info.get("limit").map(|limit| {
        obj(vec![
            ("context", limit.get("context").map(int)),
            ("input", limit.get("input").map(int)),
            ("output", limit.get("output").map(int)),
        ])
    });
    obj(vec![
        ("family", get(info, "family")),
        ("name", get(info, "name")),
        ("api", api),
        ("capabilities", capabilities),
        ("request", model_request),
        ("variants", variants),
        ("cost", costs),
        (
            "disabled",
            (info.get("status") == Some(&json!("deprecated"))).then_some(Value::Bool(true)),
        ),
        ("limit", limit),
    ])
}

fn int(value: &Value) -> Value {
    let n = value.as_f64().unwrap_or_default().trunc();
    let clamped = n.clamp(-9_007_199_254_740_991.0, 9_007_199_254_740_991.0);
    crate::jsonc::number_value(clamped)
}
