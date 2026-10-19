//! Purpose: per-resource consumers of the current ordered config (`packages/core/src/config/plugin/
//! {agent,command,skill,reference,external}.ts`): resolve each resource's ordered contributions
//! from documents and supplemental directories, ready for the owning registries.
//! Scope: pure resolution + markdown discovery reads.
//! Uses:
//! rust/crates/
//! |-- config/src/current.rs
//! |-- config/src/fs.rs
//! |-- config/src/legacy/markdown.rs
//! |-- config/src/legacy/plugin.rs
//! |-- config/src/migrate.rs
//! |-- config/src/schema/mod.rs
//! |-- config/src/variable.rs
//! `-- platform-api/src/path.rs
//! External:
//! |-- char::is_whitespace
//! |-- fs::glob
//! |-- fs::read_safe
//! |-- fs::relative_slash
//! |-- markdown::parse
//! |-- migrate::migrate_agent
//! |-- plugin::discover
//! |-- schema::decode
//! |-- schema::v1::agent
//! |-- schema::v2::info
//! |-- serde_json::Map
//! |-- serde_json::Value
//! |-- serde_json::json
//! |-- str::to_string
//! `-- url::Url::parse
//! Used by:
//! rust/crates/
//! |-- config/src/lib.rs
//! `-- config/tests/legacy_update.rs
//! Invariants: directory entries expand in place (agent/command markdown documents keep their
//! priority slot); unparseable markdown is skipped; references with invalid aliases are dropped;
//! path permissions expand `~`/`$HOME` only for path actions.
//! Contract tests: config/tests/legacy_update.rs

use crate::current::{Document, Entry, latest};
use crate::legacy::{markdown, plugin};
use crate::{fs, migrate, schema};
use opencode_platform_api::path::resolve;
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

const AGENT_KEYS: [&str; 11] = [
    "model",
    "variant",
    "request",
    "system",
    "description",
    "mode",
    "hidden",
    "color",
    "steps",
    "disabled",
    "permissions",
];
const PATH_ACTIONS: [&str; 3] = ["external_directory", "read", "edit"];

/// Agent documents: config documents with directory entries expanded into markdown agent docs.
pub fn agent_documents(entries: &[Entry]) -> Vec<Document> {
    entries
        .iter()
        .flat_map(|entry| match entry {
            Entry::Document(doc) => vec![doc.clone()],
            Entry::Directory(dir) => discover_agents(dir),
        })
        .collect()
}

fn discover_agents(dir: &Path) -> Vec<Document> {
    let sources = [
        (&["agent", "agents"], true, false),
        (&["mode", "modes"], false, true),
    ];
    sources
        .iter()
        .flat_map(|(dirs, recursive, primary)| {
            fs::glob(dir, &dirs[..], *recursive, &["md"])
                .into_iter()
                .filter_map(move |file| decode_agent(dir, &file, *primary))
        })
        .collect()
}

fn decode_agent(dir: &Path, file: &Path, primary: bool) -> Option<Document> {
    let md = markdown::parse(&fs::read_safe(file).ok()??).ok()?;
    let relative = fs::relative_slash(dir, file);
    let name = ["agent/", "agents/", "mode/", "modes/"]
        .iter()
        .find_map(|prefix| relative.strip_prefix(prefix))
        .unwrap_or(&relative);
    let name = name.strip_suffix(".md").unwrap_or(name).to_string();
    let body = crate::variable::js_trim(&md.content).to_string();
    let legacy = md
        .data
        .keys()
        .any(|key| !AGENT_KEYS.contains(&key.as_str()));
    let mut agent = if legacy {
        let mut input = Map::from_iter([("name".to_string(), json!(name))]);
        input.extend(md.data.clone());
        input.insert("prompt".into(), json!(body));
        migrate::migrate_agent(&schema::decode(&schema::v1::agent(), &Value::Object(input)).ok()?)
    } else {
        let mut input = md.data.clone();
        input.insert("system".into(), json!(body));
        Value::Object(input)
    };
    if primary {
        agent["mode"] = json!("primary");
    }
    let info = schema::decode(schema::v2::info(), &json!({ "agents": { name: agent } })).ok()?;
    Some(Document {
        path: Some(file.to_path_buf()),
        info,
    })
}

