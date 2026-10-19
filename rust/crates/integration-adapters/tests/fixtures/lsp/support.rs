//! Purpose: shared helpers for lsp integration tests: temp workspace, fake server launch env,
//! log reading. Included via `#[path]` from tests/lsp_*.rs (not a test crate itself).
//! Scope: test-only.
//! Uses:
//! rust/crates/
//! `-- integration-adapters/src/lsp/mod.rs
//! External:
//! |-- serde_json::Value
//! |-- serde_json::from_str
//! |-- tempfile::TempDir
//! |-- tempfile::tempdir
//! `-- tokio::time::sleep
//! Used by:
//! rust/crates/
//! |-- integration-adapters/tests/lsp_client.rs
//! |-- integration-adapters/tests/lsp_inventory.rs
//! |-- integration-adapters/tests/lsp_live.rs
//! `-- integration-adapters/tests/lsp_service.rs
//! Invariants: every path (bin dir, npm cache, home, logs) lives inside the TempDir.
//! Contract tests: none, helper

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use opencode_integration_adapters::lsp::{LaunchEnv, RootContext, Spec, Timeouts};
use serde_json::Value;
use tempfile::TempDir;

pub struct Workspace {
    pub tmp: TempDir,
    pub dir: PathBuf,
    pub log: PathBuf,
}

impl Workspace {
    pub fn new() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        // Canonicalize so file:// URIs round-trip on macOS (/var -> /private/var).
        let base = tmp.path().canonicalize().expect("canon");
        let dir = base.join("ws");
        std::fs::create_dir_all(&dir).expect("ws");
        let log = base.join("fake.log");
        Self { tmp, dir, log }
    }

    pub fn write(&self, rel: &str, text: &str) -> PathBuf {
        let p = self.dir.join(rel);
        std::fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
        std::fs::write(&p, text).expect("write");
        p
    }

    pub fn ctx(&self) -> RootContext {
        RootContext {
            directory: self.dir.clone(),
            worktree: self.dir.clone(),
        }
    }

    pub fn launch(&self) -> LaunchEnv {
        let base = self.tmp.path().canonicalize().expect("canon");
        let mut env = LaunchEnv::new(base.join("bin"), base.join("npm"), base.join("home"));
        env.disable_download = true;
        env
    }

    pub fn messages(&self) -> Vec<Value> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }

    pub fn methods(&self) -> Vec<String> {
        self.messages()
            .iter()
            .filter_map(|m| m["method"].as_str().map(String::from))
            .collect()
    }

    pub fn find(&self, method: &str) -> Vec<Value> {
        self.messages()
            .into_iter()
            .filter(|m| m["method"] == method)
            .collect()
    }

    pub async fn wait_for(&self, pred: impl Fn(&Workspace) -> bool) {
        for _ in 0..200 {
            if pred(self) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("condition not reached, log: {:?}", self.methods());
    }
}

pub fn fake_script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lsp/fake_lsp.py")
}

/// Fake server spec with FAKE_* knobs.
pub fn fake_spec(ws: &Workspace, vars: &[(&str, &str)]) -> Spec {
    let mut spec = Spec::new("python3", &["-u", &fake_script().to_string_lossy()]);
    spec.env
        .insert("FAKE_LSP_LOG".into(), ws.log.to_string_lossy().into_owned());
    for (k, v) in vars {
        spec.env.insert((*k).into(), (*v).into());
    }
    spec
}

pub fn fast_timeouts() -> Timeouts {
    Timeouts {
        initialize: Duration::from_secs(10),
        debounce: Duration::from_millis(50),
        document_wait: Duration::from_millis(1500),
        full_wait: Duration::from_millis(2000),
        diagnostics_request: Duration::from_millis(1000),
        request: Some(Duration::from_secs(5)),
    }
}

pub fn pid_alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .is_ok_and(|s| s.success())
}
