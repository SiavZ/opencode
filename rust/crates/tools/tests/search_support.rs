//! Purpose: shared fixtures for the search and shell suites: a recording authorizer with optional
//! denial, progress recorder, tool context, and real ripgrep/process wiring over temp dirs.
//! Scope: test support only (fixture authorizer; process runner and ripgrep are live).
//! Uses:
//! rust/crates/
//! |-- platform-native/src/process/mod.rs
//! |-- tool-api/src/context.rs
//! `-- tools/src/search/mod.rs
//! External:
//! `-- async_trait::async_trait
//! Used by:
//! rust/crates/
//! |-- tools/tests/search_ripgrep.rs
//! |-- tools/tests/search_tools.rs
//! |-- tools/tests/shell_core.rs
//! |-- tools/tests/shell_legacy.rs
//! `-- tools/tests/shell_scan.rs
//! Invariants: every authorization is recorded in order before the deny decision is applied.
//! Contract tests: the suites listed above
#![allow(dead_code)]

use async_trait::async_trait;
use opencode_permissions::{AssertInput, PermissionError};
use opencode_platform_native::process::NativeProcessRunner;
use opencode_tool_api::{Authorizer, ToolContext, ToolProgress};
use opencode_tools::search::{NoInstaller, Ripgrep, RipgrepBinary, SearchServices};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Default)]
pub struct Recorder {
    pub asked: Mutex<Vec<AssertInput>>,
    pub deny: Mutex<Option<String>>,
}

impl Recorder {
    pub fn deny(action: &str) -> Arc<Self> {
        let recorder = Self::default();
        *recorder.deny.lock().unwrap() = Some(action.into());
        Arc::new(recorder)
    }

    pub fn actions(&self) -> Vec<String> {
        self.asked
            .lock()
            .unwrap()
            .iter()
            .map(|input| input.action.clone())
            .collect()
    }

    pub fn find(&self, action: &str) -> Option<AssertInput> {
        self.asked
            .lock()
            .unwrap()
            .iter()
            .find(|input| input.action == action)
            .cloned()
    }
}

#[async_trait]
impl Authorizer for Recorder {
    async fn assert(&self, input: AssertInput) -> Result<(), PermissionError> {
        let denied = self.deny.lock().unwrap().as_deref() == Some(input.action.as_str());
        self.asked.lock().unwrap().push(input);
        if denied {
            return Err(PermissionError::Declined);
        }
        Ok(())
    }
}

pub type Update = (Option<String>, Map<String, Value>);

#[derive(Debug, Default)]
pub struct Progress {
    pub updates: Mutex<Vec<Update>>,
}

impl ToolProgress for Progress {
    fn update(&self, title: Option<&str>, metadata: &Map<String, Value>) {
        self.updates
            .lock()
            .unwrap()
            .push((title.map(str::to_string), metadata.clone()));
    }
}

pub fn context(authorizer: Arc<dyn Authorizer>, progress: Arc<dyn ToolProgress>) -> ToolContext {
    ToolContext {
        session_id: "ses_test".into(),
        agent: "build".into(),
        assistant_message_id: "msg_test".into(),
        tool_call_id: "call_test".into(),
        cancel: CancellationToken::new(),
        authorizer,
        progress,
    }
}

pub fn ripgrep() -> Ripgrep {
    let binary = RipgrepBinary::new(
        std::env::temp_dir().join("opencode-test-bin"),
        None,
        Arc::new(NoInstaller),
    );
    Ripgrep::new(Arc::new(NativeProcessRunner), Arc::new(binary))
}

pub fn services(directory: &Path, worktree: &Path) -> SearchServices {
    SearchServices {
        ripgrep: ripgrep(),
        directory: directory.to_path_buf(),
        worktree: worktree.to_path_buf(),
    }
}

/// Canonical temp dir (macOS /var -> /private/var) so containment checks are stable.
pub fn temp() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = std::fs::canonicalize(dir.path()).unwrap();
    (dir, path)
}

pub fn write(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, content).unwrap();
}

pub fn text(output: &opencode_tool_api::ToolOutput) -> String {
    output
        .content
        .iter()
        .map(|content| match content {
            opencode_tool_api::ToolContent::Text { text } => text.clone(),
            _ => String::new(),
        })
        .collect::<Vec<_>>()
        .join("\u{0}")
}
