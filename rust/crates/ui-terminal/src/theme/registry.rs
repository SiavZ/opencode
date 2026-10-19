//! Purpose: theme catalog with the baseline precedence (bundled < plugin < custom files < system),
//! add/upsert rules, custom theme directory discovery and active theme selection, porting
//! packages/tui/src/theme/index.ts (registry functions) and packages/tui/src/context/theme.tsx
//! (`discoverThemes`, directory list, active/saved/opencode fallback).
//! Scope: one registry per TUI instance (the baseline uses module globals, here it is a value).
//! Uses:
//! rust/crates/
//! |-- ui-terminal/src/theme/mod.rs
//! |-- ui-terminal/src/theme/resolve.rs
//! `-- ui-terminal/src/theme/system.rs
//! External:
//! |-- serde_json::Error
//! |-- serde_json::Map
//! |-- serde_json::Value
//! `-- serde_json::from_str
//! Used by:
//! rust/crates/
//! `-- ui-terminal/src/theme/mod.rs
//! Invariants: first plugin registration of a name wins, custom files replace by directory order,
//! generated `system` overrides custom names only while set, selection falls back to `opencode`.
//! Contract tests: ui-terminal/tests/theme.rs

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

/// Ordered theme catalog (serde_json is built with `preserve_order`).
pub type ThemeMap = Map<String, Value>;

use super::BUNDLED_THEMES;
use super::resolve::{Mode, ResolvedTheme, ThemeError, resolve_theme};
use super::system::{SystemThemeError, TerminalColors, generate_system};

/// Ordered name -> raw theme JSON catalog.
#[derive(Debug, Clone, Default)]
pub struct ThemeRegistry {
    defaults: ThemeMap,
    plugin: ThemeMap,
    custom: ThemeMap,
    system: Option<Value>,
}

/// Baseline `isTheme`: an object whose `theme` field is a non-array object.
pub fn is_theme(value: &Value) -> bool {
    value.get("theme").is_some_and(Value::is_object)
}

impl ThemeRegistry {
    /// Registry seeded with the 33 bundled themes in `DEFAULT_THEMES` order.
    pub fn with_defaults() -> Self {
        let defaults = BUNDLED_THEMES
            .iter()
            .map(|(name, source)| {
                // Bundled assets are compiled in and covered by tests, a parse failure is a build defect.
                let value = serde_json::from_str(source).expect("bundled theme JSON must be valid");
                ((*name).to_string(), value)
            })
            .collect();
        Self {
            defaults,
            ..Self::default()
        }
    }

    /// Baseline `allThemes` / `listThemes`: defaults, then plugin, then custom (later wins while
    /// keeping first insertion position), then `system` last when set.
    pub fn all(&self) -> ThemeMap {
        let mut themes = self.defaults.clone();
        for (name, theme) in self.plugin.iter().chain(self.custom.iter()) {
            themes.insert(name.clone(), theme.clone());
        }
        if let Some(system) = &self.system {
            themes.insert("system".into(), system.clone());
        }
        themes
    }

    /// Theme JSON by name, honoring precedence.
    pub fn get(&self, name: &str) -> Option<Value> {
        if name == "system" && self.system.is_some() {
            return self.system.clone();
        }
        self.custom
            .get(name)
            .or_else(|| self.plugin.get(name))
            .or_else(|| self.defaults.get(name))
            .cloned()
    }

    /// Baseline `hasTheme`.
    pub fn has(&self, name: &str) -> bool {
        !name.is_empty() && self.get(name).is_some()
    }

    /// Baseline `addTheme`: rejects empty names, non-themes and existing names.
    pub fn add(&mut self, name: &str, theme: Value) -> bool {
        if name.is_empty() || !is_theme(&theme) || self.has(name) {
            return false;
        }
        self.plugin.insert(name.to_string(), theme);
        true
    }

    /// Baseline `upsertTheme`: replaces a custom theme in place, otherwise writes the plugin layer.
    pub fn upsert(&mut self, name: &str, theme: Value) -> bool {
        if name.is_empty() || !is_theme(&theme) {
            return false;
        }
        if let Some(slot) = self.custom.get_mut(name) {
            *slot = theme;
            return true;
        }
        self.plugin.insert(name.to_string(), theme);
        true
    }

