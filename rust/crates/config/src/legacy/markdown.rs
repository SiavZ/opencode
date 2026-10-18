//! Purpose: Markdown frontmatter parsing (gray-matter + js-yaml semantics, with the colon
//! sanitizing fallback from `packages/core/src/config/markdown.ts`) and the legacy `.opencode`
//! directory loaders for agents, modes and commands (`packages/opencode/src/config/{agent,command}.ts`).
//! Scope: file reads + pure transforms.
//! Uses:
//! rust/crates/
//! |-- config/src/error.rs
//! |-- config/src/fs.rs
//! |-- config/src/schema/v1.rs
//! `-- config/src/variable.rs
//! External:
//! |-- serde_json::{Map, Value}
//! `-- yaml_rust2::{Yaml, YamlLoader}
//! Used by:
//! rust/crates/
//! |-- config/src/legacy/load.rs
//! `-- config/src/plugins.rs
//! Invariants: entry names strip the anchored `agent/`/`agents/`/`mode/`/`modes/`/`command/`/
//! `commands/` prefix of the path relative to the scanned dir (#25713); invalid agent markdown
//! fails with `ConfigInvalidError`, invalid modes are skipped, unparseable files are skipped.
//! Contract tests: config/tests/legacy_load.rs

use crate::error::ConfigError;
use crate::schema::{self, v1};
use crate::{fs, variable};
use serde_json::{Map, Value};
use std::path::Path;
use yaml_rust2::{Yaml, YamlLoader};

#[derive(Debug, Clone, PartialEq)]
pub struct Markdown {
    pub data: Map<String, Value>,
    pub content: String,
}

/// `ConfigMarkdown.parse`: try strict YAML, then retry after sanitizing unquoted colons.
pub fn parse(content: &str) -> Result<Markdown, String> {
    matter(content).or_else(|_| matter(&sanitize(content)))
}

fn matter(input: &str) -> Result<Markdown, String> {
    let Some(rest) = input.strip_prefix("---") else {
        return Ok(Markdown {
            data: Map::new(),
            content: input.to_string(),
        });
    };
    if rest.starts_with('-') {
        return Ok(Markdown {
            data: Map::new(),
            content: input.to_string(),
        });
    }
    let language_end = rest.find(['\r', '\n']).unwrap_or(0);
    let rest = &rest[language_end..];
    let close = rest.find("\n---");
    let block = &rest[..close.unwrap_or(rest.len())];
    let content = match close {
        None => String::new(),
        Some(index) => {
            let after = &rest[index + 4..];
            let after = after.strip_prefix('\r').unwrap_or(after);
            after.strip_prefix('\n').unwrap_or(after).to_string()
        }
    };
    let stripped: String = block
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");
    if stripped.trim().is_empty() {
        return Ok(Markdown {
            data: Map::new(),
            content,
        });
    }
    let docs = YamlLoader::load_from_str(block).map_err(|error| error.to_string())?;
    let data = match docs.into_iter().next().map(yaml_to_json) {
        Some(Value::Object(map)) => map,
        _ => Map::new(),
    };
    Ok(Markdown { data, content })
}

fn yaml_to_json(yaml: Yaml) -> Value {
    match yaml {
        Yaml::Real(text) => text
            .parse::<f64>()
            .map(crate::jsonc::number_value)
            .unwrap_or(Value::String(text)),
        Yaml::Integer(n) => Value::from(n),
        Yaml::String(text) => Value::String(text),
        Yaml::Boolean(b) => Value::Bool(b),
        Yaml::Array(items) => Value::Array(items.into_iter().map(yaml_to_json).collect()),
        Yaml::Hash(hash) => Value::Object(
            hash.into_iter()
                .map(|(key, value)| {
                    let key = match key {
                        Yaml::String(text) | Yaml::Real(text) => text,
                        Yaml::Integer(n) => n.to_string(),
                        Yaml::Boolean(b) => b.to_string(),
                        _ => "null".into(),
                    };
                    (key, yaml_to_json(value))
                })
                .collect(),
        ),
        _ => Value::Null,
    }
}

