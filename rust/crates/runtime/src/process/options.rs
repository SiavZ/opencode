//! Purpose: capture explicit production and isolated process inputs before service construction.
//! Scope: paths, configuration environment, build identity and background refresh policy.
//! Uses:
//! rust/crates/
//! |-- config/src/env.rs
//! |-- runtime/src/paths.rs
//! `-- storage-sqlite/src/path.rs
//! External:
//! `-- std::env
//! Used by:
//! rust/crates/
//! `-- runtime/src/process.rs
//! Invariants: isolated options never consult ambient environment; Debug never exposes credentials.
//! Contract tests: runtime/tests/process.rs, runtime/tests/model_refresh.rs

use crate::paths::GlobalPaths;
use opencode_storage_sqlite::PathEnv;
use std::time::Duration;

#[derive(Clone)]
pub struct ProcessOptions {
    pub paths: GlobalPaths,
    pub channel: String,
    pub version: String,
    pub db: PathEnv,
    pub idle_ttl: Duration,
    pub experimental_workspaces: bool,
    pub workspace_id: Option<String>,
    pub git_env: Vec<(String, String)>,
    pub environment: opencode_config::env::Environment,
    /// Completion generation must not start the periodic models refresh daemon.
    pub model_refresh: bool,
    /// Generated embedded UI manifest; values are explicit files, never request-derived paths.
    pub ui_assets: Option<std::collections::BTreeMap<String, std::path::PathBuf>>,
    pub ui_upstream: String,
}

impl std::fmt::Debug for ProcessOptions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProcessOptions")
            .field("paths", &self.paths)
            .field("channel", &self.channel)
            .field("version", &self.version)
            .field("db", &self.db)
            .field("idle_ttl", &self.idle_ttl)
            .field("experimental_workspaces", &self.experimental_workspaces)
            .field("workspace_id", &self.workspace_id)
            .field("model_refresh", &self.model_refresh)
            .field("environment", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl ProcessOptions {
    pub fn from_process(version: impl Into<String>, channel: impl Into<String>) -> Self {
        let flag = |key: &str| {
            std::env::var(key)
                .ok()
                .map(|value| matches!(value.to_lowercase().as_str(), "true" | "1"))
        };
        Self {
            paths: GlobalPaths::from_process(),
            channel: channel.into(),
            version: version.into(),
            db: PathEnv::from_process(),
            idle_ttl: opencode_locations::DEFAULT_IDLE_TTL,
            experimental_workspaces: flag("OPENCODE_EXPERIMENTAL_WORKSPACES")
                .or_else(|| flag("OPENCODE_EXPERIMENTAL"))
                .unwrap_or(false),
            workspace_id: std::env::var("OPENCODE_WORKSPACE_ID")
                .ok()
                .filter(|id| !id.is_empty()),
            git_env: Vec::new(),
            environment: opencode_config::env::Environment::from_process(),
            model_refresh: !std::env::args_os()
                .any(|argument| argument == "--get-yargs-completions"),
            ui_assets: None,
            ui_upstream: "https://app.opencode.ai".into(),
        }
    }

    /// Everything under `root`, nothing read from the process environment.
    pub fn isolated(root: &std::path::Path) -> Self {
        Self {
            paths: GlobalPaths::rooted(root),
            channel: "local".to_owned(),
            version: "local".to_owned(),
            db: PathEnv::default(),
            idle_ttl: opencode_locations::DEFAULT_IDLE_TTL,
            experimental_workspaces: false,
            workspace_id: None,
            git_env: Vec::new(),
            environment: {
                let mut environment =
                    opencode_config::env::Environment::isolated(root.join("home"));
                environment.global_config = GlobalPaths::rooted(root).config;
                environment.vars.extend([
                    ("HOME".into(), root.join("home").display().to_string()),
                    (
                        "XDG_CONFIG_HOME".into(),
                        root.join("config").display().to_string(),
                    ),
                    (
                        "GIT_CONFIG_GLOBAL".into(),
                        root.join("empty-gitconfig").display().to_string(),
                    ),
                    ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
                    ("OPENCODE_DISABLE_MODELS_FETCH".into(), "true".into()),
                ]);
                environment
            },
            model_refresh: true,
            ui_assets: None,
            ui_upstream: "http://127.0.0.1:9".into(),
        }
    }
}
