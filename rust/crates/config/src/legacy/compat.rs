//! Purpose: V2-compat lowering for the legacy loader (`packages/opencode/src/config/v2-compat.ts`):
//! rejects native `permissions`, lowers native settings/model/skills/compaction/experimental,
//! agents, commands, MCP and LSP into their V1 shapes with diagnostics.
//! Scope: pure transform.
//! Uses:
//! rust/crates/
//! |-- config/src/error.rs
//! |-- config/src/legacy/compat_mcp.rs
//! |-- config/src/schema/mod.rs
//! `-- config/src/schema/v1.rs
//! External:
//! |-- schema::array
//! |-- schema::decode
//! |-- schema::is_valid
//! |-- serde_json::Map
//! |-- serde_json::Value
//! `-- str::to_string
//! Used by:
//! rust/crates/
//! |-- config/src/legacy/compat_mcp.rs
//! |-- config/src/legacy/mod.rs
//! |-- config/src/legacy/parse.rs
//! `-- config/src/legacy/update.rs
//! Invariants: never mutates its input; existing V1 keys win over native ones and report
//! `conflict` diagnostics when they differ; diagnostics never include values.
//! Contract tests: config/tests/compat_fixtures.rs

use super::compat_mcp::{normalize_lsp, normalize_mcp};
use crate::error::{ConfigError, Issue};
use crate::schema::{self, Shape, opt, record, req, strukt, union};
use serde_json::{Map, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticKind {
    Invalid,
    Unsupported,
    Conflict,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub kind: DiagnosticKind,
    pub path: Vec<String>,
    pub message: &'static str,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Lowered {
    pub value: Value,
    pub diagnostics: Vec<Diagnostic>,
}

pub(super) type Out = Map<String, Value>;
pub(super) type Diags = Vec<Diagnostic>;

pub(super) fn path(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|p| p.to_string()).collect()
}

pub(super) fn unsupported(path: Vec<String>, diags: &mut Diags) {
    diags.push(Diagnostic {
        kind: DiagnosticKind::Unsupported,
        path,
        message: "Omitted native setting that cannot be represented in V1",
    });
}

pub(super) fn conflict(path: Vec<String>, diags: &mut Diags) {
    diags.push(Diagnostic {
        kind: DiagnosticKind::Conflict,
        path,
        message: "Retained legacy value over native value",
    });
}

pub(super) fn decode_value(
    shape: &Shape,
    value: &Value,
    path: Vec<String>,
    diags: &mut Diags,
) -> Option<Value> {
    match schema::decode(shape, value) {
        Ok(decoded) => Some(decoded),
        Err(_) => {
            diags.push(Diagnostic {
                kind: DiagnosticKind::Invalid,
                path,
                message: "Native setting could not be lowered because it is malformed",
            });
            None
        }
    }
}

pub(super) fn prefer_legacy(
    target: &mut Out,
    key: &str,
    value: Value,
    path: Vec<String>,
    diags: &mut Diags,
) {
    if let Some(existing) = target.get(key) {
        if existing != &value {
            conflict(path, diags);
        }
        return;
    }
    target.insert(key.into(), value);
}

/// `ConfigV2Compat.lower(input, source)`.
pub fn lower(input: &Value, source: &str) -> Result<Lowered, ConfigError> {
    let Some(parsed) = input.as_object() else {
        return Ok(Lowered {
            value: input.clone(),
            diagnostics: Vec::new(),
        });
    };
    let mut permissions = Vec::new();
    if parsed.contains_key("permissions") {
        permissions.push(path(&["permissions"]));
    }
    for key in ["agents", "agent", "mode"] {
        for (name, agent) in parsed
            .get(key)
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            if agent
                .as_object()
                .is_some_and(|agent| agent.contains_key("permissions"))
            {
                permissions.push(vec![key.to_string(), name.clone(), "permissions".into()]);
            }
        }
    }
    if !permissions.is_empty() {
        let message = "V2 permissions are not supported by OpenCode V1. Use V1 \"permission\" rules or run opencode2.";
        return Err(ConfigError::Invalid {
            path: source.into(),
            issues: permissions.iter().map(|p| Issue::new(p, message)).collect(),
            message: None,
        });
    }
    let mut result = parsed.clone();
    let mut diags = Vec::new();
    for key in ["plugins", "providers", "websearch", "warming"] {
        if parsed.contains_key(key) {
            unsupported(path(&[key]), &mut diags);
        }
    }
    normalize_settings(parsed, &mut result, &mut diags);
    normalize_model(parsed, &mut result, &mut diags);
    normalize_skills(parsed, &mut result, &mut diags);
    normalize_compaction(parsed, &mut result, &mut diags);
    normalize_experimental(parsed, &mut result, &mut diags);
    normalize_agents(parsed, &mut result, &mut diags);
    normalize_commands(parsed, &mut result, &mut diags);
    normalize_mcp(parsed, &mut result, &mut diags);
    normalize_lsp(parsed, &mut result, &mut diags);
    Ok(Lowered {
        value: Value::Object(result),
        diagnostics: diags,
    })
}

