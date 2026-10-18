//! Purpose: legacy config writes (`packages/opencode/src/config/config.ts:638-680`): project
//! `update` (deep merge into `<dir>/config.json`) and `updateGlobal` (JSON merge or
//! comment-preserving JSONC patch of the global file, empty-shell removal, validation before
//! writing).
//! Scope: file IO.
//! Uses:
//! rust/crates/
//! |-- config/src/env.rs
//! |-- config/src/error.rs
//! |-- config/src/fs.rs
//! |-- config/src/jsonc/mod.rs
//! |-- config/src/legacy/compat.rs
//! |-- config/src/legacy/parse.rs
//! `-- config/src/value.rs
//! External:
//! `-- serde_json::{Map, Value}
//! Used by:
//! rust/crates/
//! `-- config/src/legacy/mod.rs
//! Invariants: writes start from the raw on-disk text/JSON (never from the substituted, loaded
//! config), so `{env:}`/`{file:}` tokens survive and expanded secrets are never written back;
//! files that fail validation (including native V2 `permissions`) are left untouched.
//! Contract tests: config/tests/legacy_update.rs

use super::compat;
use super::parse::{self, Diagnostics};
use crate::env::Environment;
use crate::error::ConfigError;
use crate::value::{merge_deep, stringify_pretty};
use crate::{fs, jsonc, schema};
use serde_json::{Map, Value};
use std::path::Path;

/// Result of `updateGlobal`.
#[derive(Debug, Clone, PartialEq)]
pub struct GlobalUpdate {
    pub info: Value,
    pub changed: bool,
    pub diagnostics: Diagnostics,
}

/// `writable(info)`: drop derived `plugin_origins`.
fn writable(info: &Value) -> Value {
    let mut next = info.clone();
    if let Some(map) = next.as_object_mut() {
        map.remove("plugin_origins");
    }
    next
}

/// `writableGlobal(info)`: an empty `shell` becomes `undefined`, which removes the key from the
/// written file (reset to default in the Desktop app). Returns the patch and whether to remove.
fn writable_global(info: &Value) -> (Value, bool) {
    let mut next = writable(info);
    let clear = next.get("shell") == Some(&Value::String(String::new()));
    if clear && let Some(map) = next.as_object_mut() {
        map.remove("shell");
    }
    (next, clear)
}

/// `Config.update(config)` for the instance directory: merge into `<dir>/config.json`.
pub fn update(directory: &Path, patch: &Value, env: &Environment) -> Result<(), ConfigError> {
    let file = directory.join("config.json");
    let mut diags = Diagnostics::default();
    let existing = parse::load_file(&file, env, None, &mut diags)?;
    let original = match fs::read_safe(&file)? {
        Some(text) if !text.is_empty() => parse::parse_jsonc(&text, &file.display().to_string())?,
        _ => writable(&existing),
    };
    let base = if original.is_object() {
        original
    } else {
        writable(&existing)
    };
    let merged = merge_deep(&base, &writable(patch));
    std::fs::write(&file, stringify_pretty(&merged))
        .map_err(|error| ConfigError::io("write", &file, &error))
}

/// `Config.updateGlobal(config)`.
pub fn update_global(env: &Environment, patch: &Value) -> Result<GlobalUpdate, ConfigError> {
    let file = parse::global_config_file(env);
    let name = file.display().to_string();
    let before = fs::read_safe(&file)?.unwrap_or_else(|| "{}".into());
    let (patch, clear_shell) = writable_global(patch);
    let mut diagnostics = Diagnostics::default();
    let is_jsonc = file.extension().is_some_and(|ext| ext == "jsonc");
    let (info, updated) = if !is_jsonc {
        let existing = parse::parse_jsonc(&before, &name)?;
        let lowered = compat::lower(&parse::normalize_loaded(&existing), &name)?;
        schema::decode(schema::v1::info(), &lowered.value).map_err(|issues| {
            ConfigError::Invalid {
                path: name.clone(),
                issues,
                message: None,
            }
        })?;
        let base = if existing.is_object() {
            existing
        } else {
            Value::Object(Map::new())
        };
        let mut merged = merge_deep(&base, &patch);
        if let (true, Some(map)) = (clear_shell, merged.as_object_mut()) {
            map.remove("shell");
        }
        let info = parse::decode_config(&merged, &name, &mut diagnostics)?;
        (info, stringify_pretty(&merged))
    } else {
        let edit_error = |error: jsonc::EditError| ConfigError::Edit {
            path: name.clone(),
            message: error.to_string(),
        };
        let mut updated = patch_jsonc(&before, &patch, &mut Vec::new()).map_err(edit_error)?;
        if clear_shell {
            updated = jsonc::modify(&updated, &["shell".to_string()], None).map_err(edit_error)?;
        }
        let info = parse::decode_config(
            &parse::parse_jsonc(&updated, &name)?,
            &name,
            &mut diagnostics,
        )?;
        (info, updated)
    };
    let changed = updated != before;
    if changed {
        std::fs::write(&file, &updated).map_err(|error| ConfigError::io("write", &file, &error))?;
    }
    Ok(GlobalUpdate {
        info,
        changed,
        diagnostics,
    })
}

/// `patchJsonc(input, patch, path)`: recurse through objects, `modify` leaves. Keys whose value
/// is absent in the patch (removed `shell`) are untouched; JSON `null` sets null.
fn patch_jsonc(
    input: &str,
    patch: &Value,
    path: &mut Vec<String>,
) -> Result<String, jsonc::EditError> {
    let Some(map) = patch.as_object() else {
        return jsonc::modify(input, path, Some(patch));
    };
    let mut text = input.to_string();
    for (key, value) in map {
        path.push(key.clone());
        text = patch_jsonc(&text, value, path)?;
        path.pop();
    }
    Ok(text)
}