    /// Baseline `setCustomThemes` after the context filters entries with `isTheme`.
    pub fn set_custom(&mut self, themes: ThemeMap) {
        self.custom = themes
            .into_iter()
            .filter(|(_, theme)| is_theme(theme))
            .collect();
    }

    /// Baseline `setSystemTheme`.
    pub fn set_system(&mut self, theme: Option<Value>) {
        self.system = theme;
    }

    /// Apply a queried palette like the context's `resolveSystemTheme`. A missing
    /// slot zero or failed refresh preserves an already resolved system theme.
    /// Before the first successful query, selection can fall back to opencode.
    /// The caller resolves mode locks and terminal mode before invoking this.
    pub fn refresh_system(
        &mut self,
        colors: Option<&TerminalColors>,
        mode: Mode,
    ) -> Result<bool, SystemThemeError> {
        let Some(colors) = colors.filter(|colors| {
            colors
                .palette
                .first()
                .and_then(Option::as_ref)
                .is_some_and(|hex| !hex.is_empty())
        }) else {
            return Ok(false);
        };
        self.system = Some(generate_system(colors, mode)?);
        Ok(true)
    }

    /// Resolve the active theme like the context memo: active name, else saved name, else
    /// `opencode`.
    pub fn resolve_selected(
        &self,
        active: &str,
        saved: Option<&str>,
        mode: Mode,
    ) -> Result<ResolvedTheme, ThemeError> {
        let theme = self
            .get(active)
            .or_else(|| saved.and_then(|name| self.get(name)))
            .or_else(|| self.get("opencode"))
            .ok_or_else(|| ThemeError::NotFound("opencode".into()))?;
        resolve_theme(&theme, mode)
    }
}

/// Baseline theme directories: the global config dir, then `.opencode` in the working directory
/// and every ancestor up to the root (nearest first, root last).
pub fn theme_directories(global_config: &Path, cwd: &Path) -> Vec<PathBuf> {
    std::iter::once(global_config.to_path_buf())
        .chain(cwd.ancestors().map(|dir| dir.join(".opencode")))
        .collect()
}

/// Port of `discoverThemes`: for each directory in order, every `themes/*.json` file (including
/// dot files) keyed by file stem, later directories override earlier ones. Like the baseline, any
/// unreadable or invalid JSON file fails the whole scan (the caller then keeps the previous custom
/// set and falls back to `opencode`).
pub fn discover_themes(directories: &[PathBuf]) -> Result<ThemeMap, DiscoverError> {
    let mut result = ThemeMap::new();
    for directory in directories {
        let Ok(entries) = std::fs::read_dir(directory.join("themes")) else {
            continue;
        };
        let mut files: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name().is_some_and(|name| {
                    let name = name.to_string_lossy();
                    // The baseline glob uses case-insensitive magic on Darwin/Windows.
                    if cfg!(any(target_os = "macos", windows)) {
                        return name.to_ascii_lowercase().ends_with(".json");
                    }
                    name.ends_with(".json")
                }) && path.is_file()
            })
            .collect();
        files.sort();
        for file in files {
            let text = std::fs::read_to_string(&file).map_err(|source| DiscoverError::Read {
                path: file.clone(),
                source,
            })?;
            let value =
                serde_json::from_str::<Value>(&text).map_err(|source| DiscoverError::Json {
                    path: file.clone(),
                    source,
                })?;
            let name = if file.extension().is_some_and(|ext| ext == "json") {
                file.file_stem()
            } else {
                // path.basename(file, ".json") does not remove uppercase .JSON.
                file.file_name()
            }
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
            result.insert(name, value);
        }
    }
    Ok(result)
}

/// A custom theme file could not be read or parsed.
#[derive(Debug, thiserror::Error)]
pub enum DiscoverError {
    #[error("failed to read theme {path:?}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse theme {path:?}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}