/// `expandPermissions`: `~`, `~/`, `$HOME`, `$HOME/` (and `$HOME\`) for path actions only.
pub fn expand_permissions(rules: &[Value], home: &Path) -> Vec<Value> {
    let home = home.display().to_string();
    rules
        .iter()
        .map(|rule| {
            let action = rule
                .get("action")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let Some(resource) = rule.get("resource").and_then(Value::as_str) else {
                return rule.clone();
            };
            if !PATH_ACTIONS.contains(&action) {
                return rule.clone();
            }
            let expanded = if let Some(rest) = resource.strip_prefix("~/") {
                format!("{home}/{rest}")
            } else if resource == "~" || resource == "$HOME" {
                home.clone()
            } else if let Some(rest) = resource
                .strip_prefix("$HOME/")
                .or_else(|| resource.strip_prefix("$HOME\\"))
            {
                format!("{home}{}{rest}", &resource[5..6])
            } else {
                resource.to_string()
            };
            let mut rule = rule.clone();
            rule["resource"] = json!(expanded);
            rule
        })
        .collect()
}

/// Resolved agent configuration: default agent, shared permissions, ordered per-agent edits.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentConfig {
    pub default_agent: Option<String>,
    pub permissions: Vec<Value>,
    pub agents: Vec<(String, Value)>,
}

pub fn agents(entries: &[Entry], home: &Path) -> AgentConfig {
    let documents = agent_documents(entries);
    let permissions: Vec<Value> = documents
        .iter()
        .flat_map(|doc| {
            doc.info
                .get("permissions")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        })
        .collect();
    let entries: Vec<Entry> = documents.iter().cloned().map(Entry::Document).collect();
    let agents = documents
        .iter()
        .flat_map(|doc| {
            doc.info
                .get("agents")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default()
        })
        .map(|(id, mut item)| {
            if let Some(rules) = item.get("permissions").and_then(Value::as_array) {
                item["permissions"] = Value::Array(expand_permissions(rules, home));
            }
            (id, item)
        })
        .collect();
    AgentConfig {
        default_agent: latest(&entries, "default_agent")
            .and_then(Value::as_str)
            .map(str::to_string),
        permissions: expand_permissions(&permissions, home),
        agents,
    }
}

