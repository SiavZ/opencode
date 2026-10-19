//! Purpose: formatter service of packages/opencode/src/format/index.ts: merge config overrides into
//! the built-in table, report status, and run every enabled formatter for an edited file.
//! Scope: one instance per Location (`InstanceState`). Probe results are cached once enabled.
//! Uses:
//! rust/crates/
//! |-- platform-api/src/process.rs
//! `-- platform-native/src/vcs/format/table.rs
//! External:
//! |-- serde::Deserialize
//! |-- serde::Serialize
//! |-- serde_json::Value
//! |-- serde_json::Value::Bool
//! |-- serde_json::Value::Object
//! |-- serde_json::Value::String
//! |-- serde_json::from_str
//! |-- serde_json::from_value
//! |-- tracing::error
//! `-- which::which_in
//! Used by:
//! rust/crates/
//! |-- platform-native/src/vcs/format/table.rs
//! |-- platform-native/src/vcs/mod.rs
//! `-- platform-native/tests/format.rs
//! Invariants: a disabled `ruff` or `uv` override disables both; `formatter: false` (or absent)
//! disables all; formatter failures are logged and never fail the edit; `$FILE` is substituted
//! in every argument.
//! Contract tests: platform-native/tests/format.rs

pub mod table;

use opencode_platform_api::process::{Command, ProcessRunner, RunOptions};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use table::{BUILT_INS, Probe};

/// One `formatter.<name>` config entry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormatterEntry {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub disabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub command: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub environment: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub extensions: Option<Vec<String>>,
}

/// `cfg.formatter`: `true`, `false`/absent, or per-formatter overrides (order preserved).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatterConfig {
    Toggle(bool),
    Entries(Vec<(String, FormatterEntry)>),
}

impl FormatterConfig {
    /// Decode the `formatter` config value (bool or object of entries). Invalid input is None.
    pub fn from_json(value: &serde_json::Value) -> Option<Self> {
        match value {
            serde_json::Value::Bool(flag) => Some(Self::Toggle(*flag)),
            serde_json::Value::Object(map) => map
                .iter()
                .map(|(name, entry)| {
                    serde_json::from_value(entry.clone())
                        .ok()
                        .map(|entry| (name.clone(), entry))
                })
                .collect::<Option<Vec<_>>>()
                .map(Self::Entries),
            _ => None,
        }
    }
}

/// `FormatterStatus` wire shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormatterStatus {
    pub name: String,
    pub extensions: Vec<String>,
    pub enabled: bool,
}

/// `Formatter.Context` plus the host lookups the probes need.
#[derive(Debug, Clone)]
pub struct FormatContext {
    pub directory: PathBuf,
    pub worktree: PathBuf,
    pub experimental_oxfmt: bool,
    /// `PATH` used by `which`; `Global.Path.bin` is appended like the baseline.
    pub path: Option<String>,
    /// `Global.Path.bin`.
    pub bin_dir: Option<PathBuf>,
    /// `Global.cache`; npm tools resolve from `<cache>/packages/<pkg>/node_modules/.bin`.
    pub cache_dir: Option<PathBuf>,
    pub runner: Arc<dyn ProcessRunner>,
}

impl FormatContext {
    /// `which(cmd)`: PATH plus the global bin dir.
    pub fn which(&self, bin: &str) -> Option<String> {
        let base = self
            .path
            .clone()
            .or_else(|| std::env::var("PATH").ok())
            .unwrap_or_default();
        let mut dirs: Vec<PathBuf> = std::env::split_paths(&base).collect();
        dirs.extend(self.bin_dir.clone());
        let joined = std::env::join_paths(dirs).ok()?;
        let cwd = std::env::current_dir().ok()?;
        which::which_in(bin, Some(joined), cwd)
            .ok()
            .map(|path| path.to_string_lossy().into_owned())
    }

    /// `Npm.which(pkg)` restricted to already-installed packages. Installing missing packages is a
    /// network side effect owned by the runtime npm service and is not performed here.
    pub async fn npm_which(&self, package: &str) -> Option<String> {
        let dir = self.cache_dir.as_ref()?.join("packages").join(package);
        let bin_dir = dir.join("node_modules").join(".bin");
        let mut files: Vec<String> = std::fs::read_dir(&bin_dir)
            .ok()?
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        files.sort();
        let pick = match files.len() {
            0 => return None,
            1 => files[0].clone(),
            _ => pick_bin(&dir, package, &files),
        };
        Some(bin_dir.join(pick).to_string_lossy().into_owned())
    }

    /// `Process.run([bin, ...args], { nothrow: true })` returning exit code and stdout.
    pub async fn probe(&self, bin: &str, args: &[&str]) -> Option<(i32, String)> {
        let command = Command::new(bin).args(args.iter().copied());
        let result = self.runner.run(&command, RunOptions::default()).await;
        Some(match result {
            Ok(out) => (out.exit_code, String::from_utf8_lossy(&out.stdout).into()),
            Err(_) => (1, String::new()),
        })
    }
}

fn pick_bin(dir: &Path, package: &str, files: &[String]) -> String {
    let manifest = dir.join("node_modules").join(package).join("package.json");
    let parsed = std::fs::read_to_string(manifest)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
    let unscoped = match package.strip_prefix('@') {
        Some(rest) => rest.split('/').nth(1).unwrap_or(rest).to_string(),
        None => package.to_string(),
    };
    match parsed.as_ref().and_then(|json| json.get("bin")) {
        Some(serde_json::Value::String(_)) => unscoped,
        Some(serde_json::Value::Object(map)) if !map.is_empty() => {
            if map.len() == 1 || !map.contains_key(&unscoped) {
                map.keys().next().cloned().unwrap_or(unscoped)
            } else {
                unscoped
            }
        }
        _ => files[0].clone(),
    }
}

