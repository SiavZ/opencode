//! Purpose: legacy merged configuration (`packages/opencode/src/config/config.ts:328-610`
//! `loadInstanceState`): well-known remote config, global, `OPENCODE_CONFIG`, project files,
//! `.opencode`/home/`OPENCODE_CONFIG_DIR` directories with markdown agents/commands/modes and
//! plugins, `OPENCODE_CONFIG_CONTENT`, active org config, managed dir, MDM preferences, then
//! post-processing (mode->agent, `OPENCODE_PERMISSION`, tools->permission, username, autoshare,
//! compaction flags). Records per-value provenance and plugin origins.
//! Scope: one instance (directory + worktree) load.
//! Uses:
//! rust/crates/
//! |-- config/src/env.rs
//! |-- config/src/error.rs
//! |-- config/src/legacy/markdown.rs
//! |-- config/src/legacy/parse.rs
//! |-- config/src/legacy/plugin.rs
//! |-- config/src/legacy/remote.rs
//! |-- config/src/legacy/steps.rs
//! |-- config/src/value.rs
//! |-- config/src/variable.rs
//! `-- platform-api/src/path.rs
//! External:
//! |-- markdown::load_agents
//! |-- markdown::load_commands
//! |-- markdown::load_modes
//! |-- parse::load_config
//! |-- parse::load_file
//! |-- parse::load_global
//! |-- plugin::dedupe
//! |-- plugin::discover
//! |-- serde_json::Map
//! |-- serde_json::Value
//! |-- serde_json::json
//! |-- steps::directories
//! |-- steps::dirname
//! |-- steps::ensure_gitignore
//! |-- steps::load_wellknown
//! |-- steps::post_process
//! `-- steps::project_files
//! Used by:
//! rust/crates/
//! |-- config/src/legacy/mod.rs
//! `-- config/src/legacy/steps.rs
//! Invariants: precedence and merge order match the baseline line by line (see
//! contracts/phase-02/leaves/config.md); `instructions` concatenate with dedupe; plugin lists
//! dedupe by identity keeping the highest-priority origin; the process env is never mutated.
//! Contract tests: config/tests/legacy_load.rs

use super::parse::{self, Diagnostics};
use super::plugin::{self, Origin, Scope};
use super::remote::{AccountSource, ManagedPreferences, RemoteFetcher};
use super::{markdown, steps};
use crate::env::Environment;
use crate::error::ConfigError;
use crate::value::{Provenance, merge_concat, merge_deep};
use crate::variable::Source;
use opencode_platform_api::path::contains;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// `InstanceContext` subset used by config.
#[derive(Debug, Clone)]
pub struct Instance {
    pub directory: PathBuf,
    pub worktree: PathBuf,
}

/// `ConsoleState`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConsoleState {
    pub console_managed_providers: Vec<String>,
    pub active_org_name: Option<String>,
    pub switchable_org_count: u32,
}

/// Loader dependencies (traits default to typed unsupported/empty implementations).
pub struct Sources<'a> {
    pub accounts: &'a dyn AccountSource,
    pub remote: &'a dyn RemoteFetcher,
    pub managed: &'a dyn ManagedPreferences,
}

/// The loaded legacy state (`State` minus dependency fibers).
#[derive(Debug, Clone, PartialEq)]
pub struct LegacyState {
    pub config: Value,
    pub plugin_origins: Vec<Origin>,
    pub directories: Vec<PathBuf>,
    pub console_state: ConsoleState,
    pub provenance: Provenance,
    pub diagnostics: Diagnostics,
    /// `OPENCODE_CONSOLE_TOKEN` the baseline writes into the process env; callers decide.
    pub console_token: Option<String>,
}

struct Loader<'a> {
    instance: &'a Instance,
    result: Value,
    origins: Vec<Origin>,
    provenance: Provenance,
    diags: Diagnostics,
}

impl Loader<'_> {
    fn scope_for(&self, source: &str) -> Scope {
        if source.starts_with("http://")
            || source.starts_with("https://")
            || source == "OPENCODE_CONFIG_CONTENT"
        {
            return if source == "OPENCODE_CONFIG_CONTENT" {
                Scope::Local
            } else {
                Scope::Global
            };
        }
        let path = Path::new(source);
        let inside = contains(&self.instance.directory, path)
            || (self.instance.worktree != Path::new("/")
                && contains(&self.instance.worktree, path));
        if inside { Scope::Local } else { Scope::Global }
    }

    fn merge_origins(&mut self, source: &str, list: Option<&Vec<Value>>, kind: Option<Scope>) {
        let Some(list) = list.filter(|list| !list.is_empty()) else {
            return;
        };
        let scope = kind.unwrap_or_else(|| self.scope_for(source));
        let mut all = std::mem::take(&mut self.origins);
        all.extend(list.iter().map(|spec| Origin {
            spec: spec.clone(),
            source: source.into(),
            scope,
        }));
        self.origins = plugin::dedupe(all);
        self.result["plugin"] = Value::Array(self.origins.iter().map(|o| o.spec.clone()).collect());
        self.provenance.set(&["plugin".into()], source);
    }

    fn merge(&mut self, source: &str, next: &Value, kind: Option<Scope>) {
        self.provenance.record(&[], &self.result, next, source);
        self.result = merge_concat(&self.result, next);
        let plugins = next.get("plugin").and_then(Value::as_array).cloned();
        self.merge_origins(source, plugins.as_ref(), kind);
    }

    fn merge_at(&mut self, key: &str, next: Map<String, Value>, source: &str) {
        let before = self.result.get(key).cloned().unwrap_or_else(|| json!({}));
        let next = Value::Object(next);
        self.provenance
            .record(&[key.into()], &before, &next, source);
        self.result[key] = merge_deep(&before, &next);
    }

    fn ensure_defaults(&mut self) {
        for (key, empty) in [
            ("agent", json!({})),
            ("mode", json!({})),
            ("plugin", json!([])),
        ] {
            if self.result.get(key).is_none_or(Value::is_null) {
                self.result[key] = empty;
            }
        }
    }
}