fn normalize_settings(input: &Out, result: &mut Out, diags: &mut Diags) {
    if let Some(value) = input.get("snapshots")
        && let Some(value) = decode_value(&Shape::Bool, value, path(&["snapshots"]), diags)
    {
        prefer_legacy(result, "snapshot", value, path(&["snapshots"]), diags);
    }
    if let Some(value) = input.get("media")
        && let Some(value) = decode_value(
            &crate::schema::v1::attachment(),
            value,
            path(&["media"]),
            diags,
        )
    {
        prefer_legacy(result, "attachment", value, path(&["media"]), diags);
    }
}

fn no_slash_hash(text: &str) -> bool {
    !text.contains('/') && !text.contains('#') && !text.is_empty()
}

fn no_hash(text: &str) -> bool {
    !text.contains('#') && !text.is_empty()
}

fn selection_string(text: &str) -> bool {
    let Some((provider, rest)) = text.split_once('/') else {
        return false;
    };
    if !no_slash_hash(provider) || rest.is_empty() {
        return false;
    }
    match rest.split_once('#') {
        None => true,
        Some((model, variant)) => no_hash(model) && no_hash(variant),
    }
}

pub(super) fn selection() -> Shape {
    union(vec![
        Shape::Pattern(selection_string, "^[^/#]+\\/[^#]+(?:#[^#]+)?$"),
        strukt(vec![
            req("providerID", Shape::Pattern(no_slash_hash, "^[^/#]+$")),
            req("model", Shape::Pattern(no_hash, "^[^#]+$")),
            opt("variant", Shape::Pattern(no_hash, "^[^#]+$")),
        ]),
    ])
}