#[derive(Debug, Clone)]
enum Enable {
    Probe(Probe),
    Command(Option<Vec<String>>),
}

#[derive(Debug, Clone)]
struct Info {
    name: String,
    extensions: Vec<String>,
    environment: BTreeMap<String, String>,
    enabled: Enable,
}

/// Location-scoped formatter service (`Format.Service`).
#[derive(Debug)]
pub struct Formatters {
    ctx: FormatContext,
    formatters: Vec<Info>,
    commands: Mutex<BTreeMap<String, Vec<String>>>,
}

impl Formatters {
    /// `Format.state`: build the formatter set from config.
    pub fn new(ctx: FormatContext, config: Option<&FormatterConfig>) -> Self {
        let mut formatters: Vec<Info> = Vec::new();
        let entries = match config {
            None | Some(FormatterConfig::Toggle(false)) => None,
            Some(FormatterConfig::Toggle(true)) => Some(&[][..]),
            Some(FormatterConfig::Entries(entries)) => Some(entries.as_slice()),
        };
        if let Some(entries) = entries {
            formatters = BUILT_INS.iter().map(built_in_info).collect();
            apply_overrides(&mut formatters, entries);
        }
        Self {
            ctx,
            formatters,
            commands: Mutex::default(),
        }
    }

    async fn command(&self, item: &Info) -> Option<Vec<String>> {
        if let Some(hit) = self.cached(&item.name) {
            return Some(hit);
        }
        let resolved = match &item.enabled {
            Enable::Probe(probe) => probe.enabled(&self.ctx).await,
            Enable::Command(command) => command.clone(),
        }?;
        self.commands
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .insert(item.name.clone(), resolved.clone());
        Some(resolved)
    }

    fn cached(&self, name: &str) -> Option<Vec<String>> {
        let commands = self.commands.lock().unwrap_or_else(|p| p.into_inner());
        commands.get(name).cloned()
    }

    /// `Format.status`.
    pub async fn status(&self) -> Vec<FormatterStatus> {
        let mut out = Vec::with_capacity(self.formatters.len());
        for item in &self.formatters {
            out.push(FormatterStatus {
                name: item.name.clone(),
                extensions: item.extensions.clone(),
                enabled: self.command(item).await.is_some(),
            });
        }
        out
    }

    /// `Format.file`: run every enabled formatter for the file's extension; false when none match.
    pub async fn file(&self, filepath: &Path) -> bool {
        let ext = extname(filepath);
        let mut matched = Vec::new();
        for item in self
            .formatters
            .iter()
            .filter(|i| i.extensions.contains(&ext))
        {
            if let Some(command) = self.command(item).await {
                matched.push((item, command));
            }
        }
        if matched.is_empty() {
            return false;
        }
        let file = filepath.to_string_lossy();
        for (item, template) in matched {
            let replaced: Vec<String> =
                template.iter().map(|a| a.replace("$FILE", &file)).collect();
            let Some((program, args)) = replaced.split_first() else {
                continue;
            };
            let mut command = Command::new(program.clone())
                .args(args.iter().cloned())
                .cwd(&self.ctx.directory);
            command.env.extend(item.environment.clone());
            match self.ctx.runner.run(&command, RunOptions::default()).await {
                Ok(out) if out.exit_code != 0 => {
                    tracing::error!(command = ?template, "failed");
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::error!(command = ?template, %error, file = %file, "failed to format file");
                }
            }
        }
        true
    }
}

fn built_in_info(item: &table::BuiltIn) -> Info {
    Info {
        name: item.name.to_string(),
        extensions: item.extensions.iter().map(|e| e.to_string()).collect(),
        environment: item
            .environment
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        enabled: Enable::Probe(item.probe),
    }
}

fn apply_overrides(formatters: &mut Vec<Info>, entries: &[(String, FormatterEntry)]) {
    let ruff_off = entries
        .iter()
        .any(|(name, e)| (name == "ruff" || name == "uv") && e.disabled == Some(true));
    for (name, item) in entries {
        if (name == "ruff" || name == "uv") && ruff_off {
            formatters.retain(|f| f.name != "ruff" && f.name != "uv");
            continue;
        }
        if item.disabled == Some(true) {
            formatters.retain(|f| &f.name != name);
            continue;
        }
        let builtin = table::built_in(name);
        let mut environment: BTreeMap<String, String> = builtin
            .map(|b| {
                b.environment
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        environment.extend(item.environment.clone().unwrap_or_default());
        let extensions = item.extensions.clone().unwrap_or_else(|| {
            builtin
                .map(|b| b.extensions.iter().map(|e| e.to_string()).collect())
                .unwrap_or_default()
        });
        let enabled = match (builtin, &item.command) {
            (Some(b), None) => Enable::Probe(b.probe),
            (_, command) => Enable::Command(command.clone()),
        };
        let info = Info {
            name: name.clone(),
            extensions,
            environment,
            enabled,
        };
        match formatters.iter_mut().find(|f| &f.name == name) {
            Some(slot) => *slot = info,
            None => formatters.push(info),
        }
    }
}

/// Node `path.extname`: the last `.suffix` of the basename, empty for dotfiles.
pub fn extname(path: &Path) -> String {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    match name.rfind('.') {
        Some(index) if index > 0 => name[index..].to_string(),
        _ => String::new(),
    }
}
