//! Purpose: legacy loader steps (`packages/opencode/src/config/config.ts` and `paths.ts`): the
//! well-known remote config fetch and merge, project file and directory discovery,
//! `ensureGitignore`, and the final post-processing pass.
//! Scope: helpers for `load.rs`.
//! Uses:
//! rust/crates/
//! |-- config/src/env.rs
//! |-- config/src/error.rs
//! |-- config/src/fs.rs
//! |-- config/src/legacy/load.rs
//! |-- config/src/legacy/parse.rs
//! |-- config/src/legacy/remote.rs
//! |-- config/src/schema/v1.rs
//! |-- config/src/value.rs
//! `-- config/src/variable.rs
//! External:
//! `-- serde_json::{Map, Value, json}
//! Used by:
//! rust/crates/
//! `-- config/src/legacy/load.rs
//! Invariants: remote `remote_config.url`/headers substitute `{env:}` with the auth overlay only
//! (process env never mutated); nested `config` in a remote payload wins over the payload root;
//! `tools` lower into `permission` with explicit `permission` entries taking precedence.
//! Contract tests: config/tests/legacy_load.rs

use super::load::Instance;
use super::parse::{self, Diagnostics, SCHEMA_URL};
use super::remote::{RemoteFetcher, fetch_json};
use crate::env::Environment;
use crate::error::ConfigError;
use crate::value::{Provenance, merge_deep};
use crate::variable::{self, Missing, Source};
use crate::{fs, schema};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub(super) fn load_wellknown(
    env: &Environment,
    diags: &mut Diagnostics,
    remote: &dyn RemoteFetcher,
    url: &str,
    source: &str,
    auth_env: &BTreeMap<String, String>,
) -> Result<Value, ConfigError> {
    let wellknown = fetch_json(remote, source, &BTreeMap::new(), url)?;
    let wellknown = schema::decode(&schema::v1::well_known(), &wellknown).map_err(|_| {
        ConfigError::Remote(crate::error::RemoteError::Decode {
            url: source.into(),
            message: "expected object".into(),
        })
    })?;
    let virtual_source = Source::Virtual {
        source: source.into(),
        dir: PathBuf::from(url),
    };
    let sub = |text: &str| {
        variable::substitute(text, &virtual_source, env, Some(auth_env), Missing::Error)
    };
    let remote_config = wellknown
        .get("remote_config")
        .filter(|v| v.get("url").is_some_and(Value::is_string));
    let fetched = match remote_config {
        None => json!({}),
        Some(remote_config) => {
            let target = sub(remote_config["url"].as_str().unwrap_or_default())?;
            let mut headers = BTreeMap::new();
            for (key, value) in remote_config
                .get("headers")
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
            {
                if let Some(value) = value.as_str() {
                    headers.insert(key.clone(), sub(value)?);
                }
            }
            let data = fetch_json(remote, &target, &headers, url)?;
            match (data.get("config"), &data) {
                (Some(config @ Value::Object(_)), _) => config.clone(),
                (_, Value::Object(_)) => data,
                _ => {
                    let message = "expected object".to_string();
                    return Err(crate::error::RemoteError::Decode {
                        url: target,
                        message,
                    }
                    .into());
                }
            }
        }
    };
    let base = wellknown
        .get("config")
        .filter(|c| c.is_object())
        .cloned()
        .unwrap_or_else(|| json!({}));
    let mut remote_config = merge_deep(&base, &fetched);
    if remote_config
        .get("$schema")
        .is_none_or(|s| s.as_str().is_none_or(str::is_empty))
    {
        remote_config["$schema"] = json!(SCHEMA_URL);
    }
    let virtual_source = Source::Virtual {
        source: source.into(),
        dir: PathBuf::from(dirname(source)),
    };
    parse::load_config(
        &remote_config.to_string(),
        &virtual_source,
        env,
        Some(auth_env),
        diags,
    )
}

/// `path.dirname` on a source string (also used for URLs).
pub(super) fn dirname(source: &str) -> &str {
    match source.trim_end_matches('/').rfind('/') {
        Some(0) => "/",
        Some(index) => &source[..index],
        None => ".",
    }
}