/// `lowerSelection`: `(model, variant)`.
pub(super) fn lower_selection(input: &Value) -> (String, Option<String>) {
    if let Some(text) = input.as_str() {
        return match text.find('#') {
            None => (text.to_string(), None),
            Some(index) => (
                text[..index].to_string(),
                Some(text[index + 1..].to_string()),
            ),
        };
    }
    let get = |key: &str| {
        input
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    (
        format!("{}/{}", get("providerID"), get("model")),
        input
            .get("variant")
            .and_then(Value::as_str)
            .map(str::to_string),
    )
}

fn normalize_model(input: &Out, result: &mut Out, diags: &mut Diags) {
    let Some(model) = input.get("model") else {
        return;
    };
    if !schema::is_valid(&selection(), model) {
        return;
    }
    let (model, variant) = lower_selection(model);
    result.insert("model".into(), Value::String(model));
    if variant.is_some() {
        unsupported(path(&["model", "variant"]), diags);
    }
}

fn is_url(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

fn normalize_skills(input: &Out, result: &mut Out, diags: &mut Diags) {
    let Some(skills) = input.get("skills").filter(|s| s.is_array()) else {
        return;
    };
    let Some(skills) = decode_value(
        &schema::array(Shape::String),
        skills,
        path(&["skills"]),
        diags,
    ) else {
        return;
    };
    let items: Vec<&str> = skills
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let pick = |url: bool| {
        Value::Array(
            items
                .iter()
                .filter(|s| is_url(s) == url)
                .map(|s| Value::from(*s))
                .collect(),
        )
    };
    let mut map = Out::new();
    map.insert("paths".into(), pick(false));
    map.insert("urls".into(), pick(true));
    result.insert("skills".into(), Value::Object(map));
}

fn normalize_compaction(input: &Out, result: &mut Out, diags: &mut Diags) {
    let Some(compaction) = input.get("compaction").and_then(Value::as_object) else {
        return;
    };
    let mut value = compaction.clone();
    if let Some(keep) = compaction.get("keep")
        && let Some(keep) = decode_value(
            &record(Shape::Any),
            keep,
            path(&["compaction", "keep"]),
            diags,
        )
        && let Some(tokens) = keep.get("tokens")
    {
        let p = path(&["compaction", "keep", "tokens"]);
        if let Some(tokens) = decode_value(&Shape::NonNegativeInt, tokens, p.clone(), diags) {
            prefer_legacy(&mut value, "preserve_recent_tokens", tokens, p, diags);
        }
    }
    if let Some(buffer) = compaction.get("buffer") {
        let p = path(&["compaction", "buffer"]);
        if let Some(buffer) = decode_value(&Shape::NonNegativeInt, buffer, p.clone(), diags) {
            prefer_legacy(&mut value, "reserved", buffer, p, diags);
        }
    }
    result.insert("compaction".into(), Value::Object(value));
}

fn normalize_experimental(input: &Out, result: &mut Out, diags: &mut Diags) {
    let Some(experimental) = input.get("experimental").and_then(Value::as_object) else {
        return;
    };
    if experimental.contains_key("portable_shell_scanner") {
        unsupported(path(&["experimental", "portable_shell_scanner"]), diags);
    }
    let Some(depth) = experimental.get("subagent_depth") else {
        return;
    };
    let p = path(&["experimental", "subagent_depth"]);
    if let Some(depth) = decode_value(&Shape::NonNegativeInt, depth, p.clone(), diags) {
        prefer_legacy(result, "subagent_depth", depth, p, diags);
    }
}

fn agent_shape() -> Shape {
    let body = strukt(vec![
        opt("headers", record(Shape::String)),
        opt("body", record(Shape::Any)),
    ]);
    strukt(vec![
        opt("model", selection()),
        opt("request", body),
        opt("system", Shape::String),
        opt("description", Shape::String),
        opt("mode", Shape::Literals(&["subagent", "primary", "all"])),
        opt("hidden", Shape::Bool),
        opt(
            "color",
            Shape::Pattern(
                |t| {
                    t.len() == 7
                        && t.starts_with('#')
                        && t[1..].chars().all(|c| c.is_ascii_hexdigit())
                },
                "^#[0-9a-fA-F]{6}$",
            ),
        ),
        opt("steps", Shape::PositiveInt),
        opt("disabled", Shape::Bool),
    ])
}

fn lower_agent(input: &Value) -> Value {
    let mut out = Out::new();
    for key in ["description", "mode", "hidden", "color", "steps"] {
        if let Some(value) = input.get(key) {
            out.insert(key.into(), value.clone());
        }
    }
    if let Some(system) = input.get("system") {
        out.insert("prompt".into(), system.clone());
    }
    if let Some(disabled) = input.get("disabled") {
        out.insert("disable".into(), disabled.clone());
    }
    if let Some(model) = input.get("model") {
        let (model, variant) = lower_selection(model);
        out.insert("model".into(), Value::String(model));
        if let Some(variant) = variant {
            out.insert("variant".into(), Value::String(variant));
        }
    }
    if let Some(body) = input.get("request").and_then(|r| r.get("body")) {
        out.insert("options".into(), body.clone());
    }
    Value::Object(out)
}

fn normalize_agents(input: &Out, result: &mut Out, diags: &mut Diags) {
    let Some(agents) = input.get("agents") else {
        return;
    };
    let Some(agents) = decode_value(&record(Shape::Any), agents, path(&["agents"]), diags) else {
        return;
    };
    let legacy = result.get("agent").and_then(Value::as_object).cloned();
    let mut merged = legacy.clone().unwrap_or_default();
    for (name, value) in agents.as_object().into_iter().flatten() {
        let p = vec!["agents".to_string(), name.clone()];
        if let Some(existing) = merged.get(name) {
            if existing != value {
                conflict(p, diags);
            }
            continue;
        }
        let Some(parsed) = decode_value(&agent_shape(), value, p.clone(), diags) else {
            continue;
        };
        if parsed
            .get("request")
            .and_then(|r| r.get("headers"))
            .is_some()
        {
            unsupported([p, path(&["request", "headers"])].concat(), diags);
        }
        merged.insert(name.clone(), lower_agent(&parsed));
    }
    if result.contains_key("agent") && legacy.is_none() {
        return;
    }
    if !merged.is_empty() || legacy.is_some() {
        result.insert("agent".into(), Value::Object(merged));
    }
}

fn normalize_commands(input: &Out, result: &mut Out, diags: &mut Diags) {
    let Some(commands) = input.get("commands") else {
        return;
    };
    let Some(commands) = decode_value(&record(Shape::Any), commands, path(&["commands"]), diags)
    else {
        return;
    };
    let legacy = result.get("command").and_then(Value::as_object).cloned();
    if result.contains_key("command") && legacy.is_none() {
        return;
    }
    let shape = strukt(vec![
        req("template", Shape::String),
        opt("description", Shape::String),
        opt("agent", Shape::String),
        opt("model", selection()),
        opt("subtask", Shape::Bool),
    ]);
    let mut merged = legacy.clone().unwrap_or_default();
    for (name, value) in commands.as_object().into_iter().flatten() {
        let p = vec!["commands".to_string(), name.clone()];
        let Some(parsed) = decode_value(&shape, value, p.clone(), diags) else {
            continue;
        };
        let mut lowered = parsed.as_object().cloned().unwrap_or_default();
        if let Some(model) = parsed.get("model") {
            let (model, variant) = lower_selection(model);
            lowered.insert("model".into(), Value::String(model));
            if let Some(variant) = variant {
                lowered.insert("variant".into(), Value::String(variant));
            }
        }
        prefer_legacy(&mut merged, name, Value::Object(lowered), p, diags);
    }
    if !merged.is_empty() || legacy.is_some() {
        result.insert("command".into(), Value::Object(merged));
    }
}
