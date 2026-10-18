//! Purpose: legacy config file loading (`packages/opencode/src/config/config.ts:54-293`):
//! `normalizeLoadedConfig`, `decodeConfig` (V2-compat lower + V1 decode), `loadConfig`
//! (substitution, JSONC, plugin spec resolution, `$schema` seeding), `loadFile`, `loadGlobal`
//! (seed file, config.json/opencode.json/opencode.jsonc, legacy TOML migration).
//! Scope: file IO + pure transforms.
//! Uses:
//! rust/crates/
//! |-- config/src/env.rs
//! |-- config/src/error.rs
//! |-- config/src/fs.rs
//! |-- config/src/jsonc/mod.rs
//! |-- config/src/legacy/compat.rs
//! |-- config/src/legacy/plugin.rs
//! |-- config/src/schema/v1.rs
//! |-- config/src/value.rs
//! `-- config/src/variable.rs
//! External:
//! |-- serde_json::{Map, Value}
//! `-- toml::Value
//! Used by:
//! rust/crates/
//! |-- config/src/legacy/load.rs
//! `-- config/src/legacy/update.rs
//! Invariants: the `$schema` seed edits the original (unsubstituted) file text, so expanded
//! `{env:}`/`{file:}` values are never written back; seed write failures are ignored.
//! Contract tests: config/tests/legacy_load.rs

use super::compat::{self, Diagnostic};
use super::plugin;
use crate::env::Environment;
use crate::error::ConfigError;
use crate::value::{merge_deep, stringify_pretty};
use crate::variable::{self, Missing, Source};
use crate::{fs, jsonc, schema};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const SCHEMA_URL: &str = "https://opencode.ai/config.json";

/// A compatibility diagnostic with the source it came from (values are never included).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourcedDiagnostic {
    pub source: String,
    pub diagnostic: Diagnostic,
}

/// Collected diagnostics for one load.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Diagnostics(pub Vec<SourcedDiagnostic>);

/// `normalizeLoadedConfig`: drop legacy TUI keys.
pub fn normalize_loaded(data: &Value) -> Value {
    let mut copy = data.clone();
    if let Some(map) = copy.as_object_mut() {
        for key in ["theme", "keybinds", "tui"] {
            map.remove(key);
        }
    }
    copy
}

/// `decodeConfig(input, source)`.
pub fn decode_config(
    input: &Value,
    source: &str,
    diags: &mut Diagnostics,
) -> Result<Value, ConfigError> {
    let lowered = compat::lower(&normalize_loaded(input), source)?;
    diags.0.extend(
        lowered
            .diagnostics
            .into_iter()
            .map(|diagnostic| SourcedDiagnostic {
                source: source.into(),
                diagnostic,
            }),
    );
    schema::decode(schema::v1::info(), &lowered.value).map_err(|issues| ConfigError::Invalid {
        path: source.into(),
        issues,
        message: None,
    })
}

/// `ConfigParse.jsonc` with the baseline `ConfigJsonError`.
pub fn parse_jsonc(text: &str, path: &str) -> Result<Value, ConfigError> {
    jsonc::parse_strict(text).map_err(|message| ConfigError::Json {
        path: path.into(),
        message,
    })
}

/// `loadConfig(text, options, env)`.
pub fn load_config(
    text: &str,
    source: &Source,
    env: &Environment,
    overlay: Option<&BTreeMap<String, String>>,
    diags: &mut Diagnostics,
) -> Result<Value, ConfigError> {
    let name = source.name();
    let expanded = variable::substitute(text, source, env, overlay, Missing::Error)?;
    let parsed = parse_jsonc(&expanded, &name)?;
    let mut data = decode_config(&parsed, &name, diags)?;
    let Source::Path(path) = source else {
        return Ok(data);
    };
    if let Some(plugins) = data.get_mut("plugin").and_then(Value::as_array_mut) {
        for spec in plugins.iter_mut() {
            *spec = plugin::resolve_spec(spec, path);
        }
    }
    if data.get("$schema").is_none() {
        if let Some(map) = data.as_object_mut() {
            map.insert("$schema".into(), Value::String(SCHEMA_URL.into()));
        }
        let updated = seed_schema(text);
        let _ = std::fs::write(path, updated);
    }
    Ok(data)
}

