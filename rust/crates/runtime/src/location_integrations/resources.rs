//! Purpose: discover configured skill files and materialize local reference metadata.
//! Scope: Location resource snapshot, preserving configured source and file order.
//! Uses:
//! rust/crates/
//! |-- config/src/resources.rs
//! `-- config/src/legacy/markdown.rs
//! External:
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! `-- runtime/src/location_integrations.rs
//! Invariants: later skills replace earlier names; invalid frontmatter is skipped; remote caches stay isolated.
//! Contract tests: runtime/tests/location_integrations.rs

use opencode_config::{CurrentConfig, legacy::markdown, resources};
use opencode_integration_api::IntegrationError;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Default)]
pub struct Resources {
    pub skills: Vec<Value>,
    pub references: Vec<Value>,
    pub commands: Vec<Value>,
    pub gaps: Vec<String>,
    tasks: tokio::sync::Mutex<tokio::task::JoinSet<()>>,
}

impl Resources {
    pub async fn load(
        config: &CurrentConfig,
        home: &Path,
        location: &Path,
        cache: &Path,
        environment: &BTreeMap<String, String>,
    ) -> Result<Self, IntegrationError> {
        let mut result = Self::default();
        result.skills.push(json!({
            "name":"customize-opencode",
            "description":"Use ONLY when the user is editing or creating opencode's own configuration: opencode.json, opencode.jsonc, files under .opencode/, or files under ~/.config/opencode/. Also use when creating or fixing opencode agents, subagents, commands, skills, plugins, MCP servers, or permission rules. Do not use for the user's own application code, or for any project that is not configuring opencode itself.",
            "location":"/builtin/customize-opencode.md",
            "content":include_str!("../../../../../packages/core/src/plugin/skill/customize-opencode.md")
        }));
        let mut directories = Vec::new();
        for source in resources::skills(config.entries(), home, location) {
            match source {
                resources::SkillSource::Directory(directory) => directories.push(directory),
                resources::SkillSource::Url(url) => {
                    match opencode_integration_adapters::skills::discover(cache, &url).await {
                        Ok(loaded) => directories.extend(loaded),
                        Err(error) => tracing::warn!(%error, "skill index unavailable"),
                    }
                }
            }
        }
        for directory in directories {
            let mut files = Vec::new();
            collect(&directory, &directory, &mut HashSet::new(), &mut files);
            files.sort();
            for file in files {
                let Some(markdown) = std::fs::read_to_string(&file)
                    .ok()
                    .and_then(|content| markdown::parse(&content).ok())
                else {
                    continue;
                };
                if ["name", "description"].iter().any(|name| {
                    markdown
                        .data
                        .get(*name)
                        .is_some_and(|value| !value.is_string())
                }) || markdown
                    .data
                    .get("slash")
                    .is_some_and(|value| !value.is_boolean())
                {
                    continue;
                }
                let name = markdown
                    .data
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .or_else(|| {
                        (file.parent() == Some(directory.as_path()))
                            .then(|| {
                                file.file_stem()
                                    .and_then(|name| name.to_str())
                                    .map(str::to_owned)
                            })
                            .flatten()
                    });
                let Some(name) = name.filter(|name| !name.is_empty()) else {
                    continue;
                };
                let mut skill = json!({"name":name,"location":file,"content":markdown.content});
                for key in ["description", "slash"] {
                    if let Some(value) = markdown.data.get(key) {
                        skill[key] = value.clone();
                    }
                }
                if let Some(existing) = result.skills.iter_mut().find(|skill| skill["name"] == name)
                {
                    *existing = skill;
                } else {
                    result.skills.push(skill);
                }
            }
        }
        for (name, source) in resources::references(config.entries(), home, location) {
            let path = if source["type"] == "local" {
                source["path"].clone()
            } else {
                let Some(repository) = source["repository"].as_str().and_then(|value| {
                    opencode_integration_adapters::references::Repository::parse(
                        value,
                        environment
                            .get("OPENCODE_TEST_GITHUB_BASE_URL")
                            .map(String::as_str),
                    )
                }) else {
                    continue;
                };
                let branch = source["branch"].as_str().map(str::to_owned);
                let cache = cache.join("references");
                let Ok(path) = repository.cache_path(&cache, branch.as_deref()) else {
                    continue;
                };
                let environment = environment.clone();
                let alias = name.clone();
                result.tasks.get_mut().spawn(async move {
                    if let Err(error) = opencode_integration_adapters::references::ensure(
                        &repository,
                        &cache,
                        branch.as_deref(),
                        &environment,
                    )
                    .await
                    {
                        tracing::warn!(%error, name = alias, "failed to materialize reference");
                    }
                });
                json!(path)
            };
            let mut reference = json!({"name":name,"path":path,"source":source});
            for key in ["description", "hidden"] {
                if let Some(value) = reference["source"].get(key).cloned() {
                    reference[key] = value;
                }
            }
            result.references.push(reference);
        }
        let mut commands = BTreeMap::new();
        for (name, mut command) in resources::commands(config.entries()) {
            command["name"] = json!(name);
            commands.insert(name, command);
        }
        result.commands = commands.into_values().collect();
        Ok(result)
    }
    pub fn legacy_skills(&self) -> Vec<Value> {
        self.skills
            .iter()
            .map(|skill| {
                let mut skill = skill.clone();
                if skill["name"] == "customize-opencode"
                    && skill["location"] == "/builtin/customize-opencode.md"
                {
                    skill["location"] = json!("<built-in>");
                    if let Some(description) = skill["description"].as_str() {
                        skill["description"] = json!(
                            description.replace("subagents, commands, skills", "subagents, skills")
                        );
                    }
                }
                skill
            })
            .collect()
    }
    pub async fn shutdown(&self) {
        let mut tasks = self.tasks.lock().await;
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
    }
}

fn collect(
    root: &Path,
    directory: &Path,
    visited: &mut HashSet<PathBuf>,
    files: &mut Vec<PathBuf>,
) {
    let Ok(real) = directory.canonicalize() else {
        return;
    };
    if !visited.insert(real) {
        return;
    }
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let file = entry.path();
        if file.is_dir() {
            collect(root, &file, visited, files);
            continue;
        }
        if file.is_file()
            && ((directory == root && file.extension().is_some_and(|ext| ext == "md"))
                || file.file_name().is_some_and(|name| name == "SKILL.md"))
        {
            files.push(file);
        }
    }
}