/// Run the full legacy discovery for one instance.
pub fn load(
    env: &Environment,
    instance: &Instance,
    sources: &Sources<'_>,
) -> Result<LegacyState, ConfigError> {
    let mut loader = Loader {
        instance,
        result: json!({}),
        origins: Vec::new(),
        provenance: Provenance::default(),
        diags: Diagnostics::default(),
    };
    let mut auth_env = BTreeMap::new();
    for auth in sources.accounts.wellknown() {
        let url = auth.url.trim_end_matches('/').to_string();
        auth_env.insert(auth.key.clone(), auth.token.clone());
        let source = format!("{url}/.well-known/opencode");
        let next = steps::load_wellknown(
            env,
            &mut loader.diags,
            sources.remote,
            &url,
            &source,
            &auth_env,
        )?;
        loader.merge(&source, &next, Some(Scope::Global));
    }
    let global = if auth_env.is_empty() {
        parse::load_global(env, None, &mut loader.diags).unwrap_or_else(|_| json!({}))
    } else {
        parse::load_global(env, Some(&auth_env), &mut loader.diags)?
    };
    loader.merge(
        &env.global_config.display().to_string(),
        &global,
        Some(Scope::Global),
    );
    if let Some(custom) = env.opencode_config() {
        let next = parse::load_file(&custom, env, Some(&auth_env), &mut loader.diags)?;
        loader.merge(&custom.display().to_string(), &next, None);
    }
    if !env.disable_project_config() {
        for file in steps::project_files(instance) {
            let next = parse::load_file(&file, env, Some(&auth_env), &mut loader.diags)?;
            loader.merge(&file.display().to_string(), &next, Some(Scope::Local));
        }
    }
    loader.ensure_defaults();
    let directories = steps::directories(env, instance);
    let config_dir = env.opencode_config_dir();
    for dir in &directories {
        if dir.file_name().is_some_and(|name| name == ".opencode")
            || config_dir.as_ref() == Some(dir)
        {
            for name in ["opencode.json", "opencode.jsonc"] {
                let source = dir.join(name);
                let next = parse::load_file(&source, env, Some(&auth_env), &mut loader.diags)?;
                loader.merge(&source.display().to_string(), &next, None);
                loader.ensure_defaults();
            }
        }
        steps::ensure_gitignore(dir)?;
        let origin = dir.display().to_string();
        loader.merge_at("command", markdown::load_commands(dir)?, &origin);
        loader.merge_at("agent", markdown::load_agents(dir)?, &origin);
        loader.merge_at("agent", markdown::load_modes(dir), &origin);
        let discovered = plugin::discover(dir);
        loader.merge_origins(&origin, Some(&discovered), None);
    }
    if let Some(content) = env.opencode_config_content() {
        let source = Source::Virtual {
            source: "OPENCODE_CONFIG_CONTENT".into(),
            dir: instance.directory.clone(),
        };
        let next = parse::load_config(content, &source, env, None, &mut loader.diags)?;
        loader.merge("OPENCODE_CONFIG_CONTENT", &next, Some(Scope::Local));
    }
    let mut console = ConsoleState::default();
    let mut process_overlay = BTreeMap::new();
    if let Ok(Some(org)) = sources.accounts.active_org() {
        if let Some(token) = &org.token {
            process_overlay.insert("OPENCODE_CONSOLE_TOKEN".to_string(), token.clone());
        }
        if let Some(config) = &org.config {
            let source = format!("{}/api/config", org.url);
            let virtual_source = Source::Virtual {
                source: source.clone(),
                dir: PathBuf::from(steps::dirname(&source)),
            };
            // Org config failures are logged and ignored in the baseline.
            if let Ok(next) = parse::load_config(
                &config.to_string(),
                &virtual_source,
                env,
                Some(&process_overlay),
                &mut loader.diags,
            ) {
                console.console_managed_providers.extend(
                    next.get("provider")
                        .and_then(Value::as_object)
                        .into_iter()
                        .flat_map(|p| p.keys().cloned()),
                );
                loader.merge(&source, &next, Some(Scope::Global));
            }
        }
    }
    let managed_dir = env.managed_config_dir();
    if managed_dir.exists() {
        for name in ["opencode.json", "opencode.jsonc"] {
            let source = managed_dir.join(name);
            let next = parse::load_file(&source, env, Some(&process_overlay), &mut loader.diags)?;
            loader.merge(&source.display().to_string(), &next, Some(Scope::Global));
        }
    }
    if let Some((source, text)) = sources.managed.read(env) {
        let virtual_source = Source::Virtual {
            source: source.clone(),
            dir: PathBuf::from(steps::dirname(&source)),
        };
        let next = parse::load_config(
            &text,
            &virtual_source,
            env,
            Some(&process_overlay),
            &mut loader.diags,
        )?;
        loader
            .provenance
            .record(&[], &loader.result, &next, &source);
        loader.result = merge_concat(&loader.result, &next);
    }
    steps::post_process(env, &mut loader.result, &mut loader.provenance);
    Ok(LegacyState {
        config: loader.result,
        plugin_origins: loader.origins,
        directories,
        console_state: console,
        provenance: loader.provenance,
        diagnostics: loader.diags,
        console_token: process_overlay.remove("OPENCODE_CONSOLE_TOKEN"),
    })
}