/// `ConfigMarkdown.sanitize`: rewrite `key: value: with colons` into block scalars.
pub fn sanitize(content: &str) -> String {
    let Some(body) = content.strip_prefix("---") else {
        return content.to_string();
    };
    let body = body
        .strip_prefix("\r\n")
        .or_else(|| body.strip_prefix('\n'));
    let Some(body) = body else {
        return content.to_string();
    };
    let Some(end) = body.find("\n---").map(|end| {
        if body[..end].ends_with('\r') {
            end - 1
        } else {
            end
        }
    }) else {
        return content.to_string();
    };
    let frontmatter = &body[..end];
    let lines: Vec<String> = frontmatter
        .split('\n')
        .flat_map(|raw| {
            let line = raw.strip_suffix('\r').unwrap_or(raw);
            let trimmed = line.trim();
            if trimmed.starts_with('#')
                || trimmed.is_empty()
                || line.starts_with(char::is_whitespace)
            {
                return vec![line.to_string()];
            }
            let Some((key, value)) = line.split_once(':') else {
                return vec![line.to_string()];
            };
            let key = key.trim_end();
            let valid_key = key
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
            let value = value.trim();
            if !valid_key
                || value.is_empty()
                || value == ">"
                || value == "|"
                || value.starts_with(['"', '\''])
            {
                return vec![line.to_string()];
            }
            if !value.contains(':') {
                return vec![line.to_string()];
            }
            vec![format!("{key}: |-"), format!("  {value}")]
        })
        .collect();
    content.replacen(frontmatter, &lines.join("\n"), 1)
}

/// `configEntryNameFromPath`.
pub fn entry_name(relative: &str, prefixes: &[&str]) -> String {
    let normalized = relative.replace('\\', "/");
    let candidate = prefixes
        .iter()
        .find_map(|prefix| normalized.strip_prefix(prefix).map(str::to_string))
        .unwrap_or_else(|| {
            normalized
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .to_string()
        });
    match candidate.rfind('.') {
        Some(dot) if dot > 0 && !candidate[dot..].contains('/') => candidate[..dot].to_string(),
        _ => candidate,
    }
}

fn read_markdown(path: &Path) -> Option<Markdown> {
    let text = fs::read_safe(path).ok().flatten()?;
    parse(&text).ok()
}

fn with_body(name: &str, data: &Map<String, Value>, key: &str, body: &str) -> Value {
    let mut config = Map::new();
    config.insert("name".into(), Value::String(name.into()));
    for (k, v) in data {
        config.insert(k.clone(), v.clone());
    }
    config.insert(
        key.into(),
        Value::String(variable::js_trim(body).to_string()),
    );
    Value::Object(config)
}

fn invalid(path: &Path, issues: Vec<crate::error::Issue>) -> ConfigError {
    ConfigError::Invalid {
        path: path.display().to_string(),
        issues,
        message: None,
    }
}

/// `ConfigAgent.load(dir)`.
pub fn load_agents(dir: &Path) -> Result<Map<String, Value>, ConfigError> {
    let mut result = Map::new();
    for item in fs::glob(dir, &["agent", "agents"], true, &["md"]) {
        let Some(md) = read_markdown(&item) else {
            continue;
        };
        let name = entry_name(&fs::relative_slash(dir, &item), &["agent/", "agents/"]);
        let config = with_body(&name, &md.data, "prompt", &md.content);
        let decoded =
            schema::decode(&v1::agent(), &config).map_err(|issues| invalid(&item, issues))?;
        let key = config
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(&name)
            .to_string();
        result.insert(key, decoded);
    }
    Ok(result)
}

/// `ConfigAgent.loadMode(dir)`: invalid modes are skipped; every mode becomes `primary`.
pub fn load_modes(dir: &Path) -> Map<String, Value> {
    let mut result = Map::new();
    for item in fs::glob(dir, &["mode", "modes"], false, &["md"]) {
        let Some(md) = read_markdown(&item) else {
            continue;
        };
        let name = entry_name(&fs::relative_slash(dir, &item), &["mode/", "modes/"]);
        let config = with_body(&name, &md.data, "prompt", &md.content);
        let Ok(Value::Object(mut decoded)) = schema::decode(&v1::agent(), &config) else {
            continue;
        };
        decoded.insert("mode".into(), Value::String("primary".into()));
        let key = config
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(&name)
            .to_string();
        result.insert(key, Value::Object(decoded));
    }
    result
}

/// `ConfigCommand.load(dir)`.
pub fn load_commands(dir: &Path) -> Result<Map<String, Value>, ConfigError> {
    let mut result = Map::new();
    for item in fs::glob(dir, &["command", "commands"], true, &["md"]) {
        let Some(md) = read_markdown(&item) else {
            continue;
        };
        let name = entry_name(&fs::relative_slash(dir, &item), &["command/", "commands/"]);
        let config = with_body(&name, &md.data, "template", &md.content);
        let decoded =
            schema::decode(&v1::command(), &config).map_err(|issues| invalid(&item, issues))?;
        let key = config
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(&name)
            .to_string();
        result.insert(key, decoded);
    }
    Ok(result)
}