/// `text.replace(/^\s*\{/, '{\n  "$schema": "...",')`.
fn seed_schema(text: &str) -> String {
    let trimmed = text.trim_start();
    match trimmed.strip_prefix('{') {
        Some(rest) => format!("{{\n  \"$schema\": \"{SCHEMA_URL}\",{rest}"),
        None => text.to_string(),
    }
}

/// `loadFile(filepath, env)`: missing or empty files load as `{}`.
pub fn load_file(
    path: &Path,
    env: &Environment,
    overlay: Option<&BTreeMap<String, String>>,
    diags: &mut Diagnostics,
) -> Result<Value, ConfigError> {
    match fs::read_safe(path)? {
        Some(text) if !text.is_empty() => load_config(
            &text,
            &Source::Path(path.to_path_buf()),
            env,
            overlay,
            diags,
        ),
        _ => Ok(Value::Object(Map::new())),
    }
}

/// `globalConfigFile()`.
pub fn global_config_file(env: &Environment) -> PathBuf {
    let candidates: Vec<PathBuf> = ["opencode.jsonc", "opencode.json", "config.json"]
        .iter()
        .map(|f| env.global_config.join(f))
        .collect();
    candidates
        .iter()
        .find(|file| file.exists())
        .cloned()
        .unwrap_or_else(|| candidates[0].clone())
}

/// `loadGlobal(env)`.
pub fn load_global(
    env: &Environment,
    overlay: Option<&BTreeMap<String, String>>,
    diags: &mut Diagnostics,
) -> Result<Value, ConfigError> {
    if env.opencode_config().is_none()
        && env.opencode_config_dir().is_none()
        && env.opencode_config_content().is_none()
    {
        let file = global_config_file(env);
        if !file.exists() {
            let seed = stringify_pretty(&serde_json::json!({ "$schema": SCHEMA_URL }));
            if let Some(parent) = file.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&file, seed);
        }
    }
    let mut result = Value::Object(Map::new());
    for name in ["config.json", "opencode.json", "opencode.jsonc"] {
        result = merge_deep(
            &result,
            &load_file(&env.global_config.join(name), env, overlay, diags)?,
        );
    }
    let legacy = env.global_config.join("config");
    if legacy.exists() {
        // Errors are swallowed like the baseline `.catch(() => {})`.
        if let Some(migrated) = migrate_toml(&legacy, &env.global_config, &result) {
            result = migrated;
        }
    }
    Ok(result)
}

fn migrate_toml(legacy: &Path, dir: &Path, current: &Value) -> Option<Value> {
    let text = std::fs::read_to_string(legacy).ok()?;
    let parsed: toml::Value = toml::from_str(&text).ok()?;
    let Value::Object(mut rest) = serde_json::to_value(parsed).ok()? else {
        return None;
    };
    let provider = rest.remove("provider");
    let model = rest.remove("model");
    let mut result = current.clone();
    let truthy = |v: &Option<Value>| {
        v.as_ref()
            .is_some_and(|v| !matches!(v, Value::Null | Value::Bool(false)) && v != "")
    };
    if truthy(&provider) && truthy(&model) {
        let text = |v: Option<Value>| match v {
            Some(Value::String(s)) => s,
            Some(other) => other.to_string(),
            None => String::new(),
        };
        result["model"] = Value::String(format!("{}/{}", text(provider), text(model)));
    }
    result["$schema"] = Value::String(SCHEMA_URL.into());
    let result = merge_deep(&result, &Value::Object(rest));
    std::fs::write(dir.join("config.json"), stringify_pretty(&result)).ok()?;
    std::fs::remove_file(legacy).ok()?;
    Some(result)
}
