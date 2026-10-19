//! Purpose: legacy plugin spec handling (`packages/opencode/src/config/plugin.ts` and the spec
//! helpers in `src/plugin/shared.ts`): path-like spec resolution relative to the declaring file,
//! auto-discovered `.opencode/plugin(s)` files, and origin-preserving dedupe.
//! Scope: pure transforms + stat calls.
//! Uses:
//! rust/crates/
//! |-- config/src/fs.rs
//! `-- platform-api/src/path.rs
//! External:
//! |-- fs::exists
//! |-- fs::glob
//! |-- serde_json::Value
//! |-- url::Url::from_file_path
//! |-- url::Url::parse
//! `-- usize::from
//! Used by:
//! rust/crates/
//! |-- config/src/legacy/load.rs
//! |-- config/src/legacy/mod.rs
//! |-- config/src/legacy/parse.rs
//! `-- config/src/resources.rs
//! Invariants: dedupe keeps the last (highest priority) origin per identity (package name for npm
//! specs, exact file URL for local specs) and preserves the relative order of survivors.
//! Contract tests: config/tests/legacy_load.rs

use crate::fs;
use opencode_platform_api::path::resolve;
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Global,
    Local,
}

/// `ConfigPlugin.Origin`: a winning plugin spec with the source that declared it.
#[derive(Debug, Clone, PartialEq)]
pub struct Origin {
    pub spec: Value,
    pub source: String,
    pub scope: Scope,
}

const INDEX_FILES: [&str; 5] = [
    "index.ts",
    "index.tsx",
    "index.js",
    "index.mjs",
    "index.cjs",
];

/// `pluginSpecifier`: the string part of `string | [string, options]`.
pub fn specifier(plugin: &Value) -> String {
    match plugin {
        Value::Array(items) => items
            .first()
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        other => other.as_str().unwrap_or_default().to_string(),
    }
}

fn is_windows_absolute(spec: &str) -> bool {
    let bytes = spec.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
}

/// `isPathPluginSpec`.
pub fn is_path_spec(spec: &str) -> bool {
    spec.starts_with("file://")
        || spec.starts_with('.')
        || Path::new(spec).is_absolute()
        || is_windows_absolute(spec)
}

pub fn file_url(path: &Path) -> String {
    url::Url::from_file_path(path)
        .map(|url| url.to_string())
        .unwrap_or_else(|_| format!("file://{}", path.display()))
}

fn url_path(spec: &str) -> PathBuf {
    url::Url::parse(spec)
        .ok()
        .and_then(|url| url.to_file_path().ok())
        .unwrap_or_else(|| PathBuf::from(spec))
}

/// `resolvePathPluginTarget`: directories resolve to themselves (with package.json) or an index.
fn resolve_target(spec: &str) -> Result<String, String> {
    let file = if spec.starts_with("file://") {
        url_path(spec)
    } else {
        PathBuf::from(spec)
    };
    if !file.is_dir() {
        return Ok(if spec.starts_with("file://") {
            spec.to_string()
        } else {
            file_url(&file)
        });
    }
    if fs::exists(&file.join("package.json")) {
        return Ok(file_url(&file));
    }
    INDEX_FILES
        .iter()
        .map(|name| file.join(name))
        .find(|candidate| fs::exists(candidate))
        .map(|index| file_url(&index))
        .ok_or_else(|| {
            format!(
                "Plugin directory {} is missing package.json or index file",
                file.display()
            )
        })
}

/// `resolvePluginSpec(plugin, configFilepath)`.
pub fn resolve_spec(plugin: &Value, config_file: &Path) -> Value {
    let spec = specifier(plugin);
    if !is_path_spec(&spec) {
        return plugin.clone();
    }
    let base = config_file.parent().unwrap_or(Path::new("/"));
    let file = if spec.starts_with("file://") {
        spec.clone()
    } else if Path::new(&spec).is_absolute() || is_windows_absolute(&spec) {
        file_url(Path::new(&spec))
    } else {
        file_url(&resolve(base, Path::new(&spec)))
    };
    let resolved = resolve_target(&file).unwrap_or(file);
    match plugin {
        Value::Array(items) => Value::Array(vec![
            Value::String(resolved),
            items.get(1).cloned().unwrap_or(Value::Null),
        ]),
        _ => Value::String(resolved),
    }
}

/// `ConfigPlugin.load(dir)`: `{plugin,plugins}/*.{ts,js}` as file URLs.
pub fn discover(dir: &Path) -> Vec<Value> {
    fs::glob(dir, &["plugin", "plugins"], false, &["ts", "js"])
        .iter()
        .map(|file| Value::String(file_url(file)))
        .collect()
}

/// `parsePluginSpecifier(spec).pkg` (npm-package-arg name extraction).
pub fn package_name(spec: &str) -> String {
    if let Some(sub) = spec.strip_prefix("npm:") {
        return package_name(sub);
    }
    if spec.contains("://")
        || spec.starts_with("git+")
        || spec.starts_with("github:")
        || is_path_spec(spec)
    {
        return spec.to_string();
    }
    let search_from = usize::from(spec.starts_with('@'));
    match spec[search_from..].find('@') {
        Some(at) => {
            let name = &spec[..search_from + at];
            let rest = &spec[search_from + at + 1..];
            match rest.strip_prefix("npm:") {
                Some(alias) if name.is_empty() => package_name(alias),
                _ => name.to_string(),
            }
        }
        None => spec.to_string(),
    }
}

/// `deduplicatePluginOrigins`.
pub fn dedupe(plugins: Vec<Origin>) -> Vec<Origin> {
    let mut seen = std::collections::HashSet::new();
    let mut list: Vec<Origin> = plugins
        .into_iter()
        .rev()
        .filter(|plugin| {
            let spec = specifier(&plugin.spec);
            let name = if spec.starts_with("file://") {
                spec
            } else {
                package_name(&spec)
            };
            seen.insert(name)
        })
        .collect();
    list.reverse();
    list
}