/// Ordered command definitions (documents and `.opencode/command(s)/**/*.md`).
pub fn commands(entries: &[Entry]) -> Vec<(String, Value)> {
    let shape = || schema::v2::info();
    entries
        .iter()
        .flat_map(|entry| match entry {
            Entry::Document(doc) => doc
                .info
                .get("commands")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .collect::<Vec<_>>(),
            Entry::Directory(dir) => fs::glob(dir, &["command", "commands"], true, &["md"])
                .into_iter()
                .filter_map(|file| {
                    let md = markdown::parse(&fs::read_safe(&file).ok()??).ok()?;
                    let relative = fs::relative_slash(dir, &file);
                    let name = relative
                        .strip_prefix("command/")
                        .or_else(|| relative.strip_prefix("commands/"))
                        .unwrap_or(&relative);
                    let name = name.strip_suffix(".md").unwrap_or(name).to_string();
                    let mut input = md.data.clone();
                    input.insert(
                        "template".into(),
                        json!(crate::variable::js_trim(&md.content)),
                    );
                    let info =
                        schema::decode(shape(), &json!({ "commands": { &name: input } })).ok()?;
                    Some((name.clone(), info["commands"][&name].clone()))
                })
                .collect(),
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillSource {
    Directory(PathBuf),
    Url(String),
}

/// Skill sources: `skill`/`skills` under each directory entry, then configured paths/URLs.
pub fn skills(entries: &[Entry], home: &Path, location: &Path) -> Vec<SkillSource> {
    let mut out: Vec<SkillSource> = entries
        .iter()
        .filter_map(Entry::directory)
        .flat_map(|dir| {
            [
                SkillSource::Directory(dir.join("skill")),
                SkillSource::Directory(dir.join("skills")),
            ]
        })
        .collect();
    for item in entries.iter().filter_map(Entry::document).flat_map(|doc| {
        doc.info
            .get("skills")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }) {
        let Some(item) = item.as_str() else { continue };
        if url::Url::parse(item).is_ok_and(|url| matches!(url.scheme(), "http" | "https")) {
            out.push(SkillSource::Url(item.into()));
            continue;
        }
        let expanded = item
            .strip_prefix("~/")
            .map(|rest| home.join(rest))
            .unwrap_or_else(|| PathBuf::from(item));
        out.push(SkillSource::Directory(if expanded.is_absolute() {
            expanded
        } else {
            location.join(expanded)
        }));
    }
    out
}

/// References keyed by alias (last document wins), as `Reference.Source` JSON.
pub fn references(entries: &[Entry], home: &Path, location: &Path) -> Vec<(String, Value)> {
    let mut out: Vec<(String, Value)> = Vec::new();
    for doc in entries.iter().filter_map(Entry::document) {
        let directory = doc
            .path
            .as_ref()
            .and_then(|p| p.parent())
            .unwrap_or(location);
        for (name, entry) in doc
            .info
            .get("references")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            if name.is_empty()
                || name.contains(['/', '`', ','])
                || name.chars().any(char::is_whitespace)
            {
                continue;
            }
            let text = entry.as_str();
            let local = text
                .map(|t| t.starts_with(['.', '/', '~']))
                .unwrap_or_else(|| entry.get("path").is_some());
            let mut source = Map::new();
            if local {
                let value = text
                    .or_else(|| entry.get("path").and_then(Value::as_str))
                    .unwrap_or_default();
                let path = match value.strip_prefix("~/") {
                    Some(rest) => home.join(rest),
                    None => resolve(directory, Path::new(value)),
                };
                source.insert("type".into(), json!("local"));
                source.insert("path".into(), json!(path.display().to_string()));
            } else {
                source.insert("type".into(), json!("git"));
                source.insert(
                    "repository".into(),
                    text.map(Value::from)
                        .or_else(|| entry.get("repository").cloned())
                        .unwrap_or_default(),
                );
                if let Some(branch) = entry.get("branch") {
                    source.insert("branch".into(), branch.clone());
                }
            }
            for key in ["description", "hidden"] {
                if let Some(value) = entry.get(key) {
                    source.insert(key.into(), value.clone());
                }
            }
            out.retain(|(existing, _)| existing != name);
            out.push((name.clone(), Value::Object(source)));
        }
    }
    out
}

/// External plugin refs: `(package-or-absolute-path, options)` in load order.
pub fn plugins(entries: &[Entry], location: &Path) -> Vec<(String, Option<Value>)> {
    let mut out = Vec::new();
    for entry in entries {
        match entry {
            Entry::Document(doc) => {
                let directory = doc
                    .path
                    .as_ref()
                    .and_then(|p| p.parent())
                    .unwrap_or(location);
                for item in doc
                    .info
                    .get("plugins")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let package = item
                        .as_str()
                        .or_else(|| item.get("package").and_then(Value::as_str))
                        .unwrap_or_default();
                    let resolved = if package.starts_with("file://") {
                        url::Url::parse(package)
                            .ok()
                            .and_then(|u| u.to_file_path().ok())
                            .map(|p| p.display().to_string())
                            .unwrap_or(package.into())
                    } else if package.starts_with("./") || package.starts_with("../") {
                        resolve(directory, Path::new(package)).display().to_string()
                    } else {
                        package.to_string()
                    };
                    out.push((resolved, item.get("options").cloned()));
                }
            }
            Entry::Directory(dir) => {
                out.extend(plugin::discover(dir).iter().filter_map(|url| {
                    let url = url::Url::parse(url.as_str()?).ok()?;
                    Some((url.to_file_path().ok()?.display().to_string(), None))
                }));
            }
        }
    }
    out
}
