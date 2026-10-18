//! Purpose: process launch seam for LSP servers: injectable launch environment (PATH, install
//! bin dir, npm cache, home, download-disable / experimental-ty flags), `which` with the bin dir
//! appended, node-style module resolution, `Npm.which` cache lookup, and stdio-piped spawning.
//! Ports packages/opencode/src/lsp/launch.ts, core/src/util/which.ts, core/src/npm.ts:200-246
//! (lookup half), core/src/util/module.ts.
//! Scope: native process + filesystem. Never writes outside `bin_dir` / `npm_cache`.
//! Uses:
//! rust/crates/
//! |-- integration-adapters/src/lsp/client.rs
//! `-- integration-api/src/error.rs
//! External:
//! |-- tokio::process::Command
//! `-- which::which_in
//! Used by:
//! rust/crates/
//! |-- integration-adapters/src/lsp/resolve.rs
//! |-- integration-adapters/src/lsp/service.rs
//! `-- integration-adapters/tests/lsp_inventory.rs
//! Invariants: spawned children have piped stdio, cwd = root, kill_on_drop, and inherit the
//! process env overlaid with `LaunchEnv::env` and the spec env. Package installs through npm are a
//! typed NotYetPorted, never a silent success.
//! Contract tests: integration-adapters/tests/lsp_inventory.rs

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use opencode_integration_api::IntegrationError;
use serde_json::{Map, Value};
use tokio::process::Command;

use super::client::Handle;

/// Everything server resolution may read from the host. Tests point every path at a temp dir.
#[derive(Debug, Clone)]
pub struct LaunchEnv {
    /// PATH used for lookups and given to children. None uses the process PATH.
    pub path: Option<OsString>,
    /// `Global.Path.bin`: install target for go/gem/dotnet/download installs.
    pub bin_dir: PathBuf,
    /// `<global.cache>/packages` for `Npm.which`.
    pub npm_cache: PathBuf,
    pub home: PathBuf,
    /// Extra env for lookups and children (VIRTUAL_ENV, DOTNET_CLI_HOME, VSCODE_EXTENSIONS, ...).
    pub env: BTreeMap<String, String>,
    /// `OPENCODE_DISABLE_LSP_DOWNLOAD`.
    pub disable_download: bool,
    /// `OPENCODE_EXPERIMENTAL_LSP_TY`.
    pub experimental_ty: bool,
    /// Serializes shared installs (baseline `roslynLanguageServerInstall` promise).
    pub install_lock: Arc<tokio::sync::Mutex<()>>,
}

impl LaunchEnv {
    pub fn new(bin_dir: PathBuf, npm_cache: PathBuf, home: PathBuf) -> Self {
        Self {
            path: None,
            bin_dir,
            npm_cache,
            home,
            env: BTreeMap::new(),
            disable_download: false,
            experimental_ty: false,
            install_lock: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    pub fn var(&self, key: &str) -> Option<String> {
        self.env
            .get(key)
            .cloned()
            .or_else(|| std::env::var(key).ok())
    }

    fn search_path(&self) -> OsString {
        let base = self
            .path
            .clone()
            .or_else(|| std::env::var_os("PATH"))
            .unwrap_or_default();
        let mut parts: Vec<PathBuf> = std::env::split_paths(&base).collect();
        parts.push(self.bin_dir.clone());
        std::env::join_paths(parts).unwrap_or(base)
    }

    /// core/src/util/which.ts: PATH plus `Global.Path.bin`.
    pub fn which(&self, cmd: &str) -> Option<PathBuf> {
        which::which_in(cmd, Some(self.search_path()), &self.home).ok()
    }

    pub fn command(&self, program: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut cmd = Command::new(program);
        if let Some(path) = &self.path {
            cmd.env("PATH", path);
        }
        cmd.envs(&self.env);
        cmd
    }

    /// Run a helper to completion with piped, discarded stdio. Returns (exit code, stdout, stderr).
    pub async fn run(
        &self,
        program: &Path,
        args: &[&str],
        cwd: Option<&Path>,
        env: &[(&str, &str)],
    ) -> Option<(i32, String, String)> {
        let mut cmd = self.command(program);
        cmd.args(args)
            .envs(env.iter().copied())
            .stdin(Stdio::null())
            .kill_on_drop(true);
        if let Some(cwd) = cwd {
            cmd.current_dir(cwd);
        }
        let out = cmd.output().await.ok()?;
        let text = |b: Vec<u8>| String::from_utf8_lossy(&b).into_owned();
        Some((
            out.status.code().unwrap_or(-1),
            text(out.stdout),
            text(out.stderr),
        ))
    }

    /// `Npm.which(pkg, bin)` cache lookup. A miss would trigger an npm install in the baseline.
    pub fn npm_which(&self, pkg: &str, bin: Option<&str>) -> Result<PathBuf, IntegrationError> {
        let dir = self.npm_cache.join(pkg);
        let bin_dir = dir.join("node_modules").join(".bin");
        let mut files: Vec<String> = std::fs::read_dir(&bin_dir)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        files.sort();
        let picked = match bin {
            Some(b) => files.iter().find(|f| *f == b).cloned(),
            None if files.len() <= 1 => files.first().cloned(),
            None => Some(pick_package_bin(&dir, pkg).unwrap_or_else(|| files[0].clone())),
        };
        picked
            .map(|b| bin_dir.join(b))
            .ok_or(IntegrationError::NotYetPorted(
                "lsp: npm package install (Npm.which cache miss)",
            ))
    }

    /// Spawn a resolved server with piped stdio in `root`.
    pub fn spawn(&self, spec: &Spec, root: &Path) -> Result<Handle, IntegrationError> {
        let mut cmd = self.command(&spec.program);
        cmd.args(&spec.args)
            .envs(&spec.env)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let child = cmd.spawn().map_err(|e| IntegrationError::Failed {
            name: spec.program.display().to_string(),
            message: e.to_string(),
        })?;
        Ok(Handle {
            child,
            initialization: spec.initialization.clone(),
        })
    }
}

fn pick_package_bin(dir: &Path, pkg: &str) -> Option<String> {
    let text =
        std::fs::read_to_string(dir.join("node_modules").join(pkg).join("package.json")).ok()?;
    let json: Value = serde_json::from_str(&text).ok()?;
    let unscoped = pkg.rsplit('/').next().unwrap_or(pkg).to_string();
    match json.get("bin")? {
        Value::String(_) => Some(unscoped),
        Value::Object(map) if map.len() == 1 => map.keys().next().cloned(),
        Value::Object(map) if map.contains_key(&unscoped) => Some(unscoped),
        Value::Object(map) => map.keys().next().cloned(),
        _ => None,
    }
}

/// `Module.resolve(id, dir)` for package subpaths: nearest `node_modules/<id>` walking up.
pub fn module_resolve(id: &str, dir: &Path) -> Option<PathBuf> {
    dir.ancestors()
        .map(|d| d.join("node_modules").join(id))
        .find(|p| {
            if p.is_file() {
                return true;
            }
            p.join("package.json").is_file()
        })
}

/// Resolved command line for one server.
#[derive(Debug, Clone, PartialEq)]
pub struct Spec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub initialization: Option<Map<String, Value>>,
}

impl Spec {
    pub fn new(program: impl Into<PathBuf>, args: &[&str]) -> Self {
        Self {
            program: program.into(),
            args: args.iter().map(|s| s.to_string()).collect(),
            env: BTreeMap::new(),
            initialization: None,
        }
    }

    pub fn init(mut self, value: Value) -> Self {
        self.initialization = value.as_object().cloned();
        self
    }
}