/// `ConfigPaths.files("opencode", directory, worktree)`: outermost first, jsonc after json.
pub fn project_files(instance: &Instance) -> Vec<PathBuf> {
    let mut files = fs::up(
        &["opencode.jsonc", "opencode.json"],
        &instance.directory,
        Some(&instance.worktree),
    );
    files.reverse();
    files
}

/// `ConfigPaths.directories(directory, worktree)`.
pub fn directories(env: &Environment, instance: &Instance) -> Vec<PathBuf> {
    let mut list = vec![env.global_config.clone()];
    if !env.disable_project_config() {
        list.extend(fs::up(
            &[".opencode"],
            &instance.directory,
            Some(&instance.worktree),
        ));
    }
    list.extend(fs::up(&[".opencode"], &env.home, Some(&env.home)));
    list.extend(env.opencode_config_dir());
    let mut unique = Vec::new();
    for dir in list {
        if !unique.contains(&dir) {
            unique.push(dir);
        }
    }
    unique
}

/// `ensureGitignore(dir)`: create the dir and a default `.gitignore`; permission errors ignored.
pub(super) fn ensure_gitignore(dir: &Path) -> Result<(), ConfigError> {
    if let Err(error) = std::fs::create_dir_all(dir)
        && error.kind() != std::io::ErrorKind::PermissionDenied
    {
        return Err(ConfigError::io("mkdir", dir, &error));
    }
    let gitignore = dir.join(".gitignore");
    if fs::exists(&gitignore) {
        return Ok(());
    }
    let content = [
        "node_modules",
        "package.json",
        "package-lock.json",
        "bun.lock",
        ".gitignore",
    ]
    .join("\n");
    match std::fs::write(&gitignore, content) {
        Err(error) if error.kind() != std::io::ErrorKind::PermissionDenied => {
            Err(ConfigError::io("write", &gitignore, &error))
        }
        _ => Ok(()),
    }
}

pub(super) fn post_process(env: &Environment, result: &mut Value, provenance: &mut Provenance) {
    let modes = result
        .get("mode")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    for (name, mode) in modes {
        let mut mode = mode.as_object().cloned().unwrap_or_default();
        mode.insert("mode".into(), json!("primary"));
        let agent = result.get("agent").cloned().unwrap_or_else(|| json!({}));
        result["agent"] = merge_deep(&agent, &json!({ name: mode }));
    }
    if let Some(raw) = env.var("OPENCODE_PERMISSION") {
        // Invalid JSON is logged and skipped in the baseline.
        if let Ok(parsed) = serde_json::from_str::<Value>(raw) {
            let current = result
                .get("permission")
                .cloned()
                .unwrap_or_else(|| json!({}));
            result["permission"] = merge_deep(&current, &parsed);
            provenance.set(&["permission".into()], "OPENCODE_PERMISSION");
        }
    }
    if let Some(tools) = result.get("tools").and_then(Value::as_object).cloned() {
        let mut perms = Map::new();
        for (tool, enabled) in tools {
            let action = if enabled.as_bool() == Some(true) {
                "allow"
            } else {
                "deny"
            };
            let key = if matches!(tool.as_str(), "write" | "edit" | "patch") {
                "edit".to_string()
            } else {
                tool
            };
            perms.insert(key, json!(action));
        }
        let current = result
            .get("permission")
            .cloned()
            .unwrap_or_else(|| json!({}));
        result["permission"] = merge_deep(&Value::Object(perms), &current);
    }
    if result
        .get("username")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        result["username"] = json!(env.username_or_default());
    }
    if result.get("autoshare") == Some(&json!(true))
        && result
            .get("share")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
    {
        result["share"] = json!("auto");
    }
    for (flag, key) in [
        ("OPENCODE_DISABLE_AUTOCOMPACT", "auto"),
        ("OPENCODE_DISABLE_PRUNE", "prune"),
    ] {
        if env.truthy(flag) {
            let mut compaction = result
                .get("compaction")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            compaction.insert(key.into(), json!(false));
            result["compaction"] = Value::Object(compaction);
        }
    }
}
